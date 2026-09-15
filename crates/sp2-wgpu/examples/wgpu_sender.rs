//! Render a rotating triangle with wgpu and publish it through Spout / Syphon.
//!
//! ```text
//! cargo run -p sp2-wgpu --example wgpu_sender -- [name] [width] [height]
//! WGPU_BACKEND=vulkan cargo run -p sp2-wgpu --example wgpu_sender   # force a backend
//! ```
//!
//! The triangle is rendered into an offscreen texture that is both shared
//! (`WgpuSender::send`) and blitted to the window, so the window shows exactly
//! what receivers get.

mod common;

use std::time::Instant;

use bytemuck::{Pod, Zeroable};
use sp2::{PixelFormat, SenderBackend};
use sp2_wgpu::WgpuSender;

use common::{Blit, Demo, WindowGpu};

const TRIANGLE_SHADER: &str = r#"
struct Params {
    angle: f32,
    aspect: f32,
    _pad: vec2<f32>,
};
@group(0) @binding(0) var<uniform> params: Params;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec3<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 0.6),
        vec2<f32>(-0.55, -0.4),
        vec2<f32>(0.55, -0.4),
    );
    var colors = array<vec3<f32>, 3>(
        vec3<f32>(1.0, 0.25, 0.2),
        vec3<f32>(0.2, 1.0, 0.35),
        vec3<f32>(0.25, 0.45, 1.0),
    );
    let p = positions[index];
    let c = cos(params.angle);
    let s = sin(params.angle);
    let rotated = vec2<f32>(p.x * c - p.y * s, p.x * s + p.y * c);
    var out: VsOut;
    out.pos = vec4<f32>(rotated.x / params.aspect, rotated.y, 0.0, 1.0);
    out.color = colors[index];
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    angle: f32,
    aspect: f32,
    _pad: [f32; 2],
}

struct SenderDemo {
    sender: WgpuSender,
    offscreen: wgpu::Texture,
    offscreen_view: wgpu::TextureView,
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    blit: Blit,
    blit_bind: wgpu::BindGroup,
    start: Instant,
    frames_since_status: u32,
    last_status: Instant,
}

impl Demo for SenderDemo {
    fn new(ctx: &mut WindowGpu) -> Self {
        let mut args = std::env::args().skip(1);
        let name = args.next().unwrap_or_else(|| "sp2 wgpu Sender".to_owned());
        let width: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1280);
        let height: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(720);
        let format = PixelFormat::Bgra8Unorm;
        let device = &ctx.gpu.device;

        let sender = WgpuSender::new(device, &ctx.gpu.queue, &name, width, height, format)
            .expect("failed to create sender");
        println!(
            "sending \"{}\" {}x{} {:?} via {} backend, transfer path {:?}",
            sender.name(),
            width,
            height,
            format,
            sp2::BACKEND,
            sender.path()
        );

        let offscreen = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: sp2_wgpu::format::texture_format(format),
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let offscreen_view = offscreen.create_view(&wgpu::TextureViewDescriptor::default());

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("triangle"),
            source: wgpu::ShaderSource::Wgsl(TRIANGLE_SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("triangle"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("triangle"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("triangle"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(offscreen.format().into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("triangle"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });

        let blit = Blit::new(device, ctx.config.format, false);
        let blit_bind = blit.bind(device, &offscreen);

        SenderDemo {
            sender,
            offscreen,
            offscreen_view,
            pipeline,
            uniforms,
            bind_group,
            blit,
            blit_bind,
            start: Instant::now(),
            frames_since_status: 0,
            last_status: Instant::now(),
        }
    }

    fn frame(&mut self, ctx: &mut WindowGpu, frame: &wgpu::SurfaceTexture) {
        let device = &ctx.gpu.device;
        let queue = &ctx.gpu.queue;

        let params = Params {
            angle: self.start.elapsed().as_secs_f32(),
            aspect: self.offscreen.width() as f32 / self.offscreen.height() as f32,
            _pad: [0.0; 2],
        };
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&params));

        // 1. Render the shared frame.
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("render"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("triangle"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.offscreen_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.05,
                            g: 0.05,
                            b: 0.08,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit([encoder.finish()]);

        // 2. Share it. The copy is queued behind the render submission above.
        if let Err(e) = self.sender.send(&self.offscreen) {
            eprintln!("send failed: {e}");
        }
        self.frames_since_status += 1;

        // 3. Show the same texture in the window.
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("present"),
        });
        self.blit.draw(
            &mut encoder,
            &view,
            Some(&self.blit_bind),
            wgpu::Color::BLACK,
        );
        queue.submit([encoder.finish()]);
    }

    fn status(&mut self) -> Option<String> {
        let elapsed = self.last_status.elapsed().as_secs_f32();
        let fps = self.frames_since_status as f32 / elapsed.max(1e-3);
        self.frames_since_status = 0;
        self.last_status = Instant::now();
        Some(format!(
            "{:>7} frames sent, {:.1} fps, path {:?}, receivers: {}",
            self.sender.frame_count(),
            fps,
            self.sender.path(),
            match self.sender.has_receivers() {
                Some(true) => "yes",
                Some(false) => "no",
                None => "unknown",
            }
        ))
    }
}

fn main() {
    env_logger::init();
    common::run::<SenderDemo>("sp2 wgpu sender", 960, 540);
}
