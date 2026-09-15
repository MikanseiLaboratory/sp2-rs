//! Vulkan bridge: imports the Spout shared handle with
//! `VK_KHR_external_memory_win32`.
//!
//! Spout publishes legacy (KMT) D3D11 shared handles. Vulkan can import them
//! as `VK_EXTERNAL_MEMORY_HANDLE_TYPE_D3D11_TEXTURE_KMT_BIT` memory bound to
//! a dedicated `VkImage`, which is then wrapped with
//! `wgpu::hal::vulkan::Device::texture_from_raw`. The wrapped image aliases
//! the D3D11 texture, so a single wgpu copy moves a frame in or out and
//! [`ReceiveMode::Shared`] can sample the sender's texture directly.
//!
//! Requirements: the wgpu device must have `VK_KHR_external_memory_win32`
//! enabled (wgpu enables it automatically when the driver supports it) and
//! the Vulkan device must be the same adapter the sender created the D3D11
//! texture on. When either check fails the bridge reports
//! [`Error::Unsupported`] and the caller falls back to the CPU path.
//!
//! Synchronisation: Spout receivers read the texture through D3D11 as soon as
//! the access mutex is released, so the bridge waits for the wgpu copy to
//! complete while holding the mutex (the D3D11 reference implementation
//! achieves the same ordering with `Flush()` on a shared device).

use ash::vk;
use sp2::backend::spout::{SpoutReceiver, SpoutSender};
use sp2_core::{Error, FrameInfo, PixelFormat, ReceiverBackend, Result, TransferPath};
use windows::Win32::Foundation::HANDLE;

use super::{
    ensure_texture, texture_descriptor, Gpu, ReceiverInterop, SenderInterop,
    RECEIVED_TEXTURE_USAGE, SHARED_TEXTURE_USAGE,
};
use crate::cpu::wait_for_gpu;
use crate::format::texture_format;
use crate::{ReceiveMode, SyncMode};

type Vulkan = wgpu::hal::api::Vulkan;

const KMT: vk::ExternalMemoryHandleTypeFlags = vk::ExternalMemoryHandleTypeFlags::D3D11_TEXTURE_KMT;

/// Whether `gpu` is a Vulkan device with the Win32 external memory extension.
///
/// Returns `Ok(false)` for non-Vulkan devices and `Err(Unsupported)` for
/// Vulkan devices without the extension.
fn check_support(gpu: &Gpu) -> Result<bool> {
    // SAFETY: the guard is dropped at the end of the block.
    let Some(hal) = (unsafe { gpu.device.as_hal::<Vulkan>() }) else {
        return Ok(false);
    };
    if !hal
        .enabled_device_extensions()
        .contains(&ash::khr::external_memory_win32::NAME)
    {
        return Err(Error::Unsupported(
            "wgpu Vulkan device lacks VK_KHR_external_memory_win32",
        ));
    }
    Ok(true)
}

/// Vulkan format for a shared texture format.
fn vk_format(format: PixelFormat) -> Result<vk::Format> {
    Ok(match format {
        PixelFormat::Bgra8Unorm => vk::Format::B8G8R8A8_UNORM,
        PixelFormat::Bgra8UnormSrgb => vk::Format::B8G8R8A8_SRGB,
        PixelFormat::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
        PixelFormat::Rgba8UnormSrgb => vk::Format::R8G8B8A8_SRGB,
        PixelFormat::Rgba16Unorm => vk::Format::R16G16B16A16_UNORM,
        PixelFormat::Rgba16Float => vk::Format::R16G16B16A16_SFLOAT,
        PixelFormat::Rgba32Float => vk::Format::R32G32B32A32_SFLOAT,
        PixelFormat::Rgb10a2Unorm => vk::Format::A2B10G10R10_UNORM_PACK32,
        _ => return Err(Error::InvalidFormat(format)),
    })
}

