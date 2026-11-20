//! Focus stacking: per pixel, keep the frame whose neighbourhood is sharpest.
//!
//! Same input pipeline as the HDR stack (decode, align, masks, recipes, export);
//! only the selection rule differs, which is why it lives here as a stack
//! method instead of a second application.

/// Sharpness is the local sum of a 3x3 Laplacian on luminance: a high-frequency
/// edge produces a large response, a defocused area produces almost none.
/// The 5x5 box around it keeps single-pixel noise from winning.
pub struct FocusAccumulator {
    width: usize,
    height: usize,
    radius: usize,
    best: Vec<f32>,
    index: Vec<u16>,
}

impl FocusAccumulator {
    pub fn new(width: usize, height: usize, radius: usize) -> Self {
        Self {
            width,
            height,
            radius: radius.max(1),
            best: vec![f32::NEG_INFINITY; width * height],
            index: vec![0; width * height],
        }
    }

    /// Feed one frame's luminance plane (row major, `width * height`).
    pub fn push(&mut self, luma: &[f32], frame_index: usize) {
        self.push_masked(luma, frame_index, None);
    }

    /// Same, but pixels excluded by the frame's mask are never selected.
    pub fn push_masked(&mut self, luma: &[f32], frame_index: usize, excluded: Option<&[u8]>) {
        if luma.len() < self.width * self.height {
            return;
        }
        let response = self.response(luma);
        for (pixel, value) in response.iter().enumerate() {
            if excluded.is_some_and(|mask| mask.get(pixel).is_some_and(|v| *v > 127)) {
                continue;
            }
            if *value > self.best[pixel] {
                self.best[pixel] = *value;
                self.index[pixel] = frame_index.min(u16::MAX as usize) as u16;
            }
        }
    }

    /// Index of the sharpest frame for every pixel.
    pub fn result(&self) -> &[u16] {
        &self.index
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
        box_sum(&edges, width, height, self.radius)
    }
}

/// Separable moving-window sum, O(pixels) regardless of the radius.
fn box_sum(source: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    let mut horizontal = vec![0.0f32; source.len()];
    for y in 0..height {
        let row = y * width;
        for x in 0..width {
            let from = x.saturating_sub(radius);
            let to = (x + radius).min(width - 1);
            horizontal[row + x] = source[row + from..=row + to].iter().sum();
        }
    }
    let mut out = vec![0.0f32; source.len()];
    for y in 0..height {
        let from = y.saturating_sub(radius);
        let to = (y + radius).min(height - 1);
        for x in 0..width {
            let mut sum = 0.0;
            for sy in from..=to {
                sum += horizontal[sy * width + x];
            }
            out[y * width + x] = sum;
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
