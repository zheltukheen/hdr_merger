//! The merge pipeline: decode -> align -> cache -> stack.
//!
//! Streaming shape: a 43-frame 60 MP set is ~31 GB of f32 in RAM, so frames are
//! decoded one at a time into a temporary f16 cache and the stack runs strip by
//! strip. f16 keeps the pipeline honest about highlights (no 1.0 clamp) while
//! halving the disk footprint.

use std::fs::{self, File};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use half::f16;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::align::{self, Shift};
use crate::error::{HdrError, Result};
use crate::image::LinearImage;
use crate::io::{self, InputEncoding};
use crate::mask::Mask;
use crate::stack::StackMethod;

/// Rows processed per strip. Big enough to amortise syscalls, small enough to
/// stay cache friendly and keep the per-strip buffers bounded.
const STRIP_ROWS: u32 = 128;

/// Longest edge used for the alignment probe. Correlation is O(n log n), no
/// point running it at full resolution.
const ALIGN_PROBE: u32 = 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct FrameInput {
    pub path: PathBuf,
    /// Pixels masked here are skipped for this frame only.
    pub mask: Option<Mask>,
}

impl FrameInput {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            mask: None,
        }
    }

    pub fn with_mask(path: impl Into<PathBuf>, mask: Mask) -> Self {
        Self {
            path: path.into(),
            mask: Some(mask),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MergeOptions {
    pub method: StackMethod,
    pub align: bool,
    /// Frame used as the alignment anchor. Defaults to the first one.
    pub reference: Option<usize>,
    /// Longest edge of the result. `None` keeps full resolution.
    pub max_dim: Option<u32>,
    pub input_encoding: InputEncoding,
}

impl Default for MergeOptions {
    fn default() -> Self {
        Self {
            method: StackMethod::Median,
            align: true,
            reference: None,
            max_dim: None,
            input_encoding: InputEncoding::Auto,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MergeOutput {
    pub image: LinearImage,
    /// Per input frame, in input order.
    pub shifts: Vec<Shift>,
    pub warnings: Vec<String>,
}

/// Merge `frames`. `progress(done, total)` is called once per decoded frame
/// and once per stacked strip.
pub fn merge(
    frames: &[FrameInput],
    options: &MergeOptions,
    progress: &dyn Fn(u32, u32),
) -> Result<MergeOutput> {
    if frames.is_empty() {
        return Err(HdrError::NoFrames);
    }
    let reference = options.reference.unwrap_or(0).min(frames.len() - 1);
    let mut order: Vec<usize> = (0..frames.len()).filter(|i| *i != reference).collect();
    order.insert(0, reference);

    let cache = Cache::new()?;
    let mut shifts = vec![Shift::identity(); frames.len()];
    let mut masks: Vec<Option<Mask>> = vec![None; frames.len()];
    let mut cache_paths: Vec<Option<PathBuf>> = vec![None; frames.len()];
    let mut size: Option<(u32, u32)> = None;
    let mut reference_probe: Option<LinearImage> = None;
    let mut warnings = Vec::new();
    let total = frames.len() as u32;

    for (step, &index) in order.iter().enumerate() {
        let frame = &frames[index];
        let mut image = io::load(&frame.path, options.input_encoding)?;
        if let Some(max_dim) = options.max_dim {
            image = image.fit_within(max_dim);
        }
        let current = (image.width, image.height);
        match size {
            None => size = Some(current),
            Some(expected) if expected != current => {
                return Err(HdrError::SizeMismatch {
                    path: frame.path.clone(),
                    want_w: expected.0,
                    want_h: expected.1,
                    got_w: current.0,
                    got_h: current.1,
                })
            }
            _ => {}
        }

        let shift = if options.align {
            let probe = image.fit_within(ALIGN_PROBE);
            if index == reference {
                reference_probe = Some(probe);
                Shift::identity()
            } else {
                let anchor = reference_probe.as_ref().ok_or_else(|| {
                    HdrError::Engine("alignment reference not initialised".into())
                })?;
                let measured = align::estimate_shift(anchor, &probe)?;
                let scale = image.width as f32 / probe.width as f32;
                Shift {
                    dx: (measured.dx as f32 * scale).round() as i32,
                    dy: (measured.dy as f32 * scale).round() as i32,
                    confidence: measured.confidence,
                }
            }
        } else {
            Shift::identity()
        };
        if shift.magnitude() > 2 {
            warnings.push(format!(
                "{}: shift {:+.0},{:+.0} px (confidence {:.1})",
                frame.path.file_name().unwrap_or_default().to_string_lossy(),
                shift.dx,
                shift.dy,
                shift.confidence
            ));
        }

        masks[index] = frame.mask.as_ref().map(|mask| {
            if (mask.width, mask.height) == current {
                mask.clone()
            } else {
                mask.resized(current.0, current.1)
            }
        });
        let path = cache.path(index);
        write_cache(&path, &image, shift)?;
        cache_paths[index] = Some(path);
        shifts[index] = shift;
        progress(step as u32 + 1, total);
    }

    let (width, height) = size.ok_or(HdrError::NoFrames)?;
    let image = stack_cached(
        &cache_paths,
        &masks,
        width,
        height,
        options.method,
        progress,
    )?;
    Ok(MergeOutput {
        image,
        shifts,
        warnings,
    })
}

/// Convenience for callers that only have paths.
pub fn merge_paths(
    paths: &[PathBuf],
    options: &MergeOptions,
    progress: &dyn Fn(u32, u32),
) -> Result<MergeOutput> {
    let frames: Vec<FrameInput> = paths.iter().cloned().map(FrameInput::new).collect();
    merge(&frames, options, progress)
}

fn write_cache(path: &Path, image: &LinearImage, shift: Shift) -> Result<()> {
    let width = image.width as i32;
    let height = image.height as i32;
    let mut writer = BufWriter::with_capacity(1 << 20, File::create(path)?);
    let mut row: Vec<u8> = Vec::with_capacity(image.width as usize * 3 * 2);
    for y in 0..height {
        row.clear();
        for x in 0..width {
            let source = if shift.magnitude() == 0 {
                image.pixel(x as u32, y as u32)
            } else {
                // `shift` is the content offset: content at x lives at x+dx.
                let sx = (x + shift.dx).clamp(0, width - 1) as u32;
                let sy = (y + shift.dy).clamp(0, height - 1) as u32;
                image.pixel(sx, sy)
            };
            for channel in source {
                row.extend_from_slice(&f16::from_f32(channel).to_bits().to_le_bytes());
            }
        }
        writer.write_all(&row)?;
    }
    writer.flush()?;
    Ok(())
}

fn stack_cached(
    paths: &[Option<PathBuf>],
    masks: &[Option<Mask>],
    width: u32,
    height: u32,
    method: StackMethod,
    progress: &dyn Fn(u32, u32),
) -> Result<LinearImage> {
    let count = paths.len();
    let row_bytes = width as usize * 3 * 2;
    let mut readers: Vec<File> = Vec::with_capacity(count);
    for path in paths {
        let path = path
            .as_ref()
            .ok_or_else(|| HdrError::Engine("cache entry missing".into()))?;
        readers.push(File::open(path)?);
    }
    let mut buffers: Vec<Vec<u8>> = vec![Vec::new(); count];
    let mut out = LinearImage::new(width, height);
    let strips = height.div_ceil(STRIP_ROWS);
    let row_pixels = width as usize;

    for (strip, y0) in (0..height).step_by(STRIP_ROWS as usize).enumerate() {
        let rows = (height - y0).min(STRIP_ROWS) as usize;
        let strip_bytes = rows * row_bytes;
        for (index, reader) in readers.iter_mut().enumerate() {
            let buffer = &mut buffers[index];
            buffer.resize(strip_bytes, 0);
            reader.seek(SeekFrom::Start(y0 as u64 * row_bytes as u64))?;
            reader.read_exact(buffer)?;
        }

        let start = y0 as usize * row_pixels * 3;
        let end = start + rows * row_pixels * 3;
        out.data[start..end]
            .par_chunks_mut(row_pixels * 3)
            .enumerate()
            .for_each(|(row, row_out)| {
                let y = y0 as usize + row;
                let mut values: Vec<f32> = Vec::with_capacity(count);
                for x in 0..row_pixels {
                    let mask_index = y * row_pixels + x;
                    for channel in 0..3 {
                        values.clear();
                        for (index, buffer) in buffers.iter().enumerate() {
                            if let Some(mask) = &masks[index] {
                                if mask.excluded(mask_index) {
                                    continue;
                                }
                            }
                            let offset = (row * row_pixels + x) * 6 + channel * 2;
                            let bits = u16::from_le_bytes([buffer[offset], buffer[offset + 1]]);
                            values.push(f16::from_bits(bits).to_f32());
                        }
                        row_out[x * 3 + channel] = if values.is_empty() {
                            0.0
                        } else {
                            values.sort_by(f32::total_cmp);
                            method.pick_sorted(&values)
                        };
                    }
                }
            });
        progress(strip as u32 + 1, strips);
    }
    Ok(out)
}

/// Temporary f16 frame cache, removed on drop.
struct Cache {
    dir: PathBuf,
}

impl Cache {
    fn new() -> Result<Self> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "hdr-merger-{}-{}-{}",
            std::process::id(),
            nanos,
            unique
        ));
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    fn path(&self, index: usize) -> PathBuf {
        self.dir.join(format!("frame_{index:05}.f16"))
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tonemap::ToneMapOptions;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hdr-core-pipeline-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn linear_tonemap() -> ToneMapOptions {
        ToneMapOptions {
            curve: crate::tonemap::Curve::None,
            srgb: false,
            exposure_ev: 0.0,
        }
    }

    fn write_frames(dir: &Path, frames: &[Vec<[f32; 3]>], width: u32, height: u32) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        for (index, pixels) in frames.iter().enumerate() {
            let mut image = LinearImage::new(width, height);
            image.data = pixels.iter().flat_map(|p| p.iter().copied()).collect();
            let path = dir.join(format!("frame_{index}.tif"));
            io::save(&path, &image, &linear_tonemap()).unwrap();
            paths.push(path);
        }
        paths
    }

    #[test]
    fn median_ignores_a_single_bright_intruder() {
        let dir = temp_dir("intruder");
        let w = 4;
        let h = 1;
        let base = vec![[0.2, 0.2, 0.2]; (w * h) as usize];
        let mut spike = base.clone();
        spike[1] = [1.0, 1.0, 1.0];
        let paths = write_frames(&dir, &[base.clone(), spike, base.clone()], w, h);

        let options = MergeOptions {
            align: false,
            method: StackMethod::Median,
            ..Default::default()
        };
        let merged = merge_paths(&paths, &options, &|_, _| {}).unwrap();
        let pixel = merged.image.pixel(1, 0);
        assert!(
            (pixel[0] - 0.2).abs() < 2e-3,
            "median kept the intruder: {pixel:?}"
        );

        let options_max = MergeOptions {
            align: false,
            method: StackMethod::Max,
            ..Default::default()
        };
        let bright = merge_paths(&paths, &options_max, &|_, _| {}).unwrap();
        assert!((bright.image.pixel(1, 0)[0] - 1.0).abs() < 2e-3);
    }

    #[test]
    fn mask_falls_through_to_other_frames() {
        let dir = temp_dir("mask");
        let (w, h) = (8u32, 1u32);
        let dim = vec![[0.1, 0.1, 0.1]; w as usize];
        let bright = vec![[0.9, 0.9, 0.9]; w as usize];
        let paths = write_frames(&dir, &[dim.clone(), bright], w, h);

        let mut mask = Mask::new(w, h);
        mask.paint_circle(4.0, 0.0, 2.0, 0.0, 255);
        let frames = vec![
            FrameInput::with_mask(&paths[0], mask.clone()),
            FrameInput::new(&paths[1]),
        ];
        let options = MergeOptions {
            align: false,
            method: StackMethod::Max,
            ..Default::default()
        };
        let merged = merge(&frames, &options, &|_, _| {}).unwrap();
        // masked-out pixels: only the bright frame contributes
        assert!((merged.image.pixel(4, 0)[0] - 0.9).abs() < 2e-3);
    }

    #[test]
    fn empty_input_is_rejected() {
        assert!(merge(&[], &MergeOptions::default(), &|_, _| {}).is_err());
    }

    #[test]
    fn progress_reaches_one() {
        let dir = temp_dir("progress");
        let image = LinearImage::filled(8, 8, [0.3, 0.3, 0.3]);
        let path = dir.join("one.tif");
        io::save(&path, &image, &linear_tonemap()).unwrap();
        let seen = std::sync::atomic::AtomicU32::new(0);
        merge_paths(
            &[path],
            &MergeOptions {
                align: false,
                ..Default::default()
            },
            &|done, total| {
                assert!(done <= total);
                seen.fetch_max(done, Ordering::Relaxed);
            },
        )
        .unwrap();
        assert!(seen.load(Ordering::Relaxed) > 0);
    }
}
