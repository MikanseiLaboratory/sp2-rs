//! Print every sender / server visible on the system.
//!
//! ```text
//! cargo run -p sp2 --example list_senders
//! ```

use std::time::{Duration, Instant};

use sp2::{Directory, DirectoryBackend};

fn main() -> sp2::Result<()> {
    env_logger::init();
    println!("backend: {}", sp2::BACKEND);

    let mut directory = Directory::new()?;

    // Syphon servers answer discovery asynchronously; give them a moment while
    // pumping the main thread run loop. On Windows the loop returns at once.
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        sp2::pump_events(Duration::from_millis(50));
        if sp2::BACKEND == "spout" {
            break;
        }
    }

    let senders = directory.senders()?;
    let active = directory.active_sender()?;

    if senders.is_empty() {
        println!("no senders found");
        return Ok(());
    }

    for info in &senders {
        let marker = if active.as_ref().map(|a| a.id == info.id).unwrap_or(false) {
            "*"
        } else {
            " "
        };
        println!(
            "{marker} {:<40} {}x{} {:?} app={} id={}",
            info.name,
            info.width,
            info.height,
            info.format,
            info.app_name.as_deref().unwrap_or("-"),
            info.id
        );
    }
    Ok(())
}
