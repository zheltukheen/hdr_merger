//! GPU stacking through wgpu (Metal / Vulkan / DX12).
//!
//! One invocation per pixel *channel*: it walks the frame axis, keeps the
//! values in a small local array, and reduces them to one number. Consecutive
//! threads read consecutive samples of the same frame, so the read pattern is
//! coalesced even though the cache is frame-major.
//!
//! Deliberately dumb about masks: a set with any painted mask stays on the CPU.
//! A mask would add a second 30 MB buffer per strip for a case that is not
//! performance-critical (the user is painting a handful of frames by hand).

use wgpu::util::DeviceExt;

/// The shader keeps values in `array<f32, 64>`: more frames than that and the
/// CPU path takes over.
pub const MAX_FRAMES: usize = 64;

const SHADER: &str = include_str!("stack.wgsl");

pub struct GpuStacker {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    adapter: String,
}

impl GpuStacker {
    /// `None` when there is no usable adapter — the caller stays on the CPU.
    pub fn new() -> Option<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("hdr-merger-gpu"),
                required_features: wgpu::Features::empty(),
                required_limits: adapter.limits(),
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))
        .ok()?;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("stack"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("stack"),
            layout: None,
            module: &module,
            entry_point: "main",
            compilation_options: Default::default(),
            cache: None,
        });
        let bind_group_layout = pipeline.get_bind_group_layout(0);
        Some(Self {
            device,
            queue,
            pipeline,
            bind_group_layout,
            adapter: format!("{:?} {}", info.backend, info.name),
        })
    }

    pub fn adapter(&self) -> &str {
        &self.adapter
    }

    /// `input` is `frames * pixels * 3` f16 samples, little endian, exactly the
    /// layout the strip cache already has. Returns `pixels * 3` f32 values.
    pub fn stack(
        &self,
        input: &[u8],
        frames: usize,
        pixels: usize,
        method_code: u32,
        method_param: f32,
    ) -> Option<Vec<f32>> {
        let samples = frames.checked_mul(pixels)?.checked_mul(3)?;
        if input.len() < samples * 2 || frames == 0 || frames > MAX_FRAMES {
            return None;
        }
        let total = (pixels * 3) as u64;

        let data = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("stack-input"),
                contents: input,
                usage: wgpu::BufferUsages::STORAGE,
            });
        let output = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stack-output"),
            size: total * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let params = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("stack-params"),
                contents: bytemuck_cast(&[
                    frames as u32,
                    pixels as u32,
                    method_code,
                    method_param.to_bits(),
                ]),
                usage: wgpu::BufferUsages::UNIFORM,
            });

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("stack-bindings"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: data.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: output.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params.as_entire_binding(),
                },
            ],
        });

        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stack-readback"),
            size: total * 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("stack"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("stack-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            let workgroups = total.div_ceil(64) as u32;
            pass.dispatch_workgroups(workgroups, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &staging, 0, total * 4);
        self.queue.submit(Some(encoder.finish()));

        let slice = staging.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device.poll(wgpu::Maintain::Wait);
        receiver.recv().ok()?.ok()?;
        let view = slice.get_mapped_range();
        let mut values = Vec::with_capacity(total as usize);
        for chunk in view.chunks_exact(4) {
            values.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        drop(view);
        staging.unmap();
        Some(values)
    }
}

fn bytemuck_cast(values: &[u32]) -> &[u8] {
    // Safety: u32 -> u8 is a plain reinterpretation, alignment of the source is
    // not required for a byte slice, and the lifetime is tied to the input.
    unsafe { std::slice::from_raw_parts(values.as_ptr() as *const u8, values.len() * 4) }
}

/// `HDR_MERGER_GPU=1` turns the GPU stacker on for every caller, including the
/// GUI. It is opt-in because it measured slower than the CPU path on real
/// sets — see docs/adr/0002-gpu-stacking.md.
pub fn enabled_by_env() -> bool {
    std::env::var("HDR_MERGER_GPU")
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use half::f16;

    fn packed(values: &[f32]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for value in values {
            bytes.extend_from_slice(&f16::from_f32(*value).to_bits().to_le_bytes());
        }
        bytes
    }

    #[test]
    fn median_of_two_frames() {
        let Some(stacker) = GpuStacker::new() else {
            eprintln!("no adapter, skipping");
            return;
        };
        // 1 pixel, 2 frames: median of 0.25 and 0.75 -> 0.5
        let data = packed(&[0.25, 0.25, 0.25, 0.75, 0.75, 0.75]);
        let out = stacker.stack(&data, 2, 1, 2, 0.0).expect("stack");
        eprintln!("gpu median: {out:?}");
        assert!((out[0] - 0.5).abs() < 1e-3, "{out:?}");
    }
}
