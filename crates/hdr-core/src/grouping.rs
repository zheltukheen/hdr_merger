//! Automatic grouping of a whole shoot into stacks.
//!
//! One group is one merge set. A group ends when the capture time jumps, when
//! the exposure changes, or when the scene changes. The decision itself is a
//! pure function ([`segment`]); [`group_paths`] is the only part that touches
//! files and EXIF.

use std::path::{Path, PathBuf};

use exif::{Exif, In, Tag, Value};

use crate::color;
use crate::error::Result;
use crate::io::{self, InputEncoding};

/// How a shoot is cut into stacks.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GroupOptions {
    /// A gap larger than this starts a new stack, in seconds. Default 120.
    pub max_gap_secs: u64,
    /// Mean absolute luminance difference (0..1) above which two consecutive
    /// frames are considered different scenes. Default 0.12.
    pub similarity: f32,
    /// Longest edge used for the similarity thumbnails. Default 96.
    pub probe_edge: u32,
}

impl Default for GroupOptions {
    fn default() -> Self {
        Self {
            max_gap_secs: 120,
            similarity: 0.12,
            probe_edge: 96,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// Indices into the input slice, in capture order.
    pub frames: Vec<usize>,
    /// Human-readable reason for the boundary that started this group,
    /// e.g. "start", "gap 3 min", "exposure changed", "scene changed".
    pub reason: String,
}

/// Per-frame input to the pure grouping decision.
pub(crate) struct FrameSignal {
    pub time: u64,
    pub exposure: String,
    pub luma: Vec<f32>,
}

/// Groups `paths` into stacks. Never returns empty groups; every input index
/// appears exactly once across the returned groups, in input order.
pub fn group_paths(
    paths: &[PathBuf],
    options: &GroupOptions,
    progress: &dyn Fn(u32, u32),
) -> Result<Vec<Group>> {
    let total = paths.len() as u32;
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let meta: Vec<FrameMeta> = paths.iter().map(|path| FrameMeta::read(path)).collect();
    let order = capture_order(&meta);
    let mut signals = Vec::with_capacity(paths.len());
    for (done, &index) in order.iter().enumerate() {
        let luma = luma_plane(&paths[index], options.probe_edge)?;
        signals.push(FrameSignal {
            time: meta[index].time,
            exposure: meta[index].exposure.clone(),
            luma,
        });
        progress(done as u32 + 1, total);
    }
    Ok(remap(segment(&signals, options), &order))
}

/// The pure boundary decision. Group frames are positions in `signals`.
pub(crate) fn segment(signals: &[FrameSignal], options: &GroupOptions) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for (index, signal) in signals.iter().enumerate() {
        let reason = if index == 0 {
            Some("start".to_string())
        } else {
            boundary_reason(&signals[index - 1], signal, options)
        };
        match reason {
            Some(reason) => groups.push(Group {
                frames: vec![index],
                reason,
            }),
            None => {
                if let Some(last) = groups.last_mut() {
                    last.frames.push(index);
                }
            }
        }
    }
    groups
}

fn boundary_reason(
    previous: &FrameSignal,
    current: &FrameSignal,
    options: &GroupOptions,
) -> Option<String> {
    let gap = current.time.saturating_sub(previous.time);
    if gap > options.max_gap_secs {
        return Some(describe_gap(gap));
    }
    if previous.exposure != current.exposure {
        return Some("exposure changed".to_string());
    }
    if luma_difference(&previous.luma, &current.luma) > options.similarity {
        return Some("scene changed".to_string());
    }
    None
}

fn describe_gap(secs: u64) -> String {
    if secs >= 60 {
        format!("gap {} min", (secs as f64 / 60.0).round() as u64)
    } else {
        format!("gap {secs} s")
    }
}

fn luma_difference(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().min(b.len());
    if len == 0 {
        return 0.0;
    }
    let sum: f32 = a.iter().zip(b).take(len).map(|(x, y)| (x - y).abs()).sum();
    (sum / len as f32).clamp(0.0, 1.0)
}

fn remap(groups: Vec<Group>, order: &[usize]) -> Vec<Group> {
    groups
        .into_iter()
        .map(|group| Group {
            frames: group
                .frames
                .iter()
                .map(|&position| order[position])
                .collect(),
            reason: group.reason,
        })
        .collect()
}

fn capture_order(meta: &[FrameMeta]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..meta.len()).collect();
    order.sort_by(|&a, &b| {
        meta[a]
            .time
            .cmp(&meta[b].time)
            .then_with(|| meta[a].name.cmp(&meta[b].name))
    });
    order
}

