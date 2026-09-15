//! DX12 bridge: D3D11On12 device on the wgpu command queue.
//!
//! Spout shares D3D11 textures through legacy (`GetSharedHandle`) handles
//! that D3D12 cannot open, so — like the official `spoutDX12` helper — a
//! D3D11 device is layered on top of the wgpu `ID3D12Device` with
//! `D3D11On12CreateDevice`. Because the D3D11 device submits to the wgpu
//! command queue, D3D11 copies are ordered after previously submitted wgpu
//! work without extra fences.
//!
//! wgpu tracks the resource state of its textures, so the wgpu texture handed
//! to D3D11 must be in a state this bridge knows at `AcquireWrappedResources`
//! time. The bridge therefore never wraps application textures directly;
//! it owns intermediate textures whose last wgpu use is always the same
//! (`COPY_DST` for sending, `COPY_SRC` for receiving) and wraps those:
//!
//! * send: `app texture --wgpu copy--> intermediate --D3D11 CopyResource--> shared`
//! * receive: `shared --D3D11 CopyResource--> intermediate --wgpu copy--> app texture`

use sp2::backend::spout::{SpoutReceiver, SpoutSender};
use sp2_core::{
    Error, FrameInfo, PixelFormat, ReceiverBackend, Result, SenderBackend, TransferPath,
};
use sp2_spout::d3d11::Device as D3D11Device;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
};
use windows::Win32::Graphics::Direct3D11on12::{
    D3D11On12CreateDevice, ID3D11On12Device, D3D11_RESOURCE_FLAGS,
};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_RESOURCE_STATES, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE,
};

use super::{Gpu, ReceiverInterop, SenderInterop, RECEIVED_TEXTURE_USAGE};
use crate::format::texture_format;
use crate::{ReceiveMode, SyncMode};

type Dx12 = wgpu::hal::api::Dx12;

/// D3D11 device layered over the wgpu D3D12 device and queue.
pub struct Bridge {
    device: D3D11Device,
    on12: ID3D11On12Device,
}

// SAFETY: D3D11On12 devices are free threaded like D3D11 devices; every use
// in this crate is additionally serialised by `&mut self`.
unsafe impl Send for Bridge {}
unsafe impl Sync for Bridge {}

impl Bridge {
    /// Create the bridge, or `None` when `gpu` is not a DX12 device.
    pub fn new(gpu: &Gpu) -> Result<Option<Self>> {
        // SAFETY: the guards are dropped at the end of the block and the
        // device / queue are only cloned (COM reference counted).
        let (device12, queue12) = unsafe {
            let Some(hal_device) = gpu.device.as_hal::<Dx12>() else {
                return Ok(None);
            };
            let Some(hal_queue) = gpu.queue.as_hal::<Dx12>() else {
                return Ok(None);
            };
            (hal_device.raw_device().clone(), hal_queue.as_raw().clone())
        };

        let mut device11: Option<ID3D11Device> = None;
        let mut context11: Option<ID3D11DeviceContext> = None;
        let queues = [Some(
            queue12.cast::<windows::core::IUnknown>().map_err(os_err)?,
        )];
        // SAFETY: all pointers are valid for the duration of the call.
        unsafe {
            D3D11On12CreateDevice(
                &device12,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT.0,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                Some(&queues),
                0,
                Some(&mut device11),
                Some(&mut context11),
                None,
            )
        }
        .map_err(|e| Error::os(e.code().0, format!("D3D11On12CreateDevice: {e}")))?;
        let device11 =
            device11.ok_or_else(|| Error::backend("D3D11On12CreateDevice returned no device"))?;
        let on12: ID3D11On12Device = device11.cast().map_err(os_err)?;
        let device = D3D11Device::from_raw(device11)?;
        log::debug!("created D3D11On12 device on the wgpu D3D12 queue");
        Ok(Some(Bridge { device, on12 }))
    }

    /// The layered D3D11 device.
    pub fn device(&self) -> &D3D11Device {
        &self.device
    }

