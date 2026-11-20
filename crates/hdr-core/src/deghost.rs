//! Automatic deghosting: find the frames where the photographer (or any other
//! moving subject) blocks the scene, and mask those frames only.
//!
//! Detection runs on a small proxy. The temporal median of every pixel is the
//! "clean" scene; a pixel that deviates from it in one frame is a mover. The
//! bitmap is cleaned (3x3 erode + dilate, small components dropped) and then
//! upscaled to the frame's native size with nearest-neighbour sampling, so the
//! result plugs straight into [`crate::FrameInput::with_mask`].

use std::path::PathBuf;

use crate::error::Result;
use crate::image::LinearImage;
use crate::io::{self, InputEncoding};
use crate::mask::Mask;

/// Luminance below this is treated as black sensor noise, never as a mover.
const MEDIAN_FLOOR: f32 = 0.002;

/// Components smaller than this share of the proxy are noise, not a subject.
const MIN_COMPONENT_FRACTION: f32 = 0.0002;

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DeghostOptions {
    /// How far a pixel must deviate from the temporal median to count as a
    /// mover, in linear luminance. 0.02..1.0, default 0.15.
    pub threshold: f32,
    /// Longest edge used for detection. Detection runs at proxy size and the
    /// mask is upscaled; default 1024.
    pub probe_edge: u32,
}

impl Default for DeghostOptions {
    fn default() -> Self {
        Self {
            threshold: 0.15,
            probe_edge: 1024,
        }
    }
}

#[derive(Debug)]
pub struct DeghostResult {
    /// One entry per input frame, in input order. `None` when the frame is clean.
    pub masks: Vec<Option<crate::Mask>>,
    /// Frames that got a mask, for a progress/summary line.
    pub moved_frames: Vec<usize>,
}

/// Detect the frames that contain a moving subject. `progress(done, total)` is
/// called once per decoded frame.
pub fn detect_movers(
    paths: &[PathBuf],
    options: &DeghostOptions,
    progress: &dyn Fn(u32, u32),
) -> Result<DeghostResult> {
    if paths.is_empty() {
        return Err(crate::HdrError::NoFrames);
    }
    let lumas = load_probes(paths, options.probe_edge, progress)?;
    let width = lumas.width;
    let height = lumas.height;
    let medians = temporal_median(&lumas.planes);

    let mut masks = Vec::with_capacity(paths.len());
    let mut moved_frames = Vec::new();
    for (index, luma) in lumas.planes.iter().enumerate() {
        let candidates = candidate_bitmap(luma, &medians, options.threshold);
        let mut cleaned = dilate(&erode(&candidates, width, height), width, height);
        if drop_small_components(&mut cleaned, width, height) == 0 {
            masks.push(None);
            continue;
        }
        let mut proxy = Mask::new(width, height);
        for (slot, hit) in proxy.data.iter_mut().zip(&cleaned) {
            if *hit {
                *slot = 255;
            }
        }
        let (native_w, native_h) = io::dimensions(&paths[index])?;
        masks.push(Some(proxy.resized(native_w, native_h)));
        moved_frames.push(index);
    }
    Ok(DeghostResult {
        masks,
        moved_frames,
    })
}

/// A frame's linear luminance at proxy size.
struct Probes {
    width: u32,
    height: u32,
    planes: Vec<Vec<f32>>,
}

fn load_probes(paths: &[PathBuf], probe_edge: u32, progress: &dyn Fn(u32, u32)) -> Result<Probes> {
    let mut planes = Vec::with_capacity(paths.len());
    let mut size: Option<(u32, u32)> = None;
    let total = paths.len() as u32;
    for (index, path) in paths.iter().enumerate() {
        let image = io::load(path, InputEncoding::Auto)?.fit_within(probe_edge);
        let current = (image.width, image.height);
        match size {
            None => size = Some(current),
            Some(expected) if expected != current => {
                return Err(crate::HdrError::SizeMismatch {
                    path: path.clone(),
                    want_w: expected.0,
                    want_h: expected.1,
                    got_w: current.0,
                    got_h: current.1,
                })
            }
            _ => {}
        }
        planes.push(luminance_plane(&image));
        progress(index as u32 + 1, total);
    }
    let (width, height) = size.ok_or(crate::HdrError::NoFrames)?;
    Ok(Probes {
        width,
        height,
        planes,
    })
}

