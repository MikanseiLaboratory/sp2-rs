//! wgpu integration for `sp2-rs`.
//!
//! Provides `WgpuSender` / `WgpuReceiver` that move frames between
//! `wgpu::Texture` objects and the platform shared texture (Spout2 on Windows,
//! Syphon on macOS) with as few copies as the graphics backend allows.
