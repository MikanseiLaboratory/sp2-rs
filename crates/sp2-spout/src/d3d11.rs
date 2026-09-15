//! Minimal Direct3D 11 helpers: device creation, shared textures, staging
//! textures and CPU upload / readback.

use sp2_core::{copy_rows, Error, PixelFormat, Result};
use windows::core::Interface;
use windows::Win32::Foundation::{HANDLE, HMODULE};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_10_0,
    D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_FLAG, D3D11_MAPPED_SUBRESOURCE,
    D3D11_MAP_READ, D3D11_RESOURCE_MISC_SHARED, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIResource;

use crate::shared_memory::os_error;

/// A D3D11 device and its immediate context.
#[derive(Clone)]
pub struct Device {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device").finish_non_exhaustive()
    }
}

// SAFETY: D3D11 devices and immediate contexts are free-threaded for the
// operations used here (the immediate context is externally synchronised by
// the Spout access mutex and by `&mut self` on senders / receivers).
unsafe impl Send for Device {}
unsafe impl Sync for Device {}

impl Device {
    /// Create a hardware device with BGRA support (falls back to WARP).
    pub fn new() -> Result<Self> {
        match Self::create(D3D_DRIVER_TYPE_HARDWARE) {
            Ok(device) => Ok(device),
            Err(hw_err) => {
                log::warn!("D3D11 hardware device creation failed ({hw_err}); trying WARP");
                Self::create(D3D_DRIVER_TYPE_WARP)
            }
        }
    }

    fn create(driver_type: windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE) -> Result<Self> {
        const LEVELS: [D3D_FEATURE_LEVEL; 4] = [
            D3D_FEATURE_LEVEL_11_1,
            D3D_FEATURE_LEVEL_11_0,
            D3D_FEATURE_LEVEL_10_1,
            D3D_FEATURE_LEVEL_10_0,
        ];
        let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;
        // Older runtimes reject 11_1 with E_INVALIDARG; retry without it.
        match Self::create_with(driver_type, flags, &LEVELS) {
            Ok(d) => Ok(d),
            Err(_) => Self::create_with(driver_type, flags, &LEVELS[1..]),
        }
    }

    fn create_with(
        driver_type: windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE,
        flags: D3D11_CREATE_DEVICE_FLAG,
        levels: &[D3D_FEATURE_LEVEL],
    ) -> Result<Self> {
        let mut device = None;
        let mut context = None;
        let mut level = D3D_FEATURE_LEVEL::default();
        // SAFETY: all out-pointers are valid for the duration of the call.
        unsafe {
            D3D11CreateDevice(
                None,
                driver_type,
                HMODULE::default(),
                flags,
                Some(levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                Some(&mut level),
                Some(&mut context),
            )
        }
        .map_err(|e| os_error("D3D11CreateDevice", e))?;
        let device =
            device.ok_or_else(|| Error::backend("D3D11CreateDevice returned no device"))?;
        let context =
            context.ok_or_else(|| Error::backend("D3D11CreateDevice returned no context"))?;
        log::debug!("created D3D11 device, feature level {:#x}", level.0);
        Ok(Device { device, context })
    }

    /// Wrap an existing device (for example one owned by the application).
    pub fn from_raw(device: ID3D11Device) -> Result<Self> {
        // SAFETY: `device` is a valid ID3D11Device.
        let context = unsafe { device.GetImmediateContext() }
            .map_err(|e| os_error("GetImmediateContext", e))?;
        Ok(Device { device, context })
    }

    /// The underlying device.
    pub fn raw(&self) -> &ID3D11Device {
        &self.device
    }

    /// The immediate context.
    pub fn context(&self) -> &ID3D11DeviceContext {
        &self.context
    }

    /// Create a shared texture (`D3D11_RESOURCE_MISC_SHARED`) and return it with
    /// its legacy shared handle.
    pub fn create_shared_texture(
        &self,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<(ID3D11Texture2D, HANDLE)> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT(format.dxgi_format() as i32),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: D3D11_RESOURCE_MISC_SHARED.0 as u32,
        };
        let texture = self.create_texture(&desc)?;
        let handle = shared_handle(&texture)?;
        Ok((texture, handle))
    }

