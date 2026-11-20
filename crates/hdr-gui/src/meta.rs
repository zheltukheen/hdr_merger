//! Cheap metadata probe: size, bytes and (when the file carries it) EXIF.
//!
//! Nothing here is on the hot path: it runs when a frame is added.

use std::path::{Path, PathBuf};

pub struct Meta {
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub taken: Option<String>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub exposure: Option<String>,
    pub iso: Option<String>,
    pub aperture: Option<String>,
}

impl Meta {
    pub fn summary(&self) -> String {
        format!(
            "{}×{} · {}",
            self.width,
            self.height,
            human_bytes(self.bytes)
        )
    }

    pub fn sort_key(&self, path: &Path) -> String {
        self.taken
            .clone()
            .unwrap_or_else(|| file_stem(path).to_string())
    }
}

pub fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

pub fn probe(path: &Path) -> Meta {
    let (width, height) = hdr_core::io::dimensions(path).unwrap_or((0, 0));
    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut meta = Meta {
        width,
        height,
        bytes,
        taken: None,
        camera: None,
        lens: None,
        exposure: None,
        iso: None,
        aperture: None,
    };
    if let Ok(fields) = read_exif(path) {
        meta.taken = fields.taken;
        meta.camera = fields.camera;
        meta.lens = fields.lens;
        meta.exposure = fields.exposure;
        meta.iso = fields.iso;
        meta.aperture = fields.aperture;
    }
    meta
}

struct ExifFields {
    taken: Option<String>,
    camera: Option<String>,
    lens: Option<String>,
    exposure: Option<String>,
    iso: Option<String>,
    aperture: Option<String>,
}

fn read_exif(path: &Path) -> Result<ExifFields, Box<dyn std::error::Error>> {
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let exif = exif::Reader::new().read_from_container(&mut reader)?;
    let get = |tag: exif::Tag| {
        exif.get_field(tag, exif::In::PRIMARY)
            .map(|field| field.display_value().with_unit(&exif).to_string())
    };
    let camera = match (get(exif::Tag::Make), get(exif::Tag::Model)) {
        (Some(make), Some(model)) => Some(format!("{make} {model}")),
        (None, model) => model,
        (make, None) => make,
    };
    Ok(ExifFields {
        taken: get(exif::Tag::DateTimeOriginal),
        camera,
        lens: get(exif::Tag::LensModel),
        exposure: get(exif::Tag::ExposureTime),
        iso: get(exif::Tag::PhotographicSensitivity),
        aperture: get(exif::Tag::FNumber),
    })
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Expand dropped paths: directories become their supported image files.
pub fn expand(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for path in paths {
        if path.is_dir() {
            let Ok(entries) = std::fs::read_dir(&path) else {
                continue;
            };
            let mut found: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|entry| is_supported(entry))
                .collect();
            found.sort();
            out.extend(found);
        } else if is_supported(&path) {
            out.push(path);
        }
    }
    out
}

pub fn is_supported(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("tif") | Some("tiff") | Some("png") | Some("jpg") | Some("jpeg") | Some("exr")
    )
}
