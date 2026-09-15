//! Headless relay: receive a sender, invert its colours on the GPU and publish
//! the result under a new name.
//!
//! ```text
//! cargo run -p sp2-wgpu --example relay -- --help
//! cargo run -p sp2-wgpu --example relay -- "sp2 wgpu Sender" "sp2 Relay"
//! ```
//!
//! Demonstrates chaining a [`WgpuReceiver`] and a [`WgpuSender`] on one
//! device without any window. On macOS the run loop is still pumped from the
//! main thread so Syphon discovery works.

mod common;

use std::time::{Duration, Instant};

use clap::Parser;
use sp2::{PixelFormat, ReceiverBackend, SenderBackend};
use sp2_wgpu::{WgpuReceiver, WgpuSender};

use common::Blit;

#[derive(Parser)]
#[command(about = "Receive a sender, invert colours, and republish")]
struct Args {
    /// Source sender name or id. Omit to follow the active sender
    source: Option<String>,
    /// Output sender name
    #[arg(default_value = "sp2 Relay")]
    output: String,
}

fn main() -> sp2::Result<()> {
    env_logger::init();
    let Args {
        source,
        output: output_name,
    } = Args::parse();

    let gpu = common::init_gpu(common::create_instance(), None);
    let device = &gpu.device;
    let queue = &gpu.queue;

    let mut receiver = match &source {
        Some(source) => WgpuReceiver::connect(device, queue, source)?,
        None => WgpuReceiver::new(device, queue)?,
    };
    let format = PixelFormat::Bgra8Unorm;
    let mut sender: Option<WgpuSender> = None;
    let blit = Blit::new(device, sp2_wgpu::format::texture_format(format), true);
    let mut output: Option<wgpu::Texture> = None;
    let mut bind_group: Option<wgpu::BindGroup> = None;

    println!(
        "relaying {} -> \"{output_name}\" via {} backend (receive path {:?})",
        source.as_deref().unwrap_or("the active sender"),
        sp2::BACKEND,
        receiver.path()
    );

    let mut frames = 0u32;
    let mut last_status = Instant::now();
    loop {
        sp2::pump_events(Duration::from_millis(1));

        let Some(frame) = receiver.receive()? else {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        };
        let Some(input) = receiver.texture() else {
            continue;
        };

        let needs_output = output
            .as_ref()
            .is_none_or(|t| t.width() != frame.width || t.height() != frame.height);
        if needs_output {
            output = Some(device.create_texture(&wgpu::TextureDescriptor {
                label: Some("relay output"),
                size: wgpu::Extent3d {
                    width: frame.width,
                    height: frame.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: sp2_wgpu::format::texture_format(format),
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            }));
        }
        if needs_output || receiver.is_updated() || bind_group.is_none() {
            bind_group = Some(blit.bind(device, input));
        }
        let output = output.as_ref().expect("created above");

        let view = output.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("relay"),
        });
        blit.draw(&mut encoder, &view, bind_group.as_ref(), wgpu::Color::BLACK);
        queue.submit([encoder.finish()]);

        let sender = match sender.as_mut() {
            Some(sender) => sender,
            None => {
                let created = WgpuSender::new(
                    device,
                    queue,
                    &output_name,
                    frame.width,
                    frame.height,
                    format,
                )?;
                println!(
                    "created sender \"{}\" (send path {:?})",
                    created.name(),
                    created.path()
                );
                sender.insert(created)
            }
        };
        sender.send(output)?;
        frames += 1;

        if last_status.elapsed() >= Duration::from_secs(2) {
            let fps = frames as f32 / last_status.elapsed().as_secs_f32();
            let name = receiver
                .sender_info()
                .map(|i| i.name.clone())
                .unwrap_or_default();
            println!(
                "{name} {}x{} -> \"{}\": {fps:.1} fps, {} frames relayed",
                frame.width,
                frame.height,
                sender.name(),
                sender.frame_count()
            );
            frames = 0;
            last_status = Instant::now();
        }
    }
}
