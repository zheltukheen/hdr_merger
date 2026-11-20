# ADR 0002 — GPU compute for the stack

Status: **rejected as the default, kept as an opt-in** (v0.2.3)

## Question

Should the per-pixel stack run on the GPU (wgpu compute) instead of on the CPU,
and be the default?

## What was built

A wgpu compute stacker (`crates/hdr-core/src/gpu.rs` + `stack.wgsl`): one
invocation per pixel channel, insertion sort in a 64-value local array, all six
methods encoded as a method code. It is parity-tested against the CPU path
(`gpu_stack_matches_the_cpu_stack_when_an_adapter_exists`, tolerance 1e-4) and
falls back per strip when there is no adapter, the set has masks, or it has more
than 64 frames.

## Measurement

43 frames, 5496×3670, Apple M1 Pro, release build, TIFFs in the page cache.
Same machine, same run, only the stacker differs:

| method | CPU stack | GPU stack | verdict |
| --- | --- | --- | --- |
| median | 8.7 s | 12.8 s | GPU 1.5× slower |
| p90 | 8.0 s | 10.0 s | GPU 1.25× slower |
| sum (no sorting at all) | 6.6 s | 9.2 s | GPU 1.4× slower |

Total run, median: 15.6 s on CPU vs 20.8 s on GPU.

The acceptance rule written before the implementation was "median stack < 4.5 s",
so it fails by a factor of three.

## Why the GPU loses here

* **The CPU never copies the data.** The f16 strip cache is read straight from
  the page cache; the GPU path uploads ~5.2 GB and reads back ~0.5 GB through
  staging buffers, in 57 strips, each with its own bind group and a blocking
  `device.poll`.
* **The work per pixel is tiny.** One order statistic out of ~40 values is a few
  hundred comparisons. Ten CPU cores with the data already in cache beat a GPU
  that has to be fed.
* **Per-thread state is the wrong shape.** A 64-float local array spills to
  scratch memory and kills occupancy; sorting 40 values per thread is the
  textbook anti-pattern for a GPU.
* Even `sum`, which needs no ordering, lost — which isolates the cause to
  transfer and dispatch overhead, not to the sorting.

## Decision

* CPU stays the default (`MergeOptions::use_gpu = false`).
* The GPU path remains, reachable with `--gpu` / `HDR_MERGER_GPU=1`, because it
  is correct and is the base for a future design that avoids the per-strip round
  trip.
* The performance target moves to **decode + cache** (31–38 % of the run, no
  algorithmic risk) and to SIMD selection on the CPU path.

## What would change the answer

Only a design that does not ship the stack to the GPU per strip:

* keep the whole set resident (needs ~15 GB of VRAM — not available), or
* a fused decode→stack pipeline where the GPU writes the cache and never reads
  it back (removes the 5.2 GB upload and the strip sync), or
* a different algorithm (per-pixel histogram fusion) that trades the sort for
  bandwidth the GPU actually has.

Until one of those is measured to win, "GPU by default" would make the tool
slower for every user.
