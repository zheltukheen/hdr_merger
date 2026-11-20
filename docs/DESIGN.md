# Design system

One look, one spacing scale, one set of controls. Panels do not invent styles;
they call `crates/hdr-gui/src/theme.rs`. If a widget is missing, it is added
there first.

## Tokens

| Token | Value | Used for |
| --- | --- | --- |
| `SP_XS` | 4 px | label to control, badge padding |
| `SP_S` | 8 px | between controls in a group |
| `SP_M` | 12 px | inside cards, button padding |
| `SP_L` | 16 px | between groups, panel margins |
| `SP_XL` | 24 px | section to section |
| `RADIUS` | 6 px | every widget, card, panel |
| `LABEL_W` | 104 px | fixed label column, so sliders align across sections |
| `CONTROL_W` | 132 px | slider track width |
| `PANEL_W` | 336 px | left rail default width |

Colours come from `theme::color` only: `BG`, `PANEL`, `CARD`, `CARD_HOVER`,
`STROKE`, `TEXT`, `TEXT_DIM`, `ACCENT`, `ACCENT_DIM`, `OK`, `WARN`, `ERROR`.
Nothing else defines a colour literal.

Type scale: heading 17, body 13, small 11, monospace 12. Section headers are
10.5 px uppercase `TEXT_DIM`.

## Layout

```
┌───────────────────────────────────────────────────────────┐
│ top bar: name · version · frame badges · status · progress │ 46 px
├───────────────┬───────────────────────────────────────────┤
│ left rail     │ canvas                                    │
│ 336 px        │  empty state / image + histogram          │
│ Sources       │                                           │
│ Stack         │                                           │
│ Masks         │                                           │
│ Tone map      │                                           │
│ Export        │                                           │
├───────────────┴───────────────────────────────────────────┤
│ filmstrip: frame chips, mask dot, click to select          │ 62 px
└───────────────────────────────────────────────────────────┘
```

Every left-rail section is: `theme::section(title)` → control rows → optional
hint. Sections never nest scroll areas except the frame list (max 228 px).

## Components

| Component | Rule |
| --- | --- |
| `control_row` | label column `LABEL_W`, control right after; used by every setting |
| `slider` | slider **plus** `DragValue`; a slider is never the only way to set a value |
| `int_slider` | same for integers |
| `section` | uppercase dim title with `SP_L` above and `SP_XS` below |
| `hint` | 11 px `TEXT_DIM`, one line |
| `badge` | tinted pill for counts and versions |
| `primary_button` / `ghost_button` | accent for the main action, card fill for the rest |
| `icon_button` | 24×22 square, no stroke |

## States

* **Empty** — centred card, drop target copy, path field, "Add folder".
  Highlights accent when files are hovered over the window.
* **Busy** — progress bar in the top bar with the job name; buttons disabled.
* **Error** — status text in `ERROR`, message comes from `HdrError` verbatim.
* **Ok** — status in `OK`, e.g. `saved /path/out.exr`.

## Interaction rules

* Drag and drop anywhere in the window adds files or scans a folder.
* Scroll zooms the canvas, middle/right drag pans, `Space`+drag pans with the
  inspect tool.
* Painting requires an explicit tool (`B` brush, `E` eraser, `I` inspect).
* `Ctrl/Cmd+S` saves the recipe, `Ctrl/Cmd+O` loads it, `Ctrl/Cmd+E` exports.
* Every destructive action (clear all masks, clear frames) confirms nothing:
  the recipe file is the undo.
