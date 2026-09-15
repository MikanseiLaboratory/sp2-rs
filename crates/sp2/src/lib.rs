//! Unified, platform independent API for Spout2 (Windows) and Syphon (macOS)
//! compatible texture sharing.
//!
//! ```no_run
//! use sp2::{PixelBuffer, PixelFormat, Sender, SenderBackend};
//!
//! let mut sender = Sender::new("My Sender", 640, 480, PixelFormat::Bgra8Unorm)?;
//! let pixels = vec![0u8; 640 * 480 * 4];
//! sender.send_pixels(PixelBuffer::packed(&pixels, 640, 480, PixelFormat::Bgra8Unorm)?)?;
//! # Ok::<(), sp2::Error>(())
//! ```
//!
//! The [`Sender`], [`Receiver`] and [`Directory`] types wrap the platform
//! implementation selected at compile time:
//!
//! | Platform | Backend crate | Protocol |
//! | --- | --- | --- |
//! | Windows | `sp2-spout` | Spout2 2.007 (D3D11 shared textures) |
//! | macOS | `sp2-syphon` | Syphon (IOSurface) |
//! | other | built-in stub | every call returns [`Error::Unsupported`] |
//!
//! All three types implement the traits from [`sp2_core`] so generic code can
//! be written against [`SenderBackend`], [`ReceiverBackend`] and
//! [`DirectoryBackend`].
//!
//! # Event pumping
//!
//! Syphon discovers servers through distributed notifications that are only
//! delivered while the **main thread** runs its run loop. Applications that do
//! not already run a Cocoa event loop must call [`pump_events`] regularly from
//! the main thread (typically once per frame). On Windows the function is a
//! no-op, so it is safe to call unconditionally.

#![warn(missing_docs)]

use std::time::Duration;

pub use sp2_core::{
    copy_rows, DirectoryBackend, Error, FrameInfo, PixelBuffer, PixelFormat, ReceiverBackend,
    Result, SenderBackend, SenderInfo, TransferPath,
};

pub mod backend;
mod options;

pub use options::SenderOptions;

use backend::imp;

/// Platform sender (Spout sender / Syphon server).
pub struct Sender {
    inner: imp::Sender,
}

impl Sender {
    /// Create a sender with the given name, size and format.
    pub fn new(name: &str, width: u32, height: u32, format: PixelFormat) -> Result<Self> {
        Self::with_options(SenderOptions::new(name, width, height).format(format))
    }

    /// Create a sender from detailed options.
    pub fn with_options(options: SenderOptions) -> Result<Self> {
        options.validate()?;
        Ok(Sender {
            inner: imp::Sender::new(&options)?,
        })
    }

    /// Access the platform implementation.
    pub fn platform(&self) -> &imp::Sender {
        &self.inner
    }

    /// Mutable access to the platform implementation.
    pub fn platform_mut(&mut self) -> &mut imp::Sender {
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

/// Platform receiver (Spout receiver / Syphon client).
pub struct Receiver {
    inner: imp::Receiver,
}

impl Receiver {
    /// Create a receiver that follows the active (or first available) sender.
    ///
    /// The connection is established lazily by the first
    /// [`ReceiverBackend::receive_pixels`] call and re-established
    /// automatically if the sender disappears.
    pub fn new() -> Result<Self> {
        Ok(Receiver {
            inner: imp::Receiver::new(None)?,
        })
    }

    /// Create a receiver bound to a specific sender name or identifier.
    pub fn connect(name_or_id: &str) -> Result<Self> {
        Ok(Receiver {
            inner: imp::Receiver::new(Some(name_or_id))?,
        })
    }

    /// Change the sender this receiver follows (`None` = active sender).
    pub fn set_target(&mut self, name_or_id: Option<&str>) {
        self.inner.set_target(name_or_id);
    }

    /// Access the platform implementation.
    pub fn platform(&self) -> &imp::Receiver {
        &self.inner
    }

    /// Mutable access to the platform implementation.
    pub fn platform_mut(&mut self) -> &mut imp::Receiver {
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

/// Enumerates senders available on the system.
pub struct Directory {
    inner: imp::Directory,
}

impl Directory {
    /// Open the system sender directory.
    pub fn new() -> Result<Self> {
        Ok(Directory {
            inner: imp::Directory::new()?,
        })
    }

    /// Access the platform implementation.
    pub fn platform(&self) -> &imp::Directory {
        &self.inner
    }

    /// Mutable access to the platform implementation.
    pub fn platform_mut(&mut self) -> &mut imp::Directory {
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

/// Process pending platform events for at most `timeout`.
///
/// Required on macOS for server discovery (see the crate documentation);
/// a no-op elsewhere. Must be called from the main thread.
pub fn pump_events(timeout: Duration) {
    imp::pump_events(timeout)
}

/// Name of the backend compiled into this build (`"spout"`, `"syphon"` or
/// `"unsupported"`).
pub const BACKEND: &str = imp::BACKEND;
