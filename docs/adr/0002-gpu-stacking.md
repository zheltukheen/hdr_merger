# ADR 0002 — GPU compute for the stack

Status: **proposed** (decision blocked on a measurement, see "Acceptance")

## Context

The user wants GPU compute to be the default stacking path. Before building it,
the CPU path was measured on the real set (43 frames, 5496×3670, macOS M1 Pro,
16 GB, release build, TIFFs in the page cache):

| phase | before | share |
| --- | --- | --- |
| decode (TIFF → linear f32) | 2.5 s | 13 % |
| cache write (shift + f16) | 3.9 s | 20 % |
| stack (median) | 11.4 s | 59 % |
| **total** | **19.4 s** | |

So the stack *is* the biggest phase — the premise "GPU has something to win" is
correct. One CPU fix landed before any GPU code: the stacker sorted all N values
per pixel/channel although only an order statistic is needed.

| method | stack before | stack after (selection) | total after |
| --- | --- | --- | --- |
| median | 11.4 s | 9.1 s | 16.1 s |
| p90 | 18.1 s | 10.0 s | 17.1 s |
| sum | — | 7.1 s | 14.2 s |

The proxy preview (1400 px) is 0.4 s of stack and 2.3 s of decode, so the GPU
does not touch it.

## What is actually hard

The arithmetic is trivial — pick one of 43 values. The difficulty is the data
layout and per-thread state:

* 43 frames × 60 MP × 3 channels × 2 B = **15.5 GB**; it does not fit in VRAM,
  so the GPU pass must be tiled (upload N tiles, dispatch, read back).
* every pixel needs its own order statistic → per-thread private array of 43
  floats (spills to local memory), or one workgroup per pixel (poor occupancy),
  or a multi-pass counting scheme (more bandwidth, no sort).
* the current cache layout `[frame][pixel]` is coalescing-friendly if a thread
  maps to a pixel — that part is fine.

## Options

| Option | Note |
| --- | --- |
| Per-thread insertion sort in a compute shader | simplest, ~460 compare-swaps per pixel/channel, spills |
| Workgroup bitonic sort, one workgroup per pixel | correct, but 180 M tiny workgroups |
| Multi-pass count-below-pivot | no sort, but 6+ passes over 15 GB |
| GPU for sum/mean/max only | trivially parallel, but those are not the default method |
| No GPU | 9.1 s median, memory-bound-ish |

## Acceptance

The GPU path ships as the default only if, on the set above:

* median stack **< 4.5 s** (2× the CPU path), and
* the result is byte-identical to the CPU stack for integer-valued inputs
  (parity test, tolerance 1e-6), and
* it falls back to CPU automatically when no adapter exists or
  `HDR_MERGER_GPU=0` is set.

If the measured win is smaller, the GPU path stays behind a flag and the
roadmap target moves to decode and cache instead (they are 33 % of the run and
have no algorithmic risk).

## Consequences either way

* `StackMethod` is already the seam: both paths produce a `LinearImage` from the
  same cache, so the CLI and the GUI do not care which one ran.
* The cache must stay tile-addressable; the f16 layout is unchanged.
* CI runners have no GPU adapter, so the parity test skips itself and the CPU
  path keeps covering behaviour.
