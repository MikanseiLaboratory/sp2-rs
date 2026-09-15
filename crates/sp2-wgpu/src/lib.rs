//! wgpu integration for `sp2-rs`.
//!
//! [`WgpuSender`] publishes `wgpu::Texture`s through the platform sharing
//! protocol (Spout2 on Windows, Syphon on macOS) and [`WgpuReceiver`] turns
//! received frames into `wgpu::Texture`s, using the fastest path the wgpu
//! backend allows:
//!
//! | wgpu backend | path | notes |
//! | --- | --- | --- |
//! | Metal (macOS) | [`TransferPath::GpuCopy`] / [`TransferPath::ZeroCopy`] | The Syphon IOSurface is wrapped as a `wgpu::Texture`; frames are blitted into it, or applications render into it directly. |
//! | DX12 (Windows) | [`TransferPath::GpuCopy`] | A D3D11On12 device on the wgpu command queue copies between wgpu resources and the Spout D3D11 shared texture, like the official `spoutDX12` helper. |
//! | Vulkan (Windows) | [`TransferPath::GpuCopy`] / [`TransferPath::ZeroCopy`] | The legacy (KMT) shared handle is imported with `VK_KHR_external_memory_win32`; requires driver support, otherwise falls back. |
//! | anything else | [`TransferPath::CpuCopy`] | Staging buffer read-back / `write_texture`. |
//!
//! The chosen path is reported by [`WgpuSender::path`] / [`WgpuReceiver::path`]
//! so applications can detect silent fallbacks.
//!
//! ```no_run
//! # fn demo(device: &wgpu::Device, queue: &wgpu::Queue, frame: &wgpu::Texture) -> sp2::Result<()> {
//! use sp2_wgpu::{WgpuReceiver, WgpuSender};
//!
//! let mut sender = WgpuSender::new(device, queue, "My App", 1280, 720, sp2::PixelFormat::Bgra8Unorm)?;
//! // every frame, after rendering into `frame` (`COPY_SRC` usage required):
//! sender.send(frame)?;
//!
//! let mut receiver = WgpuReceiver::connect(device, queue, "My App")?;
//! if let Some(_info) = receiver.receive()? {
//!     let _texture = receiver.texture();
//! }
//! # Ok(()) }
//! ```
//!
//! # Threading and event pumping
//!
//! Everything must be called from a single thread, and on macOS the main
//! thread must pump its run loop (`sp2::pump_events`) for server discovery to
//! work. Publish callbacks ([`SyncMode::Callback`]) fire from `device.poll` /
//! `queue.submit`, so call one of them every frame.

#![warn(missing_docs)]

use sp2_core::{
    Error, FrameInfo, PixelBuffer, PixelFormat, ReceiverBackend, Result, SenderBackend, SenderInfo,
    TransferPath,
};

pub mod cpu;
pub mod format;
pub mod interop;

use interop::{Gpu, ReceiverInterop, SenderInterop};

pub use sp2;
pub use sp2::SenderOptions;
pub use wgpu;

/// How a sender makes sure the GPU finished writing the shared texture
/// before receivers are notified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SyncMode {
    /// Notify receivers from a `Queue::on_submitted_work_done` callback, the
    /// same technique the Syphon framework uses (`addCompletedHandler`). No
    /// CPU stall; the callback runs on the next `device.poll` /
    /// `queue.submit`.
    ///
    /// The DX12 bridge does not need a callback (the D3D11On12 copy is queued
    /// behind the wgpu work on the same command queue) and the Vulkan bridge
    /// always waits, because the Spout access mutex must be released after
    /// the write completed.
    #[default]
    Callback,
    /// Block until the GPU finished, then notify receivers. Lowest latency,
    /// costs a CPU stall per frame.
    Wait,
}

