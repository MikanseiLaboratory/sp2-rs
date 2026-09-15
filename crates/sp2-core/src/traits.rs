use crate::{FrameInfo, PixelBuffer, PixelFormat, Result, SenderInfo};

/// How a frame travelled between the application texture and the shared
/// texture. Returned by GPU integrations (for example `sp2-wgpu`) so callers
/// can detect silent fallbacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransferPath {
    /// The shared texture is sampled or written in place (Metal / IOSurface).
    ZeroCopy,
    /// One GPU-side copy between the application texture and the shared texture.
    GpuCopy,
    /// The frame was read back to (or uploaded from) CPU memory.
    CpuCopy,
}

/// A sender (Spout) / server (Syphon) publishing frames to other processes.
///
/// Constructors are backend specific; the trait covers the operations that are
/// common to every platform. Frames are always described by [`PixelBuffer`]
/// for the CPU path. Backends additionally expose native texture entry points
/// (`ID3D11Texture2D`, `IOSurfaceRef`, `MTLTexture`, ...).
pub trait SenderBackend {
    /// Human readable sender name.
    fn name(&self) -> &str;

    /// Stable identifier other processes use to connect (see [`SenderInfo::id`]).
    fn id(&self) -> &str;

    /// Current shared texture width in pixels.
    fn width(&self) -> u32;

    /// Current shared texture height in pixels.
    fn height(&self) -> u32;

    /// Pixel format of the shared texture.
    fn format(&self) -> PixelFormat;

    /// Snapshot of the sender description as seen by receivers.
    fn info(&self) -> SenderInfo {
        SenderInfo {
            id: self.id().to_owned(),
            name: self.name().to_owned(),
            app_name: None,
            width: self.width(),
            height: self.height(),
            format: self.format(),
        }
    }

    /// Recreate the shared texture with a new size, keeping the same format.
    fn resize(&mut self, width: u32, height: u32) -> Result<()>;

    /// Publish CPU pixels as the next frame.
    ///
    /// If the geometry of `pixels` differs from the current texture the sender
    /// resizes itself first, mirroring the behaviour of the reference
    /// implementations. The pixel format must match [`SenderBackend::format`].
    fn send_pixels(&mut self, pixels: PixelBuffer<'_>) -> Result<()>;

    /// Number of frames published since creation.
    fn frame_count(&self) -> u64;

    /// Whether at least one receiver is connected, if the backend can tell.
    ///
    /// Syphon tracks clients explicitly; Spout has no such notion and returns
    /// `None`.
    fn has_receivers(&self) -> Option<bool> {
        None
    }
}

/// A receiver (Spout) / client (Syphon) consuming frames from a sender.
///
/// Receivers are polled: every call to [`ReceiverBackend::receive_pixels`]
/// checks whether the sender still exists, follows size changes and copies the
/// latest frame when it is new. This mirrors the "ReceiveTexture" loop of the
/// Spout SDK and keeps the Syphon push notifications behind the same API.
pub trait ReceiverBackend {
    /// Description of the sender currently connected, if any.
    fn sender_info(&self) -> Option<&SenderInfo>;

    /// Whether a sender is currently connected.
    fn is_connected(&self) -> bool {
        self.sender_info().is_some()
    }

    /// Poll the sender and copy the latest frame into `out` when it is new.
    ///
    /// `out` is resized to the tightly packed frame length. Returns
    /// `Ok(Some(frame))` when a new frame was copied, `Ok(None)` when no new
    /// frame is available (or no sender is connected yet).
    fn receive_pixels(&mut self, out: &mut Vec<u8>) -> Result<Option<FrameInfo>>;

    /// Whether the last poll observed a new frame.
    fn is_frame_new(&self) -> bool;

    /// Identifier of the last received frame (`0` if unknown).
    fn frame_id(&self) -> u64;

    /// Drop the current connection. The next poll reconnects if possible.
    fn disconnect(&mut self);
}

/// Enumerates senders / servers available on the system.
pub trait DirectoryBackend {
    /// All senders currently visible.
    fn senders(&mut self) -> Result<Vec<SenderInfo>>;

    /// Find a sender by identifier or name.
    fn find(&mut self, name_or_id: &str) -> Result<Option<SenderInfo>> {
        let senders = self.senders()?;
        Ok(senders
            .iter()
            .find(|s| s.id == name_or_id)
            .or_else(|| senders.iter().find(|s| s.name == name_or_id))
            .cloned())
    }

    /// The "active" sender: the one selected by the user or, failing that, the
    /// first registered sender. Syphon has no notion of an active server and
    /// returns the first server in the directory.
    fn active_sender(&mut self) -> Result<Option<SenderInfo>>;

    /// Mark a sender as active. No-op on platforms without the concept.
    fn set_active_sender(&mut self, id: &str) -> Result<()>;
}
