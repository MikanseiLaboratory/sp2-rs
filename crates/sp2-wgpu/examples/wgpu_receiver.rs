//! Receive a Spout / Syphon sender into a wgpu texture and show it fullscreen.
//!
//! ```text
//! cargo run -p sp2-wgpu --example wgpu_receiver -- --help
//! cargo run -p sp2-wgpu --example wgpu_receiver -- "sp2 wgpu Sender"
//! cargo run -p sp2-wgpu --example wgpu_receiver -- --shared
//! ```
//!
//! Without a sender name the active (or first) sender is followed. `--shared`
//! samples the sender's shared texture directly where the backend allows it
//! (`ReceiveMode::Shared`) instead of copying every frame.

mod common;

use std::time::Instant;

use clap::Parser;
use sp2::ReceiverBackend;
use sp2_wgpu::{ReceiveMode, WgpuReceiver};

use common::{Blit, Demo, WindowGpu};

#[derive(Parser)]
#[command(about = "Receive a Spout / Syphon sender into a wgpu texture")]
struct Args {
    /// Sender name or id. Omit to follow the active sender
    sender: Option<String>,
    /// Sample the shared texture directly (Metal / Vulkan)
    #[arg(long)]
    shared: bool,
}

struct ReceiverDemo {
    receiver: WgpuReceiver,
    blit: Blit,
    bind_group: Option<wgpu::BindGroup>,
    frames_since_status: u32,
    last_status: Instant,
}

impl Demo for ReceiverDemo {
    type Args = Args;

    fn new(ctx: &mut WindowGpu, args: Args) -> Self {
        let target = args.sender.as_deref();
        let mode = if args.shared {
            ReceiveMode::Shared
        } else {
            ReceiveMode::Copy
        };

        let receiver = WgpuReceiver::with_mode(&ctx.gpu.device, &ctx.gpu.queue, target, mode)
            .expect("failed to create receiver");
        println!(
            "receiving {} via {} backend, transfer path {:?}, mode {:?}",
            target.unwrap_or("the active sender"),
            sp2::BACKEND,
            receiver.path(),
            mode
        );
        let blit = Blit::new(&ctx.gpu.device, ctx.config.format, false);
        ReceiverDemo {
            receiver,
            blit,
            bind_group: None,
            frames_since_status: 0,
            last_status: Instant::now(),
        }
    }

    fn frame(&mut self, ctx: &mut WindowGpu, frame: &wgpu::SurfaceTexture) {
        match self.receiver.receive() {
            Ok(Some(_)) => {
                self.frames_since_status += 1;
                if self.receiver.is_updated() || self.bind_group.is_none() {
                    self.bind_group = self
                        .receiver
                        .texture()
                        .map(|texture| self.blit.bind(&ctx.gpu.device, texture));
                }
            }
            Ok(None) => {
                if !self.receiver.is_connected() {
                    self.bind_group = None;
                }
            }
            Err(e) => {
                eprintln!("receive failed: {e}");
                self.bind_group = None;
            }
        }

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = ctx
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("present"),
            });
        let clear = if self.bind_group.is_some() {
            wgpu::Color::BLACK
        } else {
            wgpu::Color {
                r: 0.15,
                g: 0.05,
                b: 0.05,
                a: 1.0,
            }
        };
        self.blit
            .draw(&mut encoder, &view, self.bind_group.as_ref(), clear);
        ctx.gpu.queue.submit([encoder.finish()]);
    }

    fn status(&mut self) -> Option<String> {
        let elapsed = self.last_status.elapsed().as_secs_f32();
        let fps = self.frames_since_status as f32 / elapsed.max(1e-3);
        self.frames_since_status = 0;
        self.last_status = Instant::now();
        Some(match self.receiver.sender_info() {
            Some(info) => format!(
                "{} {}x{} {:?}: {:.1} fps, frame {}, path {:?}",
                info.name,
                info.width,
                info.height,
                info.format,
                fps,
                self.receiver.frame_id(),
                self.receiver.path()
            ),
            None => "waiting for a sender...".to_owned(),
        })
    }
}

fn main() {
    env_logger::init();
    common::run::<ReceiverDemo>("sp2 wgpu receiver", 960, 540);
}
