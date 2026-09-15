//! IOSurface creation, lookup and CPU access.

// `kIOSurfaceIsGlobal` is deprecated by Apple because global surfaces can be
// looked up by any process, but it is exactly what the Syphon protocol relies
// on (`IOSurfaceLookup` by ID), so it is required for interoperability.
#![allow(deprecated)]

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_io_surface::{
    kIOSurfaceBytesPerElement, kIOSurfaceHeight, kIOSurfaceIsGlobal, kIOSurfacePixelFormat,
    kIOSurfaceWidth, IOSurfaceID, IOSurfaceLockOptions, IOSurfaceRef,
};
use objc2_metal::{
    MTLDevice, MTLPixelFormat, MTLStorageMode, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};
use sp2_core::{copy_rows, Error, PixelFormat, Result};

/// A BGRA8 IOSurface shared between processes.
///
/// Created with `kIOSurfaceIsGlobal` so that other processes can obtain it
/// with `IOSurfaceLookup`, exactly like the reference framework.
pub struct Surface {
    surface: CFRetained<IOSurfaceRef>,
    width: u32,
    height: u32,
}

// SAFETY: IOSurface objects are reference counted kernel objects that may be
// used from any thread; CPU access is serialised through IOSurfaceLock.
unsafe impl Send for Surface {}
unsafe impl Sync for Surface {}

impl std::fmt::Debug for Surface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Surface")
            .field("id", &self.id())
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

