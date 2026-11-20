//! Decoding and encoding. The only place that knows about file formats.
//!
//! Encoding contract (documented, not guessed):
//!   * 8-bit sources are treated as sRGB display values   -> decoded to linear
//!   * 16-bit sources are treated as linear light          -> scaled by 1/65535
//!   * 32-bit float / EXR sources are linear already
//!
//! `InputEncoding` overrides that inference per run.

use std::collections::hash_map::DefaultHasher;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::BufWriter;
use std::path::Path;

use ::image::{DynamicImage, ImageBuffer, Rgb};

use crate::color;
use crate::error::{HdrError, Result};
use crate::image::LinearImage;
use crate::tonemap::{to_display16, to_display8, ToneMapOptions};

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum InputEncoding {
    /// Pick from the bit depth (see module docs).
    #[default]
    Auto,
    Srgb,
    Linear,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    Exr,
    Tiff16,
    Png8,
    Jpg8,
}

pub fn output_format(path: &Path) -> OutputFormat {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("tif") | Some("tiff") => OutputFormat::Tiff16,
        Some("png") => OutputFormat::Png8,
        Some("jpg") | Some("jpeg") => OutputFormat::Jpg8,
        _ => OutputFormat::Exr,
    }
}

/// Cheap header-only size probe, used by the UI to size masks without
/// decoding the whole frame.
pub fn dimensions(path: &Path) -> Result<(u32, u32)> {
    ::image::image_dimensions(path).map_err(|e| HdrError::Decode {
        path: path.to_path_buf(),
        source: e,
    })
}

/// Camera RAW extensions we can hand to an external decoder.
const RAW_EXTENSIONS: &[&str] = &[
    "cr2", "cr3", "nef", "nrw", "arw", "srf", "sr2", "dng", "raf", "orf", "rw2", "pef", "srw",
    "raw",
];