/// Which texture a [`WgpuReceiver`] exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReceiveMode {
    /// Copy every new frame into a texture owned by the receiver. The texture
    /// is stable while the sender keeps its size and never changes while the
    /// application samples it.
    #[default]
    Copy,
    /// Expose the shared texture itself when the backend can alias it (Metal,
    /// Vulkan). Saves a copy but the sender may overwrite the texture while
    /// the application reads it, exactly like using the shared texture
    /// directly with the reference SDKs. Backends without aliasing behave
    /// like [`ReceiveMode::Copy`].
    Shared,
}

/// A sender publishing `wgpu::Texture`s.
pub struct WgpuSender {
    gpu: Gpu,
    inner: sp2::Sender,
    interop: Box<dyn SenderInterop>,
    sync: SyncMode,
}

impl std::fmt::Debug for WgpuSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WgpuSender")
            .field("name", &self.inner.name())
            .field("path", &self.path())
            .field("sync", &self.sync)
            .finish_non_exhaustive()
    }
}

impl WgpuSender {
    /// Create a sender with the given name, size and format.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<Self> {
        Self::with_options(
            device,
            queue,
            SenderOptions::new(name, width, height).format(format),
        )
    }

    /// Create a sender from detailed options.
    pub fn with_options(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        options: SenderOptions,
    ) -> Result<Self> {
        options.validate()?;
        let gpu = Gpu {
            device: device.clone(),
            queue: queue.clone(),
        };
        let (inner, interop) = interop::create_sender(&gpu, &options)?;
        log::info!(
            "sp2-wgpu sender {:?}: transfer path {:?}",
            inner.name(),
            interop.path()
        );
        Ok(WgpuSender {
            gpu,
            inner,
            interop,
            sync: SyncMode::default(),
        })
    }

    /// The transfer path in use.
    pub fn path(&self) -> TransferPath {
        self.interop.path()
    }

    /// The synchronisation mode used by [`WgpuSender::send`].
    pub fn sync_mode(&self) -> SyncMode {
        self.sync
    }

    /// Change the synchronisation mode.
    pub fn set_sync_mode(&mut self, sync: SyncMode) {
        self.sync = sync;
    }

    /// The platform sender.
    pub fn inner(&self) -> &sp2::Sender {
        &self.inner
    }

    /// Mutable access to the platform sender.
    pub fn inner_mut(&mut self) -> &mut sp2::Sender {
        &mut self.inner
    }

    /// The wgpu device.
    pub fn device(&self) -> &wgpu::Device {
        &self.gpu.device
    }

    /// The wgpu queue.
    pub fn queue(&self) -> &wgpu::Queue {
        &self.gpu.queue
    }

    /// Publish mip level 0 of `texture` as the next frame.
    ///
    /// `texture` must be a 2D texture with `COPY_SRC` usage whose format is
    /// copy compatible with the sender format (sRGB variants are accepted).
    /// The sender resizes itself when the texture size differs. Returns the
    /// transfer path used for this frame.
    pub fn send(&mut self, texture: &wgpu::Texture) -> Result<TransferPath> {
        self.validate_source(texture)?;
        if texture.width() != self.inner.width() || texture.height() != self.inner.height() {
            self.inner.resize(texture.width(), texture.height())?;
        }
        match self
            .interop
            .send(&self.gpu, &mut self.inner, texture, self.sync)
        {
            Ok(()) => Ok(self.interop.path()),
            Err(Error::Unsupported(reason)) if self.interop.path() != TransferPath::CpuCopy => {
                log::warn!("sp2-wgpu: {reason}; switching sender to the CPU transfer path");
                self.interop = Box::new(interop::CpuSender::default());
                self.interop
                    .send(&self.gpu, &mut self.inner, texture, self.sync)?;
                Ok(TransferPath::CpuCopy)
            }
            Err(e) => Err(e),
        }
    }

    /// A texture aliasing the shared texture, for rendering into it directly
    /// (Metal and Vulkan only; `None` elsewhere).
    ///
    /// Call [`WgpuSender::publish`] after submitting the work that writes it.
    /// The texture is recreated when the sender is resized, so query it every
    /// frame instead of caching it.
    pub fn shared_texture(&mut self) -> Result<Option<&wgpu::Texture>> {
        self.interop.shared_texture(&self.gpu, &mut self.inner)
    }

    /// Notify receivers that [`WgpuSender::shared_texture`] holds a new frame.
    pub fn publish(&mut self) -> Result<()> {
        self.interop.publish(&self.gpu, &mut self.inner, self.sync)
    }

    fn validate_source(&self, texture: &wgpu::Texture) -> Result<()> {
        if texture.dimension() != wgpu::TextureDimension::D2 {
            return Err(Error::InvalidArgument(
                "only 2D textures can be sent".into(),
            ));
        }
        if texture.sample_count() != 1 {
            return Err(Error::InvalidArgument(
                "multisampled textures must be resolved before sending".into(),
            ));
        }
        if !texture.usage().contains(wgpu::TextureUsages::COPY_SRC) {
            return Err(Error::InvalidArgument(
                "source texture needs COPY_SRC usage".into(),
            ));
        }
        let expected = format::texture_format(self.inner.format());
        if !format::copy_compatible(texture.format(), expected) {
            return Err(Error::InvalidArgument(format!(
                "texture format {:?} does not match sender format {:?}",
                texture.format(),
                self.inner.format()
            )));
        }
        if texture.width() == 0 || texture.height() == 0 {
            return Err(Error::InvalidArgument(
                "texture size must not be zero".into(),
            ));
        }
        Ok(())
    }
}

