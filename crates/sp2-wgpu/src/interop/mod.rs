//! Backend specific bridges between wgpu textures and the platform shared
//! texture.
//!
//! | wgpu backend | platform | module | path |
//! | --- | --- | --- | --- |
//! | Metal | macOS (Syphon) | `metal` | IOSurface backed `MTLTexture` wrapped as `wgpu::Texture` |
//! | DX12 | Windows (Spout) | `dx12` | D3D11On12 device shares the wgpu queue, `CopyResource` to / from the Spout texture |
//! | Vulkan | Windows (Spout) | `vulkan` | `VK_KHR_external_memory_win32` import of the legacy (KMT) shared handle |
//! | other | any | [`cpu`] | staging buffer read-back / upload |
//!
//! Every bridge implements [`SenderInterop`] / [`ReceiverInterop`]; the
//! public types in the crate root pick one at construction time and fall back
//! to the CPU path when a bridge reports [`Error::Unsupported`].

use sp2_core::{Error, FrameInfo, PixelFormat, Result, SenderInfo, TransferPath};

use crate::cpu::{self, Readback};
use crate::format::texture_format;
use crate::{ReceiveMode, SyncMode};

#[cfg(windows)]
pub mod dx12;
#[cfg(target_os = "macos")]
pub mod metal;
#[cfg(windows)]
pub mod vulkan;

/// The wgpu device and queue a sender / receiver works with.
#[derive(Clone)]
pub struct Gpu {
    /// Device that owns every texture handed to the integration.
    pub device: wgpu::Device,
    /// Queue used for copies and publish callbacks.
    pub queue: wgpu::Queue,
}

impl Gpu {
    /// Record a full mip-0 copy between two textures of the same size.
    pub fn copy_texture(&self, src: &wgpu::Texture, dst: &wgpu::Texture, label: &str) {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: src,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: dst,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: dst.width().min(src.width()),
                height: dst.height().min(src.height()),
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
    }

    /// Create a plain 2D texture.
    pub fn create_texture(
        &self,
        label: &str,
        width: u32,
        height: u32,
        format: PixelFormat,
        usage: wgpu::TextureUsages,
    ) -> wgpu::Texture {
        self.device.create_texture(&texture_descriptor(
            Some(label),
            width,
            height,
            format,
            usage,
        ))
    }
}

/// Descriptor for a single-level 2D texture in a shared format.
pub fn texture_descriptor(
    label: Option<&str>,
    width: u32,
    height: u32,
    format: PixelFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::TextureDescriptor<'_> {
    wgpu::TextureDescriptor {
        label,
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: texture_format(format),
        usage,
        view_formats: &[],
    }
}

/// Usage flags of textures handed to applications by a receiver.
pub const RECEIVED_TEXTURE_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::TEXTURE_BINDING
    .union(wgpu::TextureUsages::COPY_SRC)
    .union(wgpu::TextureUsages::COPY_DST);

/// Usage flags of shared textures applications may render into.
pub const SHARED_TEXTURE_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::RENDER_ATTACHMENT
    .union(wgpu::TextureUsages::TEXTURE_BINDING)
    .union(wgpu::TextureUsages::COPY_SRC)
    .union(wgpu::TextureUsages::COPY_DST);

/// Bridge from wgpu textures to the platform sender.
pub trait SenderInterop {
    /// How frames travel to the shared texture.
    fn path(&self) -> TransferPath;

    /// Copy `src` (validated: 2D, copy compatible format, same size as the
    /// sender) into the shared texture and publish it.
    fn send(
        &mut self,
        gpu: &Gpu,
        sender: &mut sp2::Sender,
        src: &wgpu::Texture,
        sync: SyncMode,
    ) -> Result<()>;

    /// A wgpu texture aliasing the shared texture, when the backend can
    /// provide one for applications to render into directly.
    fn shared_texture(
        &mut self,
        _gpu: &Gpu,
        _sender: &mut sp2::Sender,
    ) -> Result<Option<&wgpu::Texture>> {
        Ok(None)
    }

    /// Publish the contents of [`SenderInterop::shared_texture`].
    fn publish(&mut self, _gpu: &Gpu, _sender: &mut sp2::Sender, _sync: SyncMode) -> Result<()> {
        Err(Error::Unsupported(
            "rendering into the shared texture directly is not supported on this backend",
        ))
    }
}

/// Bridge from the platform receiver to wgpu textures.
pub trait ReceiverInterop {
    /// How frames travel from the shared texture.
    fn path(&self) -> TransferPath;

    /// Poll the receiver and, when a new frame is available, make it visible
    /// through [`ReceiverInterop::texture`].
    fn receive(
        &mut self,
        gpu: &Gpu,
        receiver: &mut sp2::Receiver,
        mode: ReceiveMode,
    ) -> Result<Option<FrameInfo>>;

    /// The texture holding the latest frame for `mode`.
    fn texture(&self, mode: ReceiveMode) -> Option<&wgpu::Texture>;

    /// A texture aliasing the shared texture, if the backend has one.
    fn shared_texture(&self) -> Option<&wgpu::Texture> {
        None
    }

    /// Whether the last [`ReceiverInterop::receive`] (re)created textures.
    fn is_updated(&self) -> bool;

    /// Drop every texture; the next poll starts over.
    fn disconnect(&mut self);
}