/// HAL usage flags for the wgpu usages this crate hands out.
fn hal_uses(usage: wgpu::TextureUsages) -> wgpu::TextureUses {
    let mut uses = wgpu::TextureUses::empty();
    if usage.contains(wgpu::TextureUsages::COPY_SRC) {
        uses |= wgpu::TextureUses::COPY_SRC;
    }
    if usage.contains(wgpu::TextureUsages::COPY_DST) {
        uses |= wgpu::TextureUses::COPY_DST;
    }
    if usage.contains(wgpu::TextureUsages::TEXTURE_BINDING) {
        uses |= wgpu::TextureUses::RESOURCE;
    }
    if usage.contains(wgpu::TextureUsages::RENDER_ATTACHMENT) {
        uses |= wgpu::TextureUses::COLOR_TARGET;
    }
    if usage.contains(wgpu::TextureUsages::STORAGE_BINDING) {
        uses |= wgpu::TextureUses::STORAGE_READ_WRITE;
    }
    uses
}

/// Initial state that makes wgpu treat the image as `VK_IMAGE_LAYOUT_GENERAL`.
///
/// wgpu derives the "old layout" of the first barrier from this value. The
/// receiver imports images whose contents were written by D3D11, so the
/// first transition must not start from `UNDEFINED` (which allows the
/// driver to discard the contents). A combination of uses that does not map
/// to a dedicated layout yields `GENERAL`, which preserves them.
fn general_layout_state() -> wgpu::TextureUses {
    wgpu::TextureUses::COPY_SRC | wgpu::TextureUses::COPY_DST
}

/// Import a legacy shared handle as a wgpu texture.
///
/// Every failure is reported as [`Error::Unsupported`] so callers can fall
/// back to the CPU path.
#[allow(clippy::too_many_arguments)]
fn import_handle(
    gpu: &Gpu,
    handle: HANDLE,
    width: u32,
    height: u32,
    format: PixelFormat,
    usage: wgpu::TextureUsages,
    initial_state: wgpu::TextureUses,
    label: &str,
) -> Result<wgpu::Texture> {
    match import_handle_inner(
        gpu,
        handle,
        width,
        height,
        format,
        usage,
        initial_state,
        label,
    ) {
        Ok(texture) => Ok(texture),
        Err(e) => {
            log::warn!(
                "Vulkan import of Spout shared handle {:#x} failed: {e}",
                handle.0 as usize
            );
            Err(Error::Unsupported(
                "Vulkan could not import the Spout shared texture",
            ))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn import_handle_inner(
    gpu: &Gpu,
    handle: HANDLE,
    width: u32,
    height: u32,
    format: PixelFormat,
    usage: wgpu::TextureUsages,
    initial_state: wgpu::TextureUses,
    label: &str,
) -> Result<wgpu::Texture> {
    if handle.is_invalid() {
        return Err(Error::InvalidArgument("null shared handle".into()));
    }
    let vk_format = vk_format(format)?;
    let uses = hal_uses(usage);
    let vk_usage = wgpu::hal::vulkan::conv::map_texture_usage(uses);

    // SAFETY: the HAL device guard is dropped before `create_texture_from_hal`
    // is called on the wgpu device; all Vulkan handles created here are
    // either handed to wgpu or destroyed on error.
    let hal_texture = unsafe {
        let hal = gpu
            .device
            .as_hal::<Vulkan>()
            .ok_or(Error::Unsupported("wgpu device is not a Vulkan device"))?;
        let instance = hal.shared_instance();
        let physical_device = hal.raw_physical_device();
        let raw = hal.raw_device();

        if instance.instance_api_version() >= vk::API_VERSION_1_1 {
            let mut external_info =
                vk::PhysicalDeviceExternalImageFormatInfo::default().handle_type(KMT);
            let format_info = vk::PhysicalDeviceImageFormatInfo2::default()
                .format(vk_format)
                .ty(vk::ImageType::TYPE_2D)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(vk_usage)
                .push_next(&mut external_info);
            let mut external_props = vk::ExternalImageFormatProperties::default();
            let mut props = vk::ImageFormatProperties2::default().push_next(&mut external_props);
            instance
                .raw_instance()
                .get_physical_device_image_format_properties2(
                    physical_device,
                    &format_info,
                    &mut props,
                )
                .map_err(|e| Error::backend(format!("image format query failed: {e}")))?;
            let features = external_props
                .external_memory_properties
                .external_memory_features;
            if !features.contains(vk::ExternalMemoryFeatureFlags::IMPORTABLE) {
                return Err(Error::Unsupported(
                    "driver cannot import D3D11 KMT handles for this format",
                ));
            }
        }

        let mut external_image_info =
            vk::ExternalMemoryImageCreateInfo::default().handle_types(KMT);
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk_format)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk_usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut external_image_info);
        let image = raw
            .create_image(&image_info, None)
            .map_err(|e| Error::backend(format!("vkCreateImage failed: {e}")))?;

        let requirements = raw.get_image_memory_requirements(image);
        let memory_properties = instance
            .raw_instance()
            .get_physical_device_memory_properties(physical_device);
        let Some(type_index) = find_memory_type(&memory_properties, requirements.memory_type_bits)
        else {
            raw.destroy_image(image, None);
            return Err(Error::backend("no suitable memory type for imported image"));
        };

        let mut dedicated_info = vk::MemoryDedicatedAllocateInfo::default().image(image);
        let mut import_info = vk::ImportMemoryWin32HandleInfoKHR::default()
            .handle_type(KMT)
            .handle(handle.0 as vk::HANDLE);
        let allocate_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(type_index)
            .push_next(&mut import_info)
            .push_next(&mut dedicated_info);
        let memory = match raw.allocate_memory(&allocate_info, None) {
            Ok(memory) => memory,
            Err(e) => {
                raw.destroy_image(image, None);
                return Err(Error::backend(format!(
                    "vkAllocateMemory (import) failed: {e}"
                )));
            }
        };
        if let Err(e) = raw.bind_image_memory(image, memory, 0) {
            raw.free_memory(memory, None);
            raw.destroy_image(image, None);
            return Err(Error::backend(format!("vkBindImageMemory failed: {e}")));
        }

        let hal_desc = wgpu::hal::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: texture_format(format),
            usage: uses,
            memory_flags: wgpu::hal::MemoryFlags::empty(),
            view_formats: Vec::new(),
        };
        hal.texture_from_raw(
            image,
            &hal_desc,
            None,
            wgpu::hal::vulkan::TextureMemory::Dedicated(memory),
        )
    };

    let desc = texture_descriptor(Some(label), width, height, format, usage);
    // SAFETY: `hal_texture` was created on this device and matches `desc`.
    Ok(unsafe {
        gpu.device
            .create_texture_from_hal::<Vulkan>(hal_texture, &desc, initial_state)
    })
}

