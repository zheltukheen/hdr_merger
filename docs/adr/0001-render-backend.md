# ADR 0001 — render backend

Status: accepted (v0.2.1)

## Context

The window was drawn with eframe's default **glow** backend, i.e. OpenGL.
That is the path of least resistance, but it is the weakest link for a
cross-platform desktop app:

* macOS deprecated OpenGL in 10.14; it still works through a compatibility
  layer, but Apple's supported path is Metal.
* Windows OpenGL depends on the vendor driver; on some laptops and VMs it is
  software-rendered or refuses to create a core context.
* Linux/Wayland gives different GL/EGL behaviour per compositor and driver.

The app renders a single full-screen textured quad plus egui's own geometry.
There is no GPU compute in the pipeline — stacking is CPU-bound — so the choice
is about **reachability**, not speed.

## Options

| Option | Verdict |
| --- | --- |
| Stay on glow (OpenGL) | Works today, weakest on macOS and on old Windows drivers |
| Raw Vulkan | **No macOS support at all** (only via MoltenVK translation); not universal, exactly what the user asked to avoid |
| Raw Metal / DX12 | Two more code paths to write and test for one texture blit |
| Switch toolkit (Slint, Iced, Tauri) | Rewrites the whole UI for no benefit; the problem is the backend, not egui |
| **wgpu** | One API, picks Metal on macOS, Vulkan/DX12 on Windows, Vulkan (or GL) on Linux |

## Decision

Use **wgpu** through eframe (`eframe/features = ["wgpu"]`), and keep glow as an
opt-in build feature:

```sh
cargo build --release -p hdr-merger-gui                       # wgpu (default)
cargo build --release -p hdr-merger-gui --features glow-backend
HDR_MERGER_RENDERER=glow ./hdr-merger-gui                     # runtime fallback
```

The renderer is chosen in `select_renderer()`, and the startup line prints which
one is active. `RUST_LOG=wgpu_core=info` prints the adapter that was actually
picked, which is the first thing to ask a user for when the window refuses to
open.

## Consequences

* macOS runs on Metal instead of deprecated OpenGL; Windows gets DX12/Vulkan.
* ~60 extra crates in the dependency tree and a longer cold build; binary is
  ~10.6 MB on macOS arm64.
* Linux without a Vulkan driver now needs the `glow-backend` build — documented
  in the README rather than silently shipping a binary that cannot start.
* The headless UI tests are unaffected: they drive an `egui::Context` and never
  touch a GPU, so CI still runs on a bare runner.
* GPU **compute** is still out of scope; if the stack ever moves to the GPU it
  lands as a compute pass behind the existing `StackMethod` boundary
  (ROADMAP v0.5), not as a renderer change.

## Verification

* `hdr-merger-gui` builds and starts on macOS arm64 with
  `Adapter Metal AdapterInfo { name: "Apple M1 Pro", backend: Metal }`.
* CI builds the wgpu path on Ubuntu, macOS and Windows; release archives carry
  the same binaries.
