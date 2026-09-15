//! Spout sender: publishes a D3D11 shared texture under a registered name.

use sp2_core::{Error, PixelBuffer, PixelFormat, Result, SenderBackend, SenderInfo};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::System::LibraryLoader::GetModuleFileNameA;

use crate::d3d11::{texture_desc, Device};
use crate::frame_count::{AccessMutex, FrameCounter};
use crate::layout::{SharedTextureInfo, PARTNER_GLDX_COMPATIBLE};
use crate::sender_names::{SenderInfoMap, SenderNames};

/// A Spout 2.007 compatible sender.
///
/// Creating a sender allocates a `D3D11_RESOURCE_MISC_SHARED` texture,
/// registers the (uniquified) name in `SpoutSenderNames`, publishes the
/// texture handle and geometry in the `<name>` information map and creates the
/// frame count semaphore and access mutex. Dropping the sender releases the
/// name; the shared texture disappears once every receiver has closed it.
pub struct SpoutSender {
    device: Device,
    names: SenderNames,
    name: String,
    width: u32,
    height: u32,
    format: PixelFormat,
    texture: ID3D11Texture2D,
    handle: HANDLE,
    info_map: SenderInfoMap,
    frame: FrameCounter,
    access: AccessMutex,
    description: String,
}

impl std::fmt::Debug for SpoutSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpoutSender")
            .field("name", &self.name)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .field("handle", &self.handle.0)
            .finish_non_exhaustive()
    }
}

impl SpoutSender {
    /// Create a sender on a new D3D11 device.
    pub fn new(name: &str, width: u32, height: u32, format: PixelFormat) -> Result<Self> {
        Self::with_device(Device::new()?, name, width, height, format)
    }

    /// Create a sender that shares textures created on `device`.
    ///
    /// Use this when the application already owns a D3D11 device so that
    /// [`SpoutSender::send_texture`] can copy directly from application
    /// textures.
    pub fn with_device(
        device: Device,
        name: &str,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<Self> {
        if name.is_empty() {
            return Err(Error::InvalidArgument(
                "sender name must not be empty".into(),
            ));
        }
        if width == 0 || height == 0 {
            return Err(Error::InvalidArgument(
                "sender size must not be zero".into(),
            ));
        }

        let (texture, handle) = device.create_shared_texture(width, height, format)?;
        let names = SenderNames::new()?;
        let registered = names.register(name)?;
        if registered != name {
            log::info!("sender name {name:?} already in use, registered as {registered:?}");
        }

        let info_map = match SenderInfoMap::create(&registered) {
            Ok(map) => map,
            Err(e) => {
                let _ = names.release(&registered);
                return Err(e);
            }
        };
        let description = executable_path();
        let frame = FrameCounter::new(&registered)?;
        let access = AccessMutex::new(&registered)?;

        let sender = SpoutSender {
            device,
            names,
            name: registered,
            width,
            height,
            format,
            texture,
            handle,
            info_map,
            frame,
            access,
            description,
        };
        sender.write_info()?;
        log::info!(
            "created Spout sender {:?} {}x{} {:?} handle {:#x}",
            sender.name,
            width,
            height,
            format,
            sender.handle.0 as usize
        );
        Ok(sender)
    }

    /// The device owning the shared texture.
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// The shared texture receivers open.
    pub fn shared_texture(&self) -> &ID3D11Texture2D {
        &self.texture
    }

    /// Legacy shared handle of the texture (as stored in the info map).
    pub fn share_handle(&self) -> HANDLE {
        self.handle
    }

    /// Make this the active sender.
    pub fn set_active(&self) -> Result<()> {
        self.names.set_active_sender(&self.name)
    }

    /// Publish `src` (a texture on the same device) as the next frame.
    ///
    /// The sender is resized when `src` has a different size. The formats must
    /// match; D3D11 cannot copy between different formats.
    pub fn send_texture(&mut self, src: &ID3D11Texture2D) -> Result<()> {
        let desc = texture_desc(src);
        if desc.Format.0 as u32 != self.format.dxgi_format() {
            return Err(Error::InvalidArgument(format!(
                "texture format {} does not match sender format {:?}",
                desc.Format.0, self.format
            )));
        }
        if desc.Width != self.width || desc.Height != self.height {
            self.resize(desc.Width, desc.Height)?;
        }
        let _guard = self.access.lock()?;
        self.device.copy_texture(&self.texture, src);
        self.device.flush();
        self.frame.set_new_frame();
        Ok(())
    }

    fn write_info(&self) -> Result<()> {
        let info = SharedTextureInfo {
            share_handle: SharedTextureInfo::truncate_handle(self.handle.0 as isize),
            width: self.width,
            height: self.height,
            format: self.format.dxgi_format(),
            usage: 0,
            description: self.description.clone(),
            partner_id: PARTNER_GLDX_COMPATIBLE,
        };
        self.info_map.write(&info)
    }
}

impl SenderBackend for SpoutSender {
    fn name(&self) -> &str {
        &self.name
    }

    fn id(&self) -> &str {
        &self.name
    }

    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn format(&self) -> PixelFormat {
        self.format
    }

    fn info(&self) -> SenderInfo {
        SenderInfo {
            id: self.name.clone(),
            name: self.name.clone(),
            app_name: Some(self.description.clone()),
            width: self.width,
            height: self.height,
            format: self.format,
        }
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        if width == 0 || height == 0 {
            return Err(Error::InvalidArgument(
                "sender size must not be zero".into(),
            ));
        }
        if width == self.width && height == self.height {
            return Ok(());
        }
        let (texture, handle) = self
            .device
            .create_shared_texture(width, height, self.format)?;
        {
            // Receivers re-read the info map every frame; swapping under the
            // access mutex keeps them from copying a half-updated texture.
            let _guard = self.access.lock()?;
            self.texture = texture;
            self.handle = handle;
            self.width = width;
            self.height = height;
            self.write_info()?;
        }
        log::info!(
            "resized Spout sender {:?} to {}x{}",
            self.name,
            width,
            height
        );
        Ok(())
    }

    fn send_pixels(&mut self, pixels: PixelBuffer<'_>) -> Result<()> {
        pixels.validate()?;
        if pixels.format != self.format {
            return Err(Error::InvalidFormat(pixels.format));
        }
        if pixels.width != self.width || pixels.height != self.height {
            self.resize(pixels.width, pixels.height)?;
        }
        let _guard = self.access.lock()?;
        self.device
            .upload_pixels(&self.texture, pixels.data, pixels.stride)?;
        self.device.flush();
        self.frame.set_new_frame();
        Ok(())
    }

    fn frame_count(&self) -> u64 {
        self.frame.frame_count()
    }
}

impl Drop for SpoutSender {
    fn drop(&mut self) {
        if let Err(e) = self.names.release(&self.name) {
            log::warn!("failed to release Spout sender name {:?}: {e}", self.name);
        }
    }
}

/// Path of the current executable, as written into the `description` field.
fn executable_path() -> String {
    let mut buf = [0u8; 260];
    // SAFETY: `buf` is a valid writable buffer.
    let len = unsafe { GetModuleFileNameA(None, &mut buf) } as usize;
    if len == 0 {
        return std::env::current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
    }
    String::from_utf8_lossy(&buf[..len.min(buf.len())]).into_owned()
}
