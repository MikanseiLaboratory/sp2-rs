//! Helpers shared by the `sp2-wgpu` examples: device setup, a fullscreen blit
//! pipeline and a winit application skeleton.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

/// wgpu objects shared by every example.
pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

/// Create an instance (backend selectable with `WGPU_BACKEND`), pick an adapter
/// compatible with `surface` and request a device.
pub fn init_gpu(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>) -> Gpu {
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: surface,
        ..Default::default()
    }))
    .expect("no compatible GPU adapter");
    let info = adapter.get_info();
    println!(
        "adapter: {} ({:?}, {:?})",
        info.name, info.backend, info.device_type
    );
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("sp2 example device"),
        ..Default::default()
    }))
    .expect("failed to create wgpu device");
    Gpu {
        instance,
        adapter,
        device,
        queue,
    }
}

/// Create the wgpu instance, honouring `WGPU_BACKEND` (`dx12`, `vulkan`, `metal`, ...).
pub fn create_instance() -> wgpu::Instance {
    wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env())
}

const BLIT_SHADER: &str = r#"
override invert: bool = false;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    // One triangle covering the whole viewport.
    let x = f32(i32(index & 1u) * 4 - 1);
    let y = f32(i32(index & 2u) * 2 - 1);
    var out: VsOut;
    out.pos = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>((x + 1.0) * 0.5, 1.0 - (y + 1.0) * 0.5);
    return out;
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let color = textureSample(source, source_sampler, in.uv);
    if invert {
        return vec4<f32>(vec3<f32>(1.0) - color.rgb, color.a);
    }
    return color;
}
"#;

/// Fullscreen textured-triangle pipeline used to display or post-process
/// received frames.
pub struct Blit {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl Blit {
    /// Create a blit pipeline rendering into `target_format`. `invert`
    /// selects the colour inverting variant used by the relay example.
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat, invert: bool) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blit"),
            source: wgpu::ShaderSource::Wgsl(BLIT_SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blit"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let constants = [("invert", if invert { 1.0 } else { 0.0 })];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blit"),
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
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &constants,
                    ..Default::default()
                },
                targets: &[Some(target_format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blit"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Blit {
            pipeline,
            layout,
            sampler,
        }
    }

    /// Bind `texture` as the blit source.
    pub fn bind(&self, device: &wgpu::Device, texture: &wgpu::Texture) -> wgpu::BindGroup {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// Draw the bound texture over the whole of `target`.
    pub fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        bind_group: Option<&wgpu::BindGroup>,
        clear: wgpu::Color,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("blit"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if let Some(bind_group) = bind_group {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

/// A window with a configured wgpu surface.
pub struct WindowGpu {
    pub window: Arc<Window>,
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
    pub gpu: Gpu,
}

impl WindowGpu {
    /// Create the window, instance, surface and device.
    pub fn new(event_loop: &ActiveEventLoop, title: &str, width: u32, height: u32) -> Self {
        let attributes = Window::default_attributes()
            .with_title(title)
            .with_inner_size(winit::dpi::LogicalSize::new(width, height));
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .expect("failed to create window"),
        );
        let instance = create_instance();
        let surface = instance
            .create_surface(window.clone())
            .expect("failed to create surface");
        let gpu = init_gpu(instance, Some(&surface));
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&gpu.adapter, size.width.max(1), size.height.max(1))
            .expect("surface not supported by adapter");
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&gpu.device, &config);
        WindowGpu {
            window,
            surface,
            config,
            gpu,
        }
    }

    /// Reconfigure the surface after a resize.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.gpu.device, &self.config);
    }

    /// Acquire the next frame, reconfiguring the surface if needed.
    pub fn acquire(&mut self) -> Option<wgpu::SurfaceTexture> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Some(frame),
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => None,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.gpu.device, &self.config);
                None
            }
            wgpu::CurrentSurfaceTexture::Validation => None,
        }
    }
}

/// Per-frame callbacks implemented by the windowed examples.
pub trait Demo: Sized {
    /// CLI parsed in `main` before the window opens (`--help` exits cleanly).
    type Args: Parser;
    /// Create GPU resources once the window exists.
    fn new(ctx: &mut WindowGpu, args: Self::Args) -> Self;
    /// Render one frame into `frame` and do the sharing work.
    fn frame(&mut self, ctx: &mut WindowGpu, frame: &wgpu::SurfaceTexture);
    /// Optional periodic status line (every 2 s).
    fn status(&mut self) -> Option<String> {
        None
    }
}

struct App<D: Demo> {
    title: String,
    size: (u32, u32),
    args: Option<D::Args>,
    ctx: Option<WindowGpu>,
    demo: Option<D>,
    last_status: std::time::Instant,
}

impl<D: Demo> ApplicationHandler for App<D> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.ctx.is_none() {
            let args = self.args.take().expect("args already consumed");
            let mut ctx = WindowGpu::new(event_loop, &self.title, self.size.0, self.size.1);
            self.demo = Some(D::new(&mut ctx, args));
            self.ctx = Some(ctx);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let (Some(ctx), Some(demo)) = (self.ctx.as_mut(), self.demo.as_mut()) else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => ctx.resize(size.width, size.height),
            WindowEvent::RedrawRequested => {
                // Syphon discovery needs the main thread run loop; a no-op on Windows.
                sp2::pump_events(Duration::ZERO);
                if let Some(frame) = ctx.acquire() {
                    demo.frame(ctx, &frame);
                    ctx.window.pre_present_notify();
                    ctx.gpu.queue.present(frame);
                }
                if self.last_status.elapsed() >= Duration::from_secs(2) {
                    if let Some(line) = demo.status() {
                        println!("{line}");
                    }
                    self.last_status = std::time::Instant::now();
                }
                ctx.window.request_redraw();
            }
            _ => {}
        }
    }
}

/// Run a windowed demo until the window is closed.
pub fn run<D: Demo>(title: &str, width: u32, height: u32) {
    let args = D::Args::parse();
    let event_loop = EventLoop::new().expect("failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::<D> {
        title: title.to_owned(),
        size: (width, height),
        args: Some(args),
        ctx: None,
        demo: None,
        last_status: std::time::Instant::now(),
    };
    event_loop.run_app(&mut app).expect("event loop failed");
}
