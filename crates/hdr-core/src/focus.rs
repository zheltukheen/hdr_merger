//! Focus stacking: blend the sharpest frames per pixel and record which frame
//! won as a depth map.
//!
//! Same input pipeline as the HDR stack (decode, align, masks, recipes, export);
//! only the selection rule differs, which is why it lives here as a stack
//! method instead of a second application.
//!
//! Hard selection (feather = 0) keeps the old per-pixel argmax: the single
//! sharpest frame wins and its value is copied. Soft selection (feather > 0)
//! smooths the sharpness response over a scene boundary, so the two frames
//! blend instead of switching at one pixel. The winning frame is still the
//! argmax of the smoothed response, which is what the depth map reports.

/// Radius of the box sum applied on top of the 3x3 Laplacian. The 5x5 box keeps
/// single-pixel noise from winning.
const RESPONSE_RADIUS: usize = 2;

/// Added to the sharpness response before normalising. It is what gives a fully
/// defocused frame a tiny, non-zero weight instead of dropping out completely:
/// `(0 + floor) / (max + floor)` is small but positive. Without it the weights
/// would be a hard `0`, which is the seam this module exists to remove.
const RESPONSE_FLOOR: f32 = 1e-4;

/// Soft focus stacking: feed every frame of a strip, then finish with the
/// blended RGB and a per-pixel depth map.
pub struct FocusStacker {
    width: usize,
    height: usize,
    /// Box radius of the sharpness sum; the legacy API can pick 1 or 2.
    response_radius: usize,
    /// Box radius of the weight smoothing. `0` selects hard mode.
    feather: usize,
    frames: u32,
    sum_rgb: Vec<f32>,
    sum_w: Vec<f32>,
    /// Largest response seen so far, per pixel. Doubles as the normaliser when
    /// a later frame becomes the new maximum (see [`Self::accumulate`]).
    best_response: Vec<f32>,
    best_index: Vec<u16>,
    /// First frame that was allowed at this pixel, used when no frame has any
    /// sharpness signal (or every frame is excluded).
    fallback_rgb: Vec<f32>,
    has_allowed: Vec<bool>,
}

impl FocusStacker {
    /// `feather` = box radius in pixels used to soften the per-frame focus
    /// weights. 0 means "hard selection" (the old behaviour).
    pub fn new(width: usize, height: usize, feather: usize) -> Self {
        Self::with_response_radius(width, height, feather, RESPONSE_RADIUS)
    }

    fn with_response_radius(
        width: usize,
        height: usize,
        feather: usize,
        response_radius: usize,
    ) -> Self {
        let pixels = width * height;
        Self {
            width,
            height,
            response_radius: response_radius.max(1),
            feather,
            frames: 0,
            sum_rgb: vec![0.0; pixels * 3],
            sum_w: vec![0.0; pixels],
            best_response: vec![f32::NEG_INFINITY; pixels],
            best_index: vec![0; pixels],
            fallback_rgb: vec![0.0; pixels * 3],
            has_allowed: vec![false; pixels],
        }
    }

    /// Feed one frame of the current strip.
    /// * `luma`  - luminance plane, width*height
    /// * `rgb`   - interleaved RGB plane, width*height*3 (linear light)
    /// * `frame_index` - 0-based index, used for the depth map
    /// * `excluded` - optional per-pixel mask; >127 means "never use this frame here"
    pub fn push(&mut self, luma: &[f32], rgb: &[f32], frame_index: u32, excluded: Option<&[u8]>) {
        let pixels = self.width * self.height;
        if luma.len() < pixels || rgb.len() < pixels * 3 {
            return;
        }
        let response = self.response(luma);
        let response = if self.feather > 0 {
            box_blur(&response, self.width, self.height, self.feather)
        } else {
            response
        };
        if self.frames == 0 {
            self.fallback_rgb.copy_from_slice(&rgb[..pixels * 3]);
        }
        for (pixel, sharpness) in response.iter().enumerate() {
            self.accumulate(pixel, *sharpness, rgb, frame_index, excluded);
        }
        self.frames += 1;
    }

