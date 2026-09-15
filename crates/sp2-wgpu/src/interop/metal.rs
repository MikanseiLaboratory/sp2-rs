//! Metal bridge: wraps the Syphon IOSurface as a `wgpu::Texture`.
//!
//! `IOSurface` backed `MTLTexture`s share memory with every other process
//! that looked the surface up, so wrapping one with
//! `wgpu::hal::metal::Device::texture_from_raw` gives wgpu direct access to the
//! shared frame. Senders blit into it (or render into it directly) and
//! publish from a completion callback, receivers blit out of it (or sample
//! it directly in [`ReceiveMode::Shared`]).

use objc2_metal::MTLTextureType;
use sp2::backend::syphon::{FramePublisher, SyphonReceiver, SyphonServer};
use sp2_core::{Error, FrameInfo, PixelFormat, Result, SenderBackend, TransferPath};
use sp2_syphon::iosurface::Surface;

use super::{
    ensure_texture, texture_descriptor, Gpu, ReceiverInterop, SenderInterop,
    RECEIVED_TEXTURE_USAGE, SHARED_TEXTURE_USAGE,
};
use crate::cpu::wait_for_gpu;
use crate::{ReceiveMode, SyncMode};

type Metal = wgpu::hal::api::Metal;

/// Whether `gpu` runs on the Metal backend.
fn is_metal(gpu: &Gpu) -> bool {
    // SAFETY: the guard is dropped immediately without touching the device.
    unsafe { gpu.device.as_hal::<Metal>() }.is_some()
}

/// Wrap `surface` as a wgpu texture on `gpu`.
fn wrap_surface(
    gpu: &Gpu,
    surface: &Surface,
    usage: wgpu::TextureUsages,
    initial_state: wgpu::TextureUses,
    label: &str,
) -> Result<wgpu::Texture> {
    // SAFETY: the HAL device guard only lives for the duration of this
    // function and the device is not destroyed through it.
    let hal_device = unsafe { gpu.device.as_hal::<Metal>() }
        .ok_or(Error::Unsupported("wgpu device is not a Metal device"))?;
    let raw = surface.metal_texture(hal_device.raw_device())?;
    let extent = wgpu::hal::CopyExtent {
        width: surface.width(),
        height: surface.height(),
        depth: 1,
    };
    // SAFETY: `raw` is a 2D BGRA8 texture with one mip level and one layer,
    // matching the descriptor passed to `create_texture_from_hal` below.
    let hal_texture = unsafe {
        wgpu::hal::metal::Device::texture_from_raw(
            raw,
            wgpu::TextureFormat::Bgra8Unorm,
            MTLTextureType::Type2D,
            1,
            1,
            extent,
            None,
        )
    };
    drop(hal_device);
    let desc = texture_descriptor(
        Some(label),
        surface.width(),
        surface.height(),
        PixelFormat::Bgra8Unorm,
        usage,
    );
    // SAFETY: `hal_texture` was created on this device and describes `desc`.
    Ok(unsafe {
        gpu.device
            .create_texture_from_hal::<Metal>(hal_texture, &desc, initial_state)
    })
}

/// Create a Syphon server plus the Metal bridge, or `None` when `gpu` is not
/// a Metal device.
pub fn create_sender(
    gpu: &Gpu,
    options: &sp2::SenderOptions,
) -> Result<Option<(sp2::Sender, Box<dyn SenderInterop>)>> {
    if !is_metal(gpu) {
        return Ok(None);
    }
    let sender = sp2::Sender::with_options(options.clone())?;
    let publisher = sender.platform().as_syphon().frame_publisher();
    Ok(Some((
        sender,
        Box::new(MetalSender {
            shared: None,
            publisher,
        }),
    )))
}

/// Create a Syphon receiver plus the Metal bridge, or `None` when `gpu` is
/// not a Metal device.
pub fn create_receiver(
    gpu: &Gpu,
    target: Option<&str>,
) -> Result<Option<(sp2::Receiver, Box<dyn ReceiverInterop>)>> {
    if !is_metal(gpu) {
        return Ok(None);
    }
    let receiver = super::plain_receiver(target)?;
    Ok(Some((receiver, Box::new(MetalReceiver::default()))))
}

struct SharedTexture {
    surface_id: u32,
    texture: wgpu::Texture,
}

/// Sender bridge for Metal.
pub struct MetalSender {
    shared: Option<SharedTexture>,
    publisher: FramePublisher,
}