fn luminance_plane(image: &LinearImage) -> Vec<f32> {
    image
        .data
        .chunks_exact(3)
        .map(|pixel| crate::color::luminance([pixel[0], pixel[1], pixel[2]]))
        .collect()
}

/// Median value of every pixel across the whole set.
fn temporal_median(planes: &[Vec<f32>]) -> Vec<f32> {
    let Some(first) = planes.first() else {
        return Vec::new();
    };
    let mut values = Vec::with_capacity(planes.len());
    let mut medians = vec![0.0; first.len()];
    for (pixel, median) in medians.iter_mut().enumerate() {
        values.clear();
        values.extend(planes.iter().map(|plane| plane[pixel]));
        values.sort_unstable_by(f32::total_cmp);
        *median = median_of(&values);
    }
    medians
}

fn median_of(sorted: &[f32]) -> f32 {
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 0 && mid > 0 {
        (sorted[mid - 1] + sorted[mid]) * 0.5
    } else {
        sorted[mid]
    }
}

fn candidate_bitmap(luma: &[f32], medians: &[f32], threshold: f32) -> Vec<bool> {
    luma.iter()
        .zip(medians)
        .map(|(value, median)| (value - median).abs() > threshold && *median > MEDIAN_FLOOR)
        .collect()
}

/// Apply `op` to the in-bounds 3x3 neighbourhood of every pixel. Out-of-bounds
/// neighbours are skipped, so border pixels are not vetoed by the frame edge.
fn neighbourhood(
    bitmap: &[bool],
    width: u32,
    height: u32,
    op: impl Fn(&[bool]) -> bool,
) -> Vec<bool> {
    let cols = width as i32;
    let rows = height as i32;
    let mut out = vec![false; bitmap.len()];
    let mut values = [false; 9];
    for y in 0..rows {
        for x in 0..cols {
            let mut count = 0;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= cols || ny >= rows {
                        continue;
                    }
                    values[count] = bitmap[ny as usize * width as usize + nx as usize];
                    count += 1;
                }
            }
            out[y as usize * width as usize + x as usize] = op(&values[..count]);
        }
    }
    out
}

fn erode(bitmap: &[bool], width: u32, height: u32) -> Vec<bool> {
    neighbourhood(bitmap, width, height, |values| values.iter().all(|v| *v))
}

fn dilate(bitmap: &[bool], width: u32, height: u32) -> Vec<bool> {
    neighbourhood(bitmap, width, height, |values| values.iter().any(|v| *v))
}

