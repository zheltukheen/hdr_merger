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
    /// Reject a frame whose estimated shift is longer than this many pixels
    /// (`|dx| + |dy|`). `None` accepts any drift.
    #[serde(default)]
    pub max_shift: Option<u32>,
    /// Crop the result to the area that is valid in every frame.
    #[serde(default = "default_crop_to_intersection")]
    pub crop_to_intersection: bool,
    /// Use the wgpu stacker instead of the CPU one. Off by default: measured
    /// slower than the CPU path on 43-frame 60 MP sets (ADR 0002). Env
    /// `HDR_MERGER_GPU=1` forces it on.
    #[serde(default)]
    pub use_gpu: bool,
}

fn default_crop_to_intersection() -> bool {
    true
}

impl Default for MergeOptions {
    fn default() -> Self {
        Self {
            method: StackMethod::Median,
            align: true,
            reference: None,
            max_dim: None,
            input_encoding: InputEncoding::Auto,
            max_shift: None,
            crop_to_intersection: true,
            use_gpu: false,
        }
    }
}

/// Where the wall clock went. Reported by the CLI and shown in the UI, because
/// "the stack is slow" is usually not the stack — it is the decode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MergeTimings {
    pub decode_ms: u64,
    pub cache_ms: u64,
    pub stack_ms: u64,
    pub total_ms: u64,
    /// True when the GPU path actually ran (not just when it was requested).
    pub gpu: bool,
}

impl MergeTimings {
    /// Share of the total spent before the stacker even starts.
    pub fn decode_share(&self) -> f32 {
        if self.total_ms == 0 {
            return 0.0;
        }
        ((self.decode_ms + self.cache_ms) as f32 / self.total_ms as f32).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Debug)]
pub struct MergeOutput {
    pub image: LinearImage,
    /// Per input frame, in input order.
    pub shifts: Vec<Shift>,
    pub warnings: Vec<String>,
    pub timings: MergeTimings,
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

    let started = std::time::Instant::now();
    let mut decode_ns: u128 = 0;
    let mut cache_ns: u128 = 0;
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
        let decode_started = std::time::Instant::now();
        let mut image = io::load(&frame.path, options.input_encoding)?;
        decode_ns += decode_started.elapsed().as_nanos();
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
        if let Some(limit) = options.max_shift {
            if shift.magnitude().unsigned_abs() > limit {
                return Err(HdrError::ShiftTooLarge {
                    path: frame.path.clone(),
                    dx: shift.dx,
                    dy: shift.dy,
                    limit,
                });
            }
        }
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
        let cache_started = std::time::Instant::now();
        write_cache(&path, &image, shift)?;
        cache_ns += cache_started.elapsed().as_nanos();
        cache_paths[index] = Some(path);
        shifts[index] = shift;
        progress(step as u32 + 1, total);
    }

    let (width, height) = size.ok_or(HdrError::NoFrames)?;
    let stack_started = std::time::Instant::now();
    let (image, used_gpu) = stack_cached(
        &cache_paths,
        &masks,
        width,
        height,
        options.method,
        options.use_gpu,
        progress,
    )?;
    let stack_ns = stack_started.elapsed().as_nanos();
    let image = if options.crop_to_intersection {
        let (x0, y0, w, h) = intersection(&shifts, width, height)
            .ok_or_else(|| HdrError::Engine("crop left nothing: shifts exceed the frame".into()))?;
        // The common case: identity shifts leave the frame untouched, don't
        // copy 60 MP for nothing.
        if (x0, y0, w, h) == (0, 0, width, height) {
            image
        } else {
            crop(&image, x0, y0, w, h)
        }
    } else {
        image
    };
    let timings = MergeTimings {
        decode_ms: (decode_ns / 1_000_000) as u64,
        cache_ms: (cache_ns / 1_000_000) as u64,
        stack_ms: (stack_ns / 1_000_000) as u64,
        total_ms: started.elapsed().as_millis() as u64,
        gpu: used_gpu,
    };
    Ok(MergeOutput {
        image,
        shifts,
        warnings,
        timings,
    })
}

