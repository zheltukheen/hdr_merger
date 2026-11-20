//! Layered export: every frame becomes a PSD/PSB layer with its mask, which is
//! what a retoucher wants after the merge — the composite plus the material.

use std::path::Path;

use crate::error::{HdrError, Result};
use crate::image::LinearImage;
use crate::io::{self, InputEncoding};
use crate::pipeline::FrameInput;
use crate::psd::{self, PsdLayer};
use crate::tonemap::{self, ToneMapOptions};

/// Write `frames` as layers. `merged` becomes the composite Photoshop shows
/// with layers hidden — the first layer is the closest thing we have without
/// stacking again.
pub fn export(
    path: &Path,
    frames: &[FrameInput],
    tonemap: &ToneMapOptions,
    max_edge: Option<u32>,
    progress: &dyn Fn(u32, u32),
) -> Result<()> {
    if frames.is_empty() {
        return Err(HdrError::NoFrames);
    }
    let mut layers: Vec<PsdLayer> = Vec::with_capacity(frames.len());
    let total = frames.len() as u32;
    let mut size: Option<(u32, u32)> = None;

    for (index, frame) in frames.iter().enumerate() {
        let mut image = io::load(&frame.path, InputEncoding::Auto)?;
        if let Some(edge) = max_edge {
            image = image.fit_within(edge);
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
        let rgb = tonemap::to_display8(&image, tonemap);
        let mask = frame.mask.as_ref().map(psd::mask_channel_from_exclusion);
        layers.push(PsdLayer {
            name: layer_name(&frame.path),
            rgb,
            mask,
            visible: true,
            opacity: 255,
        });
        progress(index as u32 + 1, total);
    }

    let (width, height) = size.ok_or(HdrError::NoFrames)?;
    let psb = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("psb"))
        .unwrap_or(false)
        || psd::should_use_psb(width, height, layers.len());
    let merged = layers.first().map(|layer| layer.rgb.clone());
    psd::save(path, width, height, &layers, merged.as_deref(), psb)
}

/// Tone-map the first frame so `auto` exposure has something to measure on.
pub fn auto_exposure(frames: &[FrameInput], target: f32) -> Result<f32> {
    let first = frames.first().ok_or(HdrError::NoFrames)?;
    let image: LinearImage = io::load(&first.path, InputEncoding::Auto)?;
    Ok(tonemap::auto_exposure_ev(&image, target))
}

fn layer_name(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "layer".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mask::Mask;
    use std::path::PathBuf;

    fn workdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hdr-core-layers-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn frame(path: &Path, level: f32) {
        let mut image = LinearImage::filled(16, 8, [level, level, level]);
        image.set_pixel(2, 2, [1.0, 1.0, 1.0]);
        let linear = ToneMapOptions {
            exposure_ev: 0.0,
            curve: crate::tonemap::Curve::None,
            srgb: false,
            ..Default::default()
        };
        io::save(path, &image, &linear).unwrap();
    }

    #[test]
    fn writes_one_layer_per_frame_with_masks() {
        let dir = workdir("basic");
        let a = dir.join("A.tif");
        let b = dir.join("B.tif");
        frame(&a, 0.3);
        frame(&b, 0.6);

        let mut mask = Mask::new(16, 8);
        mask.paint_circle(8.0, 4.0, 2.0, 0.0, 255);
        let frames = vec![FrameInput::with_mask(&a, mask), FrameInput::new(&b)];
        let out = dir.join("layers.psd");
        let tonemap = ToneMapOptions {
            srgb: true,
            ..Default::default()
        };
        export(&out, &frames, &tonemap, None, &|_, _| {}).unwrap();

        let bytes = std::fs::read(&out).unwrap();
        let parsed = ::psd::Psd::from_bytes(&bytes).expect("parse psd");
        let names: Vec<String> = parsed
            .layers()
            .iter()
            .map(|layer| layer.name().to_owned())
            .collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"A".to_owned()), "{names:?}");
        assert!(names.contains(&"B".to_owned()), "{names:?}");
    }

    #[test]
    fn empty_frames_are_rejected() {
        let dir = workdir("empty");
        let out = dir.join("none.psd");
        let tonemap = ToneMapOptions::default();
        assert!(export(&out, &[], &tonemap, None, &|_, _| {}).is_err());
    }
}
