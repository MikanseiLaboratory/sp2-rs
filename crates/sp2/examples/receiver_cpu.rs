//! Receive frames into CPU memory and print statistics.
//!
//! ```text
//! cargo run -p sp2 --example receiver_cpu -- [sender name]
//! ```
//!
//! Without an argument the receiver follows the active (first) sender.

use std::time::{Duration, Instant};

use sp2::{Receiver, ReceiverBackend};

fn main() -> sp2::Result<()> {
    env_logger::init();

    let target = std::env::args().nth(1);
    let mut receiver = match target.as_deref() {
        Some(name) => Receiver::connect(name)?,
        None => Receiver::new()?,
    };
    println!(
        "receiving from {} via {} backend; press Ctrl+C to stop",
        target.as_deref().unwrap_or("<active sender>"),
        sp2::BACKEND
    );

    let mut pixels = Vec::new();
    let mut last_report = Instant::now();
    let mut frames_since_report = 0u32;
    let mut was_connected = false;

    loop {
        sp2::pump_events(Duration::from_millis(1));

        match receiver.receive_pixels(&mut pixels)? {
            Some(frame) => {
                frames_since_report += 1;
                if !was_connected {
                    if let Some(info) = receiver.sender_info() {
                        println!(
                            "connected to \"{}\" ({}x{} {:?}, app={})",
                            info.name,
                            info.width,
                            info.height,
                            info.format,
                            info.app_name.as_deref().unwrap_or("-")
                        );
                    }
                    was_connected = true;
                }
                if last_report.elapsed() >= Duration::from_secs(2) {
                    let fps = frames_since_report as f32 / last_report.elapsed().as_secs_f32();
                    let (b, g, r) = average_bgr(&pixels, frame.format.bytes_per_pixel());
                    println!(
                        "{}x{} frame #{:<8} {:.1} fps  avg BGR = ({b}, {g}, {r})",
                        frame.width, frame.height, frame.frame_id, fps
                    );
                    last_report = Instant::now();
                    frames_since_report = 0;
                }
            }
            None => {
                if was_connected && !receiver.is_connected() {
                    println!("sender disconnected, waiting...");
                    was_connected = false;
                }
                std::thread::sleep(Duration::from_millis(4));
            }
        }
    }
}

/// Average of the first three channels, sampled every 64th pixel.
fn average_bgr(pixels: &[u8], bytes_per_pixel: usize) -> (u8, u8, u8) {
    if bytes_per_pixel < 3 || pixels.len() < bytes_per_pixel {
        return (0, 0, 0);
    }
    let mut sum = [0u64; 3];
    let mut count = 0u64;
    for px in pixels.chunks_exact(bytes_per_pixel).step_by(64) {
        sum[0] += px[0] as u64;
        sum[1] += px[1] as u64;
        sum[2] += px[2] as u64;
        count += 1;
    }
    if count == 0 {
        return (0, 0, 0);
    }
    (
        (sum[0] / count) as u8,
        (sum[1] / count) as u8,
        (sum[2] / count) as u8,
    )
}
