//! Integration tests at the public API boundary.
//!
//! Each test states a promise the README makes and checks it through the same
//! entry points a user of the library would call.

use std::path::{Path, PathBuf};

use hdr_core::io::InputEncoding;
use hdr_core::tonemap::Curve;
use hdr_core::{
    merge, merge_paths, FrameInput, LinearImage, Mask, MergeOptions, StackMethod, ToneMapOptions,
};

fn workdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hdr-merger-it-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create workdir");
    dir
}

fn linear_tonemap() -> ToneMapOptions {
    ToneMapOptions { exposure_ev: 0.0, curve: Curve::None, srgb: false }
}

fn write_frame(path: &Path, pixels: &[[f32; 3]], width: u32, height: u32) {
    let mut image = LinearImage::new(width, height);
    image.data = pixels.iter().flat_map(|p| p.iter().copied()).collect();
    hdr_core::io::save(path, &image, &linear_tonemap()).expect("write frame");
}

#[test]
fn median_removes_a_mover_and_writes_an_exr() {
    let dir = workdir("mover");
    let (w, h) = (16u32, 4u32);
    let clean = vec![[0.25, 0.3, 0.35]; (w * h) as usize];
    let mut with_person = clean.clone();
    for y in 0..h {
        for x in 4..8 {
            with_person[(y * w + x) as usize] = [0.95, 0.95, 0.95];
        }
    }
    let paths: Vec<PathBuf> = [clean.clone(), with_person, clean.clone(), clean.clone()]
        .iter()
        .enumerate()
        .map(|(index, pixels)| {
            let path = dir.join(format!("f{index}.tif"));
            write_frame(&path, pixels, w, h);
            path
        })
        .collect();

    let options = MergeOptions { method: StackMethod::Median, align: false, ..Default::default() };
    let merged = merge_paths(&paths, &options, &|_, _| {}).expect("merge");

    for y in 0..h {
        let pixel = merged.image.pixel(5, y);
        assert!((pixel[0] - 0.25).abs() < 2e-3, "mover survived at {y}: {pixel:?}");
    }

    let out = dir.join("result.exr");
    hdr_core::io::save(&out, &merged.image, &ToneMapOptions::default()).expect("save exr");
    let reloaded = hdr_core::io::load(&out, InputEncoding::Linear).expect("read exr");
    assert_eq!((reloaded.width, reloaded.height), (w, h));
    assert!((reloaded.pixel(5, 0)[0] - 0.25).abs() < 2e-3);
}

#[test]
fn a_mask_hands_the_pixel_to_the_other_frames() {
    let dir = workdir("mask");
    let (w, h) = (8u32, 1u32);
    let dark = vec![[0.1, 0.1, 0.1]; w as usize];
    let bright = vec![[0.8, 0.8, 0.8]; w as usize];
    let a = dir.join("a.tif");
    let b = dir.join("b.tif");
    write_frame(&a, &dark, w, h);
    write_frame(&b, &bright, w, h);

    let mut mask = Mask::new(w, h);
    mask.paint_circle(0.0, 0.0, 1.5, 0.0, 255);

    let frames = vec![FrameInput::with_mask(&a, mask), FrameInput::new(&b)];
    let options = MergeOptions { method: StackMethod::Max, align: false, ..Default::default() };
    let merged = merge(&frames, &options, &|_, _| {}).expect("merge");

    assert!((merged.image.pixel(0, 0)[0] - 0.8).abs() < 2e-3, "mask was ignored");
    assert!((merged.image.pixel(4, 0)[0] - 0.8).abs() < 2e-3);
}

#[test]
fn percentile_keeps_highlights_that_median_drops() {
    let dir = workdir("percentile");
    let (w, h) = (4u32, 1u32);
    let mut paths = Vec::new();
    for level in [0.1f32, 0.2, 0.3, 0.4] {
        let pixels = vec![[level, level, level]; w as usize];
        let path = dir.join(format!("p{level}.tif"));
        write_frame(&path, &pixels, w, h);
        paths.push(path);
    }
    let median = merge_paths(
        &paths,
        &MergeOptions { method: StackMethod::Median, align: false, ..Default::default() },
        &|_, _| {},
    )
    .expect("median");
    let p90 = merge_paths(
        &paths,
        &MergeOptions { method: StackMethod::Percentile(90.0), align: false, ..Default::default() },
        &|_, _| {},
    )
    .expect("p90");

    assert!(p90.image.pixel(0, 0)[0] > median.image.pixel(0, 0)[0]);
}

#[test]
fn a_shifted_frame_is_aligned_before_stacking() {
    let dir = workdir("align");
    let (w, h) = (64u32, 64u32);
    // Pseudo-random texture: a periodic pattern would make the correlation
    // surface ambiguous and the test would be about luck, not about alignment.
    let mut seed = 0x1234_5678u32;
    let mut next = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((seed >> 8) & 0xffff) as f32 / 65535.0
    };
    let mut base = LinearImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let value = next();
            base.set_pixel(x, y, [value, value * 0.5, value * 0.2]);
        }
    }
    let mut moved = LinearImage::new(w, h);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let sx = (x - 3).clamp(0, w as i32 - 1) as u32;
            let sy = (y - 2).clamp(0, h as i32 - 1) as u32;
            moved.set_pixel(x as u32, y as u32, base.pixel(sx, sy));
        }
    }

    let a = dir.join("a.tif");
    let b = dir.join("b.tif");
    hdr_core::io::save(&a, &base, &linear_tonemap()).expect("save a");
    hdr_core::io::save(&b, &moved, &linear_tonemap()).expect("save b");

    let options = MergeOptions { method: StackMethod::Mean, align: true, ..Default::default() };
    let merged = merge_paths(&[a, b], &options, &|_, _| {}).expect("merge");

    let registered = merged.shifts[1];
    assert_eq!((registered.dx, registered.dy), (3, 2), "shift not recovered");
    let sample = merged.image.pixel(32, 32);
    let reference = base.pixel(32, 32);
    assert!((sample[0] - reference[0]).abs() < 0.05, "aligned stack drifted: {sample:?}");
}

#[test]
fn exr_master_keeps_values_above_one() {
    let dir = workdir("hdr-range");
    let (w, h) = (4u32, 2u32);
    // 16-bit TIFF inputs cannot exceed 1.0, so the headroom has to come from
    // accumulating light: four frames of 0.5 summed = 2.0.
    let pixels = vec![[0.5, 0.5, 0.5]; (w * h) as usize];
    let mut paths = Vec::new();
    for index in 0..4 {
        let path = dir.join(format!("bright{index}.tif"));
        write_frame(&path, &pixels, w, h);
        paths.push(path);
    }
    let merged = merge_paths(
        &paths,
        &MergeOptions { method: StackMethod::Sum, align: false, ..Default::default() },
        &|_, _| {},
    )
    .expect("merge");
    let out = dir.join("master.exr");
    hdr_core::io::save(&out, &merged.image, &ToneMapOptions::default()).expect("save");

    let reloaded = hdr_core::io::load(&out, InputEncoding::Linear).expect("read");
    assert!(reloaded.pixel(0, 0)[0] > 1.8, "cumulative light lost: {:?}", reloaded.pixel(0, 0));
}