    /// Finalise the strip: returns (blended interleaved RGB, depth in 0..=1).
    /// Depth is `best_index / (frames_pushed - 1)` so 0 is the first frame and
    /// 1 the last; a single frame yields an all-zero depth.
    pub fn finish(self) -> (Vec<f32>, Vec<f32>) {
        let pixels = self.width * self.height;
        let mut rgb = vec![0.0; pixels * 3];
        let mut depth = vec![0.0; pixels];
        let scale = if self.frames > 1 {
            1.0 / (self.frames - 1) as f32
        } else {
            0.0
        };
        for pixel in 0..pixels {
            let base = pixel * 3;
            let blended = self.blended_pixel(pixel);
            let source: &[f32] = match &blended {
                Some(values) => values,
                None => &self.fallback_rgb[base..base + 3],
            };
            rgb[base..base + 3].copy_from_slice(source);
            depth[pixel] = (self.best_index[pixel] as f32 * scale).clamp(0.0, 1.0);
        }
        (rgb, depth)
    }

    /// Fold one allowed pixel of one frame into the running blend.
    fn accumulate(
        &mut self,
        pixel: usize,
        response: f32,
        rgb: &[f32],
        frame_index: u32,
        excluded: Option<&[u8]>,
    ) {
        if excluded.is_some_and(|mask| mask.get(pixel).is_some_and(|value| *value > 127)) {
            return;
        }
        let base = pixel * 3;
        if !self.has_allowed[pixel] {
            self.has_allowed[pixel] = true;
            self.fallback_rgb[base..base + 3].copy_from_slice(&rgb[base..base + 3]);
        }
        let best = self.best_response[pixel];
        let first = !best.is_finite();
        let weight = if first || response > best {
            self.best_response[pixel] = response;
            self.best_index[pixel] = frame_index.min(u16::MAX as u32) as u16;
            if !first {
                self.rescale(pixel, best, response);
            }
            1.0
        } else if self.feather > 0 {
            weight(response, best)
        } else {
            0.0
        };
        if weight > 0.0 {
            self.sum_w[pixel] += weight;
            for channel in 0..3 {
                self.sum_rgb[base + channel] += weight * rgb[base + channel];
            }
        }
    }

    /// A new maximum changes the normaliser for the whole pixel, so the weights
    /// already accumulated move by the same factor. In hard mode the previous
    /// winner must be dropped outright (its weight was 1, everyone else's 0).
    fn rescale(&mut self, pixel: usize, old_best: f32, new_best: f32) {
        if self.feather == 0 {
            self.sum_w[pixel] = 0.0;
            self.sum_rgb[pixel * 3..pixel * 3 + 3].fill(0.0);
            return;
        }
        let factor = ((old_best + RESPONSE_FLOOR) / (new_best + RESPONSE_FLOOR)).powi(2);
        self.sum_w[pixel] *= factor;
        for channel in 0..3 {
            self.sum_rgb[pixel * 3 + channel] *= factor;
        }
    }

    /// Blended value for a pixel, or `None` when it has no focus signal at all.
    fn blended_pixel(&self, pixel: usize) -> Option<[f32; 3]> {
        if self.best_response[pixel] <= 0.0 || self.sum_w[pixel] <= 0.0 {
            return None;
        }
        let inverse = 1.0 / self.sum_w[pixel].max(f32::EPSILON);
        let base = pixel * 3;
        let rgb = [
            self.sum_rgb[base] * inverse,
            self.sum_rgb[base + 1] * inverse,
            self.sum_rgb[base + 2] * inverse,
        ];
        rgb.iter().all(|value| value.is_finite()).then_some(rgb)
    }

