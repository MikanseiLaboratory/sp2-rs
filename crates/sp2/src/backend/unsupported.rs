//! Fallback backend for platforms without Spout2 / Syphon.

use std::time::Duration;

use crate::{
    DirectoryBackend, Error, FrameInfo, PixelBuffer, PixelFormat, ReceiverBackend, Result,
    SenderBackend, SenderInfo, SenderOptions,
};

/// Backend identifier.
pub const BACKEND: &str = "unsupported";

const REASON: &str = "texture sharing is only available on Windows (Spout2) and macOS (Syphon)";

/// Stub sender; construction always fails.
pub struct Sender {
    _private: (),
}

impl Sender {
    /// Always fails with [`Error::Unsupported`].
    pub fn new(_options: &SenderOptions) -> Result<Self> {
        Err(Error::Unsupported(REASON))
    }
}

impl SenderBackend for Sender {
    fn name(&self) -> &str {
        ""
    }

    fn id(&self) -> &str {
        ""
    }

    fn width(&self) -> u32 {
        0
    }

    fn height(&self) -> u32 {
        0
    }

    fn format(&self) -> PixelFormat {
        PixelFormat::Bgra8Unorm
    }

    fn resize(&mut self, _width: u32, _height: u32) -> Result<()> {
        Err(Error::Unsupported(REASON))
    }

    fn send_pixels(&mut self, _pixels: PixelBuffer<'_>) -> Result<()> {
        Err(Error::Unsupported(REASON))
    }

    fn frame_count(&self) -> u64 {
        0
    }
}

/// Stub receiver; construction always fails.
pub struct Receiver {
    _private: (),
}

impl Receiver {
    /// Always fails with [`Error::Unsupported`].
    pub fn new(_target: Option<&str>) -> Result<Self> {
        Err(Error::Unsupported(REASON))
    }

    /// No-op.
    pub fn set_target(&mut self, _target: Option<&str>) {}
}

impl ReceiverBackend for Receiver {
    fn sender_info(&self) -> Option<&SenderInfo> {
        None
    }

    fn receive_pixels(&mut self, _out: &mut Vec<u8>) -> Result<Option<FrameInfo>> {
        Err(Error::Unsupported(REASON))
    }

    fn is_frame_new(&self) -> bool {
        false
    }

    fn frame_id(&self) -> u64 {
        0
    }

    fn disconnect(&mut self) {}
}

/// Stub directory; construction always fails.
pub struct Directory {
    _private: (),
}

impl Directory {
    /// Always fails with [`Error::Unsupported`].
    pub fn new() -> Result<Self> {
        Err(Error::Unsupported(REASON))
    }
}

impl DirectoryBackend for Directory {
    fn senders(&mut self) -> Result<Vec<SenderInfo>> {
        Err(Error::Unsupported(REASON))
    }

    fn active_sender(&mut self) -> Result<Option<SenderInfo>> {
        Err(Error::Unsupported(REASON))
    }

    fn set_active_sender(&mut self, _id: &str) -> Result<()> {
        Err(Error::Unsupported(REASON))
    }
}

/// No-op.
pub fn pump_events(_timeout: Duration) {}