impl MetalSender {
    fn ensure_shared(&mut self, gpu: &Gpu, server: &SyphonServer) -> Result<&wgpu::Texture> {
        let surface = server.surface();
        let current = self.shared.as_ref().map(|s| s.surface_id);
        if current != Some(surface.id()) {
            let texture = wrap_surface(
                gpu,
                surface,
                SHARED_TEXTURE_USAGE,
                wgpu::TextureUses::UNINITIALIZED,
                "sp2 Syphon surface (sender)",
            )?;
            self.shared = Some(SharedTexture {
                surface_id: surface.id(),
                texture,
            });
        }
        Ok(&self.shared.as_ref().expect("set above").texture)
    }

    fn publish_after_gpu(&self, gpu: &Gpu, sync: SyncMode) -> Result<()> {
        match sync {
            SyncMode::Callback => {
                let publisher = self.publisher.clone();
                gpu.queue
                    .on_submitted_work_done(move || publisher.publish());
                Ok(())
            }
            SyncMode::Wait => {
                wait_for_gpu(&gpu.device)?;
                self.publisher.publish();
                Ok(())
            }
        }
    }
}

impl SenderInterop for MetalSender {
    fn path(&self) -> TransferPath {
        TransferPath::GpuCopy
    }

    fn send(
        &mut self,
        gpu: &Gpu,
        sender: &mut sp2::Sender,
        src: &wgpu::Texture,
        sync: SyncMode,
    ) -> Result<()> {
        if sender.format() != PixelFormat::Bgra8Unorm {
            return Err(Error::InvalidFormat(sender.format()));
        }
        let server = sender.platform().as_syphon();
        let shared = self.ensure_shared(gpu, server)?;
        gpu.copy_texture(src, shared, "sp2 Syphon publish");
        self.publish_after_gpu(gpu, sync)
    }

    fn shared_texture(
        &mut self,
        gpu: &Gpu,
        sender: &mut sp2::Sender,
    ) -> Result<Option<&wgpu::Texture>> {
        let server = sender.platform().as_syphon();
        self.ensure_shared(gpu, server).map(Some)
    }

    fn publish(&mut self, gpu: &Gpu, _sender: &mut sp2::Sender, sync: SyncMode) -> Result<()> {
        if self.shared.is_none() {
            return Err(Error::InvalidArgument(
                "call shared_texture() and render into it before publish()".into(),
            ));
        }
        self.publish_after_gpu(gpu, sync)
    }
}

/// Receiver bridge for Metal.
#[derive(Default)]
pub struct MetalReceiver {
    shared: Option<SharedTexture>,
    copy: Option<wgpu::Texture>,
    updated: bool,
}

impl ReceiverInterop for MetalReceiver {
    fn path(&self) -> TransferPath {
        TransferPath::GpuCopy
    }

    fn supports_shared(&self) -> bool {
        true
    }

    fn receive(
        &mut self,
        gpu: &Gpu,
        receiver: &mut sp2::Receiver,
        mode: ReceiveMode,
    ) -> Result<Option<FrameInfo>> {
        self.updated = false;
        let syphon: &mut SyphonReceiver = receiver.platform_mut().as_syphon_mut();
        if !syphon.update()? {
            self.disconnect();
            return Ok(None);
        }
        let client = syphon.client_mut().ok_or(Error::NotConnected)?;
        let Some(frame) = client.acknowledge_frame()? else {
            return Ok(None);
        };
        let surface = client.surface().ok_or(Error::NotConnected)?;

        if self.shared.as_ref().map(|s| s.surface_id) != Some(surface.id()) {
            let texture = wrap_surface(
                gpu,
                surface,
                RECEIVED_TEXTURE_USAGE,
                wgpu::TextureUses::COPY_SRC,
                "sp2 Syphon surface (receiver)",
            )?;
            self.shared = Some(SharedTexture {
                surface_id: surface.id(),
                texture,
            });
            self.updated = true;
        }

        if mode == ReceiveMode::Copy {
            let shared = &self.shared.as_ref().expect("set above").texture;
            let copy = ensure_texture(
                gpu,
                &mut self.copy,
                "sp2 received frame",
                frame.width,
                frame.height,
                frame.format,
                RECEIVED_TEXTURE_USAGE,
                &mut self.updated,
            );
            gpu.copy_texture(shared, copy, "sp2 Syphon receive");
        }
        Ok(Some(frame))
    }

    fn texture(&self, mode: ReceiveMode) -> Option<&wgpu::Texture> {
        match mode {
            ReceiveMode::Copy => self.copy.as_ref(),
            ReceiveMode::Shared => self.shared_texture(),
        }
    }

    fn shared_texture(&self) -> Option<&wgpu::Texture> {
        self.shared.as_ref().map(|s| &s.texture)
    }

    fn is_updated(&self) -> bool {
        self.updated
    }

    fn disconnect(&mut self) {
        self.shared = None;
        self.copy = None;
        self.updated = false;
    }
}
