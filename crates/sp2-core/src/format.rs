/// Pixel formats that can be shared between processes.
///
/// Spout2 shares any `DXGI_FORMAT`, but in practice senders use one of the
/// formats listed here. Syphon only shares 8-bit BGRA IOSurfaces; other
/// formats are rejected by the macOS backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum PixelFormat {
    /// 8-bit BGRA, unsigned normalized. Default for both Spout2 and Syphon.
    #[default]
    Bgra8Unorm,
    /// 8-bit RGBA, unsigned normalized.
    Rgba8Unorm,
    /// 8-bit BGRA, sRGB encoded.
    Bgra8UnormSrgb,
    /// 8-bit RGBA, sRGB encoded.
    Rgba8UnormSrgb,
    /// 16-bit RGBA, unsigned normalized.
    Rgba16Unorm,
    /// 16-bit RGBA, half precision floating point.
    Rgba16Float,
    /// 32-bit RGBA, single precision floating point.
    Rgba32Float,
    /// 10:10:10:2 RGBA, unsigned normalized.
    Rgb10a2Unorm,
}

impl PixelFormat {
    /// All formats known to this crate, in declaration order.
    pub const ALL: [PixelFormat; 8] = [
        PixelFormat::Bgra8Unorm,
        PixelFormat::Rgba8Unorm,
        PixelFormat::Bgra8UnormSrgb,
        PixelFormat::Rgba8UnormSrgb,
        PixelFormat::Rgba16Unorm,
        PixelFormat::Rgba16Float,
        PixelFormat::Rgba32Float,
        PixelFormat::Rgb10a2Unorm,
    ];

    /// Number of bytes occupied by one pixel.
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            PixelFormat::Bgra8Unorm
            | PixelFormat::Rgba8Unorm
            | PixelFormat::Bgra8UnormSrgb
            | PixelFormat::Rgba8UnormSrgb
            | PixelFormat::Rgb10a2Unorm => 4,
            PixelFormat::Rgba16Unorm | PixelFormat::Rgba16Float => 8,
            PixelFormat::Rgba32Float => 16,
        }
    }

    /// Minimum number of bytes for a tightly packed row of `width` pixels.
    pub const fn min_stride(self, width: u32) -> usize {
        width as usize * self.bytes_per_pixel()
    }

    /// Minimum number of bytes for a tightly packed `width` x `height` image.
    pub const fn min_buffer_len(self, width: u32, height: u32) -> usize {
        self.min_stride(width) * height as usize
    }

    /// Numeric `DXGI_FORMAT` value used by Spout2 in `SharedTextureInfo.format`.
    pub const fn dxgi_format(self) -> u32 {
        match self {
            PixelFormat::Bgra8Unorm => 87,     // DXGI_FORMAT_B8G8R8A8_UNORM
            PixelFormat::Rgba8Unorm => 28,     // DXGI_FORMAT_R8G8B8A8_UNORM
            PixelFormat::Bgra8UnormSrgb => 91, // DXGI_FORMAT_B8G8R8A8_UNORM_SRGB
            PixelFormat::Rgba8UnormSrgb => 29, // DXGI_FORMAT_R8G8B8A8_UNORM_SRGB
            PixelFormat::Rgba16Unorm => 11,    // DXGI_FORMAT_R16G16B16A16_UNORM
            PixelFormat::Rgba16Float => 10,    // DXGI_FORMAT_R16G16B16A16_FLOAT
            PixelFormat::Rgba32Float => 2,     // DXGI_FORMAT_R32G32B32A32_FLOAT
            PixelFormat::Rgb10a2Unorm => 24,   // DXGI_FORMAT_R10G10B10A2_UNORM
        }
    }

    /// Map a numeric `DXGI_FORMAT` back to a [`PixelFormat`].
    ///
    /// Spout2 writes `0` for DirectX 9 senders and for some legacy senders;
    /// following the reference implementation this is treated as BGRA8.
    pub const fn from_dxgi_format(value: u32) -> Option<PixelFormat> {
        Some(match value {
            0 | 87 | 21 => PixelFormat::Bgra8Unorm, // 21 = D3DFMT_A8R8G8B8 (DX9)
            28 => PixelFormat::Rgba8Unorm,
            91 => PixelFormat::Bgra8UnormSrgb,
            29 => PixelFormat::Rgba8UnormSrgb,
            11 => PixelFormat::Rgba16Unorm,
            10 => PixelFormat::Rgba16Float,
            2 => PixelFormat::Rgba32Float,
            24 => PixelFormat::Rgb10a2Unorm,
            _ => return None,
        })
    }

    /// `kIOSurfacePixelFormat` four character code, if IOSurface can describe
    /// this format.
    pub const fn iosurface_pixel_format(self) -> Option<u32> {
        Some(match self {
            PixelFormat::Bgra8Unorm | PixelFormat::Bgra8UnormSrgb => fourcc(b"BGRA"),
            PixelFormat::Rgba8Unorm | PixelFormat::Rgba8UnormSrgb => fourcc(b"RGBA"),
            PixelFormat::Rgba16Float => fourcc(b"RGhA"),
            PixelFormat::Rgba32Float => fourcc(b"RGfA"),
            PixelFormat::Rgba16Unorm | PixelFormat::Rgb10a2Unorm => return None,
        })
    }

    /// Numeric `MTLPixelFormat` value.
    pub const fn metal_pixel_format(self) -> u64 {
        match self {
            PixelFormat::Bgra8Unorm => 80,
            PixelFormat::Rgba8Unorm => 70,
            PixelFormat::Bgra8UnormSrgb => 81,
            PixelFormat::Rgba8UnormSrgb => 71,
            PixelFormat::Rgba16Unorm => 110,
            PixelFormat::Rgba16Float => 115,
            PixelFormat::Rgba32Float => 125,
            PixelFormat::Rgb10a2Unorm => 90,
        }
    }

    /// Whether the format is shareable through Syphon (BGRA8 only).
    pub const fn is_syphon_compatible(self) -> bool {
        matches!(self, PixelFormat::Bgra8Unorm)
    }
}

const fn fourcc(code: &[u8; 4]) -> u32 {
    ((code[0] as u32) << 24) | ((code[1] as u32) << 16) | ((code[2] as u32) << 8) | (code[3] as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dxgi_round_trip() {
        for format in PixelFormat::ALL {
            assert_eq!(
                PixelFormat::from_dxgi_format(format.dxgi_format()),
                Some(format)
            );
        }
    }

    #[test]
    fn legacy_dxgi_values_map_to_bgra() {
        assert_eq!(
            PixelFormat::from_dxgi_format(0),
            Some(PixelFormat::Bgra8Unorm)
        );
        assert_eq!(
            PixelFormat::from_dxgi_format(21),
            Some(PixelFormat::Bgra8Unorm)
        );
        assert_eq!(PixelFormat::from_dxgi_format(9999), None);
    }

    #[test]
    fn buffer_sizes() {
        assert_eq!(
            PixelFormat::Bgra8Unorm.min_buffer_len(1920, 1080),
            1920 * 1080 * 4
        );
        assert_eq!(PixelFormat::Rgba16Float.min_stride(4), 32);
        assert_eq!(PixelFormat::Rgba32Float.bytes_per_pixel(), 16);
    }

    #[test]
    fn iosurface_fourcc() {
        assert_eq!(
            PixelFormat::Bgra8Unorm.iosurface_pixel_format(),
            Some(0x4247_5241)
        );
        assert_eq!(PixelFormat::Rgb10a2Unorm.iosurface_pixel_format(), None);
    }
}