    /// Wrap a wgpu texture as a D3D11 texture.
    ///
    /// `state` is both the state the D3D12 resource is in whenever it is
    /// acquired and the state it is returned to on release.
    fn wrap(&self, texture: &wgpu::Texture, state: D3D12_RESOURCE_STATES) -> Result<Wrapped> {
        // SAFETY: the HAL texture guard is dropped at the end of the block;
        // the resource is cloned (reference counted) before that.
        let resource12 = unsafe {
            let hal = texture
                .as_hal::<Dx12>()
                .ok_or(Error::Unsupported("texture is not a DX12 texture"))?;
            hal.raw_resource().clone()
        };
        let flags = D3D11_RESOURCE_FLAGS {
            BindFlags: 0,
            MiscFlags: 0,
            CPUAccessFlags: 0,
            StructureByteStride: 0,
        };
        let mut wrapped: Option<ID3D11Texture2D> = None;
        // SAFETY: `flags` and `wrapped` outlive the call.
        unsafe {
            self.on12
                .CreateWrappedResource(&resource12, &flags, state, state, &mut wrapped)
        }
        .map_err(|e| Error::os(e.code().0, format!("CreateWrappedResource: {e}")))?;
        let texture11 =
            wrapped.ok_or_else(|| Error::backend("CreateWrappedResource returned nothing"))?;
        let resource11: ID3D11Resource = texture11.cast().map_err(os_err)?;
        Ok(Wrapped {
            texture: texture11,
            resource: resource11,
        })
    }

    /// Run `f` with the wrapped resource acquired for D3D11 use, then release
    /// it and flush so the D3D12 queue observes the D3D11 work.
    fn with_acquired(&self, wrapped: &Wrapped, f: impl FnOnce(&D3D11Device)) {
        let list = [Some(wrapped.resource.clone())];
        // SAFETY: `list` holds valid wrapped resources created by `self.on12`.
        unsafe { self.on12.AcquireWrappedResources(&list) };
        f(&self.device);
        // SAFETY: as above.
        unsafe { self.on12.ReleaseWrappedResources(&list) };
        self.device.flush();
    }
}

fn os_err(e: windows::core::Error) -> Error {
    Error::os(e.code().0, e.to_string())
}

struct Wrapped {
    texture: ID3D11Texture2D,
    resource: ID3D11Resource,
}

/// Create a Spout sender on the D3D11On12 device plus the DX12 bridge.
pub fn create_sender(
    gpu: &Gpu,
    options: &sp2::SenderOptions,
) -> Result<Option<(sp2::Sender, Box<dyn SenderInterop>)>> {
    let Some(bridge) = Bridge::new(gpu)? else {
        return Ok(None);
    };
    let spout = SpoutSender::with_device(
        bridge.device.clone(),
        &options.name,
        options.width,
        options.height,
        options.format,
    )?;
    if options.set_active {
        spout.set_active()?;
    }
    let sender = sp2::Sender::from_platform(sp2::backend::spout::Sender::from_spout(spout));
    Ok(Some((
        sender,
        Box::new(Dx12Sender {
            bridge,
            intermediate: None,
        }),
    )))
}

/// Create a Spout receiver on the D3D11On12 device plus the DX12 bridge.
pub fn create_receiver(
    gpu: &Gpu,
    target: Option<&str>,
) -> Result<Option<(sp2::Receiver, Box<dyn ReceiverInterop>)>> {
    let Some(bridge) = Bridge::new(gpu)? else {
        return Ok(None);
    };
    let spout = SpoutReceiver::with_device(bridge.device.clone(), target)?;
    let receiver = sp2::Receiver::from_platform(sp2::backend::spout::Receiver::from_spout(spout));
    Ok(Some((
        receiver,
        Box::new(Dx12Receiver {
            bridge,
            staging: None,
            output: None,
            updated: false,
        }),
    )))
}

struct Intermediate {
    texture: wgpu::Texture,
    wrapped: Wrapped,
}

/// Sender bridge for DX12.
pub struct Dx12Sender {
    bridge: Bridge,
    intermediate: Option<Intermediate>,
}

impl Dx12Sender {
    fn ensure_intermediate(
        &mut self,
        gpu: &Gpu,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<()> {
        let matches = self.intermediate.as_ref().is_some_and(|i| {
            i.texture.width() == width
                && i.texture.height() == height
                && i.texture.format() == texture_format(format)
        });
        if !matches {
            let texture = gpu.create_texture(
                "sp2 Spout intermediate (sender)",
                width,
                height,
                format,
                wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            );
            let wrapped = self.bridge.wrap(&texture, D3D12_RESOURCE_STATE_COPY_DEST)?;
            self.intermediate = Some(Intermediate { texture, wrapped });
        }
        Ok(())
    }
}

impl SenderInterop for Dx12Sender {
    fn path(&self) -> TransferPath {
        TransferPath::GpuCopy
    }