struct FrameMeta {
    time: u64,
    name: String,
    exposure: String,
}

impl FrameMeta {
    fn read(path: &Path) -> Self {
        let exif = read_exif(path);
        Self {
            time: exif.as_ref().map(read_time).unwrap_or(0),
            name: file_name(path),
            exposure: exposure_key(exif.as_ref()),
        }
    }
}

fn read_exif(path: &Path) -> Option<Exif> {
    let file = std::fs::File::open(path).ok()?;
    exif::Reader::new()
        .read_from_container(&mut std::io::BufReader::new(file))
        .ok()
}

fn read_time(exif: &Exif) -> u64 {
    exif.get_field(Tag::DateTimeOriginal, In::PRIMARY)
        .and_then(|field| ascii_bytes(&field.value))
        .and_then(parse_datetime)
        .unwrap_or(0)
}

fn ascii_bytes(value: &Value) -> Option<&[u8]> {
    match value {
        Value::Ascii(entries) => entries.first().map(Vec::as_slice),
        _ => None,
    }
}

fn file_name(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => String::new(),
    }
}

fn exposure_key(exif: Option<&Exif>) -> String {
    let Some(exif) = exif else {
        return "? f/? ISO?".to_string();
    };
    let shutter = rational(exif, Tag::ExposureTime)
        .map(format_shutter)
        .unwrap_or_else(|| "?".to_string());
    let aperture = rational(exif, Tag::FNumber)
        .map(|value| format!("f/{value:.1}"))
        .unwrap_or_else(|| "f/?".to_string());
    let iso = field_uint(exif, Tag::PhotographicSensitivity)
        .map(|value| format!("ISO{value}"))
        .unwrap_or_else(|| "ISO?".to_string());
    format!("{shutter} {aperture} {iso}")
}

fn rational(exif: &Exif, tag: Tag) -> Option<f64> {
    match &exif.get_field(tag, In::PRIMARY)?.value {
        Value::Rational(values) => values.first().map(exif::Rational::to_f64),
        _ => None,
    }
}

fn field_uint(exif: &Exif, tag: Tag) -> Option<u32> {
    exif.get_field(tag, In::PRIMARY)?.value.get_uint(0)
}

fn format_shutter(seconds: f64) -> String {
    if seconds <= 0.0 {
        "?".to_string()
    } else if seconds < 1.0 {
        format!("1/{:.0}", 1.0 / seconds)
    } else {
        format!("{seconds:.1}s")
    }
}

fn parse_datetime(text: &[u8]) -> Option<u64> {
    let runs = digit_runs(text);
    let Some(&[year, month, day, hour, minute, second, ..]) = runs.get(..) else {
        return None;
    };
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let days = days_from_civil(year as i64, month as i64, day as i64);
    let seconds = days * 86_400 + hour as i64 * 3600 + minute as i64 * 60 + second as i64;
    Some(seconds.max(0) as u64)
}

fn digit_runs(text: &[u8]) -> Vec<u32> {
    let mut runs = Vec::new();
    let mut current = 0u32;
    let mut in_run = false;
    for &byte in text {
        if byte.is_ascii_digit() {
            current = current
                .saturating_mul(10)
                .saturating_add(u32::from(byte - b'0'));
            in_run = true;
        } else if in_run {
            runs.push(current);
            current = 0;
            in_run = false;
        }
    }
    if in_run {
        runs.push(current);
    }
    runs
}

