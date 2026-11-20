//! Display mapping: exposure, white balance, tone curve, local adjustments.
//!
//! Everything here is display-only. The master stays linear; these settings can
//! be changed and re-rendered at any time without re-stacking.

use serde::{Deserialize, Serialize};

use crate::color;
use crate::image::LinearImage;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Curve {
    /// Straight clip at 1.0.
    None,
    /// Narkowicz' ACES fit. Best all-rounder for light-paint sets.
    Aces,
    /// `x / (1 + x)`, gentle and neutral.
    Reinhard,
    /// Extended Reinhard with a white point of 4, keeps highlights from clipping
    /// as hard as the plain version.
    Reinhard02,
    /// Hable / Uncharted 2 filmic.
    Hable,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToneMapOptions {
    pub exposure_ev: f32,
    pub curve: Curve,
    /// Encode with the sRGB transfer function. Off = raw clamp.
    pub srgb: bool,
    /// Linear value mapped to display black.
    #[serde(default)]
    pub black_point: f32,
    /// Linear value mapped to display white.
    #[serde(default = "default_white")]
    pub white_point: f32,
    /// -1..1, S-curve around mid grey.
    #[serde(default)]
    pub contrast: f32,
    /// -1..1, recover or push highlights.
    #[serde(default)]
    pub highlights: f32,
    /// -1..1, lift or crush shadows.
    #[serde(default)]
    pub shadows: f32,
    /// -1..1, 0 keeps colour, -1 is monochrome.
    #[serde(default)]
    pub saturation: f32,
    /// -1..1, cool to warm.
    #[serde(default)]
    pub temperature: f32,
    /// -1..1, green to magenta.
    #[serde(default)]
    pub tint: f32,
    /// 0..1, unsharp micro-contrast.
    #[serde(default)]
    pub structure: f32,
    /// 0..1, box-blur blend (3x3).
    #[serde(default)]
    pub denoise: f32,
    /// 0..1, local shadow lift driven by a wide luminance blur.
    #[serde(default)]
    pub local: f32,
    /// -1..1, negative darkens the corners.
    #[serde(default)]
    pub vignette: f32,
}

fn default_white() -> f32 {
    1.0
}

impl Default for ToneMapOptions {
    fn default() -> Self {
        Self {
            exposure_ev: 0.0,
            curve: Curve::Aces,
            srgb: true,
            black_point: 0.0,
            white_point: 1.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            saturation: 0.0,
            temperature: 0.0,
            tint: 0.0,
            structure: 0.0,
            denoise: 0.0,
            local: 0.0,
            vignette: 0.0,
        }
    }
}

impl ToneMapOptions {
    /// True when any spatial pass has work to do.
    fn has_spatial(&self) -> bool {
        self.denoise > 0.0 || self.local > 0.0 || self.structure > 0.0 || self.vignette != 0.0
    }
}

pub fn apply_curve(value: f32, curve: Curve) -> f32 {
    let x = value.max(0.0);
    let mapped = match curve {
        Curve::None => x,
        Curve::Aces => {
            let (a, b, c, d, e) = (2.51f32, 0.03f32, 2.43f32, 0.59f32, 0.14f32);
            (x * (a * x + b)) / (x * (c * x + d) + e)
        }
        Curve::Reinhard => x / (1.0 + x),
        Curve::Reinhard02 => {
            let white = 4.0f32;
            (x * (1.0 + x / (white * white))) / (1.0 + x)
        }
        Curve::Hable => {
            let (a, b, c, d, e, f) = (0.15f32, 0.50f32, 0.10f32, 0.20f32, 0.02f32, 0.30f32);
            let white = 11.2f32;
            let hable =
                |v: f32| ((v * (a * v + c * b) + d * e) / (v * (a * v + b) + d * f)) - e / f;
            hable(x) / hable(white)
        }
    };
    mapped.clamp(0.0, 1.0)
}

/// Channel gains for temperature/tint, neutral at 0/0.
fn white_balance(temperature: f32, tint: f32) -> [f32; 3] {
    let t = temperature.clamp(-1.0, 1.0);
    let g = tint.clamp(-1.0, 1.0);
    [
        (1.0 + 0.25 * t) * (1.0 + 0.05 * g),
        1.0 - 0.05 * g.abs(),
        (1.0 - 0.25 * t) * (1.0 - 0.05 * g),
    ]
}

/// Shadows/highlights as a luminance multiplier. Negative values recover.
fn tone_adjust(luma: f32, shadows: f32, highlights: f32) -> f32 {
    let shadow_weight = (1.0 - luma).clamp(0.0, 1.0).powi(2);
    let highlight_weight = luma.clamp(0.0, 1.0).powi(2);
    let gain = 1.0 + 0.6 * shadows * shadow_weight + 0.6 * highlights * highlight_weight;
    gain.clamp(0.2, 2.0)
}

fn contrast_curve(value: f32, contrast: f32) -> f32 {
    let c = contrast.clamp(-1.0, 1.0);
    if c == 0.0 {
        return value;
    }
    let x = value.clamp(0.0, 1.0);
    let shaped = x + c * (x - 0.5) * x * (1.0 - x) * 2.0;
    shaped.clamp(0.0, 1.0)
}

fn apply_saturation(rgb: [f32; 3], saturation: f32) -> [f32; 3] {
    let s = 1.0 + saturation.clamp(-1.0, 1.0);
    let luma = color::luminance(rgb);
    [
        (luma + (rgb[0] - luma) * s).max(0.0),
        (luma + (rgb[1] - luma) * s).max(0.0),
        (luma + (rgb[2] - luma) * s).max(0.0),
    ]
}

/// Separable box blur on one channel, radius `r`.
fn box_blur(src: &[f32], width: usize, height: usize, r: usize) -> Vec<f32> {
    if r == 0 || width == 0 || height == 0 {
        return src.to_vec();
    }
    let mut horizontal = vec![0.0f32; src.len()];
    for y in 0..height {
        let row = y * width;
        for x in 0..width {
            let from = x.saturating_sub(r);
            let to = (x + r).min(width - 1);
            let count = (to - from + 1) as f32;
            let mut sum = 0.0;
            for sx in from..=to {
                sum += src[row + sx];
            }
            horizontal[row + x] = sum / count;
        }
    }
    let mut out = vec![0.0f32; src.len()];
    for y in 0..height {
        let from = y.saturating_sub(r);
        let to = (y + r).min(height - 1);
        let count = (to - from + 1) as f32;
        for x in 0..width {
            let mut sum = 0.0;
            for sy in from..=to {
                sum += horizontal[sy * width + x];
            }
            out[y * width + x] = sum / count;
        }
    }
    out
}

/// Render to display-referred linear values (0..=1, sRGB primaries), three per
/// pixel. Spatial passes run here, so callers that only need a preview should
/// pass the proxy image.
pub fn render(image: &LinearImage, options: &ToneMapOptions) -> Vec<f32> {
    let width = image.width as usize;
    let height = image.height as usize;
    let pixels = width * height;
    let gain = 2f32.powf(options.exposure_ev);
    let wb = white_balance(options.temperature, options.tint);
    let black = options.black_point.clamp(0.0, 0.9);
    let white = options.white_point.max(black + 0.01);
    let span = white - black;

    let mut out = vec![0.0f32; pixels * 3];
    for (index, pixel) in image.data.chunks_exact(3).enumerate() {
        let mut rgb = [
            pixel[0] * gain * wb[0],
            pixel[1] * gain * wb[1],
            pixel[2] * gain * wb[2],
        ];
        for channel in &mut rgb {
            *channel = ((*channel - black) / span).max(0.0);
            *channel = apply_curve(*channel, options.curve);
        }
        let adjusted = tone_adjust(color::luminance(rgb), options.shadows, options.highlights);
        for channel in &mut rgb {
            *channel = contrast_curve(*channel * adjusted, options.contrast);
        }
        let rgb = apply_saturation(rgb, options.saturation);
        out[index * 3..index * 3 + 3].copy_from_slice(&rgb);
    }

    if !options.has_spatial() {
        return out;
    }

    if options.denoise > 0.0 {
        let strength = options.denoise.clamp(0.0, 1.0) * 0.8;
        let mut smoothed = vec![0.0f32; out.len()];
        for channel in 0..3 {
            let plane: Vec<f32> = (0..pixels).map(|i| out[i * 3 + channel]).collect();
            let blurred = box_blur(&plane, width, height, 1);
            for i in 0..pixels {
                smoothed[i * 3 + channel] = plane[i] * (1.0 - strength) + blurred[i] * strength;
            }
        }
        out = smoothed;
    }

    if options.local > 0.0 {
        let strength = options.local.clamp(0.0, 1.0);
        let radius = (width.max(height) / 40).max(2);
        let luma: Vec<f32> = (0..pixels)
            .map(|i| color::luminance([out[i * 3], out[i * 3 + 1], out[i * 3 + 2]]))
            .collect();
        let blurred = box_blur(&luma, width, height, radius);
        for i in 0..pixels {
            let lift = 1.0 + strength * 1.2 * (1.0 - blurred[i]).clamp(0.0, 1.0);
            for channel in 0..3 {
                out[i * 3 + channel] = (out[i * 3 + channel] * lift).clamp(0.0, 1.0);
            }
        }
    }

    if options.structure > 0.0 {
        let strength = options.structure.clamp(0.0, 1.0) * 1.5;
        let luma: Vec<f32> = (0..pixels)
            .map(|i| color::luminance([out[i * 3], out[i * 3 + 1], out[i * 3 + 2]]))
            .collect();
        let blurred = box_blur(&luma, width, height, 2);
        for i in 0..pixels {
            let detail = luma[i] - blurred[i];
            let scale = (luma[i] + detail * strength).max(0.0) / luma[i].max(1e-5);
            for channel in 0..3 {
                out[i * 3 + channel] = (out[i * 3 + channel] * scale).clamp(0.0, 1.0);
            }
        }
    }

    if options.vignette != 0.0 {
        let amount = options.vignette.clamp(-1.0, 1.0);
        let cx = width as f32 / 2.0;
        let cy = height as f32 / 2.0;
        let max_distance = (cx * cx + cy * cy).sqrt().max(1.0);
        for y in 0..height {
            for x in 0..width {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let distance = (dx * dx + dy * dy).sqrt() / max_distance;
                let factor = (1.0 + amount * distance * distance).clamp(0.0, 2.0);
                let index = y * width + x;
                for channel in 0..3 {
                    out[index * 3 + channel] = (out[index * 3 + channel] * factor).clamp(0.0, 1.0);
                }
            }
        }
    }

    out
}

fn encode(value: f32, srgb: bool) -> f32 {
    if srgb {
        color::linear_to_srgb(value.clamp(0.0, 1.0))
    } else {
        value.clamp(0.0, 1.0)
    }
}

pub fn to_display8(image: &LinearImage, options: &ToneMapOptions) -> Vec<u8> {
    let rendered = render(image, options);
    let mut out = Vec::with_capacity(rendered.len());
    for value in rendered {
        out.push((encode(value, options.srgb) * 255.0 + 0.5).clamp(0.0, 255.0) as u8);
    }
    out
}

pub fn to_display16(image: &LinearImage, options: &ToneMapOptions) -> Vec<u16> {
    let rendered = render(image, options);
    let mut out = Vec::with_capacity(rendered.len());
    for value in rendered {
        out.push((encode(value, options.srgb) * 65535.0 + 0.5).clamp(0.0, 65535.0) as u16);
    }
    out
}

/// Exposure (in EV) that maps the 90th luminance percentile to `target`.
/// Used by `--exposure auto` and by the GUI's auto button.
pub fn auto_exposure_ev(image: &LinearImage, target: f32) -> f32 {
    let step = (image.pixel_count() / 200_000).max(1);
    let mut samples: Vec<f32> = Vec::new();
    for index in (0..image.pixel_count()).step_by(step) {
        let base = index * 3;
        samples.push(color::luminance([
            image.data[base],
            image.data[base + 1],
            image.data[base + 2],
        ]));
    }
    if samples.is_empty() {
        return 0.0;
    }
    samples.sort_by(f32::total_cmp);
    let index = ((samples.len() as f32 * 0.9) as usize).min(samples.len() - 1);
    let p90 = samples[index];
    let mut low = -20.0f32;
    let mut high = 20.0f32;
    for _ in 0..48 {
        let mid = (low + high) / 2.0;
        if apply_curve(p90 * 2f32.powf(mid), Curve::Aces) < target {
            low = mid;
        } else {
            high = mid;
        }
    }
    (low + high) / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(width: u32, height: u32) -> LinearImage {
        let mut image = LinearImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let value = (x as f32 / width as f32) * 0.8 + (y as f32 / height as f32) * 0.2;
                image.set_pixel(x, y, [value, value * 0.8, value * 0.6]);
            }
        }
        image
    }

    #[test]
    fn curves_are_monotonic_and_bounded() {
        for curve in [
            Curve::None,
            Curve::Aces,
            Curve::Reinhard,
            Curve::Reinhard02,
            Curve::Hable,
        ] {
            let mut previous = -1.0;
            for step in 0..=200 {
                let x = step as f32 / 100.0 * 20.0;
                let y = apply_curve(x, curve);
                assert!((0.0..=1.0).contains(&y), "{curve:?} at {x} -> {y}");
                assert!(y >= previous - 1e-6, "{curve:?} not monotonic at {x}");
                previous = y;
            }
        }
    }

    #[test]
    fn default_options_render_the_old_way() {
        let image = LinearImage::filled(4, 4, [0.2, 0.3, 0.4]);
        let rendered = render(&image, &ToneMapOptions::default());
        let expected = apply_curve(0.2, Curve::Aces);
        assert!(
            (rendered[0] - expected).abs() < 1e-5,
            "{} vs {expected}",
            rendered[0]
        );
    }

    #[test]
    fn zero_exposure_default_gives_black_at_zero() {
        let image = LinearImage::filled(2, 2, [0.0, 0.0, 0.0]);
        let out = to_display8(&image, &ToneMapOptions::default());
        assert_eq!(out, vec![0u8; 12]);
    }

    #[test]
    fn brighter_exposure_never_darkens() {
        let image = LinearImage::filled(4, 4, [0.05, 0.1, 0.2]);
        let dim = to_display8(
            &image,
            &ToneMapOptions {
                exposure_ev: 0.0,
                ..Default::default()
            },
        );
        let bright = to_display8(
            &image,
            &ToneMapOptions {
                exposure_ev: 2.0,
                ..Default::default()
            },
        );
        assert!(bright[0] >= dim[0]);
        assert!(bright[2] >= dim[2]);
    }

    #[test]
    fn full_desaturation_is_grey() {
        let image = LinearImage::filled(4, 4, [0.6, 0.2, 0.1]);
        let out = render(
            &image,
            &ToneMapOptions {
                saturation: -1.0,
                ..Default::default()
            },
        );
        assert!((out[0] - out[1]).abs() < 1e-5, "{out:?}");
        assert!((out[1] - out[2]).abs() < 1e-5, "{out:?}");
    }

    #[test]
    fn warm_temperature_raises_red_over_blue() {
        let image = LinearImage::filled(4, 4, [0.3, 0.3, 0.3]);
        let neutral = render(&image, &ToneMapOptions::default());
        let warm = render(
            &image,
            &ToneMapOptions {
                temperature: 1.0,
                ..Default::default()
            },
        );
        assert!(warm[0] > neutral[0], "red {} vs {}", warm[0], neutral[0]);
        assert!(warm[2] < neutral[2], "blue {} vs {}", warm[2], neutral[2]);
    }

    #[test]
    fn contrast_pushes_tones_away_from_mid_grey() {
        let low = render(
            &LinearImage::filled(2, 2, [0.1, 0.1, 0.1]),
            &ToneMapOptions {
                curve: Curve::None,
                contrast: 1.0,
                ..Default::default()
            },
        )[0];
        let neutral = render(
            &LinearImage::filled(2, 2, [0.1, 0.1, 0.1]),
            &ToneMapOptions {
                curve: Curve::None,
                ..Default::default()
            },
        )[0];
        assert!(
            low < neutral,
            "contrast did not darken a dark tone: {low} vs {neutral}"
        );
    }

    #[test]
    fn denoise_reduces_variation() {
        let mut image = LinearImage::filled(8, 8, [0.5, 0.5, 0.5]);
        image.set_pixel(4, 4, [1.0, 1.0, 1.0]);
        let noisy = render(
            &image,
            &ToneMapOptions {
                denoise: 0.0,
                ..Default::default()
            },
        );
        let smooth = render(
            &image,
            &ToneMapOptions {
                denoise: 1.0,
                ..Default::default()
            },
        );
        let peak = |values: &[f32]| values.iter().copied().fold(0.0f32, f32::max);
        assert!(
            peak(&smooth) < peak(&noisy),
            "denoise did not soften the spike"
        );
    }

    #[test]
    fn vignette_darkens_the_corner() {
        let image = LinearImage::filled(16, 16, [0.8, 0.8, 0.8]);
        let out = render(
            &image,
            &ToneMapOptions {
                vignette: -1.0,
                ..Default::default()
            },
        );
        assert!(
            out[0] < out[out.len() / 2 - 1],
            "corner is not darker than the centre"
        );
    }

    #[test]
    fn structure_changes_local_contrast_not_mean() {
        let image = gradient(16, 16);
        let plain = render(
            &image,
            &ToneMapOptions {
                curve: Curve::None,
                ..Default::default()
            },
        );
        let sharp = render(
            &image,
            &ToneMapOptions {
                curve: Curve::None,
                structure: 1.0,
                ..Default::default()
            },
        );
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
        assert!(
            (mean(&plain) - mean(&sharp)).abs() < 0.05,
            "structure shifted the mean"
        );
        let differs = plain
            .iter()
            .zip(sharp.iter())
            .any(|(a, b)| (a - b).abs() > 1e-3);
        assert!(differs, "structure did nothing");
    }

    #[test]
    fn old_recipes_without_new_fields_still_parse() {
        let json = r#"{"exposure_ev":1.0,"curve":"Aces","srgb":true}"#;
        let parsed: ToneMapOptions = serde_json::from_str(json).expect("parse");
        assert_eq!(parsed.exposure_ev, 1.0);
        assert_eq!(parsed.white_point, 1.0);
        assert_eq!(parsed.saturation, 0.0);
    }

    #[test]
    fn auto_exposure_brightens_a_dark_image() {
        let image = LinearImage::filled(32, 32, [0.002, 0.002, 0.002]);
        let ev = auto_exposure_ev(&image, 0.55);
        assert!(ev > 0.0, "ev {ev}");
        let out = to_display8(
            &image,
            &ToneMapOptions {
                exposure_ev: ev,
                ..Default::default()
            },
        );
        assert!(out[0] > 40, "still dark: {}", out[0]);
    }
}