impl SenderBackend for WgpuSender {
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

/// A receiver producing `wgpu::Texture`s.
pub struct WgpuReceiver {
    gpu: Gpu,
    inner: sp2::Receiver,
    interop: Box<dyn ReceiverInterop>,
    mode: ReceiveMode,
    last_frame: Option<FrameInfo>,
    frame_new: bool,
}

impl std::fmt::Debug for WgpuReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WgpuReceiver")
            .field("sender", &self.inner.sender_info())
            .field("path", &self.path())
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

impl WgpuReceiver {
    /// Create a receiver following the active (or first available) sender.
    ///
    /// ```no_run
    /// # fn demo(device: &wgpu::Device, queue: &wgpu::Queue) -> sp2::Result<()> {
    /// use sp2_wgpu::WgpuReceiver;
    ///
    /// let mut receiver = WgpuReceiver::new(device, queue)?;
    /// if let Some(frame) = receiver.receive()? {
    ///     let texture = receiver.texture().expect("frame implies a texture");
    ///     let _ = (frame.width, frame.height, texture);
    /// }
    /// # Ok(()) }
    /// ```
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Self> {
        Self::with_mode(device, queue, None, ReceiveMode::default())
    }

    /// Create a receiver bound to a sender name or identifier.
    pub fn connect(device: &wgpu::Device, queue: &wgpu::Queue, name_or_id: &str) -> Result<Self> {
        Self::with_mode(device, queue, Some(name_or_id), ReceiveMode::default())
    }

    /// Create a receiver with an explicit target and [`ReceiveMode`].
    pub fn with_mode(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: Option<&str>,
        mode: ReceiveMode,
    ) -> Result<Self> {
        let gpu = Gpu {
            device: device.clone(),
            queue: queue.clone(),
        };
        let (inner, interop) = interop::create_receiver(&gpu, target)?;
        log::info!("sp2-wgpu receiver: transfer path {:?}", interop.path());
        Ok(WgpuReceiver {
            gpu,
            inner,
            interop,
            mode,
            last_frame: None,
            frame_new: false,
        })
    }

    /// The transfer path in use.
    pub fn path(&self) -> TransferPath {
        match (self.interop.path(), self.mode) {
            (TransferPath::GpuCopy, ReceiveMode::Shared)
                if self.interop.shared_texture().is_some() =>
            {
                TransferPath::ZeroCopy
            }
            (path, _) => path,
        }
    }