/// Create the platform sender and the bridge matching the wgpu backend.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(unused_variables))]
pub fn create_sender(
    gpu: &Gpu,
    options: &sp2::SenderOptions,
) -> Result<(sp2::Sender, Box<dyn SenderInterop>)> {
    #[cfg(windows)]
    {
        match dx12::create_sender(gpu, options) {
            Ok(Some(pair)) => return Ok(pair),
            Ok(None) => {}
            Err(e) => log::warn!("D3D12 interop unavailable ({e}); falling back"),
        }
        match vulkan::create_sender(gpu, options) {
            Ok(Some(pair)) => return Ok(pair),
            Ok(None) => {}
            Err(e) => log::warn!("Vulkan interop unavailable ({e}); falling back"),
        }
    }
    #[cfg(target_os = "macos")]
    {
        match metal::create_sender(gpu, options) {
            Ok(Some(pair)) => return Ok(pair),
            Ok(None) => {}
            Err(e) => log::warn!("Metal interop unavailable ({e}); falling back"),
        }
    }
    let sender = sp2::Sender::with_options(options.clone())?;
    log::info!(
        "sp2-wgpu: using CPU transfer path for sender {:?}",
        options.name
    );
    Ok((sender, Box::new(CpuSender::default())))
}

/// Create the platform receiver and the bridge matching the wgpu backend.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(unused_variables))]
pub fn create_receiver(
    gpu: &Gpu,
    target: Option<&str>,
) -> Result<(sp2::Receiver, Box<dyn ReceiverInterop>)> {
    #[cfg(windows)]
    {
        match dx12::create_receiver(gpu, target) {
            Ok(Some(pair)) => return Ok(pair),
            Ok(None) => {}
            Err(e) => log::warn!("D3D12 interop unavailable ({e}); falling back"),
        }
        match vulkan::create_receiver(gpu, target) {
            Ok(Some(pair)) => return Ok(pair),
            Ok(None) => {}
            Err(e) => log::warn!("Vulkan interop unavailable ({e}); falling back"),
        }
    }
    #[cfg(target_os = "macos")]
    {
        match metal::create_receiver(gpu, target) {
            Ok(Some(pair)) => return Ok(pair),
            Ok(None) => {}
            Err(e) => log::warn!("Metal interop unavailable ({e}); falling back"),
        }
    }
    let receiver = plain_receiver(target)?;
    log::info!("sp2-wgpu: using CPU transfer path for receiver");
    Ok((receiver, Box::new(CpuReceiver::default())))
}

/// Create the platform receiver without any interop (used by bridges that
/// share the default platform device).
pub fn plain_receiver(target: Option<&str>) -> Result<sp2::Receiver> {
    match target {
        Some(target) => sp2::Receiver::connect(target),
        None => sp2::Receiver::new(),
    }
}

/// Sender bridge that reads the texture back to the CPU.
#[derive(Default)]
pub struct CpuSender {
    readback: Readback,
    pixels: Vec<u8>,
}

impl SenderInterop for CpuSender {
    fn path(&self) -> TransferPath {
        TransferPath::CpuCopy
    }

    fn send(
        &mut self,
        gpu: &Gpu,
        sender: &mut sp2::Sender,
        src: &wgpu::Texture,
        _sync: SyncMode,
    ) -> Result<()> {
        use sp2_core::SenderBackend;

        let (width, height, format) =
            self.readback
                .read(&gpu.device, &gpu.queue, src, &mut self.pixels)?;
        sender.send_pixels(sp2_core::PixelBuffer::packed(
            &self.pixels,
            width,
            height,
            format,
        )?)
    }
}

/// Receiver bridge that uploads CPU frames into a wgpu texture.
#[derive(Default)]
pub struct CpuReceiver {
    texture: Option<wgpu::Texture>,
    pixels: Vec<u8>,
    updated: bool,
}

impl ReceiverInterop for CpuReceiver {
    fn path(&self) -> TransferPath {
        TransferPath::CpuCopy
    }

    fn receive(
        &mut self,
        gpu: &Gpu,
        receiver: &mut sp2::Receiver,
        _mode: ReceiveMode,
    ) -> Result<Option<FrameInfo>> {
        use sp2_core::ReceiverBackend;

        self.updated = false;
        let Some(frame) = receiver.receive_pixels(&mut self.pixels)? else {
            if !receiver.is_connected() {
                self.texture = None;
            }
            return Ok(None);
        };
        let texture = ensure_texture(
            gpu,
            &mut self.texture,
            "sp2 received frame",
            frame.width,
            frame.height,
            frame.format,
            RECEIVED_TEXTURE_USAGE,
            &mut self.updated,
        );
        cpu::upload(
            &gpu.queue,
            texture,
            &self.pixels,
            frame.width,
            frame.height,
            frame.format,
        )?;
        Ok(Some(frame))
    }

    fn texture(&self, _mode: ReceiveMode) -> Option<&wgpu::Texture> {
        self.texture.as_ref()
    }

    fn is_updated(&self) -> bool {
        self.updated
    }

    fn disconnect(&mut self) {
        self.texture = None;
        self.updated = false;
    }
}

/// (Re)create `slot` when it does not match the requested geometry.
#[allow(clippy::too_many_arguments)]
pub fn ensure_texture<'a>(
    gpu: &Gpu,
    slot: &'a mut Option<wgpu::Texture>,
    label: &str,
    width: u32,
    height: u32,
    format: PixelFormat,
    usage: wgpu::TextureUsages,
    updated: &mut bool,
) -> &'a wgpu::Texture {
    let matches = slot.as_ref().is_some_and(|t| {
        t.width() == width && t.height() == height && t.format() == texture_format(format)
    });
    if !matches {
        *slot = Some(gpu.create_texture(label, width, height, format, usage));
        *updated = true;
    }
    slot.as_ref().expect("created above")
}

/// Geometry of the sender a receiver is connected to, if any.
pub fn connected_info(receiver: &sp2::Receiver) -> Option<SenderInfo> {
    use sp2_core::ReceiverBackend;
    receiver.sender_info().cloned()
}
