//! macOS backend backed by `sp2-syphon`.

use std::time::Duration;

use sp2_core::{
    DirectoryBackend, Error, FrameInfo, PixelBuffer, PixelFormat, ReceiverBackend, Result,
    SenderBackend, SenderInfo,
};
pub use sp2_syphon::{ServerOptions, SyphonClient, SyphonDirectory, SyphonReceiver, SyphonServer};

use crate::SenderOptions;

/// Backend identifier.
pub const BACKEND: &str = "syphon";

/// Syphon server wrapper.
pub struct Sender {
    inner: SyphonServer,
}

impl Sender {
    /// Create a Syphon server from the unified options.
    pub fn new(options: &SenderOptions) -> Result<Self> {
        if !options.format.is_syphon_compatible() {
            return Err(Error::InvalidFormat(options.format));
        }
        let server = SyphonServer::new(
            &options.name,
            options.width,
            options.height,
            ServerOptions {
                private: options.private,
            },
        )?;
        Ok(Sender { inner: server })
    }

    /// The Syphon specific server.
    pub fn as_syphon(&self) -> &SyphonServer {
        &self.inner
    }

    /// Mutable access to the Syphon specific server.
    pub fn as_syphon_mut(&mut self) -> &mut SyphonServer {
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

    fn has_receivers(&self) -> Option<bool> {
        self.inner.has_receivers()
    }
}

/// Syphon receiver wrapper.
pub struct Receiver {
    inner: SyphonReceiver,
}

impl Receiver {
    /// Create a receiver following `target` (UUID or name; `None` = first server).
    pub fn new(target: Option<&str>) -> Result<Self> {
        Ok(Receiver {
            inner: SyphonReceiver::new(target)?,
        })
    }

    /// Change the followed server.
    pub fn set_target(&mut self, target: Option<&str>) {
        self.inner.set_target(target)
    }

    /// The Syphon specific receiver.
    pub fn as_syphon(&self) -> &SyphonReceiver {
        &self.inner
    }

    /// Mutable access to the Syphon specific receiver.
    pub fn as_syphon_mut(&mut self) -> &mut SyphonReceiver {
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

/// Syphon directory wrapper.
pub struct Directory {
    inner: SyphonDirectory,
}

impl Directory {
    /// Start observing Syphon servers.
    pub fn new() -> Result<Self> {
        Ok(Directory {
            inner: SyphonDirectory::new()?,
        })
    }

    /// The Syphon specific directory.
    pub fn as_syphon(&self) -> &SyphonDirectory {
        &self.inner
    }

    /// Mutable access to the Syphon specific directory.
    pub fn as_syphon_mut(&mut self) -> &mut SyphonDirectory {
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

/// Pump the main thread run loop so distributed notifications are delivered.
pub fn pump_events(timeout: Duration) {
    sp2_syphon::run_loop::poll(timeout)
}
