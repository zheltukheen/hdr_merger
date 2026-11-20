# Architecture

## Shape

```
crates/hdr-core     the engine: pure library, no UI, no globals
crates/hdr-cli      hdr-merger        thin clap shell over hdr-core
crates/hdr-gui      hdr-merger-gui    egui shell, worker thread per job
```

The dependency arrow always points into `hdr-core`. A feature the CLI cannot
reach is in the wrong place.

## Data flow

```
files ──io::load──► LinearImage (f32, linear, RGB interleaved)
                        │
                        ├─ align::estimate_shift (probe ≤ 1024 px, phase correlation)
                        │
                        ▼
                 cache: frame_NNNNN.f16  (shift applied, temp dir, deleted on drop)
                        │
                        ▼  strip by 128 rows, rayon per row
                 stack::pick_sorted   (median / percentile / trimmed / mean / sum / max)
                        │
                        ▼
                 LinearImage master (linear light, may exceed 1.0)
                        │
                        ├─ io::save .exr          raw float, no tone map
                        └─ tonemap + io::save     tiff16 / png8 / jpg8
```

## Why streaming

A light-paint set is 40+ frames of 50–60 MP. At f32 that is ~31 GB resident for
43 frames of an R5-class sensor, which no desktop should be asked to hold.
The pipeline therefore:

1. decodes **one** frame at a time,
2. writes it to a temporary f16 cache (half the bytes, still HDR: f16 goes to
   65504, so nothing is clamped at 1.0),
3. stacks strip by strip, reading 128 rows of every frame per pass.

Peak resident memory is one decoded frame plus `frames × 128 rows × width × 3 ×
2 B` of cache buffers — a few hundred MB, independent of the set size. The cache
lives in `std::env::temp_dir()` and is removed by `Drop`, so a crash leaves at
most one stale `hdr-merger-*` directory behind.

## Invariants

* `LinearImage.data.len() == width * height * 3`. Constructors maintain it;
  `from_rgb_f32` is the only unchecked path and is crate-internal in spirit.
* Light values are **linear**, never display-encoded, inside the engine. sRGB
  appears only at the IO boundary.
* `Mask` is an *exclusion* mask: `> 127` drops that pixel for that frame. A
  masked pixel with no surviving frames stacks to 0, not to black magic.
* `Shift` returned by `align::estimate_shift` is the content offset of
  `moving` relative to `reference`: content that belongs at `x` sits at `x + dx`
  in the moving frame, so the cache writer samples `x + dx`. A test pins the
  sign, because getting it wrong silently doubles the smear instead of failing.
* Stack methods are pure functions of the sorted frame values. No method may
  look at the image as a whole.

## Domain decisions

| Decision | Why | Cost |
| --- | --- | --- |
| Median as default | removes anything that moved, keeps typical light | loses the "sum of all pops" look |
| Percentile is 0..=100, `p90` = 90th | matches how photographers talk | none |
| Translation-only alignment | tripod / flash-walk sets only need this | no rotation/scale (see ROADMAP) |
| f16 cache | 2× less disk than f32, still HDR | ~0.05 % quantisation |
| Auto exposure targets p90 luminance | stable across very different sets | not scene-aware |
| Masks are per frame, not per pixel-in-stack | the user knows which frame is dirty | no auto-detection yet |

## Rendering

`eframe` runs on **wgpu**: Metal on macOS, Vulkan/DX12 on Windows, Vulkan (or GL)
on Linux — one code path, see [docs/adr/0001-render-backend.md](docs/adr/0001-render-backend.md).
OpenGL is still buildable (`--features glow-backend`) and selectable at runtime
(`HDR_MERGER_RENDERER=glow`) for machines where no adapter initialises.

The canvas draws one textured quad plus egui geometry; all image work happens on
the CPU. `RUST_LOG=wgpu_core=info` prints the adapter in use.

## Threading

`stack_cached` parallelises rows with rayon inside each strip; every other step
is sequential and bounded. The GUI runs each job on one `std::thread` and
communicates through an `mpsc` channel — no `Arc<Mutex<…>>` shared state.

## Error handling

`HdrError` is the only error type; every variant carries enough context to print
a useful message (`decode <path>: <source>`, `size mismatch`, …). The CLI maps it
to a non-zero exit code; the GUI shows it in the status bar. No `unwrap` outside
tests, except where a value is guaranteed by construction and documented.

## Testing strategy

* Unit: pure functions (`stack`, `color`, `tonemap`, `mask`, `image`).
* Behavioural: `pipeline` tests write real TIFFs into a temp dir and assert on
  the merged pixels (median removes an intruder, a mask falls through to the
  other frames).
* Round trip: EXR keeps float values above 1.0, 16-bit TIFF keeps linear light.

## Extending

* New stack method: add a variant to `StackMethod`, handle it in
  `pick_sorted`, add a test — the CLI and the GUI pick it up through `parse` /
  the combo box.
* New output format: `io::OutputFormat` + one arm in `io::save`.
* New alignment model: implement a function with the same signature as
  `estimate_shift` and select it in `MergeOptions`; the cache writer already
  accepts a translation, so a rotation model needs a different sampler.