    fn send(
        &mut self,
        gpu: &Gpu,
        sender: &mut sp2::Sender,
        src: &wgpu::Texture,
        _sync: SyncMode,
    ) -> Result<()> {
        let (width, height, format) = (sender.width(), sender.height(), sender.format());
        self.ensure_intermediate(gpu, width, height, format)?;
        let intermediate = self.intermediate.as_ref().expect("ensured above");
        // Leaves the intermediate in COPY_DST, the state it was wrapped with.
        gpu.copy_texture(src, &intermediate.texture, "sp2 Spout stage");

        let bridge = &self.bridge;
        let spout = sender.platform_mut().as_spout_mut();
        spout.publish_with(|device, shared| {
            bridge.with_acquired(&intermediate.wrapped, |_| {
                device.copy_texture(shared, &intermediate.wrapped.texture);
                device.flush();
            });
            Ok(())
        })
    }
}

/// Receiver bridge for DX12.
pub struct Dx12Receiver {
    bridge: Bridge,
    /// Written by D3D11, read by wgpu; always left in `COPY_SRC`.
    staging: Option<Intermediate>,
    /// Texture handed to the application.
    output: Option<wgpu::Texture>,
    updated: bool,
}

impl Dx12Receiver {
    fn ensure_textures(
        &mut self,
        gpu: &Gpu,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<()> {
        let matches = self.staging.as_ref().is_some_and(|i| {
            i.texture.width() == width
                && i.texture.height() == height
                && i.texture.format() == texture_format(format)
        });
        if matches {
            return Ok(());
        }
        let staging = gpu.create_texture(
            "sp2 Spout intermediate (receiver)",
            width,
            height,
            format,
            wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
        );
        let output = gpu.create_texture(
            "sp2 received frame",
            width,
            height,
            format,
            RECEIVED_TEXTURE_USAGE,
        );
        // Priming copy: initialises both textures for wgpu and leaves the
        // staging texture in COPY_SRC, the state it is wrapped with.
        gpu.copy_texture(&staging, &output, "sp2 Spout prime");
        let wrapped = self
            .bridge
            .wrap(&staging, D3D12_RESOURCE_STATE_COPY_SOURCE)?;
        self.staging = Some(Intermediate {
            texture: staging,
            wrapped,
        });
        self.output = Some(output);
        self.updated = true;
        Ok(())
    }
}

impl ReceiverInterop for Dx12Receiver {
    fn path(&self) -> TransferPath {
        TransferPath::GpuCopy
    }

    fn receive(
        &mut self,
        gpu: &Gpu,
        receiver: &mut sp2::Receiver,
        _mode: ReceiveMode,
    ) -> Result<Option<FrameInfo>> {
        self.updated = false;
        let spout: &mut SpoutReceiver = receiver.platform_mut().as_spout_mut();
        if !spout.update()? {
            self.disconnect();
            return Ok(None);
        }
        let info = spout.sender_info().ok_or(Error::NotConnected)?.clone();
        self.ensure_textures(gpu, info.width, info.height, info.format)?;

        let bridge = &self.bridge;
        let staging = self.staging.as_ref().expect("ensured above");
        let frame = spout.receive_with(|device, shared| {
            bridge.with_acquired(&staging.wrapped, |_| {
                device.copy_texture(&staging.wrapped.texture, shared);
                device.flush();
            });
            Ok(())
        })?;
        if frame.is_some() {
            let output = self.output.as_ref().expect("ensured above");
            gpu.copy_texture(&staging.texture, output, "sp2 Spout receive");
        }
        Ok(frame)
    }

    fn texture(&self, _mode: ReceiveMode) -> Option<&wgpu::Texture> {
        self.output.as_ref()
    }

    fn is_updated(&self) -> bool {
        self.updated
    }

    fn disconnect(&mut self) {
        self.staging = None;
        self.output = None;
        self.updated = false;
    }
}
