//! Translation alignment by phase correlation.
//!
//! For a tripod / flash-walk set every frame is the same picture with the same
//! framing, so a pure translation model is enough and is cheap: two 2-D FFTs
//! and one cross-power spectrum. Rotation/scale would need feature matching and
//! is deliberately out of scope (see ROADMAP).

use rustfft::num_complex::Complex;
use rustfft::FftPlanner;
use serde::{Deserialize, Serialize};

use crate::color;
use crate::error::{HdrError, Result};
use crate::image::LinearImage;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shift {
    pub dx: i32,
    pub dy: i32,
    /// Peak-to-mean ratio of the correlation surface. Below ~10 is noise.
    pub confidence: f32,
}

impl Shift {
    pub fn identity() -> Self {
        Self {
            dx: 0,
            dy: 0,
            confidence: 1.0,
        }
    }

    pub fn magnitude(&self) -> i32 {
        self.dx.abs() + self.dy.abs()
    }
}

/// Estimate how far `moving` has to move to line up with `reference`.
pub fn estimate_shift(reference: &LinearImage, moving: &LinearImage) -> Result<Shift> {
    if reference.width != moving.width || reference.height != moving.height {
        return Err(HdrError::Invalid(format!(
            "alignment needs equal sizes: {}x{} vs {}x{}",
            reference.width, reference.height, moving.width, moving.height
        )));
    }
    let width = reference.width as usize;
    let height = reference.height as usize;
    if width < 8 || height < 8 {
        return Err(HdrError::Invalid(
            "alignment needs at least 8x8 pixels".into(),
        ));
    }

    let a = windowed_gray(reference);
    let b = windowed_gray(moving);
    let fa = fft2(&a, width, height);
    let fb = fft2(&b, width, height);

    let mut cross: Vec<Complex<f32>> = Vec::with_capacity(fa.len());
    for (x, y) in fa.iter().zip(fb.iter()) {
        let product = x * y.conj();
        let magnitude = product.norm();
        cross.push(if magnitude > 1e-12 {
            product / magnitude
        } else {
            Complex::new(0.0, 0.0)
        });
    }

    let surface = ifft2(&cross, width, height);
    let mut peak_index = 0usize;
    let mut peak = f32::MIN;
    let mut energy = 0.0f32;
    for (index, value) in surface.iter().enumerate() {
        energy += value.re.abs();
        if value.re > peak {
            peak = value.re;
            peak_index = index;
        }
    }
    let mean = energy / surface.len() as f32;
    let mut dy = (peak_index / width) as i32;
    let mut dx = (peak_index % width) as i32;
    if dx > width as i32 / 2 {
        dx -= width as i32;
    }
    if dy > height as i32 / 2 {
        dy -= height as i32;
    }
    // The cross-power spectrum peaks at the shift that maps `moving` back
    // onto `reference`, which is the negated content offset.
    Ok(Shift {
        dx: -dx,
        dy: -dy,
        confidence: if mean > 1e-9 { peak / mean } else { 0.0 },
    })
}

fn windowed_gray(image: &LinearImage) -> Vec<f32> {
    let width = image.width as usize;
    let height = image.height as usize;
    let mut out = vec![0.0f32; width * height];
    let denom = ((width - 1).max(1) * (height - 1).max(1)) as f32;
    for y in 0..height {
        let wy = (std::f32::consts::PI * y as f32 / (height - 1).max(1) as f32).sin();
        for x in 0..width {
            let wx = (std::f32::consts::PI * x as f32 / (width - 1).max(1) as f32).sin();
            let index = y * width + x;
            out[index] = color::luminance(image.pixel(x as u32, y as u32)) * wx * wy;
        }
    }
    let _ = denom;
    out
}

fn fft2(input: &[f32], width: usize, height: usize) -> Vec<Complex<f32>> {
    let mut planner = FftPlanner::<f32>::new();
    let rows = planner.plan_fft_forward(width);
    let columns = planner.plan_fft_forward(height);
    let mut buffer: Vec<Complex<f32>> = input.iter().map(|v| Complex::new(*v, 0.0)).collect();
    for y in 0..height {
        rows.process(&mut buffer[y * width..(y + 1) * width]);
    }
    let mut line = vec![Complex::new(0.0, 0.0); height];
    for x in 0..width {
        for y in 0..height {
            line[y] = buffer[y * width + x];
        }
        columns.process(&mut line);
        for y in 0..height {
            buffer[y * width + x] = line[y];
        }
    }
    buffer
}

fn ifft2(input: &[Complex<f32>], width: usize, height: usize) -> Vec<Complex<f32>> {
    let mut planner = FftPlanner::<f32>::new();
    let rows = planner.plan_fft_inverse(width);
    let columns = planner.plan_fft_inverse(height);
    let mut buffer: Vec<Complex<f32>> = input.to_vec();
    for y in 0..height {
        rows.process(&mut buffer[y * width..(y + 1) * width]);
    }
    let mut line = vec![Complex::new(0.0, 0.0); height];
    for x in 0..width {
        for y in 0..height {
            line[y] = buffer[y * width + x];
        }
        columns.process(&mut line);
        for y in 0..height {
            buffer[y * width + x] = line[y];
        }
    }
    buffer
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic textured image so correlation has something to lock on.
    fn textured(width: u32, height: u32) -> LinearImage {
        let mut image = LinearImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let v = (((x * 7 + y * 13) % 29) as f32) / 29.0;
                let blob = if x > 20 && x < 40 && y > 10 && y < 30 {
                    0.7
                } else {
                    0.0
                };
                image.set_pixel(x, y, [v + blob, v * 0.5 + blob, v * 0.25]);
            }
        }
        image
    }

    fn shift_image(image: &LinearImage, dx: i32, dy: i32) -> LinearImage {
        let mut out = LinearImage::new(image.width, image.height);
        for y in 0..image.height as i32 {
            for x in 0..image.width as i32 {
                let sx = (x - dx).clamp(0, image.width as i32 - 1) as u32;
                let sy = (y - dy).clamp(0, image.height as i32 - 1) as u32;
                out.set_pixel(x as u32, y as u32, image.pixel(sx, sy));
            }
        }
        out
    }

    #[test]
    fn identity_shift_is_zero() {
        let image = textured(128, 96);
        let shift = estimate_shift(&image, &image).unwrap();
        assert_eq!((shift.dx, shift.dy), (0, 0));
    }

    #[test]
    fn recovers_known_translation() {
        let reference = textured(128, 96);
        let moved = shift_image(&reference, 6, -4);
        let shift = estimate_shift(&reference, &moved).unwrap();
        assert_eq!(
            (shift.dx, shift.dy),
            (6, -4),
            "confidence {}",
            shift.confidence
        );
        assert!(shift.confidence > 5.0, "confidence {}", shift.confidence);
    }

    #[test]
    fn rejects_size_mismatch() {
        let a = LinearImage::new(32, 32);
        let b = LinearImage::new(32, 16);
        assert!(estimate_shift(&a, &b).is_err());
    }
}
