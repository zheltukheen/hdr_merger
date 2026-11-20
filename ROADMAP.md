# Roadmap

Status: **v0.1** is the first working drop. Everything below is ordered by what
unblocks the most users per unit of work.

## v0.1 — shipped

- [x] `hdr-core` library: decode (TIFF/PNG/JPEG/EXR), linear light f32 images
- [x] Stack methods: median, percentile, trimmed max, mean, sum, max
- [x] Translation alignment (phase correlation, probe ≤ 1024 px)
- [x] Per-frame exclusion masks with soft brush
- [x] Tone mapping: exposure (manual / auto), ACES, Reinhard, Hable, sRGB
- [x] Export: 32-bit float EXR, 16-bit TIFF, 8-bit PNG/JPEG
- [x] CLI with progress and warnings
- [x] egui desktop UI: frame list, masks, preview, full export
- [x] CI: fmt, clippy, tests, release builds for Windows/macOS/Linux

## v0.2 — the "no external converter" release

- [ ] **Native RAW decoding** (CR2/CR3/NEF/ARW/DNG) via a pure-Rust raw crate,
      behind a `raw` feature; the converter step in the README disappears.
- [ ] **Project files**: save/load the frame list, masks, and settings
      (`*.hdrm.json`, schema-versioned).
- [ ] **Faster preview**: decode directly at proxy size instead of full-size
      decode + downsample (roughly 6× faster on a 60 MP set).
- [ ] **Drag and drop** frames onto the window; native file dialog behind a
      `rfd` feature (GTK dependency kept optional for Linux CI).
- [ ] **Undo/redo** for mask strokes.
- [ ] **Batch mode**: one config, many folders, N outputs (`hdr-merger batch`).

## v0.3 — the "hard sets" release

- [ ] **Rotation + scale alignment** (feature matching) for hand-held sets.
      Requires a resampling cache writer, not just integer translation.
- [ ] **Auto masks**: detect the moving subject from temporal statistics
      (median-difference + connected components) and let the user accept it.
- [ ] **Exposure-bracket aware merge**: recover the camera's EV steps from
      EXIF and weight frames by SNR instead of a fixed method.
- [ ] **Debevec / Robertson HDR merge** as an alternative to stacking, for true
      radiance reconstruction.
- [ ] **Ghost-aware tone map**: local operator (e.g. Mertens fusion) for interiors.

## v0.4 — performance and polish

- [ ] SIMD percentile (sort-free selection network for fixed N).
- [ ] Optional GPU stack path (wgpu) behind a feature flag.
- [ ] Streaming export: write strips of the EXR instead of holding the master.
- [ ] Localisation (ru/en) for the UI.
- [ ] Signed/notarised macOS builds and a Windows installer.

## v1.0 — the contract

- [ ] Stable CLI flags, stable project-file schema, migration tool.
- [ ] Reproducible builds; `--version` prints the engine and the schema version.
- [ ] Documented engine API with semver guarantees.

## Non-goals

* Becoming a full raw developer (demosaic, lens corrections, colour profiles).
  Pair it with darktable / RawTherapee and feed the TIFF in.
* Batch cloud processing, asset management, catalogues.
