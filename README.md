# HDR Merger

Cross-platform HDR / light-paint stacker: a Rust engine, a headless CLI and a
native desktop UI. Built for the case where one exposure is not enough — a room
lit by a hand-held flash fired from many positions, a night shot, an interior
with windows.

![workspace](https://img.shields.io/badge/rust-1.80%2B-orange) ![license](https://img.shields.io/badge/license-MIT-blue)

## What it does

* **Stacks N frames per pixel** with a method you pick: median, percentile,
  trimmed max, mean, sum, max. Median is what removes people who walked through
  the shot; percentile keeps the strongest stable contribution of the flash.
* **Aligns frames** by translation (phase correlation) before stacking, so a
  slightly nudged tripod does not smear the result.
* **Manual masks**: paint on any frame to drop it from the stack where the
  photographer stood. Masked pixels fall through to the other frames.
* **Tone mapping**: exposure (manual or auto), ACES / Reinhard / Hable curves,
  sRGB output.
* **Outputs**: 32-bit float linear **EXR** master, 16-bit **TIFF**, 8-bit
  **PNG/JPEG** preview, chosen by the file extension.

## Install

Prebuilt binaries live in [Releases](https://github.com/zheltukheen/hdr_merger/releases)
for Windows, macOS (Intel and Apple Silicon) and Linux.

From source:

```sh
cargo build --release
# binaries: target/release/hdr-merger, target/release/hdr-merger-gui
```

The UI renders through **wgpu** (Metal / Vulkan / DX12). If a machine has no
working adapter, build the OpenGL fallback and select it at runtime:

```sh
cargo build --release -p hdr-merger-gui --features glow-backend
HDR_MERGER_RENDERER=glow ./target/release/hdr-merger-gui
```

`RUST_LOG=wgpu_core=info` prints which adapter was picked.

## CLI

```sh
# median stack, auto exposure, EXR master
hdr-merger merge frames/*.tif -o room.exr --method median --exposure auto

# KEEP the strongest 90% of each pixel and write a display JPEG
hdr-merger merge frames/*.tif -o room.jpg --method p90 --exposure 2.5

# drop the three brightest values per pixel (removes flash hit on the ceiling)
hdr-merger merge frames/*.tif -o room.exr --method trimmed:3

# proxy preview, one third of the resolution
hdr-merger merge frames/*.tif -o preview.jpg --max-dim 2000
```

`hdr-merger methods`, `hdr-merger info photo.exr` and `--help` cover the rest.

## GUI

```sh
hdr-merger-gui
```

Drop a folder (or a batch of files) anywhere in the window, pick the stack
method, press **Preview** (proxy, fast), paint masks on the frames that need it,
then **Export full**.

Everything numeric is typed as well as dragged: slider + value box on every
setting. Recipes (`*.hdrm.json`) store the frames, the masks and every setting,
so a merge can be re-opened and re-rendered later. The look is a single token
file — see [docs/DESIGN.md](docs/DESIGN.md). The UI is a thin shell over the same
engine the CLI uses — no second implementation to drift.

## Input formats

TIFF, PNG, JPEG and EXR. Raw camera files are decoded by the RAW converter of
your choice into a 16-bit linear TIFF first:

```sh
dcraw_emu -4 -T -w -o 1 -q 3 photo.CR3     # -> photo.CR3.tiff, linear 16-bit
```

Encoding is inferred per file: 8-bit means sRGB display values, 16-bit and
32-bit float mean linear light. Override with `--input srgb|linear`.

## Design

* `crates/hdr-core` — the engine. Pure library, no UI, no globals.
* `crates/hdr-cli` — `hdr-merger`.
* `crates/hdr-gui` — `hdr-merger-gui`, an [egui](https://github.com/emilk/egui) app.

See [ARCHITECTURE.md](ARCHITECTURE.md) for the data flow and the invariants,
[docs/UX-SPEC.md](docs/UX-SPEC.md) for every setting and control the app is meant
to expose (and where each idea comes from), [ROADMAP.md](ROADMAP.md) for what is
next, [AGENTS.md](AGENTS.md) for how to work on the repo.

## License

MIT.
