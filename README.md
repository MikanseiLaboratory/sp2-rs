# sp2-rs

[![CI](https://github.com/MikanseiLaboratory/sp2-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/MikanseiLaboratory/sp2-rs/actions/workflows/ci.yml)

Pure Rust [Spout2](https://spout.zeal.co/) (Windows) / [Syphon](https://syphon.github.io/) texture sharing.

> **Disclaimer:** Independent community project. Not affiliated with Spout or Syphon.

## Related projects

| Project | Description |
|---------|-------------|
| [Spout2](https://github.com/leadedge/Spout2) | Official Spout2 SDK (D3D11 shared textures) |
| [Syphon](https://github.com/Syphon/Syphon-Framework) | Official Syphon framework (IOSurface) |

## Crates

| Crate | Role |
| --- | --- |
| [`sp2`](crates/sp2) | `Sender` / `Receiver` / `Directory` |
| [`sp2-core`](crates/sp2-core) | Types and traits |
| [`sp2-spout`](crates/sp2-spout) | Spout2 (Windows) |
| [`sp2-syphon`](crates/sp2-syphon) | Syphon (macOS) |
| [`sp2-wgpu`](crates/sp2-wgpu) | wgpu (D3D12 / Vulkan / Metal) |

Linux returns `Error::Unsupported`. macOS is untested. The Syphon backend is implemented, but it has not been run on a Mac. If you try it, call `sp2::pump_events` from the main thread so server discovery works.

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

Windows, from the repo root. Pair a sender and a receiver in two terminals.
Each example accepts `--help`. macOS is untested.

These also talk to existing Spout apps (Resolume, TouchDesigner, OBS, …).

![`wgpu_receiver` showing Resolume Avenue's composition next to `wgpu_sender`](docs/spout2_resolume.jpg)

*Left: Resolume Avenue (`Avenue - Composition`). Bottom right: `wgpu_receiver` displaying that sender. Top right: `wgpu_sender` (`sp2 wgpu Sender`), a separate sender, not Resolume's output.*

| Example | Description |
|---------|-------------|
| `list_senders` | List senders / servers |
| `sender_cpu` | Animated CPU gradient |
| `receiver_cpu` | Print size / fps / average color |
| `wgpu_sender` | Rotating triangle (winit window) |
| `wgpu_receiver` | Fullscreen blit |
| `relay` | Receive → invert → republish (headless) |

```bash
# CPU
cargo run -p sp2 --example sender_cpu -- "sp2 CPU Sender" 640 360
cargo run -p sp2 --example receiver_cpu -- "sp2 CPU Sender"
cargo run -p sp2 --example list_senders

# wgpu
cargo run -p sp2-wgpu --example wgpu_sender -- "sp2 wgpu Sender" 1280 720
cargo run -p sp2-wgpu --example wgpu_receiver -- "sp2 wgpu Sender"
cargo run -p sp2-wgpu --example relay -- "sp2 wgpu Sender" "sp2 Relay"
```

Pick the wgpu backend with `WGPU_BACKEND` (`dx12` / `vulkan` / `metal`). On Windows both Vulkan and DX12 copy on the GPU. See [docs/README.md](docs/README.md) for why the shared texture cannot be sampled in place.

```bash
WGPU_BACKEND=vulkan cargo run -p sp2-wgpu --example wgpu_sender
```

See [docs/README.md](docs/README.md) and [docs/research/](docs/research/) for protocol notes.

## License

BSD 2-Clause — Copyright (c) 2026, MikanseiLaboratory. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