/// Remove 4-connected components smaller than [`MIN_COMPONENT_FRACTION`].
/// Returns how many pixels survived.
fn drop_small_components(bitmap: &mut [bool], width: u32, height: u32) -> usize {
    let cols = width as usize;
    let rows = height as usize;
    let min_area = ((bitmap.len() as f32 * MIN_COMPONENT_FRACTION).ceil() as usize).max(1);
    let mut visited = vec![false; bitmap.len()];
    let mut stack: Vec<usize> = Vec::new();
    let mut component: Vec<usize> = Vec::new();
    let mut kept = 0;
    for start in 0..bitmap.len() {
        if !bitmap[start] || visited[start] {
            continue;
        }
        stack.push(start);
        visited[start] = true;
        component.clear();
        while let Some(index) = stack.pop() {
            component.push(index);
            let (x, y) = (index % cols, index / cols);
            let neighbours = [
                (x > 0).then(|| index - 1),
                (x + 1 < cols).then(|| index + 1),
                (y > 0).then(|| index - cols),
                (y + 1 < rows).then(|| index + cols),
            ];
            for next in neighbours.into_iter().flatten() {
                if bitmap[next] && !visited[next] {
                    visited[next] = true;
                    stack.push(next);
                }
            }
        }
        if component.len() < min_area {
            for &index in &component {
                bitmap[index] = false;
            }
        } else {
            kept += component.len();
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tonemap::{Curve, ToneMapOptions};
    use std::fs;
    use std::path::Path;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hdr-core-deghost-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn linear_tonemap() -> ToneMapOptions {
        ToneMapOptions {
            curve: Curve::None,
            srgb: false,
            exposure_ev: 0.0,
            ..Default::default()
        }
    }

    fn blob(image: &mut LinearImage, cx: u32, cy: u32, half: u32, rgb: [f32; 3]) {
        for y in cy.saturating_sub(half)..=(cy + half).min(image.height - 1) {
            for x in cx.saturating_sub(half)..=(cx + half).min(image.width - 1) {
                image.set_pixel(x, y, rgb);
            }
        }
    }

    fn write_set(dir: &Path, images: &[LinearImage]) -> Vec<PathBuf> {
        images
            .iter()
            .enumerate()
            .map(|(index, image)| {
                let path = dir.join(format!("frame_{index}.tif"));
                io::save(&path, image, &linear_tonemap()).unwrap();
                path
            })
            .collect()
    }

    #[test]
    fn blob_frame_is_masked_and_clean_frames_are_not() {
        let dir = temp_dir("blob");
        let base = LinearImage::filled(64, 64, [0.1, 0.1, 0.1]);
        let mut mover = base.clone();
        blob(&mut mover, 32, 32, 8, [0.9, 0.9, 0.9]);
        let paths = write_set(
            &dir,
            &[
                base.clone(),
                base.clone(),
                mover,
                base.clone(),
                base.clone(),
            ],
        );

        let result = detect_movers(&paths, &DeghostOptions::default(), &|_, _| {}).unwrap();
        assert_eq!(result.moved_frames, vec![2]);
        for index in [0, 1, 3, 4] {
            assert!(result.masks[index].is_none(), "frame {index} is clean");
        }
        let mask = result.masks[2].as_ref().expect("blob frame must be masked");
        assert!(mask.excluded(32 * 64 + 32), "blob centre must be masked");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn identical_frames_produce_no_masks() {
        let dir = temp_dir("identical");
        let base = LinearImage::filled(48, 32, [0.2, 0.25, 0.3]);
        let paths = write_set(&dir, &[base.clone(), base.clone(), base.clone()]);

        let result = detect_movers(&paths, &DeghostOptions::default(), &|_, _| {}).unwrap();
        assert!(result.moved_frames.is_empty());
        assert!(result.masks.iter().all(Option::is_none));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn threshold_bounds_turn_masking_off_and_on() {
        let dir = temp_dir("threshold");
        let base = LinearImage::filled(64, 64, [0.1, 0.1, 0.1]);
        let mut mover = base.clone();
        blob(&mut mover, 32, 32, 5, [0.9, 0.9, 0.9]);
        let paths = write_set(
            &dir,
            &[base.clone(), mover, base.clone(), base.clone(), base],
        );

        let maximum = DeghostOptions {
            threshold: 1.0,
            ..Default::default()
        };
        let quiet = detect_movers(&paths, &maximum, &|_, _| {}).unwrap();
        assert!(quiet.moved_frames.is_empty(), "1.0 disables detection");

        let sensitive = DeghostOptions {
            threshold: 0.02,
            ..Default::default()
        };
        let loud = detect_movers(&paths, &sensitive, &|_, _| {}).unwrap();
        assert_eq!(loud.moved_frames, vec![1]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn mask_matches_native_frame_resolution() {
        let dir = temp_dir("native");
        let (width, height) = (200u32, 120u32);
        let base = LinearImage::filled(width, height, [0.1, 0.1, 0.1]);
        let mut mover = base.clone();
        blob(&mut mover, width / 2, height / 2, 10, [0.9, 0.9, 0.9]);
        let paths = write_set(
            &dir,
            &[
                base.clone(),
                base.clone(),
                mover,
                base.clone(),
                base.clone(),
            ],
        );

        let options = DeghostOptions {
            probe_edge: 64,
            ..Default::default()
        };
        let result = detect_movers(&paths, &options, &|_, _| {}).unwrap();
        let mask = result.masks[2].as_ref().expect("blob frame must be masked");
        assert_eq!((mask.width, mask.height), (width, height));
        assert!(mask.excluded((height / 2 * width + width / 2) as usize));
        let _ = fs::remove_dir_all(&dir);
    }
}
