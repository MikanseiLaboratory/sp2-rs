# sp2-rs

[![CI](https://github.com/MikanseiLaboratory/sp2-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/MikanseiLaboratory/sp2-rs/actions/workflows/ci.yml)

Pure Rust [Spout2](https://spout.zeal.co/) (Windows) / [Syphon](https://syphon.github.io/) (macOS) texture sharing.

Not a C++ / Objective-C wrapper — the on-wire / shared-memory protocols are reimplemented in Rust.

This project is not affiliated with Spout or Syphon.

## Crates

| Crate | Role |
| --- | --- |
| [`sp2`](crates/sp2) | `Sender` / `Receiver` / `Directory` |
| [`sp2-core`](crates/sp2-core) | Types and traits |
| [`sp2-spout`](crates/sp2-spout) | Spout2 (Windows) |
| [`sp2-syphon`](crates/sp2-syphon) | Syphon (macOS) |
| [`sp2-wgpu`](crates/sp2-wgpu) | wgpu (D3D12 / Vulkan / Metal) |

Linux returns `Error::Unsupported`. On macOS, call `sp2::pump_events` from the main thread so Syphon discovery works.

## Usage

```rust
use sp2::{PixelBuffer, PixelFormat, Receiver, ReceiverBackend, Sender, SenderBackend};

let mut sender = Sender::new("My Sender", 640, 360, PixelFormat::Bgra8Unorm)?;
let pixels = vec![0u8; 640 * 360 * 4];
sender.send_pixels(PixelBuffer::packed(&pixels, 640, 360, PixelFormat::Bgra8Unorm)?)?;

let mut receiver = Receiver::connect("My Sender")?;
let mut pixels = Vec::new();
if let Some(frame) = receiver.receive_pixels(&mut pixels)? {
    println!("{}x{} #{}", frame.width, frame.height, frame.frame_id);
}
```

wgpu (`COPY_SRC` on the source texture):

```rust
use sp2::PixelFormat;
use sp2_wgpu::{WgpuReceiver, WgpuSender};

let mut sender = WgpuSender::new(&device, &queue, "My App", 1280, 720, PixelFormat::Bgra8Unorm)?;
sender.send(&frame)?;

let mut receiver = WgpuReceiver::connect(&device, &queue, "My App")?;
if receiver.receive()?.is_some() {
    let _texture = receiver.texture();
}
```

## Examples

Windows / macOS, from the repo root:

```bash
cargo run -p sp2 --example list_senders
cargo run -p sp2 --example sender_cpu -- "sp2 CPU Sender" 640 360
cargo run -p sp2 --example receiver_cpu -- "sp2 CPU Sender"

cargo run -p sp2-wgpu --example wgpu_sender -- "sp2 wgpu Sender" 1280 720
cargo run -p sp2-wgpu --example wgpu_receiver -- "sp2 wgpu Sender"
cargo run -p sp2-wgpu --example wgpu_receiver -- "sp2 wgpu Sender" --shared
cargo run -p sp2-wgpu --example relay -- "sp2 wgpu Sender" "sp2 Relay"

WGPU_BACKEND=dx12 cargo run -p sp2-wgpu --example wgpu_sender
WGPU_BACKEND=vulkan cargo run -p sp2-wgpu --example wgpu_sender
```

Works with existing Spout / Syphon apps (Resolume, TouchDesigner, VDMX, OBS, …).

See [docs/](docs/) for protocol notes and the implementation plan.

## License

[BSD 2-Clause](LICENSE). Copyright (c) 2026, MikanseiLaboratory.

Spout2 and Syphon copyright notices are in [NOTICE](NOTICE).