    /// Create a CPU readable staging texture matching the given geometry.
    pub fn create_staging_texture(
        &self,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<ID3D11Texture2D> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT(format.dxgi_format() as i32),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        self.create_texture(&desc)
    }

    fn create_texture(&self, desc: &D3D11_TEXTURE2D_DESC) -> Result<ID3D11Texture2D> {
        let mut texture = None;
        // SAFETY: `desc` and the out-pointer are valid.
        unsafe { self.device.CreateTexture2D(desc, None, Some(&mut texture)) }
            .map_err(|e| os_error("CreateTexture2D", e))?;
        texture.ok_or_else(|| Error::backend("CreateTexture2D returned no texture"))
    }

    /// Open a texture shared by another device / process from its legacy handle.
    pub fn open_shared_texture(&self, handle: HANDLE) -> Result<ID3D11Texture2D> {
        let mut texture: Option<ID3D11Texture2D> = None;
        // SAFETY: `handle` is a shared resource handle; the out-pointer is valid.
        unsafe { self.device.OpenSharedResource(handle, &mut texture) }
            .map_err(|e| os_error("OpenSharedResource", e))?;
        texture.ok_or_else(|| Error::backend("OpenSharedResource returned no texture"))
    }

    /// Upload tightly packed (or strided) CPU pixels into `dst`.
    pub fn upload_pixels(
        &self,
        dst: &ID3D11Texture2D,
        data: &[u8],
        row_pitch: usize,
    ) -> Result<()> {
        if row_pitch > u32::MAX as usize {
            return Err(Error::InvalidArgument("row pitch too large".into()));
        }
        // SAFETY: `data` is valid for `row_pitch * height` bytes (validated by
        // the caller through PixelBuffer) and `dst` is a default-usage texture.
        unsafe {
            self.context.UpdateSubresource(
                dst,
                0,
                None,
                data.as_ptr() as *const _,
                row_pitch as u32,
                0,
            );
        }
        Ok(())
    }

    /// Copy `src` into `dst` (same size and format required by D3D11).
    pub fn copy_texture(&self, dst: &ID3D11Texture2D, src: &ID3D11Texture2D) {
        // SAFETY: both textures are valid resources on this device.
        unsafe { self.context.CopyResource(dst, src) }
    }

    /// Submit pending commands so other devices observe the writes.
    pub fn flush(&self) {
        // SAFETY: the context is valid.
        unsafe { self.context.Flush() }
    }

    /// Map a staging texture and copy its contents tightly packed into `out`.
    pub fn read_staging(
        &self,
        staging: &ID3D11Texture2D,
        width: u32,
        height: u32,
        format: PixelFormat,
        out: &mut Vec<u8>,
    ) -> Result<()> {
        let row_len = format.min_stride(width);
        out.resize(row_len * height as usize, 0);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: `staging` is a CPU readable staging texture; `mapped` is valid.
        unsafe {
            self.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .map_err(|e| os_error("Map", e))?;
        let src_stride = mapped.RowPitch as usize;
        let result = if mapped.pData.is_null() || src_stride < row_len {
            Err(Error::backend("Map returned an invalid mapping"))
        } else {
            // SAFETY: the mapping is valid for `RowPitch * height` bytes until Unmap.
            let src = unsafe {
                std::slice::from_raw_parts(
                    mapped.pData as *const u8,
                    src_stride * (height as usize - 1) + row_len,
                )
            };
            copy_rows(src, src_stride, out, row_len, row_len, height as usize)
        };
        // SAFETY: matches the Map above.
        unsafe { self.context.Unmap(staging, 0) };
        result
    }
}

/// Query the legacy (`GetSharedHandle`) handle of a shared texture.
pub fn shared_handle(texture: &ID3D11Texture2D) -> Result<HANDLE> {
    let resource: IDXGIResource = texture
        .cast()
        .map_err(|e| os_error("cast IDXGIResource", e))?;
    // SAFETY: `resource` is a valid DXGI resource.
    unsafe { resource.GetSharedHandle() }.map_err(|e| os_error("GetSharedHandle", e))
}

/// Read the description of a texture.
pub fn texture_desc(texture: &ID3D11Texture2D) -> D3D11_TEXTURE2D_DESC {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: `desc` is a valid out-pointer.
    unsafe { texture.GetDesc(&mut desc) };
    desc
}
