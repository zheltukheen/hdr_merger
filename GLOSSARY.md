# Glossary

Domain terms used in code, docs and commit messages. Keep it short and keep it
true — if the code disagrees with this file, one of them is a bug.

**Light painting (flash walk-around)**
One exposure setting, many frames, a flash fired from a different position in
each frame. The union of the frames is the finished lighting. This is the case
the tool was built for.

**Stack**
The N values a single pixel/channel has across all frames, plus the rule that
turns them into one value.

**Stack method**
That rule. `median` removes movers; `p90` keeps the strongest stable
contribution; `sum` is the physically correct total light; `max` is the
lightest frame everywhere.

**Percentile (`p90`)**
Sort the pixel's values ascending and take the value at rank `p/100·(N−1)`.
`p90` over 43 frames ignores the 4 brightest values at that pixel.

**Trimmed max (`trimmed:3`)**
Take the maximum after dropping the `3` brightest values. Cuts a flash hotspot
and a passer-by's highlight without dimming the rest.

**Mask**
Per-frame exclusion map. Painted pixels are dropped for that frame only; the
stack falls through to the other frames. This is how the photographer is removed
from a light-paint set.

**Alignment / shift**
Integer translation `(dx, dy)` that makes a frame line up with the reference
frame, estimated by phase correlation on a ≤1024 px probe. The engine's `Shift`
is the content offset, not the sampling offset.

**Probe**
Downscaled copy used only for alignment. Never written to disk.

**Cache**
Temporary directory of f16 frame dumps written by the pipeline. Removed when the
merge ends.

**Linear light**
Scene-referred values where 2× the number means 2× the photons. All engine math
happens here; sRGB appears only at the IO boundary.

**Proxy / max_dim**
A capped longest edge used for fast previews. `--max-dim 1400` gives a preview
in seconds instead of minutes.

**EV**
Exposure value; `+1 EV` doubles linear light.

**ACES (film curve)**
Tone curve that compresses highlights instead of clipping them. Default display
mapping.

**Auto exposure**
Pick the exposure so the 90th luminance percentile lands on a target (0.55 by
default). Chosen for stability across very different sets, not for taste.

**f16 / half float**
16-bit float used by the cache. Keeps values far above 1.0 (up to 65504), so
highlight headroom survives the trip to disk.

**Master**
The full-resolution linear output, written as 32-bit float EXR with no tone
curve applied.