/// Days since 1970-01-01 for a proleptic Gregorian date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn luma_plane(path: &Path, edge: u32) -> Result<Vec<f32>> {
    let image = io::load(path, InputEncoding::Auto)?.fit_within(edge);
    Ok(image
        .data
        .chunks_exact(3)
        .map(|pixel| color::luminance([pixel[0], pixel[1], pixel[2]]))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::LinearImage;
    use crate::tonemap::ToneMapOptions;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hdr-core-grouping-{name}"));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    fn synthetic(time: u64, exposure: &str, level: f32) -> FrameSignal {
        FrameSignal {
            time,
            exposure: exposure.to_string(),
            luma: vec![level; 16],
        }
    }

    fn flatten(groups: &[Group]) -> Vec<usize> {
        groups
            .iter()
            .flat_map(|group| group.frames.iter().copied())
            .collect()
    }

    #[test]
    fn groups_a_single_set_into_one_stack() {
        let dir = scratch("single-set");
        let image = LinearImage::filled(32, 24, [0.25, 0.5, 0.75]);
        let paths: Vec<PathBuf> = (0..4).map(|i| dir.join(format!("frame_{i}.tif"))).collect();
        for path in &paths {
            crate::io::save(path, &image, &ToneMapOptions::default()).expect("write tiff");
        }
        let seen = std::cell::Cell::new(0u32);
        let groups = group_paths(&paths, &GroupOptions::default(), &|done, total| {
            assert!(done <= total);
            seen.set(seen.get() + 1);
        })
        .expect("group paths");
        assert_eq!(groups.len(), 1, "{groups:?}");
        assert_eq!(groups[0].frames, vec![0, 1, 2, 3]);
        assert_eq!(groups[0].reason, "start");
        assert_eq!(seen.get(), 4);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn splits_on_a_large_time_gap() {
        let signals = vec![
            synthetic(0, "a", 0.5),
            synthetic(10, "a", 0.5),
            synthetic(20, "a", 0.5),
            synthetic(1000, "a", 0.5),
        ];
        let groups = segment(&signals, &GroupOptions::default());
        assert_eq!(groups.len(), 2, "{groups:?}");
        assert_eq!(groups[0].frames, vec![0, 1, 2]);
        assert_eq!(groups[1].frames, vec![3]);
        assert!(groups[1].reason.contains("gap"), "{}", groups[1].reason);
    }

    #[test]
    fn splits_when_the_exposure_changes() {
        let signals = vec![
            synthetic(0, "1/125 f/2.8 ISO400", 0.5),
            synthetic(1, "1/125 f/2.8 ISO400", 0.5),
            synthetic(2, "1/500 f/8.0 ISO100", 0.5),
        ];
        let groups = segment(&signals, &GroupOptions::default());
        assert_eq!(groups.len(), 2, "{groups:?}");
        assert_eq!(groups[0].frames, vec![0, 1]);
        assert_eq!(groups[1].frames, vec![2]);
        assert_eq!(groups[1].reason, "exposure changed");
    }

    #[test]
    fn splits_when_the_scene_changes() {
        let signals = vec![
            synthetic(0, "same", 0.0),
            synthetic(1, "same", 0.0),
            synthetic(2, "same", 1.0),
        ];
        let groups = segment(&signals, &GroupOptions::default());
        assert_eq!(groups.len(), 2, "{groups:?}");
        assert_eq!(groups[0].frames, vec![0, 1]);
        assert_eq!(groups[1].frames, vec![2]);
        assert_eq!(groups[1].reason, "scene changed");
    }

    #[test]
    fn splits_real_files_when_the_scene_changes() {
        let dir = scratch("scene-split");
        let dark = LinearImage::filled(32, 24, [0.0, 0.0, 0.0]);
        let bright = LinearImage::filled(32, 24, [1.0, 1.0, 1.0]);
        let names = [
            "a_dark_0.tif",
            "a_dark_1.tif",
            "b_bright_0.tif",
            "b_bright_1.tif",
        ];
        let paths: Vec<PathBuf> = names.iter().map(|name| dir.join(name)).collect();
        for (path, image) in paths.iter().zip([&dark, &dark, &bright, &bright]) {
            crate::io::save(path, image, &ToneMapOptions::default()).expect("write tiff");
        }
        let groups =
            group_paths(&paths, &GroupOptions::default(), &|_, _| {}).expect("group paths");
        assert_eq!(groups.len(), 2, "{groups:?}");
        assert_eq!(groups[0].frames, vec![0, 1]);
        assert_eq!(groups[1].frames, vec![2, 3]);
        assert_eq!(groups[1].reason, "scene changed");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn every_index_appears_exactly_once() {
        let signals = vec![
            synthetic(0, "a", 0.5),
            synthetic(10, "a", 0.5),
            synthetic(20, "a", 0.5),
            synthetic(1000, "a", 0.5),
            synthetic(1010, "b", 0.5),
            synthetic(1020, "b", 0.05),
            synthetic(1030, "b", 0.05),
        ];
        let groups = segment(&signals, &GroupOptions::default());
        assert_eq!(flatten(&groups), (0..signals.len()).collect::<Vec<_>>());
        assert!(groups.iter().all(|group| !group.frames.is_empty()));
        assert!(groups.len() > 1);
    }
}
