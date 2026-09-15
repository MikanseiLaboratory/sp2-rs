//! Windows backend backed by `sp2-spout`.

use std::time::Duration;

use sp2_core::{
    DirectoryBackend, FrameInfo, PixelBuffer, PixelFormat, ReceiverBackend, Result, SenderBackend,
    SenderInfo,
};
pub use sp2_spout::{SpoutDirectory, SpoutReceiver, SpoutSender};

use crate::SenderOptions;

/// Backend identifier.
pub const BACKEND: &str = "spout";

/// Spout sender wrapper.
pub struct Sender {
    inner: SpoutSender,
}

impl Sender {
    /// Create a Spout sender from the unified options.
    pub fn new(options: &SenderOptions) -> Result<Self> {
        let inner = SpoutSender::new(&options.name, options.width, options.height, options.format)?;
        if options.set_active {
            inner.set_active()?;
        }
        Ok(Sender { inner })
    }

    /// Wrap an existing Spout sender.
    pub fn from_spout(inner: SpoutSender) -> Self {
        Sender { inner }
    }

    /// The Spout specific sender.
    pub fn as_spout(&self) -> &SpoutSender {
        &self.inner
    }

    /// Mutable access to the Spout specific sender.
    pub fn as_spout_mut(&mut self) -> &mut SpoutSender {
        &mut self.inner
    }
}

impl SenderBackend for Sender {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn id(&self) -> &str {
        self.inner.id()
    }

    fn width(&self) -> u32 {
        self.inner.width()
    }

    fn height(&self) -> u32 {
        self.inner.height()
    }

    fn format(&self) -> PixelFormat {
        self.inner.format()
    }

    fn info(&self) -> SenderInfo {
        self.inner.info()
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        self.inner.resize(width, height)
    }

    fn send_pixels(&mut self, pixels: PixelBuffer<'_>) -> Result<()> {
        self.inner.send_pixels(pixels)
    }

    fn frame_count(&self) -> u64 {
        self.inner.frame_count()
    }
}

/// Spout receiver wrapper.
pub struct Receiver {
    inner: SpoutReceiver,
}

impl Receiver {
    /// Create a receiver following `target` (`None` = active sender).
    pub fn new(target: Option<&str>) -> Result<Self> {
        Ok(Receiver {
            inner: SpoutReceiver::new(target)?,
        })
    }

    /// Wrap an existing Spout receiver.
    pub fn from_spout(inner: SpoutReceiver) -> Self {
        Receiver { inner }
    }

    /// Change the followed sender.
    pub fn set_target(&mut self, target: Option<&str>) {
        self.inner.set_target(target)
    }

    /// The Spout specific receiver.
    pub fn as_spout(&self) -> &SpoutReceiver {
        &self.inner
    }

    /// Mutable access to the Spout specific receiver.
    pub fn as_spout_mut(&mut self) -> &mut SpoutReceiver {
        &mut self.inner
    }
}

impl ReceiverBackend for Receiver {
    fn sender_info(&self) -> Option<&SenderInfo> {
        self.inner.sender_info()
    }

    fn receive_pixels(&mut self, out: &mut Vec<u8>) -> Result<Option<FrameInfo>> {
        self.inner.receive_pixels(out)
    }

    fn is_frame_new(&self) -> bool {
        self.inner.is_frame_new()
    }

    fn frame_id(&self) -> u64 {
        self.inner.frame_id()
    }

    fn disconnect(&mut self) {
        self.inner.disconnect()
    }
}

/// Spout directory wrapper.
pub struct Directory {
    inner: SpoutDirectory,
}

impl Directory {
    /// Open the Spout sender registry.
    pub fn new() -> Result<Self> {
        Ok(Directory {
            inner: SpoutDirectory::new()?,
        })
    }

    /// The Spout specific directory.
    pub fn as_spout(&self) -> &SpoutDirectory {
        &self.inner
    }

    /// Mutable access to the Spout specific directory.
    pub fn as_spout_mut(&mut self) -> &mut SpoutDirectory {
        &mut self.inner
    }
}

impl DirectoryBackend for Directory {
    fn senders(&mut self) -> Result<Vec<SenderInfo>> {
        self.inner.senders()
    }

    fn find(&mut self, name_or_id: &str) -> Result<Option<SenderInfo>> {
        self.inner.find(name_or_id)
    }

    fn active_sender(&mut self) -> Result<Option<SenderInfo>> {
        self.inner.active_sender()
    }

    fn set_active_sender(&mut self, id: &str) -> Result<()> {
        self.inner.set_active_sender(id)
    }
}

/// No-op: Spout needs no event pumping.
pub fn pump_events(_timeout: Duration) {}