    /// The receive mode.
    pub fn mode(&self) -> ReceiveMode {
        self.mode
    }

    /// Change the receive mode. Takes effect on the next frame.
    pub fn set_mode(&mut self, mode: ReceiveMode) {
        self.mode = mode;
    }

    /// The platform receiver.
    pub fn inner(&self) -> &sp2::Receiver {
        &self.inner
    }

    /// Mutable access to the platform receiver.
    pub fn inner_mut(&mut self) -> &mut sp2::Receiver {
        &mut self.inner
    }

    /// Change the sender this receiver follows (`None` = active sender).
    pub fn set_target(&mut self, name_or_id: Option<&str>) {
        self.inner.set_target(name_or_id);
        self.interop.disconnect();
        self.last_frame = None;
        self.frame_new = false;
    }

    /// Poll the sender and import the latest frame when it is new.
    ///
    /// Returns `Ok(Some(frame))` when [`WgpuReceiver::texture`] was updated
    /// with a new frame. Check [`WgpuReceiver::is_updated`] to learn whether
    /// the texture object itself was recreated (size change or reconnect), in
    /// which case bind groups referencing it must be rebuilt.
    pub fn receive(&mut self) -> Result<Option<FrameInfo>> {
        let result = self.interop.receive(&self.gpu, &mut self.inner, self.mode);
        let frame = match result {
            Ok(frame) => frame,
            Err(Error::Unsupported(reason)) if self.interop.path() != TransferPath::CpuCopy => {
                log::warn!("sp2-wgpu: {reason}; switching receiver to the CPU transfer path");
                self.interop = Box::new(interop::CpuReceiver::default());
                self.interop
                    .receive(&self.gpu, &mut self.inner, self.mode)?
            }
            Err(e) => return Err(e),
        };
        self.frame_new = frame.is_some();
        if frame.is_some() {
            self.last_frame = frame;
        }
        Ok(frame)
    }

    /// The texture holding the latest received frame, if any.
    pub fn texture(&self) -> Option<&wgpu::Texture> {
        self.interop.texture(self.mode)
    }

    /// A texture aliasing the sender's shared texture, when the backend can
    /// provide one (Metal, Vulkan).
    pub fn shared_texture(&self) -> Option<&wgpu::Texture> {
        self.interop.shared_texture()
    }

    /// Whether the last [`WgpuReceiver::receive`] recreated the texture.
    pub fn is_updated(&self) -> bool {
        self.interop.is_updated()
    }

    /// Description of the last frame imported by [`WgpuReceiver::receive`].
    pub fn last_frame(&self) -> Option<&FrameInfo> {
        self.last_frame.as_ref()
    }

    /// The wgpu device.
    pub fn device(&self) -> &wgpu::Device {
        &self.gpu.device
    }

    /// The wgpu queue.
    pub fn queue(&self) -> &wgpu::Queue {
        &self.gpu.queue
    }
}

impl ReceiverBackend for WgpuReceiver {
    fn sender_info(&self) -> Option<&SenderInfo> {
        self.inner.sender_info()
    }

    /// CPU read path. Frames consumed here are not imported into the GPU
    /// texture and vice versa; use one of the two per receiver.
    fn receive_pixels(&mut self, out: &mut Vec<u8>) -> Result<Option<FrameInfo>> {
        let frame = self.inner.receive_pixels(out)?;
        self.frame_new = frame.is_some();
        if frame.is_some() {
            self.last_frame = frame;
        }
        Ok(frame)
    }

    fn is_frame_new(&self) -> bool {
        self.frame_new
    }

    fn frame_id(&self) -> u64 {
        self.last_frame
            .map(|f| f.frame_id)
            .unwrap_or_else(|| self.inner.frame_id())
    }

    fn disconnect(&mut self) {
        self.inner.disconnect();
        self.interop.disconnect();
        self.last_frame = None;
        self.frame_new = false;
    }
}