    fn response(&self, luma: &[f32]) -> Vec<f32> {
        let width = self.width;
        let height = self.height;
        let mut edges = vec![0.0f32; width * height];
        for y in 0..height {
            let up = y.saturating_sub(1);
            let down = (y + 1).min(height - 1);
            for x in 0..width {
                let left = x.saturating_sub(1);
                let right = (x + 1).min(width - 1);
                let center = luma[y * width + x];
                let laplacian = 4.0 * center
                    - luma[y * width + left]
                    - luma[y * width + right]
                    - luma[up * width + x]
                    - luma[down * width + x];
                edges[y * width + x] = laplacian.abs();
            }
        }
        box_sum(&edges, width, height, self.response_radius)
    }
}

/// Soft weight of a frame that did not raise the maximum: the squared ratio of
/// its response to the current maximum, floored so it never reaches zero.
fn weight(response: f32, best: f32) -> f32 {
    ((response + RESPONSE_FLOOR) / (best + RESPONSE_FLOOR)).powi(2)
}

/// Legacy API: the per-pixel argmax stack. Kept as a thin wrapper so existing
/// callers keep compiling; prefer [`FocusStacker`] for blending and depth.
pub struct FocusAccumulator {
    stacker: FocusStacker,
}

impl FocusAccumulator {
    pub fn new(width: usize, height: usize, radius: usize) -> Self {
        Self {
            stacker: FocusStacker::with_response_radius(width, height, 0, radius.max(1)),
        }
    }

    /// Feed one frame's luminance plane (row major, `width * height`).
    pub fn push(&mut self, luma: &[f32], frame_index: usize) {
        self.push_masked(luma, frame_index, None);
    }

    /// Same, but pixels excluded by the frame's mask are never selected.
    pub fn push_masked(&mut self, luma: &[f32], frame_index: usize, excluded: Option<&[u8]>) {
        let pixels = self.stacker.width * self.stacker.height;
        let rgb = grey_rgb(luma, pixels);
        self.stacker.push(
            luma,
            &rgb,
            frame_index.min(u32::MAX as usize) as u32,
            excluded,
        );
    }

    /// Index of the sharpest frame for every pixel.
    pub fn result(&self) -> &[u16] {
        &self.stacker.best_index
    }
}

/// Replicated grey plane, so the legacy luminance-only API can feed the
/// three-channel accumulator.
fn grey_rgb(luma: &[f32], pixels: usize) -> Vec<f32> {
    let mut rgb = Vec::with_capacity(pixels * 3);
    for value in luma.iter().take(pixels) {
        rgb.extend_from_slice(&[*value, *value, *value]);
    }
    rgb
}

/// Separable moving-window sum, O(pixels) regardless of the radius.
fn box_sum(source: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    separable_box(source, width, height, radius, false)
}

fn box_blur(source: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    if radius == 0 {
        source.to_vec()
    } else {
        separable_box(source, width, height, radius, true)
    }
}

/// Separable box filter. With `average` false it sums the window (the sharpness
/// response); with `average` true it divides by the actual, edge-clipped window
/// size so feathering does not darken the borders.
fn separable_box(
    source: &[f32],
    width: usize,
    height: usize,
    radius: usize,
    average: bool,
) -> Vec<f32> {
    let mut horizontal = vec![0.0f32; source.len()];
    for y in 0..height {
        let row = y * width;
        for x in 0..width {
            let from = x.saturating_sub(radius);
            let to = (x + radius).min(width - 1);
            let sum: f32 = source[row + from..=row + to].iter().sum();
            horizontal[row + x] = if average {
                sum / (to - from + 1) as f32
            } else {
                sum
            };
        }
    }
    let mut out = vec![0.0f32; source.len()];
    for y in 0..height {
        let from = y.saturating_sub(radius);
        let to = (y + radius).min(height - 1);
        let count = (to - from + 1) as f32;
        for x in 0..width {
            let mut sum = 0.0;
            for sy in from..=to {
                sum += horizontal[sy * width + x];
            }
            out[y * width + x] = if average { sum / count } else { sum };
        }
    }
    out
}