/// Region of the stacked frame that every input frame can fill.
///
/// The cache writer samples `x + dx` from a frame, so output column `x` is
/// valid while `0 <= x + dx < width`; the same holds for rows. Returns
/// `(x0, y0, width, height)` or `None` when the rectangles do not overlap.
fn intersection(shifts: &[Shift], width: u32, height: u32) -> Option<(u32, u32, u32, u32)> {
    let mut left = 0i64;
    let mut top = 0i64;
    let mut right = i64::from(width);
    let mut bottom = i64::from(height);
    for shift in shifts {
        left = left.max(-i64::from(shift.dx));
        right = right.min(i64::from(width) - i64::from(shift.dx));
        top = top.max(-i64::from(shift.dy));
        bottom = bottom.min(i64::from(height) - i64::from(shift.dy));
    }
    if right <= left || bottom <= top {
        return None;
    }
    Some((
        left as u32,
        top as u32,
        (right - left) as u32,
        (bottom - top) as u32,
    ))
}

/// Copy one rectangle out of `image`, re-packed with no padding.
fn crop(image: &LinearImage, x0: u32, y0: u32, width: u32, height: u32) -> LinearImage {
    let mut out = LinearImage::new(width, height);
    let row_values = width as usize * 3;
    for row in 0..height as usize {
        let source = (y0 as usize + row) * image.width as usize * 3 + x0 as usize * 3;
        let target = row * row_values;
        out.data[target..target + row_values]
            .copy_from_slice(&image.data[source..source + row_values]);
    }
    out
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
    use_gpu: bool,
    progress: &dyn Fn(u32, u32),
) -> Result<(LinearImage, bool)> {
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
    let row_pixels = width as usize;

    // A set with any painted mask stays on the CPU: shipping 30 MB of mask
    // indices per strip to save a few hundred ms is not a trade worth making.
    #[cfg(feature = "gpu")]
    let gpu = if (use_gpu || crate::gpu::enabled_by_env()) && masks.iter().all(Option::is_none) {
        crate::gpu::GpuStacker::new()
    } else {
        None
    };
    #[cfg(not(feature = "gpu"))]
    let gpu: Option<()> = None;
    let mut used_gpu = false;

    // A GPU strip has to fit one storage binding (128 MB on downlevel adapters),
    // so the tile shrinks when the stacker is enabled: 64 rows of a 5496x3 f16
    // strip times 64 frames is ~91 MB.
    let strip_rows = if gpu.is_some() { 64 } else { STRIP_ROWS };
    let strips = height.div_ceil(strip_rows);

    for (strip, y0) in (0..height).step_by(strip_rows as usize).enumerate() {
        let rows = (height - y0).min(strip_rows) as usize;
        let strip_bytes = rows * row_bytes;
        for (index, reader) in readers.iter_mut().enumerate() {
            let buffer = &mut buffers[index];
            buffer.resize(strip_bytes, 0);
            reader.seek(SeekFrom::Start(y0 as u64 * row_bytes as u64))?;
            reader.read_exact(buffer)?;
        }

        let start = y0 as usize * row_pixels * 3;
        let end = start + rows * row_pixels * 3;

        #[cfg(feature = "gpu")]
        if let Some(stacker) = &gpu {
            if count <= crate::gpu::MAX_FRAMES {
                let mut packed = Vec::with_capacity(count * strip_bytes);
                for buffer in &buffers {
                    packed.extend_from_slice(buffer);
                }
                let pixels = rows * row_pixels;
                let (code, param) = method.gpu_code();
                if let Some(values) = stacker.stack(&packed, count, pixels, code, param) {
                    out.data[start..end].copy_from_slice(&values);
                    used_gpu = true;
                    progress(strip as u32 + 1, strips);
                    continue;
                }
            }
        }

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
                            method.pick_unsorted(&mut values)
                        };
                    }
                }
            });
        progress(strip as u32 + 1, strips);
    }
    Ok((out, used_gpu))
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

    /// Deterministic texture: a periodic pattern would make the correlation
    /// ambiguous and the test would be about luck, not about the crop.
    fn textured(width: u32, height: u32) -> LinearImage {
        let mut seed = 0x2468_ace1u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((seed >> 8) & 0xffff) as f32 / 65535.0
        };
        let mut image = LinearImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let value = next();
                image.set_pixel(x, y, [value, value * 0.5, value * 0.2]);
            }
        }
        image
    }

    /// Content offset: the pixel that sits at `(x, y)` in `image` is written
    /// at `(x + dx, y + dy)`.
    fn shifted_by(image: &LinearImage, dx: i32, dy: i32) -> LinearImage {
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

    fn two_frames(dir: &Path, base: LinearImage, moved: LinearImage) -> Vec<PathBuf> {
        let a = dir.join("a.tif");
        let b = dir.join("b.tif");
        io::save(&a, &base, &linear_tonemap()).unwrap();
        io::save(&b, &moved, &linear_tonemap()).unwrap();
        vec![a, b]
    }

    #[test]
    fn max_shift_rejects_a_frame_that_drifted_too_far() {
        let dir = temp_dir("max-shift");
        let base = textured(64, 64);
        let moved = shifted_by(&base, 6, -4);
        let paths = two_frames(&dir, base, moved);

        let options = MergeOptions {
            method: StackMethod::Mean,
            max_shift: Some(5),
            ..Default::default()
        };
        let error = merge_paths(&paths, &options, &|_, _| {}).unwrap_err();
        match error {
            HdrError::ShiftTooLarge {
                path,
                dx,
                dy,
                limit,
            } => {
                assert_eq!((dx, dy, limit), (6, -4, 5));
                assert_eq!(path.file_name().unwrap(), "b.tif");
            }
            other => panic!("expected ShiftTooLarge, got {other:?}"),
        }

        // The same pair is accepted once the guard is generous enough.
        let relaxed = MergeOptions {
            max_shift: Some(50),
            ..options
        };
        assert!(merge_paths(&paths, &relaxed, &|_, _| {}).is_ok());
    }

    #[test]
    fn crop_keeps_the_region_valid_in_every_frame() {
        let dir = temp_dir("crop");
        let (w, h) = (64u32, 64u32);
        let base = textured(w, h);
        let moved = shifted_by(&base, 3, 2);
        let paths = two_frames(&dir, base.clone(), moved);

        let options = MergeOptions {
            method: StackMethod::Mean,
            ..Default::default()
        };
        let merged = merge_paths(&paths, &options, &|_, _| {}).unwrap();
        assert_eq!((merged.shifts[1].dx, merged.shifts[1].dy), (3, 2));

        // Shift (3, 2) leaves the last three columns and two rows unreachable.
        assert_eq!((merged.image.width, merged.image.height), (w - 3, h - 2));
        assert_eq!(merged.image.data.len() as u32, (w - 3) * (h - 2) * 3);

        // The crop origin is the reference frame's own (0, 0): the kept
        // pixels are the unshifted ones, not the shifted copy.
        for (x, y) in [(0, 0), (31, 30), (w - 4, h - 3)] {
            let got = merged.image.pixel(x, y);
            let want = base.pixel(x, y);
            assert!(
                (got[0] - want[0]).abs() < 2e-3,
                "pixel {x},{y}: {got:?} vs {want:?}"
            );
        }
    }

    #[test]
    fn crop_can_be_disabled() {
        let dir = temp_dir("no-crop");
        let (w, h) = (64u32, 64u32);
        let base = textured(w, h);
        let moved = shifted_by(&base, 3, 2);
        let paths = two_frames(&dir, base, moved);

        let options = MergeOptions {
            method: StackMethod::Mean,
            crop_to_intersection: false,
            ..Default::default()
        };
        let merged = merge_paths(&paths, &options, &|_, _| {}).unwrap();
        assert_eq!((merged.image.width, merged.image.height), (w, h));
        assert_eq!(merged.image.data.len() as u32, w * h * 3);
        assert_eq!((merged.shifts[1].dx, merged.shifts[1].dy), (3, 2));
    }
}
