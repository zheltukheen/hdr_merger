//! The engine's only pixel container: interleaved RGB, linear light, f32.

use crate::color;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinearImage {
    pub width: u32,
    pub height: u32,
    /// `width * height * 3` values, row major, RGB interleaved.
    pub data: Vec<f32>,
}

impl LinearImage {
    pub fn new(width: u32, height: u32) -> Self {
        let len = width as usize * height as usize * 3;
        Self {
            width,
            height,
            data: vec![0.0; len],
        }
    }

    pub fn filled(width: u32, height: u32, rgb: [f32; 3]) -> Self {
        let mut image = Self::new(width, height);
        for pixel in image.data.chunks_exact_mut(3) {
            pixel.copy_from_slice(&rgb);
        }
        image
    }

    pub fn from_rgb_f32(width: u32, height: u32, data: Vec<f32>) -> Self {
        Self {
            width,
            height,
            data,
        }
    }

    pub fn pixel_count(&self) -> usize {
        self.width as usize * self.height as usize
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    pub fn bytes_f32(&self) -> usize {
        self.data.len() * std::mem::size_of::<f32>()
    }

    #[inline]
    pub fn index(&self, x: u32, y: u32) -> usize {
        (y as usize * self.width as usize + x as usize) * 3
    }

    #[inline]
    pub fn pixel(&self, x: u32, y: u32) -> [f32; 3] {
        let i = self.index(x, y);
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }

    #[inline]
    pub fn set_pixel(&mut self, x: u32, y: u32, rgb: [f32; 3]) {
        let i = self.index(x, y);
        self.data[i..i + 3].copy_from_slice(&rgb);
    }

    pub fn max_channel(&self) -> f32 {
        self.data.iter().copied().fold(0.0f32, f32::max)
    }

    pub fn mean_luminance(&self) -> f32 {
        if self.data.is_empty() {
            return 0.0;
        }
        let sum: f32 = self
            .data
            .chunks_exact(3)
            .map(|p| color::luminance([p[0], p[1], p[2]]))
            .sum();
        sum / self.pixel_count() as f32
    }

    /// Bilinear resample. Used for proxies and for the alignment pass.
    pub fn resized(&self, width: u32, height: u32) -> Self {
        if width == self.width && height == self.height {
            return self.clone();
        }
        let mut out = LinearImage::new(width, height);
        let sx = self.width as f32 / width as f32;
        let sy = self.height as f32 / height as f32;
        let max_x = self.width.saturating_sub(1);
        let max_y = self.height.saturating_sub(1);
        for y in 0..height {
            let fy = ((y as f32 + 0.5) * sy - 0.5).max(0.0);
            let y0 = (fy as u32).min(max_y);
            let y1 = (y0 + 1).min(max_y);
            let ty = (fy - y0 as f32).clamp(0.0, 1.0);
            for x in 0..width {
                let fx = ((x as f32 + 0.5) * sx - 0.5).max(0.0);
                let x0 = (fx as u32).min(max_x);
                let x1 = (x0 + 1).min(max_x);
                let tx = (fx - x0 as f32).clamp(0.0, 1.0);
                let mut rgb = [0.0f32; 3];
                for (c, slot) in rgb.iter_mut().enumerate() {
                    let p00 = self.data[self.index(x0, y0) + c];
                    let p10 = self.data[self.index(x1, y0) + c];
                    let p01 = self.data[self.index(x0, y1) + c];
                    let p11 = self.data[self.index(x1, y1) + c];
                    let top = p00 + (p10 - p00) * tx;
                    let bottom = p01 + (p11 - p01) * tx;
                    *slot = top + (bottom - top) * ty;
                }
                out.set_pixel(x, y, rgb);
            }
        }
        out
    }

    /// Fit inside `max_dim` keeping the aspect ratio. Never upscales.
    pub fn fit_within(&self, max_dim: u32) -> Self {
        let longest = self.width.max(self.height);
        if longest <= max_dim || max_dim == 0 {
            return self.clone();
        }
        let scale = max_dim as f32 / longest as f32;
        let w = ((self.width as f32 * scale).round() as u32).max(1);
        let h = ((self.height as f32 * scale).round() as u32).max(1);
        self.resized(w, h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_keeps_uniform_image_uniform() {
        let image = LinearImage::filled(64, 32, [0.25, 0.5, 0.75]);
        let small = image.resized(16, 8);
        assert_eq!(small.width, 16);
        assert_eq!(small.height, 8);
        for pixel in small.data.chunks_exact(3) {
            assert!((pixel[0] - 0.25).abs() < 1e-6);
            assert!((pixel[2] - 0.75).abs() < 1e-6);
        }
    }

    #[test]
    fn fit_within_does_not_upscale() {
        let image = LinearImage::new(100, 50);
        assert_eq!(image.fit_within(200).width, 100);
        let fitted = image.fit_within(50);
        assert_eq!((fitted.width, fitted.height), (50, 25));
    }

    #[test]
    fn pixel_roundtrip() {
        let mut image = LinearImage::new(4, 4);
        image.set_pixel(2, 3, [1.0, 2.0, 3.0]);
        assert_eq!(image.pixel(2, 3), [1.0, 2.0, 3.0]);
    }
}
