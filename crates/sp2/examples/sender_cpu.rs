//! Publish an animated gradient generated on the CPU.
//!
//! ```text
//! cargo run -p sp2 --example sender_cpu -- --help
//! cargo run -p sp2 --example sender_cpu -- "sp2 CPU Sender" 640 360
//! ```

use std::time::{Duration, Instant};

use clap::Parser;
use sp2::{PixelBuffer, PixelFormat, Sender, SenderBackend};

#[derive(Parser)]
#[command(about = "Publish an animated CPU gradient over Spout / Syphon")]
struct Args {
    /// Sender name
    #[arg(default_value = "sp2 CPU Sender")]
    name: String,
    /// Width in pixels
    #[arg(default_value_t = 640)]
    width: u32,
    /// Height in pixels
    #[arg(default_value_t = 360)]
    height: u32,
}

fn main() -> sp2::Result<()> {
    env_logger::init();

    let Args {
        name,
        width,
        height,
    } = Args::parse();
    let format = PixelFormat::Bgra8Unorm;

    let mut sender = Sender::new(&name, width, height, format)?;
    println!(
        "sending \"{}\" ({}x{} {:?}) via {} backend; press Ctrl+C to stop",
        sender.name(),
        width,
        height,
        format,
        sp2::BACKEND
    );

    let mut pixels = vec![0u8; format.min_buffer_len(width, height)];
    let start = Instant::now();
    let frame_time = Duration::from_micros(16_667);
    let mut last_report = Instant::now();
    let mut frames_since_report = 0u32;

    loop {
        let frame_start = Instant::now();
        let t = start.elapsed().as_secs_f32();
        fill_gradient(&mut pixels, width, height, t);

        sender.send_pixels(PixelBuffer::packed(&pixels, width, height, format)?)?;
        frames_since_report += 1;

        sp2::pump_events(Duration::from_millis(1));

        if last_report.elapsed() >= Duration::from_secs(2) {
            let fps = frames_since_report as f32 / last_report.elapsed().as_secs_f32();
            println!(
                "{:>7} frames sent, {:.1} fps, receivers: {}",
                sender.frame_count(),
                fps,
                match sender.has_receivers() {
                    Some(true) => "yes",
                    Some(false) => "no",
                    None => "unknown",
                }
            );
            last_report = Instant::now();
            frames_since_report = 0;
        }

        if let Some(remaining) = frame_time.checked_sub(frame_start.elapsed()) {
            std::thread::sleep(remaining);
        }
    }
}

/// BGRA gradient that scrolls with time.
fn fill_gradient(pixels: &mut [u8], width: u32, height: u32, t: f32) {
    let phase = (t * 60.0) as u32;
    for y in 0..height {
        let row = &mut pixels[(y * width * 4) as usize..((y + 1) * width * 4) as usize];
        for x in 0..width {
            let px = &mut row[(x * 4) as usize..(x * 4 + 4) as usize];
            px[0] = ((x + phase) * 255 / width.max(1)) as u8; // B
            px[1] = (y * 255 / height.max(1)) as u8; // G
            px[2] = (((x ^ y) + phase) & 0xff) as u8; // R
            px[3] = 255; // A
        }
    }
}