/// Linear luminance of an interleaved f32 RGB buffer.
pub fn luma_plane(rgb: &[f32], pixels: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(pixels);
    for index in 0..pixels {
        let base = index * 3;
        out.push(0.2126 * rgb[base] + 0.7152 * rgb[base + 1] + 0.0722 * rgb[base + 2]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DARK: [f32; 3] = [0.2, 0.1, 0.05];
    const BRIGHT: [f32; 3] = [0.8, 0.6, 0.3];

    /// Sharp side = high-frequency stripes, blurred side = constant.
    fn plane(width: usize, height: usize, sharp_left: bool) -> Vec<f32> {
        let mut out = vec![0.0f32; width * height];
        for y in 0..height {
            for x in 0..width {
                let in_sharp_half = if sharp_left {
                    x < width / 2
                } else {
                    x >= width / 2
                };
                out[y * width + x] = if in_sharp_half {
                    if (x + y) % 2 == 0 {
                        1.0
                    } else {
                        0.0
                    }
                } else {
                    0.5
                };
            }
        }
        out
    }

    fn rgb_plane(width: usize, height: usize, colour: [f32; 3]) -> Vec<f32> {
        let mut out = vec![0.0f32; width * height * 3];
        for pixel in out.chunks_exact_mut(3) {
            pixel.copy_from_slice(&colour);
        }
        out
    }

    /// Two frames: A sharp on the left, B sharp on the right.
    fn stack(
        width: usize,
        height: usize,
        feather: usize,
        excluded_a: Option<&[u8]>,
    ) -> (Vec<f32>, Vec<f32>) {
        let mut stacker = FocusStacker::new(width, height, feather);
        stacker.push(
            &plane(width, height, true),
            &rgb_plane(width, height, DARK),
            0,
            excluded_a,
        );
        stacker.push(
            &plane(width, height, false),
            &rgb_plane(width, height, BRIGHT),
            1,
            None,
        );
        stacker.finish()
    }

    fn pixel(rgb: &[f32], width: usize, x: usize, y: usize) -> [f32; 3] {
        let base = (y * width + x) * 3;
        [rgb[base], rgb[base + 1], rgb[base + 2]]
    }

    #[test]
    fn hard_mode_matches_the_old_selection() {
        let (width, height) = (64usize, 8usize);
        let (rgb, _) = stack(width, height, 0, None);
        assert_eq!(pixel(&rgb, width, 4, 4), DARK, "left half is frame A");
        assert_eq!(pixel(&rgb, width, 60, 4), BRIGHT, "right half is frame B");
    }

    #[test]
    fn feathered_mode_has_no_hard_seam() {
        let (width, height) = (64usize, 8usize);
        let (rgb, _) = stack(width, height, 8, None);
        let seam = pixel(&rgb, width, width / 2 - 1, 4);
        for channel in 0..3 {
            assert!(
                seam[channel] > DARK[channel] && seam[channel] < BRIGHT[channel],
                "pixel next to the boundary must be a mix, got {seam:?}"
            );
        }
        let left = pixel(&rgb, width, 4, 4);
        let right = pixel(&rgb, width, 60, 4);
        for channel in 0..3 {
            assert!((left[channel] - DARK[channel]).abs() < 1e-3);
            assert!((right[channel] - BRIGHT[channel]).abs() < 1e-3);
        }
    }

    #[test]
    fn depth_map_tracks_the_winning_frame() {
        let (width, height) = (64usize, 8usize);
        let (_, depth) = stack(width, height, 8, None);
        assert_eq!(depth[4 * width + 4], 0.0, "left half is frame 0");
        assert_eq!(depth[4 * width + 60], 1.0, "right half is frame 1");
        assert!(depth
            .iter()
            .all(|value| value.is_finite() && (0.0..=1.0).contains(value)));
    }

    #[test]
    fn excluded_pixels_never_contribute() {
        let (width, height) = (64usize, 8usize);
        let excluded = vec![255u8; width * height];
        let (rgb, _) = stack(width, height, 4, Some(&excluded));
        for x in [4, 28, 36, 60] {
            assert_eq!(
                pixel(&rgb, width, x, 4),
                BRIGHT,
                "only frame B may appear at x={x}"
            );
        }
    }

    #[test]
    fn single_frame_depth_is_zero() {
        let (width, height) = (16usize, 4usize);
        let mut stacker = FocusStacker::new(width, height, 4);
        stacker.push(
            &plane(width, height, true),
            &rgb_plane(width, height, DARK),
            0,
            None,
        );
        let (rgb, depth) = stacker.finish();
        assert!(depth.iter().all(|value| *value == 0.0));
        assert_eq!(pixel(&rgb, width, 2, 2), DARK);
    }

    #[test]
    fn weights_do_not_produce_nan_or_inf() {
        let (width, height) = (16usize, 4usize);
        let flat = vec![0.5f32; width * height];
        let mut stacker = FocusStacker::new(width, height, 4);
        stacker.push(&flat, &rgb_plane(width, height, [0.25, 0.5, 0.75]), 0, None);
        stacker.push(&flat, &rgb_plane(width, height, [0.9, 0.9, 0.9]), 1, None);
        let (rgb, depth) = stacker.finish();
        assert!(rgb.iter().all(|value| value.is_finite()));
        assert!(depth.iter().all(|value| value.is_finite()));
        assert_eq!(pixel(&rgb, width, 8, 2), [0.25, 0.5, 0.75]);
    }

    // --- legacy API ---

    #[test]
    fn picks_the_sharp_frame_per_region() {
        let (width, height) = (32usize, 8usize);
        let left_sharp = plane(width, height, true);
        let right_sharp = plane(width, height, false);
        let mut accumulator = FocusAccumulator::new(width, height, 2);
        accumulator.push(&left_sharp, 0);
        accumulator.push(&right_sharp, 1);
        let result = accumulator.result();

        assert_eq!(
            result[4 * width + 4],
            0,
            "left half should come from frame 0"
        );
        assert_eq!(
            result[4 * width + 28],
            1,
            "right half should come from frame 1"
        );
    }

    #[test]
    fn identical_frames_keep_the_first() {
        let (width, height) = (8usize, 4usize);
        let flat = vec![0.4f32; width * height];
        let mut accumulator = FocusAccumulator::new(width, height, 1);
        accumulator.push(&flat, 0);
        accumulator.push(&flat, 1);
        assert!(accumulator.result().iter().all(|index| *index == 0));
    }

    #[test]
    fn luma_matches_rec709() {
        let plane = luma_plane(&[1.0, 0.0, 0.0], 1);
        assert!((plane[0] - 0.2126).abs() < 1e-6);
    }

    #[test]
    fn masked_pixels_are_never_selected() {
        let (width, height) = (16usize, 4usize);
        let sharp = plane(width, height, true);
        let flat = vec![0.5f32; width * height];
        let mut masked = vec![0u8; width * height];
        for x in 0..width {
            masked[2 * width + x] = 255;
        }
        let mut accumulator = FocusAccumulator::new(width, height, 1);
        accumulator.push_masked(&sharp, 0, Some(&masked));
        accumulator.push_masked(&flat, 1, None);
        let result = accumulator.result();
        assert!(result[2 * width..3 * width].iter().all(|index| *index == 1));
    }

    #[test]
    fn ignores_short_input() {
        let mut accumulator = FocusAccumulator::new(4, 4, 1);
        accumulator.push(&[1.0, 2.0], 1);
        assert!(accumulator.result().iter().all(|index| *index == 0));
    }
}