impl Surface {
    /// Create a new global BGRA8 surface.
    pub fn create(width: u32, height: u32) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(Error::InvalidArgument(
                "surface size must not be zero".into(),
            ));
        }
        let format = PixelFormat::Bgra8Unorm;
        let pixel_format = format
            .iosurface_pixel_format()
            .ok_or(Error::InvalidFormat(format))?;

        let width_num = CFNumber::new_i32(width as i32);
        let height_num = CFNumber::new_i32(height as i32);
        let bpe = CFNumber::new_i32(format.bytes_per_pixel() as i32);
        let pf = CFNumber::new_i32(pixel_format as i32);
        let global = CFBoolean::new(true);

        // SAFETY: reading Core Foundation string constants.
        let (k_width, k_height, k_bpe, k_pf, k_global) = unsafe {
            (
                kIOSurfaceWidth,
                kIOSurfaceHeight,
                kIOSurfaceBytesPerElement,
                kIOSurfacePixelFormat,
                kIOSurfaceIsGlobal,
            )
        };
        let keys: [&CFString; 5] = [k_width, k_height, k_bpe, k_pf, k_global];
        let values: [&CFType; 5] = [&width_num, &height_num, &bpe, &pf, global];
        let properties = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
        let properties: &CFDictionary = properties.as_opaque();

        // SAFETY: `properties` is a valid dictionary of IOSurface keys.
        let surface = unsafe { IOSurfaceRef::new(properties) }
            .ok_or_else(|| Error::backend("IOSurfaceCreate failed"))?;
        Ok(Surface {
            surface,
            width,
            height,
        })
    }

    /// Look up a surface published by another process.
    pub fn lookup(id: IOSurfaceID) -> Option<Self> {
        let surface = IOSurfaceRef::lookup(id)?;
        let width = surface.width() as u32;
        let height = surface.height() as u32;
        Some(Surface {
            surface,
            width,
            height,
        })
    }

    /// Kernel identifier sent to clients (`UpdateSurfaceID`).
    pub fn id(&self) -> IOSurfaceID {
        self.surface.id()
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Bytes per row of the surface memory.
    pub fn bytes_per_row(&self) -> usize {
        self.surface.bytes_per_row()
    }

    /// Modification counter, incremented on every unlock-after-write and every
    /// GPU write.
    pub fn seed(&self) -> u32 {
        self.surface.seed()
    }

    /// Pixel format as reported by the surface (`'BGRA'`).
    pub fn pixel_format_code(&self) -> u32 {
        self.surface.pixel_format()
    }

    /// The underlying `IOSurfaceRef`.
    pub fn raw(&self) -> &IOSurfaceRef {
        &self.surface
    }

    /// Copy `data` (rows `stride` bytes apart) into the surface.
    pub fn write_pixels(&self, data: &[u8], stride: usize) -> Result<()> {
        let row_len = PixelFormat::Bgra8Unorm.min_stride(self.width);
        let height = self.height as usize;
        let options = IOSurfaceLockOptions::empty();
        // SAFETY: the surface is valid; a null seed pointer is allowed.
        let status = unsafe { self.surface.lock(options, std::ptr::null_mut()) };
        if status != 0 {
            return Err(Error::os(status, "IOSurfaceLock"));
        }
        let dst_stride = self.surface.bytes_per_row();
        let dst_len = dst_stride * (height - 1) + row_len;
        // SAFETY: while locked, the base address is valid for `alloc_size`
        // bytes, which covers `dst_len`.
        let result = if self.surface.alloc_size() < dst_len {
            Err(Error::backend(
                "IOSurface allocation is smaller than expected",
            ))
        } else {
            let dst = unsafe {
                std::slice::from_raw_parts_mut(
                    self.surface.base_address().as_ptr() as *mut u8,
                    dst_len,
                )
            };
            copy_rows(data, stride, dst, dst_stride, row_len, height)
        };
        // SAFETY: matches the lock above.
        unsafe { self.surface.unlock(options, std::ptr::null_mut()) };
        result
    }

    /// Copy the surface contents tightly packed into `out`.
    ///
    /// Returns the surface seed observed while locked.
    pub fn read_pixels(&self, out: &mut Vec<u8>) -> Result<u32> {
        let row_len = PixelFormat::Bgra8Unorm.min_stride(self.width);
        let height = self.height as usize;
        out.resize(row_len * height, 0);
        let options = IOSurfaceLockOptions::ReadOnly;
        let mut seed = 0u32;
        // SAFETY: the surface is valid and `seed` is a valid pointer.
        let status = unsafe { self.surface.lock(options, &mut seed) };
        if status != 0 {
            return Err(Error::os(status, "IOSurfaceLock"));
        }
        let src_stride = self.surface.bytes_per_row();
        let src_len = src_stride * (height - 1) + row_len;
        let result = if self.surface.alloc_size() < src_len {
            Err(Error::backend(
                "IOSurface allocation is smaller than expected",
            ))
        } else {
            // SAFETY: while locked, the base address is valid for `alloc_size` bytes.
            let src = unsafe {
                std::slice::from_raw_parts(
                    self.surface.base_address().as_ptr() as *const u8,
                    src_len,
                )
            };
            copy_rows(src, src_stride, out, row_len, row_len, height)
        };
        // SAFETY: matches the lock above.
        unsafe { self.surface.unlock(options, &mut seed) };
        result.map(|()| seed)
    }

    /// Create a Metal texture backed by this surface.
    ///
    /// The texture shares memory with the surface, so rendering into it (or
    /// blitting to it) publishes pixels to other processes without a copy.
    pub fn metal_texture(
        &self,
        device: &ProtocolObject<dyn MTLDevice>,
    ) -> Result<Retained<ProtocolObject<dyn MTLTexture>>> {
        // SAFETY: plain constructor call with valid arguments.
        let descriptor = unsafe {
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                MTLPixelFormat::BGRA8Unorm,
                self.width as usize,
                self.height as usize,
                false,
            )
        };
        descriptor.setUsage(MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget);
        descriptor.setStorageMode(MTLStorageMode::Shared);
        device
            .newTextureWithDescriptor_iosurface_plane(&descriptor, &self.surface, 0)
            .ok_or_else(|| Error::backend("newTextureWithDescriptor:iosurface:plane: failed"))
    }
}
