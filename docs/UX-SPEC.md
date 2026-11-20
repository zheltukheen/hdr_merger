# UX spec — settings, controls, behaviour

What the app must expose, how it is grouped, and where each idea comes from.
This is the contract the roadmap is sequenced against; the code follows it, not
the other way round.

Benchmark: [Photomatix Pro](https://www.hdrsoft.com/),
[Lightroom HDR Merge](https://helpx.adobe.com/lightroom-classic/desktop/process-and-develop-photos/hdr-photo-merge.html),
[Photoshop Merge to HDR Pro](https://helpx.adobe.com/photoshop/using/high-dynamic-range-images.html),
[Luminance HDR](https://luminancehdr.com/),
[HDRMerge](https://github.com/jcelaya/hdrmerge),
[darktable](https://docs.darktable.org/),
[Aurora HDR](https://skylum.com/aurorahdr),
[StarStaX](https://starstax.net/).

## Principles

1. **One window, no modal dialogs.** Every setting lives in a panel; results
   appear in the canvas. The only modal is "unsaved changes".
2. **Preview is a proxy, export is full res.** Any slider moves the proxy
   (≤1400 px, f16 cache) immediately; full res only on Export.
3. **A slider is never the only way to set a value.** Every numeric control
   accepts typed input (right-click), arrow-key nudge, and reset to default
   (double-click label) — darktable's model.
4. **Nothing is lost.** Frames, masks and every setting are saved in a recipe
   file; StarStaX's "stack recipe" is the reference behaviour.
5. **The engine is the truth.** A control that exists only in the UI is a bug;
   the CLI must reach the same state.

## Panels

### 1. Sources

| Control | Type | Range / values | Default | Notes |
| --- | --- | --- | --- | --- |
| Frame list | table, multi-select | — | — | row: name, size, EV, include-toggle, mask badge |
| Include frame | checkbox per row | on/off | on | excluded frames stay in the recipe |
| Frame role | dropdown per row | light / dark / ambient | light | dark = black-frame subtraction, ambient = base plate |
| Add folder / Add files | buttons + drag-and-drop | — | — | filters tiff/png/jpg/exr (raw later) |
| Sort | dropdown | name / capture time / custom | capture time | EXIF order is what light-paint sets want |
| EXIF inspector | collapsible | — | — | camera, lens, EV, ISO, aperture, shutter |
| Memory estimate | read-only | MB / GB | — | StarStaX estimates before stacking |
| Cache precision | dropdown | f16 / f32 | f16 | f32 for forensic work |

### 2. Stack

| Control | Type | Range | Default | Notes |
| --- | --- | --- | --- | --- |
| Method | segmented | median / percentile / trimmed max / mean / sum / max | median | one choice, one place (`stack.rs`) |
| Percentile | slider + typed | 1–99, integer | 90 | enabled only for `percentile` |
| Trimmed count | slider + typed | 0–20 | 3 | enabled only for `trimmed max` |
| Reference frame | dropdown | any included frame | first | alignment anchor |
| Align | toggle | on/off | on | translation now, rotation+scale in v0.3 |
| Max shift | slider + typed | 0–200 px | off | refuse frames that drift further ("this frame is not the same shot") |
| Crop to intersection | toggle | on/off | on | HDRMerge crops to the overlapping area; without it alignment leaves edges |
| Proxy size | dropdown | 1000 / 1400 / 2000 / full | 1400 | preview cost knob |
| Threads | slider + typed | 1..N | all | rayon pool size |

### 3. Masks and deghosting

| Control | Type | Range | Default | Notes |
| --- | --- | --- | --- | --- |
| Brush | tool toggle (B) | — | off | paints exclusion on the selected frame |
| Brush size | slider + `[` `]` | 1–300 px | 24 | in image pixels, not screen |
| Softness | slider | 0–100 % | 35 | feather of the round brush |
| Eraser | tool toggle (E) | — | off | paints the mask back to 0 |
| Mask opacity overlay | toggle (M) | — | on | red 30 % wash over excluded pixels |
| Clear mask | button | — | — | per frame / all frames |
| Auto-mask movers | button + threshold slider | 0–100 % | 50 | median-difference detection, user reviews before apply (v0.3) |
| Show deghost overlay | toggle | — | — | Lightroom's "Show Deghost Overlay" equivalent |

### 4. Tone map (display only — never written to the EXR master)

| Control | Type | Range | Default | Borrowed from |
| --- | --- | --- | --- | --- |
| Exposure | slider + typed | −8..+8 EV (±18 typed) | auto | darktable |
| Auto exposure | toggle | on/off | on | — |
| Target luminance | slider | 0.2–0.9 | 0.55 | our engine's p90 rule |
| Curve | dropdown | ACES / Reinhard / Hable / Linear / None | ACES | Photomatix tone compression, Luminance HDR operators |
| White point / Black point | sliders | 0–100 % | 0 / 0 | Photomatix |
| Contrast | slider | −100..+100 | 0 | Photomatix / Aurora |
| Highlights / Shadows | sliders | −100..+100 | 0 | Lightroom, Aurora gradient filter |
| Saturation / Vibrance | sliders | −100..+100 | 0 | Photomatix colour saturation |
| Temperature / Tint | sliders | 2000–12000 K / −100..+100 | as shot | StarStaX white balance |
| Structure (micro-contrast) | slider | 0–100 | 0 | Aurora "HDR Structure", Photomatix detail contrast |
| Denoise | slider | 0–100 | 0 | StarStaX post-stack denoise |
| Vignette | slider | −100..+100 | 0 | StarStaX |
| Reset all | button | — | — | — |
| Presets | dropdown + save | built-in + user | — | "Light-paint interior", "Night exterior", "Bracket HDR" |

Every tone-map control is a pure function applied to the linear master at
display time, so the same recipe can be re-rendered at any time without
re-stacking.

### 5. View

| Control | Type | Notes |
| --- | --- | --- |
| Zoom | fit / 100 % / 200 % / scroll | 100 % is the defect-hunting mode |
| Pan | space+drag, middle-drag | — |
| Before / after | split handle | compares the reference frame with the merged result |
| Histogram | RGB + luminance | live, from the proxy (StarStaX) |
| Pixel inspector | read-out | x, y, RGB, luminance |
| Filmstrip | bottom strip | click a frame to paint its mask |
| Mask overlay | toggle | see panel 3 |
| Fullscreen | F11 / ⌃⌘F | — |

### 6. Export

| Control | Type | Values | Default |
| --- | --- | --- | --- |
| Format | dropdown | exr (f32 linear) / tiff16 / png8 / jpg8 | exr |
| Quality | slider | 1–100 | 92 (jpg only) |
| Resize | dropdown + number | none / long edge px | none |
| Output folder | path + picker | — | source folder |
| Filename | pattern | `{name}_{method}_{date}` | — |
| Save recipe | toggle | on/off | on |
| Batch queue | list | add many recipes, run sequentially | — |

## Keyboard

| Key | Action |
| --- | --- |
| `B` / `E` | brush / eraser |
| `[` `]` | brush size |
| `M` | mask overlay |
| `Space`+drag | pan |
| `1` / `2` | fit / 100 % |
| `Ctrl/Cmd+Z` / `Shift+Z` | undo / redo mask stroke |
| `Ctrl/Cmd+E` | export |
| `Ctrl/Cmd+S` | save recipe |

## Feature matrix (what the competition has)

| Feature | Photomatix | Lightroom | Photoshop | Luminance HDR | HDRMerge | darktable | StarStaX | Us |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Auto align | ✓ | ✓ | ✓ | ✓ | ✓ (translation) | ✓ | — | ✓ v0.1 |
| Crop to aligned area | ✓ | ✓ | ✓ | — | ✓ | — | — | v0.2 |
| Deghost auto | ✓ | ✓ (4 levels) | ✓ | — | ✓ | — | — | v0.3 |
| Deghost brush | — | overlay only | — | — | — | — | ✓ (streaks) | ✓ v0.1 (masks) |
| Blend modes (add/lighten/…) | — | — | — | — | — | — | ✓ | ✓ v0.1 |
| Percentile / trimmed stack | — | — | — | — | — | — | — | ✓ v0.1 (only us) |
| Tone operators (many) | ✓ | — | limited | ✓ (9) | — | ✓ (filmic) | levels only | 3 + more v0.4 |
| RAW decode | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ (system) | v0.2 |
| Presets | ✓ | ✓ | ✓ | ✓ | — | ✓ | — | v0.3 |
| Recipe / project file | — | — | — | — | — | ✓ (xmp) | ✓ | v0.2 |
| Live histogram | ✓ | ✓ | ✓ | ✓ | — | ✓ | ✓ | v0.3 |
| Dark-frame subtraction | — | — | — | — | — | — | ✓ | backlog |
| Video export of the stack | — | — | — | — | — | — | ✓ | no |
| Batch | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | — | v0.3 |

## Deliberately out of scope

* Replacing a RAW developer (demosaic, lens correction, colour profiles).
* Cloud, catalogue, asset management.
* LUT marketplaces, "look" packs, social export.
* Video/time-lapse rendering of the stacking process (StarStaX does it; not our
  job).

## Sources

* Photomatix Pro manual — tone mapping sliders and their defaults.
* Adobe Lightroom Classic — HDR Merge dialog: Auto Align, Auto Tone, Deghost
  None/Low/Medium/High, Show Deghost Overlay.
* Adobe Photoshop — Merge to HDR Pro: 8/16/32-bit modes, Remove Ghosts, toning
  curve.
* Luminance HDR — HDR creation with alignment plus nine tone mapping operators.
* HDRMerge — translation alignment, automatic crop to the optimal size, no tone
  mapping (raw → DNG).
* darktable — HDR merge with proxy alignment; typed values on every slider
  (right-click, ±18 EV).
* Aurora HDR — structure, HSL, gradient filters.
* StarStaX — six blending modes (lighten/darken/average/add/multiply/subtract),
  dark frames, post-stack adjustments, live histogram, stack recipes.
