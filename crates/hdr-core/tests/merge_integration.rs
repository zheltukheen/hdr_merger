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
    ToneMapOptions {
        exposure_ev: 0.0,
        curve: Curve::None,
        srgb: false,
        ..Default::default()
    }
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

    let options = MergeOptions {
        method: StackMethod::Median,
        align: false,
        ..Default::default()
    };
    let merged = merge_paths(&paths, &options, &|_, _| {}).expect("merge");

    for y in 0..h {
        let pixel = merged.image.pixel(5, y);
        assert!(
            (pixel[0] - 0.25).abs() < 2e-3,
            "mover survived at {y}: {pixel:?}"
        );
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
    let options = MergeOptions {
        method: StackMethod::Max,
        align: false,
        ..Default::default()
    };
    let merged = merge(&frames, &options, &|_, _| {}).expect("merge");

    assert!(
        (merged.image.pixel(0, 0)[0] - 0.8).abs() < 2e-3,
        "mask was ignored"
    );
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
        &MergeOptions {
            method: StackMethod::Median,
            align: false,
            ..Default::default()
        },
        &|_, _| {},
    )
    .expect("median");
    let p90 = merge_paths(
        &paths,
        &MergeOptions {
            method: StackMethod::Percentile(90.0),
            align: false,
            ..Default::default()
        },
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

    let options = MergeOptions {
        method: StackMethod::Mean,
        align: true,
        ..Default::default()
    };
    let merged = merge_paths(&[a, b], &options, &|_, _| {}).expect("merge");

    let registered = merged.shifts[1];
    assert_eq!(
        (registered.dx, registered.dy),
        (3, 2),
        "shift not recovered"
    );
    let sample = merged.image.pixel(32, 32);
    let reference = base.pixel(32, 32);
    assert!(
        (sample[0] - reference[0]).abs() < 0.05,
        "aligned stack drifted: {sample:?}"
    );
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
        &MergeOptions {
            method: StackMethod::Sum,
            align: false,
            ..Default::default()
        },
        &|_, _| {},
    )
    .expect("merge");
    let out = dir.join("master.exr");
    hdr_core::io::save(&out, &merged.image, &ToneMapOptions::default()).expect("save");

    let reloaded = hdr_core::io::load(&out, InputEncoding::Linear).expect("read");
    assert!(
        reloaded.pixel(0, 0)[0] > 1.8,
        "cumulative light lost: {:?}",
        reloaded.pixel(0, 0)
    );
}

#[test]
fn gpu_stack_matches_the_cpu_stack_when_an_adapter_exists() {
    if hdr_core::gpu::GpuStacker::new().is_none() {
        eprintln!("no GPU adapter here, skipping the parity check");
        return;
    }
    let dir = workdir("gpu-parity");
    let (w, h) = (24u32, 8u32);
    let mut paths = Vec::new();
    for (index, level) in [0.1f32, 0.35, 0.6, 0.85].iter().enumerate() {
        let mut pixels = vec![[*level, *level * 0.5, *level * 0.25]; (w * h) as usize];
        pixels[index * 7] = [1.0, 1.0, 1.0];
        let path = dir.join(format!("g{index}.tif"));
        write_frame(&path, &pixels, w, h);
        paths.push(path);
    }

    let cpu = merge_paths(
        &paths,
        &MergeOptions {
            method: StackMethod::Median,
            align: false,
            use_gpu: false,
            ..Default::default()
        },
        &|_, _| {},
    )
    .expect("cpu merge");
    let gpu = merge_paths(
        &paths,
        &MergeOptions {
            method: StackMethod::Median,
            align: false,
            use_gpu: true,
            ..Default::default()
        },
        &|_, _| {},
    )
    .expect("gpu merge");

    assert!(gpu.timings.gpu, "the GPU stacker did not run at all");
    for index in 0..cpu.image.pixel_count() {
        let base = index * 3;
        for channel in 0..3 {
            let a = cpu.image.data[base + channel];
            let b = gpu.image.data[base + channel];
            assert!(
                (a - b).abs() < 1e-4,
                "pixel {index} channel {channel}: {a} vs {b}"
            );
        }
    }
}

#[test]
fn focus_stack_keeps_the_sharp_half_of_each_frame() {
    let dir = workdir("focus");
    let (w, h) = (32u32, 8u32);
    // Each frame is sharp on one half and flat on the other, with a distinct
    // stripe amplitude so the test can tell which frame supplied the pixel.
    let stripes = |sharp_left: bool, high: f32, low: f32| {
        let mut pixels = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let sharp = if sharp_left { x < w / 2 } else { x >= w / 2 };
                let value = if sharp {
                    if (x + y) % 2 == 0 {
                        high
                    } else {
                        low
                    }
                } else {
                    0.5
                };
                pixels.push([value, value, value]);
            }
        }
        pixels
    };
    let a = dir.join("left.tif");
    let b = dir.join("right.tif");
    write_frame(&a, &stripes(true, 0.9, 0.1), w, h);
    write_frame(&b, &stripes(false, 0.7, 0.3), w, h);

    let merged = merge_paths(
        &[a, b],
        &MergeOptions {
            method: StackMethod::Focus,
            align: false,
            ..Default::default()
        },
        &|_, _| {},
    )
    .expect("focus merge");

    // x+y is even at both sample points, so the sharp frame gives its high value.
    let left = merged.image.pixel(4, 0)[0];
    let right = merged.image.pixel(28, 0)[0];
    assert!(
        (left - 0.9).abs() < 0.02,
        "left should come from frame a: {left}"
    );
    assert!(
        (right - 0.7).abs() < 0.02,
        "right should come from frame b: {right}"
    );
}

