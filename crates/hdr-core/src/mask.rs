//! Per-frame exclusion masks.
//!
//! `0` keeps a pixel, `255` throws it away for that frame. Masked pixels fall
//! through to the remaining frames, which is exactly the light-painting use
//! case: the photographer stands in one spot, mask him out of the frames where
//! he is in the way and the stack uses the light from the others.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mask {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Mask {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            data: vec![0; width as usize * height as usize],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.data.iter().all(|v| *v == 0)
    }

    pub fn excluded(&self, index: usize) -> bool {
        self.data.get(index).is_some_and(|v| *v > 127)
    }

    pub fn excluded_count(&self) -> usize {
        self.data.iter().filter(|v| **v > 127).count()
    }

    /// Soft round brush. `center` and `radius` are in mask pixels.
    pub fn paint_circle(&mut self, cx: f32, cy: f32, radius: f32, softness: f32, value: u8) {
        if radius <= 0.0 {
            return;
        }
        let x0 = ((cx - radius).floor().max(0.0)) as u32;
        let x1 = ((cx + radius).ceil().min(self.width as f32 - 1.0)).max(0.0) as u32;
        let y0 = ((cy - radius).floor().max(0.0)) as u32;
        let y1 = ((cy + radius).ceil().min(self.height as f32 - 1.0)).max(0.0) as u32;
        let feather = radius * softness.clamp(0.0, 0.99);
        let inner = (radius - feather).max(0.0);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let distance = (dx * dx + dy * dy).sqrt();
                let weight = if distance <= inner {
                    1.0
                } else if feather == 0.0 || distance >= radius {
                    0.0
                } else {
                    1.0 - (distance - inner) / feather
                };
                if weight <= 0.0 {
                    continue;
                }
                let idx = y as usize * self.width as usize + x as usize;
                let target = (value as f32 * weight) as u8;
                self.data[idx] = self.data[idx].max(target);
            }
        }
    }

    pub fn clear(&mut self) {
        self.data.iter_mut().for_each(|v| *v = 0);
    }

    /// Nearest-neighbour resize, used when the stack runs on a proxy.
    pub fn resized(&self, width: u32, height: u32) -> Self {
        if width == self.width && height == self.height {
            return self.clone();
        }
        let mut out = Mask::new(width, height);
        for y in 0..height {
            let sy = (y as f32 + 0.5) * self.height as f32 / height as f32;
            let sy = (sy as u32).min(self.height.saturating_sub(1));
            for x in 0..width {
                let sx = (x as f32 + 0.5) * self.width as f32 / width as f32;
                let sx = (sx as u32).min(self.width.saturating_sub(1));
                out.data[y as usize * width as usize + x as usize] =
                    self.data[sy as usize * self.width as usize + sx as usize];
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_mask_excludes_nothing() {
        let mask = Mask::new(10, 10);
        assert!(mask.is_empty());
        assert_eq!(mask.excluded_count(), 0);
    }

    #[test]
    fn brush_marks_center_only() {
        let mut mask = Mask::new(32, 32);
        mask.paint_circle(16.0, 16.0, 4.0, 0.0, 255);
        assert!(mask.excluded(16 * 32 + 16));
        assert!(!mask.excluded(0));
        assert!(mask.excluded_count() > 40);
    }

    #[test]
    fn resize_keeps_coverage() {
        let mut mask = Mask::new(64, 64);
        mask.paint_circle(32.0, 32.0, 8.0, 0.0, 255);
        let small = mask.resized(16, 16);
        assert!(small.excluded(8 * 16 + 8));
        assert!(!small.excluded(0));
    }
}
