//! Colour transfer functions. All of them are pure and total.

/// sRGB display encoding -> linear light.
pub fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear light -> sRGB display encoding (clamped to one).
pub fn linear_to_srgb(value: f32) -> f32 {
    if value <= 0.0031308 {
        12.92 * value
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

/// Rec.709 luminance of a linear RGB triple.
pub fn luminance(rgb: [f32; 3]) -> f32 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_roundtrip_is_stable() {
        for v in [0.0, 0.02, 0.18, 0.5, 0.9, 1.0] {
            let back = linear_to_srgb(srgb_to_linear(v));
            assert!((back - v).abs() < 1e-5, "{v} -> {back}");
        }
    }

    #[test]
    fn mid_grey_maps_to_018() {
        let linear = srgb_to_linear(0.5);
        assert!((linear - 0.214).abs() < 0.002, "{linear}");
    }

    #[test]
    fn luminance_of_white_is_one() {
        assert!((luminance([1.0, 1.0, 1.0]) - 1.0).abs() < 1e-6);
    }
}