#[test]
fn streaming_exr_matches_the_in_memory_merge() {
    let dir = workdir("stream");
    let (w, h) = (16u32, 8u32);
    let mut paths = Vec::new();
    for level in [0.2f32, 0.5, 0.8] {
        let pixels = vec![[level, level * 0.5, level * 0.25]; (w * h) as usize];
        let path = dir.join(format!("s{level}.tif"));
        write_frame(&path, &pixels, w, h);
        paths.push(path);
    }
    let options = MergeOptions {
        method: StackMethod::Median,
        align: false,
        ..Default::default()
    };
    let memory = merge_paths(&paths, &options, &|_, _| {}).expect("memory merge");

    let out = dir.join("stream.exr");
    let frames: Vec<hdr_core::FrameInput> = paths.iter().map(hdr_core::FrameInput::new).collect();
    hdr_core::pipeline::merge_to_exr(&frames, &options, &out, &|_, _| {}).expect("stream merge");

    let reloaded = hdr_core::io::load(&out, InputEncoding::Linear).expect("read stream exr");
    assert_eq!((reloaded.width, reloaded.height), (w, h));
    for index in 0..memory.image.data.len() {
        let a = memory.image.data[index];
        let b = reloaded.data[index];
        assert!((a - b).abs() < 1e-4, "pixel {index}: {a} vs {b}");
    }
}

#[test]
fn feathered_focus_blends_the_seam_and_reports_depth() {
    let dir = workdir("focus-feather");
    let (w, h) = (32u32, 8u32);
    let stripes = |sharp_left: bool, high: f32, low: f32| {
        let mut pixels = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let sharp = if sharp_left { x < w / 2 } else { x >= w / 2 };
                let value = if sharp {
                    if (x + y) % 2 == 0 {
                        high
                    } else {
                        low
                    }
                } else {
                    0.5
                };
                pixels.push([value, value, value]);
            }
        }
        pixels
    };
    let a = dir.join("a.tif");
    let b = dir.join("b.tif");
    write_frame(&a, &stripes(true, 0.9, 0.1), w, h);
    write_frame(&b, &stripes(false, 0.7, 0.3), w, h);

    let hard = merge_paths(
        &[a.clone(), b.clone()],
        &MergeOptions {
            method: StackMethod::Focus,
            align: false,
            focus_feather: 0,
            ..Default::default()
        },
        &|_, _| {},
    )
    .expect("hard focus");
    let soft = merge_paths(
        &[a, b],
        &MergeOptions {
            method: StackMethod::Focus,
            align: false,
            focus_feather: 8,
            ..Default::default()
        },
        &|_, _| {},
    )
    .expect("feathered focus");

    // Right at the handover the hard pick jumps to one frame's value; the
    // feathered version sits between the two.
    let hard_right = hard.image.pixel(20, 0)[0];
    let hard_left = hard.image.pixel(12, 0)[0];
    let hard_edge = hard.image.pixel(16, 0)[0];
    let soft_edge = soft.image.pixel(16, 0)[0];
    assert!(
        (hard_right - 0.7).abs() < 0.05,
        "hard pick should be frame b: {hard_right}"
    );
    assert!(
        (hard_left - 0.9).abs() < 0.05,
        "hard pick should be frame a: {hard_left}"
    );
    // The whole point: at the handover the soft version is not either frame's
    // value any more, it is a blend of frame a (0.5) and frame b (0.7).
    assert!(
        (soft_edge - hard_edge).abs() > 0.02,
        "feathering changed nothing at the seam: hard {hard_edge}, soft {soft_edge}"
    );
    assert!(
        (0.49..=0.71).contains(&soft_edge),
        "blend left the two frames' range: {soft_edge}"
    );
    // Far from the seam both agree with their frame (x+y even/odd chooses the
    // stripe value: 2+0 even, 29+0 odd).
    assert!((soft.image.pixel(2, 0)[0] - 0.9).abs() < 0.05);
    assert!((soft.image.pixel(29, 0)[0] - 0.3).abs() < 0.05);

    let depth = soft.depth.expect("focus merge must report a depth map");
    assert_eq!((depth.width, depth.height), (w, h), "depth size must match");
    assert!(depth.pixel(2, 0)[0] < 0.25, "left should be near frame a");
    assert!(depth.pixel(29, 0)[0] > 0.75, "right should be near frame b");
}

#[test]
fn a_dark_frame_is_subtracted_before_stacking() {
    let dir = workdir("dark");
    let (w, h) = (16u32, 4u32);
    let lit = dir.join("lit.tif");
    let dark = dir.join("dark.tif");
    write_frame(&lit, &vec![[0.5, 0.5, 0.5]; (w * h) as usize], w, h);
    write_frame(&dark, &vec![[0.2, 0.2, 0.2]; (w * h) as usize], w, h);

    let merged = merge_paths(
        &[lit],
        &MergeOptions {
            method: StackMethod::Mean,
            align: false,
            dark_frame: Some(dark),
            ..Default::default()
        },
        &|_, _| {},
    )
    .expect("merge with a dark frame");
    let value = merged.image.pixel(4, 0)[0];
    assert!(
        (value - 0.3).abs() < 5e-3,
        "dark frame not subtracted: {value}"
    );
}
