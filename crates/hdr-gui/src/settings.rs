//! Application settings, help text and the update check.
//!
//! Settings live in the OS config directory, never next to the binaries, and
//! are plain JSON so a user can fix them with an editor if the UI ever refuses
//! to start.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub theme: Theme,
    /// Preview proxy long edge; 0 means full resolution.
    pub proxy: u32,
    pub jpeg_quality: u8,
    pub output_dir: String,
    pub check_updates: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            proxy: 1400,
            jpeg_quality: 92,
            output_dir: String::new(),
            check_updates: true,
        }
    }
}

impl Settings {
    pub fn path() -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os("HDR_MERGER_CONFIG_DIR") {
            return Some(PathBuf::from(dir).join("settings.json"));
        }
        let base = if cfg!(target_os = "macos") {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join("Library/Application Support"))
        } else if cfg!(target_os = "windows") {
            std::env::var_os("APPDATA").map(PathBuf::from)
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"))
                })
        }?;
        Some(base.join("hdr-merger").join("settings.json"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let Some(path) = Self::path() else {
            return Err("no config directory on this system".into());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        std::fs::write(path, text).map_err(|error| error.to_string())
    }
}

/// Version comparison for `v1.2.3`-style tags: true when `latest` is newer.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |value: &str| -> Vec<u32> {
        value
            .trim_start_matches('v')
            .split('.')
            .map(|part| part.parse::<u32>().unwrap_or(0))
            .collect()
    };
    let latest = parse(latest);
    let current = parse(current);
    for index in 0..latest.len().max(current.len()) {
        let a = latest.get(index).copied().unwrap_or(0);
        let b = current.get(index).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    false
}

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateStatus {
    Idle,
    Checking,
    UpToDate,
    Available { version: String, url: String },
    Failed(String),
}

/// Ask GitHub for the latest release. Blocking; call it on a worker thread.
pub fn check_latest() -> UpdateStatus {
    let url = "https://api.github.com/repos/zheltukheen/hdr_merger/releases/latest";
    let response = match ureq::get(url)
        .set("User-Agent", "hdr-merger")
        .set("Accept", "application/vnd.github+json")
        .call()
    {
        Ok(response) => response,
        Err(error) => return UpdateStatus::Failed(error.to_string()),
    };
    let body = match response.into_string() {
        Ok(body) => body,
        Err(error) => return UpdateStatus::Failed(error.to_string()),
    };
    let parsed: serde_json::Value = match serde_json::from_str(&body) {
        Ok(parsed) => parsed,
        Err(error) => return UpdateStatus::Failed(error.to_string()),
    };
    let tag = parsed
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let html = parsed
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or("https://github.com/zheltukheen/hdr_merger/releases");
    if tag.is_empty() {
        return UpdateStatus::Failed("no tag in the release response".into());
    }
    if is_newer(tag, env!("CARGO_PKG_VERSION")) {
        UpdateStatus::Available {
            version: tag.to_string(),
            url: html.to_string(),
        }
    } else {
        UpdateStatus::UpToDate
    }
}

pub const HELP_TITLE: &str = "HDR Merger — quick start";

pub const HELP_STEPS: &[(&str, &str)] = &[
    ("1. Sources", "Drop a folder or files anywhere in the window, or paste a path. TIFF, PNG, JPEG, EXR. Convert RAW to 16-bit linear TIFF first."),
    ("2. Stack", "Median removes anything that moved. Percentile keeps the strongest stable contribution. Focus stack keeps the sharpest frame per pixel. Align is on for hand-held sets; keep the shift guard on to catch a frame that is not the same shot."),
    ("3. Masks", "Paint over whatever must disappear, or press Detect movers and review the result. Masked pixels fall through to the other frames."),
    ("4. Tone", "Display only — the EXR master stays linear. Start from a preset, then adjust levels, colour and detail."),
    ("5. Export", "EXR keeps 32-bit float. TIFF is 16-bit display. JPEG/PNG for sharing. Save a recipe to re-render later without stacking again."),
];

pub const HELP_SHORTCUTS: &[(&str, &str)] = &[
    ("B / E / I", "brush, eraser, inspect"),
    ("[ / ]", "brush size"),
    ("Ctrl/Cmd+Z", "undo mask stroke"),
    ("Shift+Ctrl/Cmd+Z", "redo mask stroke"),
    ("Ctrl/Cmd+S", "save recipe"),
    ("Ctrl/Cmd+O", "load recipe"),
    ("Ctrl/Cmd+E", "export"),
    ("1 / 2", "fit / 100 %"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison_handles_tags_and_padding() {
        assert!(is_newer("v0.4.0", "0.3.0"));
        assert!(is_newer("0.3.10", "0.3.9"));
        assert!(!is_newer("v0.3.0", "0.3.0"));
        assert!(!is_newer("v0.3.0", "0.3.1"));
        assert!(!is_newer("garbage", "0.3.0"));
    }

    #[test]
    fn settings_roundtrip() {
        let settings = Settings {
            proxy: 2000,
            jpeg_quality: 80,
            ..Default::default()
        };
        let text = serde_json::to_string(&settings).unwrap();
        let back: Settings = serde_json::from_str(&text).unwrap();
        assert_eq!(settings, back);
    }
}
