# Roadmap

The full control surface — every slider, toggle, shortcut and the competitor it
was borrowed from — lives in [docs/UX-SPEC.md](docs/UX-SPEC.md). This file
sequences it and states what "done" means for each phase.

Status: **v0.1** shipped. The next three phases are about controls and settings,
in the order that unlocks real editing.

## v0.1 — shipped

- [x] Engine: TIFF/PNG/JPEG/EXR decode, linear f32 images, 7 stack methods
- [x] Translation alignment, per-frame exclusion masks, ACES/Reinhard/Hable
- [x] EXR f32 / TIFF16 / PNG / JPEG export, CLI, egui UI
- [x] CI on three OSes, universal macOS release builds
- [x] Session controls: method picker, percentile slider, align toggle, brush,
      auto exposure, output path

## v0.2 — "controls that do the work"

Goal: every number in a stack is reachable, typed and remembered.

**Settings & controls**

- [ ] Typed value entry on every slider (right-click → number), arrow nudge,
      double-click reset — darktable's model
- [ ] Stack panel completes: trimmed count, reference frame, max-shift guard,
      crop-to-intersection, proxy size, thread count
- [ ] Sources panel: capture-time sort, EXIF inspector, include/exclude per
      frame, memory/disk estimate before stacking
- [ ] Export panel: resize (long edge), filename pattern, output folder picker,
      quality slider
- [ ] Project/recipe file `*.hdrm.json` (frames, masks, every setting),
      versioned schema + `--recipe` in the CLI
- [ ] Native RAW decode (CR2/CR3/NEF/ARW/DNG) behind a `raw` feature, so the
      external converter disappears
- [ ] Fast proxy decode: read frames directly at proxy resolution for preview

**Acceptance**: a user can load a folder, set the method and crop, preview,
export at a chosen size, save the recipe, reopen it next week and get the same
result byte for byte.

## v0.3 — "masks and deghosting"

Goal: no frame is thrown away because one person walked through it.

**Settings & controls**

- [ ] Auto-mask movers: median-difference detection with a threshold slider,
      preview overlay, "apply to this frame / all frames" — Lightroom's
      deghost levels, but reviewable
- [ ] Undo/redo for mask strokes (Ctrl/Cmd+Z)
- [ ] Filmstrip + mask overlay toggle (M), eraser (E), brush size (`[` `]`)
- [ ] View controls: zoom fit/100 %, pan, before/after split, live RGB histogram,
      pixel inspector
- [ ] Presets: built-ins ("light-paint interior", "night exterior",
      "bracket HDR") plus user presets stored in the recipe
- [ ] Batch queue: N recipes, one run

**Acceptance**: a 43-frame set with the photographer in every frame is cleaned
without leaving the app once, and the recipe reproduces the masks.

## v0.4 — "tone"

Goal: the display panel stops being a single exposure slider.

**Settings & controls**

- [ ] Tone operators: Filmic (Hable), Durand, Fattal, Reinhard '02 — matching
      what Luminance HDR offers, pickable per recipe
- [ ] White/black point, contrast, highlights/shadows, saturation/vibrance
- [ ] Temperature/tint (as-shot default), structure (micro-contrast), denoise,
      vignette
- [ ] Non-destructive: tone settings never touch the EXR master
- [ ] Histogram read-out under the curve controls

**Acceptance**: two recipes with different curves but the same stack produce the
same master EXR and different display outputs.

## v0.5 — performance and platform

- [ ] SIMD percentile (sort-free selection for fixed N)
- [ ] Optional GPU stack (wgpu) behind a feature flag
- [ ] Streaming EXR export (no full master in RAM)
- [ ] Signed/notarised macOS builds, Windows installer
- [ ] UI localisation (en/ru)
- [ ] Dark-frame subtraction (StarStaX parity) — backlog unless requested

## v1.0 — the contract

- [ ] Stable CLI flags, stable recipe schema with a migration tool
- [ ] `--version` prints engine and schema version
- [ ] Documented engine API with semver guarantees

## Deliberately not doing

* A RAW developer (demosaic, lens correction, colour profiles). Pair with
  darktable/RawTherapee and feed the TIFF in.
* Catalogues, cloud, asset management.
* LUT stores and "look" packs.
* Video export of the stacking process (StarStaX does it well; not our job).

## Competitive position

Only this tool stacks by **percentile / trimmed max over a light-paint set**;
everyone else ships exposure merging (Photomatix, Lightroom, Photoshop, Aurora)
or plain blend modes (StarStaX). The wedge is: keep the strongest stable
contribution per pixel, then remove what moved with masks. The feature matrix
and per-control sourcing are in [docs/UX-SPEC.md](docs/UX-SPEC.md#feature-matrix-what-the-competition-has).