pub fn is_raw(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| RAW_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

pub fn load(path: &Path, encoding: InputEncoding) -> Result<LinearImage> {
    let source = if is_raw(path) {
        raw_to_tiff(path)?
    } else {
        path.to_path_buf()
    };
    let decoded = ::image::open(&source).map_err(|e| HdrError::Decode {
        path: source.clone(),
        source: e,
    })?;
    Ok(convert(decoded, encoding))
}

/// Convert a RAW file to a 16-bit linear TIFF with an external decoder and
/// cache the result next to the system temp dir, keyed by path, size and
/// modification time.
///
/// We do not bundle a RAW pipeline: LibRaw (via `dcraw`/`dcraw_emu`) or
/// RawTherapee already do demosaic, white balance and colour, and re-doing that
/// badly would be worse than calling the tool the user already has.
fn raw_to_tiff(path: &Path) -> Result<std::path::PathBuf> {
    let metadata = std::fs::metadata(path)?;
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .hash(&mut hasher);
    let key = format!("{:016x}", hasher.finish());

    let cache_dir = std::env::temp_dir().join("hdr-merger-raw");
    std::fs::create_dir_all(&cache_dir)?;
    let cached = cache_dir.join(format!("{key}.tif"));
    if cached.exists() {
        return Ok(cached);
    }

    let (converter, binary) = find_raw_converter().ok_or_else(|| {
        HdrError::Engine(format!(
            "no RAW converter found for {}: install LibRaw (dcraw_emu / dcraw) or rawtherapee-cli",
            path.display()
        ))
    })?;
    let work = cache_dir.join(format!("{key}-work"));
    std::fs::create_dir_all(&work)?;

    let status = match converter {
        RawConverter::Dcraw => std::process::Command::new(&binary)
            .args(["-4", "-T", "-w", "-o", "1", "-q", "3"])
            .arg(path)
            .current_dir(&work)
            .status(),
        RawConverter::RawTherapee => std::process::Command::new(&binary)
            .args(["-t", "-Y", "-c"])
            .arg(path)
            .current_dir(&work)
            .arg("-o")
            .arg(&cached)
            .status(),
    }
    .map_err(|error| HdrError::Engine(format!("cannot run {}: {error}", binary.display())))?;
    if !status.success() {
        return Err(HdrError::Engine(format!(
            "{} failed to decode {}",
            binary.display(),
            path.display()
        )));
    }

    // dcraw writes <stem>.tiff into the working directory, dcraw_emu writes it
    // next to the input. Accept either and move it into the cache.
    let produced = work
        .join(format!(
            "{}.tiff",
            path.file_stem().unwrap_or_default().to_string_lossy()
        ))
        .exists()
        .then(|| {
            work.join(format!(
                "{}.tiff",
                path.file_stem().unwrap_or_default().to_string_lossy()
            ))
        })
        .or_else(|| {
            let sibling = std::path::PathBuf::from(format!("{}.tiff", path.display()));
            sibling.exists().then_some(sibling)
        });
    let produced = produced.ok_or_else(|| {
        HdrError::Engine(format!(
            "{} produced no TIFF for {}",
            binary.display(),
            path.display()
        ))
    })?;
    std::fs::rename(&produced, &cached)?;
    let _ = std::fs::remove_dir_all(&work);
    Ok(cached)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RawConverter {
    Dcraw,
    RawTherapee,
}

fn find_raw_converter() -> Option<(RawConverter, std::path::PathBuf)> {
    let candidates = [
        ("dcraw_emu", RawConverter::Dcraw),
        ("dcraw", RawConverter::Dcraw),
        ("rawtherapee-cli", RawConverter::RawTherapee),
    ];
    for (name, converter) in candidates {
        if let Some(path) = find_in_path(name) {
            return Some((converter, path));
        }
    }
    None
}

fn find_in_path(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

fn convert(decoded: DynamicImage, encoding: InputEncoding) -> LinearImage {
    let auto_srgb = |encoding: InputEncoding| match encoding {
        InputEncoding::Srgb => true,
        InputEncoding::Linear => false,
        InputEncoding::Auto => true,
    };
    let auto_linear = |encoding: InputEncoding| match encoding {
        InputEncoding::Srgb => true,
        InputEncoding::Linear => false,
        InputEncoding::Auto => false,
    };

    match decoded {
        DynamicImage::ImageRgb8(buf) => from_u8(
            buf.width(),
            buf.height(),
            buf.as_raw(),
            3,
            auto_srgb(encoding),
        ),
        DynamicImage::ImageRgba8(buf) => from_u8(
            buf.width(),
            buf.height(),
            buf.as_raw(),
            4,
            auto_srgb(encoding),
        ),
        DynamicImage::ImageLuma8(buf) => from_u8(
            buf.width(),
            buf.height(),
            buf.as_raw(),
            1,
            auto_srgb(encoding),
        ),
        DynamicImage::ImageLumaA8(buf) => from_u8(
            buf.width(),
            buf.height(),
            buf.as_raw(),
            2,
            auto_srgb(encoding),
        ),
        DynamicImage::ImageRgb16(buf) => from_u16(
            buf.width(),
            buf.height(),
            buf.as_raw(),
            3,
            auto_linear(encoding),
        ),
        DynamicImage::ImageRgba16(buf) => from_u16(
            buf.width(),
            buf.height(),
            buf.as_raw(),
            4,
            auto_linear(encoding),
        ),
        DynamicImage::ImageLuma16(buf) => from_u16(
            buf.width(),
            buf.height(),
            buf.as_raw(),
            1,
            auto_linear(encoding),
        ),
        DynamicImage::ImageRgb32F(buf) => from_f32(
            buf.width(),
            buf.height(),
            buf.as_raw(),
            3,
            auto_linear(encoding),
        ),
        DynamicImage::ImageRgba32F(buf) => from_f32(
            buf.width(),
            buf.height(),
            buf.as_raw(),
            4,
            auto_linear(encoding),
        ),
        other => {
            let buf = other.to_rgb8();
            from_u8(
                buf.width(),
                buf.height(),
                buf.as_raw(),
                3,
                auto_srgb(encoding),
            )
        }
    }
}

fn from_u8(width: u32, height: u32, data: &[u8], channels: usize, srgb: bool) -> LinearImage {
    let mut image = LinearImage::new(width, height);
    for (i, pixel) in image.data.chunks_exact_mut(3).enumerate() {
        for (c, slot) in pixel.iter_mut().enumerate() {
            let source = data
                .get(i * channels + c.min(channels - 1))
                .copied()
                .unwrap_or(0);
            let v = source as f32 / 255.0;
            *slot = if srgb { color::srgb_to_linear(v) } else { v };
        }
    }
    image
}

fn from_u16(width: u32, height: u32, data: &[u16], channels: usize, srgb: bool) -> LinearImage {
    let mut image = LinearImage::new(width, height);
    for (i, pixel) in image.data.chunks_exact_mut(3).enumerate() {
        for (c, slot) in pixel.iter_mut().enumerate() {
            let source = data
                .get(i * channels + c.min(channels - 1))
                .copied()
                .unwrap_or(0);
            let v = source as f32 / 65535.0;
            *slot = if srgb { color::srgb_to_linear(v) } else { v };
        }
    }
    image
}

fn from_f32(width: u32, height: u32, data: &[f32], channels: usize, srgb: bool) -> LinearImage {
    let mut image = LinearImage::new(width, height);
    for (i, pixel) in image.data.chunks_exact_mut(3).enumerate() {
        for (c, slot) in pixel.iter_mut().enumerate() {
            let v = data
                .get(i * channels + c.min(channels - 1))
                .copied()
                .unwrap_or(0.0);
            *slot = if srgb { color::srgb_to_linear(v) } else { v };
        }
    }
    image
}

/// Write whatever the extension says: `.exr` keeps linear float, everything
/// else is tone mapped for display.
pub fn save(path: &Path, image: &LinearImage, tonemap: &ToneMapOptions) -> Result<()> {
    save_with_quality(path, image, tonemap, 92)
}

/// Same as [`save`] with an explicit JPEG quality (1..=100); ignored by other
/// formats.
pub fn save_with_quality(
    path: &Path,
    image: &LinearImage,
    tonemap: &ToneMapOptions,
    jpeg_quality: u8,
) -> Result<()> {
    match output_format(path) {
        OutputFormat::Exr => save_exr(path, image),
        OutputFormat::Tiff16 => {
            let data = to_display16(image, tonemap);
            let buffer: ImageBuffer<Rgb<u16>, Vec<u16>> =
                ImageBuffer::from_raw(image.width, image.height, data)
                    .ok_or_else(|| HdrError::Engine("16-bit buffer size mismatch".into()))?;
            buffer
                .save(path)
                .map_err(|e| HdrError::Engine(format!("write {}: {e}", path.display())))
        }
        OutputFormat::Png8 => {
            let data = to_display8(image, tonemap);
            let buffer: ImageBuffer<Rgb<u8>, Vec<u8>> =
                ImageBuffer::from_raw(image.width, image.height, data)
                    .ok_or_else(|| HdrError::Engine("8-bit buffer size mismatch".into()))?;
            buffer
                .save(path)
                .map_err(|e| HdrError::Engine(format!("write {}: {e}", path.display())))
        }
        OutputFormat::Jpg8 => {
            let data = to_display8(image, tonemap);
            let file = File::create(path)?;
            let mut writer = BufWriter::new(file);
            let mut encoder = ::image::codecs::jpeg::JpegEncoder::new_with_quality(
                &mut writer,
                jpeg_quality.clamp(1, 100),
            );
            encoder
                .encode(
                    &data,
                    image.width,
                    image.height,
                    ::image::ExtendedColorType::Rgb8,
                )
                .map_err(|e| HdrError::Engine(format!("write {}: {e}", path.display())))
        }
    }
}

pub fn save_exr(path: &Path, image: &LinearImage) -> Result<()> {
    let buffer: ImageBuffer<Rgb<f32>, Vec<f32>> =
        ImageBuffer::from_raw(image.width, image.height, image.data.clone())
            .ok_or_else(|| HdrError::Engine("float buffer size mismatch".into()))?;
    buffer
        .save(path)
        .map_err(|e| HdrError::Engine(format!("write {}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tonemap::Curve;

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("hdr-core-io-tests");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn exr_roundtrip_keeps_float_values() {
        let mut image = LinearImage::new(4, 3);
        image.set_pixel(1, 1, [0.0, 0.25, 12.5]);
        let path = tmp("roundtrip.exr");
        save_exr(&path, &image).unwrap();
        let loaded = load(&path, InputEncoding::Linear).unwrap();
        assert_eq!((loaded.width, loaded.height), (4, 3));
        let pixel = loaded.pixel(1, 1);
        assert!((pixel[1] - 0.25).abs() < 1e-3, "{pixel:?}");
        assert!((pixel[2] - 12.5).abs() < 0.1, "{pixel:?}");
    }

    #[test]
    fn tiff16_roundtrip_is_linear() {
        let mut image = LinearImage::new(2, 2);
        image.set_pixel(0, 0, [0.5, 0.25, 0.125]);
        let path = tmp("roundtrip.tif");
        let linear = ToneMapOptions {
            curve: Curve::None,
            srgb: false,
            exposure_ev: 0.0,
            ..Default::default()
        };
        save(&path, &image, &linear).unwrap();
        let loaded = load(&path, InputEncoding::Linear).unwrap();
        let pixel = loaded.pixel(0, 0);
        assert!((pixel[0] - 0.5).abs() < 1e-3, "{pixel:?}");
    }

    #[test]
    fn raw_extensions_are_recognised() {
        assert!(is_raw(Path::new("a.CR3")));
        assert!(is_raw(Path::new("b/photo.dng")));
        assert!(!is_raw(Path::new("c.tif")));
        assert!(!is_raw(Path::new("no-extension")));
    }

    #[test]
    fn jpg_is_written_and_readable() {
        let image = LinearImage::filled(8, 8, [0.2, 0.4, 0.6]);
        let path = tmp("preview.jpg");
        save(&path, &image, &ToneMapOptions::default()).unwrap();
        let loaded = load(&path, InputEncoding::Srgb).unwrap();
        assert_eq!((loaded.width, loaded.height), (8, 8));
    }

    #[test]
    fn extension_picks_format() {
        assert_eq!(output_format(Path::new("a/b.exr")), OutputFormat::Exr);
        assert_eq!(output_format(Path::new("a/b.TIF")), OutputFormat::Tiff16);
        assert_eq!(output_format(Path::new("a/b.jpeg")), OutputFormat::Jpg8);
        assert_eq!(output_format(Path::new("a/b")), OutputFormat::Exr);
    }
}
