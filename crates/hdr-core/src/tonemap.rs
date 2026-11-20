//! Display mapping: exposure, tone curve, sRGB encoding.

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
    /// Hable / Uncharted 2 filmic.
    Hable,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToneMapOptions {
    pub exposure_ev: f32,
    pub curve: Curve,
    /// Encode with the sRGB transfer function. Off = raw clamp.
    pub srgb: bool,
}

impl Default for ToneMapOptions {
    fn default() -> Self {
        Self {
            exposure_ev: 0.0,
            curve: Curve::Aces,
            srgb: true,
        }
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

/// One pixel of linear light -> display-encoded value in 0..=1.
#[inline]
pub fn map_pixel(rgb: [f32; 3], options: &ToneMapOptions) -> [f32; 3] {
    let gain = 2f32.powf(options.exposure_ev);
    let mut out = [0.0f32; 3];
    for (c, slot) in out.iter_mut().enumerate() {
        let linear = rgb[c] * gain;
        let mapped = apply_curve(linear, options.curve);
        *slot = if options.srgb {
            color::linear_to_srgb(mapped)
        } else {
            mapped
        };
    }
    out
}

pub fn to_display8(image: &LinearImage, options: &ToneMapOptions) -> Vec<u8> {
    let mut out = Vec::with_capacity(image.data.len());
    for pixel in image.data.chunks_exact(3) {
        let mapped = map_pixel([pixel[0], pixel[1], pixel[2]], options);
        for value in mapped {
            out.push((value * 255.0 + 0.5).clamp(0.0, 255.0) as u8);
        }
    }
    out
}

pub fn to_display16(image: &LinearImage, options: &ToneMapOptions) -> Vec<u16> {
    let mut out = Vec::with_capacity(image.data.len());
    for pixel in image.data.chunks_exact(3) {
        let mapped = map_pixel([pixel[0], pixel[1], pixel[2]], options);
        for value in mapped {
            out.push((value * 65535.0 + 0.5).clamp(0.0, 65535.0) as u16);
        }
    }
    out
}

/// Exposure (in EV) that maps the 90th luminance percentile to `target`.
/// Used by `--auto-exposure` and by the GUI's "auto" button.
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

    #[test]
    fn curves_are_monotonic_and_bounded() {
        for curve in [Curve::None, Curve::Aces, Curve::Reinhard, Curve::Hable] {
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