fn find_memory_type(props: &vk::PhysicalDeviceMemoryProperties, type_bits: u32) -> Option<u32> {
    let types = &props.memory_types[..props.memory_type_count as usize];
    let matches = |flags: vk::MemoryPropertyFlags| {
        types.iter().enumerate().find_map(|(i, t)| {
            (type_bits & (1 << i) != 0 && t.property_flags.contains(flags)).then_some(i as u32)
        })
    };
    matches(vk::MemoryPropertyFlags::DEVICE_LOCAL)
        .or_else(|| matches(vk::MemoryPropertyFlags::empty()))
}

/// Create a Spout sender plus the Vulkan bridge, or `None` when `gpu` is not
/// a Vulkan device.
pub fn create_sender(
    gpu: &Gpu,
    options: &sp2::SenderOptions,
) -> Result<Option<(sp2::Sender, Box<dyn SenderInterop>)>> {
    if !check_support(gpu)? {
        return Ok(None);
    }
    let sender = sp2::Sender::with_options(options.clone())?;
    Ok(Some((sender, Box::new(VulkanSender { imported: None }))))
}

/// Create a Spout receiver plus the Vulkan bridge, or `None` when `gpu` is
/// not a Vulkan device.
pub fn create_receiver(
    gpu: &Gpu,
    target: Option<&str>,
) -> Result<Option<(sp2::Receiver, Box<dyn ReceiverInterop>)>> {
    if !check_support(gpu)? {
        return Ok(None);
    }
    let receiver = super::plain_receiver(target)?;
    Ok(Some((receiver, Box::new(VulkanReceiver::default()))))
}

struct Imported {
    handle: usize,
    width: u32,
    height: u32,
    texture: wgpu::Texture,
}

impl Imported {
    fn matches(&self, handle: HANDLE, width: u32, height: u32) -> bool {
        self.handle == handle.0 as usize && self.width == width && self.height == height
    }
}

/// Sender bridge for Vulkan.
pub struct VulkanSender {
    imported: Option<Imported>,
}

