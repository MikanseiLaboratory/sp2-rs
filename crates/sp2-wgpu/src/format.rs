//! Conversions between [`PixelFormat`] and [`wgpu::TextureFormat`].

use sp2_core::{Error, PixelFormat, Result};

/// The wgpu texture format matching a shared texture format.
///
/// Returns `None` for formats added to `sp2-core` that this crate does not
/// know yet.
pub fn try_texture_format(format: PixelFormat) -> Option<wgpu::TextureFormat> {
    Some(match format {
        PixelFormat::Bgra8Unorm => wgpu::TextureFormat::Bgra8Unorm,
        PixelFormat::Rgba8Unorm => wgpu::TextureFormat::Rgba8Unorm,
        PixelFormat::Bgra8UnormSrgb => wgpu::TextureFormat::Bgra8UnormSrgb,
        PixelFormat::Rgba8UnormSrgb => wgpu::TextureFormat::Rgba8UnormSrgb,
        PixelFormat::Rgba16Unorm => wgpu::TextureFormat::Rgba16Unorm,
        PixelFormat::Rgba16Float => wgpu::TextureFormat::Rgba16Float,
        PixelFormat::Rgba32Float => wgpu::TextureFormat::Rgba32Float,
        PixelFormat::Rgb10a2Unorm => wgpu::TextureFormat::Rgb10a2Unorm,
        _ => return None,
    })
}

/// The wgpu texture format matching a shared texture format.
///
/// # Panics
///
/// Panics for formats unknown to this crate; use [`try_texture_format`] to
/// handle that case.
pub fn texture_format(format: PixelFormat) -> wgpu::TextureFormat {
    try_texture_format(format)
        .unwrap_or_else(|| panic!("pixel format {format:?} has no wgpu equivalent"))
}

/// The shared texture format for a wgpu texture format, if it can be shared.
pub fn pixel_format(format: wgpu::TextureFormat) -> Option<PixelFormat> {
    match format {
        wgpu::TextureFormat::Bgra8Unorm => Some(PixelFormat::Bgra8Unorm),
        wgpu::TextureFormat::Rgba8Unorm => Some(PixelFormat::Rgba8Unorm),
        wgpu::TextureFormat::Bgra8UnormSrgb => Some(PixelFormat::Bgra8UnormSrgb),
        wgpu::TextureFormat::Rgba8UnormSrgb => Some(PixelFormat::Rgba8UnormSrgb),
        wgpu::TextureFormat::Rgba16Unorm => Some(PixelFormat::Rgba16Unorm),
        wgpu::TextureFormat::Rgba16Float => Some(PixelFormat::Rgba16Float),
        wgpu::TextureFormat::Rgba32Float => Some(PixelFormat::Rgba32Float),
        wgpu::TextureFormat::Rgb10a2Unorm => Some(PixelFormat::Rgb10a2Unorm),
        _ => None,
    }
}

/// Like [`pixel_format`] but returns an error naming the format.
pub fn require_pixel_format(format: wgpu::TextureFormat) -> Result<PixelFormat> {
    pixel_format(format).ok_or_else(|| {
        Error::InvalidArgument(format!("texture format {format:?} cannot be shared"))
    })
}

/// Whether two wgpu formats can be used together in `copy_texture_to_texture`.
///
/// sRGB and linear variants of the same layout are copy compatible: the
/// bytes are identical and only their interpretation differs, which is also
/// how the reference SDKs treat them.
pub fn copy_compatible(a: wgpu::TextureFormat, b: wgpu::TextureFormat) -> bool {
    a.remove_srgb_suffix() == b.remove_srgb_suffix()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for format in PixelFormat::ALL {
            let wgpu_format = try_texture_format(format).expect("known format");
            assert_eq!(pixel_format(wgpu_format), Some(format));
        }
    }

    #[test]
    fn srgb_is_copy_compatible() {
        assert!(copy_compatible(
            wgpu::TextureFormat::Rgba8UnormSrgb,
            wgpu::TextureFormat::Rgba8Unorm
        ));
        assert!(!copy_compatible(
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Bgra8Unorm
        ));
    }

    #[test]
    fn unsupported_formats_are_rejected() {
        assert_eq!(pixel_format(wgpu::TextureFormat::R8Unorm), None);
        assert!(require_pixel_format(wgpu::TextureFormat::Depth32Float).is_err());
    }
}
