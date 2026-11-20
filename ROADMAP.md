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

## v0.2 — shipped

Goal was: every number in a stack is reachable, typed and remembered.

- [x] Typed value entry on every numeric setting (slider + DragValue)
- [x] Stack panel: method picker, percentile, trimmed count, reference frame,
      max-shift guard, crop-to-intersection, preview proxy, input encoding
- [x] Sources panel: capture-time / name sort, include-exclude per frame, mask
      badge, per-frame size, EXIF in the row tooltip
- [x] Masks: brush + eraser, softness, size (`[` `]`), red exclusion overlay,
      filmstrip with the selected frame
- [x] Tone panel: auto or manual exposure, target luminance, ACES / Reinhard /
      Hable / linear
- [x] Export: output path, optional long-edge resize, background export
- [x] Recipe file `*.hdrm.json`: frames, masks, every stack and tone setting,
      machine-readable, one button to save or load
- [x] View: scroll zoom, pan, fit/100 %, live luminance histogram, frame footer
- [x] Empty state: drop target, path field, what the app expects from a set
- [x] Design system: one token file, one control set, one layout
      ([docs/DESIGN.md](docs/DESIGN.md))
- [x] Headless UI smoke tests: the shell renders with and without frames

Left over from this phase (moved to v0.3): thread-count control, memory
estimate before stacking, filename pattern, JPEG quality slider, native RAW,
decode-at-proxy.

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

Measured baseline (43 × 5496×3670, M1 Pro): decode 2.5 s, cache 3.9 s, stack
8.7 s median. The stack is the biggest single phase, but it is not the cheapest
one to attack — see the GPU result below.

- [x] Sort-free selection in the stacker: median 11.4 s → 8.7 s, p90
      18.1 s → 8.0 s
- [x] GPU stacker (wgpu compute), parity-tested against the CPU path
- [x] Measured against the 4.5 s acceptance rule: **12.8 s vs 8.7 s** on median.
      GPU is therefore **opt-in** (`--gpu`, `HDR_MERGER_GPU=1`), not the default
      — [docs/adr/0002-gpu-stacking.md](docs/adr/0002-gpu-stacking.md)
- [ ] Revisit GPU only with a design that removes the per-strip upload and
      readback (fused decode→cache, or histogram fusion)
- [ ] Faster decode and cache — 31–38 % of the run, no algorithmic risk
- [ ] SIMD selection for the CPU path (fixed N)
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