impl VulkanSender {
    fn ensure_imported(&mut self, gpu: &Gpu, spout: &SpoutSender) -> Result<()> {
        use sp2_core::SenderBackend;

        let handle = spout.share_handle();
        let (width, height) = (spout.width(), spout.height());
        if self
            .imported
            .as_ref()
            .is_some_and(|i| i.matches(handle, width, height))
        {
            return Ok(());
        }
        let texture = import_handle(
            gpu,
            handle,
            width,
            height,
            spout.format(),
            SHARED_TEXTURE_USAGE,
            wgpu::TextureUses::UNINITIALIZED,
            "sp2 Spout shared texture (sender)",
        )?;
        self.imported = Some(Imported {
            handle: handle.0 as usize,
            width,
            height,
            texture,
        });
        Ok(())
    }
}

impl SenderInterop for VulkanSender {
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
        let spout = sender.platform_mut().as_spout_mut();
        self.ensure_imported(gpu, spout)?;
        let shared = &self.imported.as_ref().expect("ensured above").texture;
        spout.publish_with(|_, _| {
            gpu.copy_texture(src, shared, "sp2 Spout publish");
            wait_for_gpu(&gpu.device)
        })
    }

    fn shared_texture(
        &mut self,
        gpu: &Gpu,
        sender: &mut sp2::Sender,
    ) -> Result<Option<&wgpu::Texture>> {
        let spout = sender.platform().as_spout();
        self.ensure_imported(gpu, spout)?;
        Ok(self.imported.as_ref().map(|i| &i.texture))
    }

    fn publish(&mut self, gpu: &Gpu, sender: &mut sp2::Sender, _sync: SyncMode) -> Result<()> {
        if self.imported.is_none() {
            return Err(Error::InvalidArgument(
                "call shared_texture() and render into it before publish()".into(),
            ));
        }
        let spout = sender.platform_mut().as_spout_mut();
        spout.publish_with(|_, _| wait_for_gpu(&gpu.device))
    }
}

/// Receiver bridge for Vulkan.
#[derive(Default)]
pub struct VulkanReceiver {
    imported: Option<Imported>,
    copy: Option<wgpu::Texture>,
    updated: bool,
}

impl ReceiverInterop for VulkanReceiver {
    fn path(&self) -> TransferPath {
        TransferPath::GpuCopy
    }

    fn receive(
        &mut self,
        gpu: &Gpu,
        receiver: &mut sp2::Receiver,
        mode: ReceiveMode,
    ) -> Result<Option<FrameInfo>> {
        self.updated = false;
        let spout: &mut SpoutReceiver = receiver.platform_mut().as_spout_mut();
        if !spout.update()? {
            self.disconnect();
            return Ok(None);
        }
        let info = spout.sender_info().ok_or(Error::NotConnected)?.clone();
        let handle = spout.share_handle().ok_or(Error::NotConnected)?;

        if !self
            .imported
            .as_ref()
            .is_some_and(|i| i.matches(handle, info.width, info.height))
        {
            let texture = import_handle(
                gpu,
                handle,
                info.width,
                info.height,
                info.format,
                RECEIVED_TEXTURE_USAGE,
                general_layout_state(),
                "sp2 Spout shared texture (receiver)",
            )?;
            self.imported = Some(Imported {
                handle: handle.0 as usize,
                width: info.width,
                height: info.height,
                texture,
            });
            self.updated = true;
        }

        let VulkanReceiver {
            imported,
            copy,
            updated,
        } = self;
        let shared = &imported.as_ref().expect("set above").texture;
        spout.receive_with(|_, _| {
            if mode == ReceiveMode::Copy {
                let dst = ensure_texture(
                    gpu,
                    copy,
                    "sp2 received frame",
                    info.width,
                    info.height,
                    info.format,
                    RECEIVED_TEXTURE_USAGE,
                    updated,
                );
                gpu.copy_texture(shared, dst, "sp2 Spout receive");
                wait_for_gpu(&gpu.device)?;
            }
            Ok(())
        })
    }

    fn texture(&self, mode: ReceiveMode) -> Option<&wgpu::Texture> {
        match mode {
            ReceiveMode::Copy => self.copy.as_ref(),
            ReceiveMode::Shared => self.shared_texture(),
        }
    }

    fn shared_texture(&self) -> Option<&wgpu::Texture> {
        self.imported.as_ref().map(|i| &i.texture)
    }

    fn is_updated(&self) -> bool {
        self.updated
    }

    fn disconnect(&mut self) {
        self.imported = None;
        self.copy = None;
        self.updated = false;
    }
}
