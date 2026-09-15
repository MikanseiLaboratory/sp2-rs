use crate::{Error, PixelFormat, Result};

/// Description of a sender (Spout) or server (Syphon) visible on the system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderInfo {
    /// Stable identifier used to connect.
    ///
    /// * Spout: the sender name (unique on the system, suffixed `_1`, `_2`, ...
    ///   when duplicated).
    /// * Syphon: the server UUID (`info.v002.Syphon.<UUID>`).
    pub id: String,
    /// Human readable name. Equal to `id` for Spout senders.
    pub name: String,
    /// Name of the application hosting the sender, if known.
    ///
    /// * Spout: executable path stored in the `description` field.
    /// * Syphon: `SyphonServerDescriptionAppNameKey`.
    pub app_name: Option<String>,
    /// Current texture width in pixels (0 if unknown).
    pub width: u32,
    /// Current texture height in pixels (0 if unknown).
    pub height: u32,
    /// Current texture format.
    pub format: PixelFormat,
}

impl SenderInfo {
    /// Number of bytes required to hold one tightly packed frame.
    pub fn frame_len(&self) -> usize {
        self.format.min_buffer_len(self.width, self.height)
    }
}

/// Description of one received frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameInfo {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Pixel format of the frame data.
    pub format: PixelFormat,
    /// Monotonically increasing frame identifier, as reported by the sender
    /// (Spout frame count) or derived from surface updates (Syphon seed).
    /// `0` when the sender does not provide frame counting.
    pub frame_id: u64,
}

impl FrameInfo {
    /// Number of bytes for one tightly packed row.
    pub fn stride(&self) -> usize {
        self.format.min_stride(self.width)
    }

    /// Number of bytes for the whole tightly packed frame.
    pub fn byte_len(&self) -> usize {
        self.format.min_buffer_len(self.width, self.height)
    }
}

/// Borrowed CPU pixel data passed to a sender.
#[derive(Debug, Clone, Copy)]
pub struct PixelBuffer<'a> {
    /// Raw pixel bytes. Rows are `stride` bytes apart.
    pub data: &'a [u8],
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Bytes between the start of consecutive rows.
    pub stride: usize,
    /// Pixel format of `data`.
    pub format: PixelFormat,
}

impl<'a> PixelBuffer<'a> {
    /// Create a tightly packed buffer description and validate its length.
    pub fn packed(data: &'a [u8], width: u32, height: u32, format: PixelFormat) -> Result<Self> {
        let buffer = PixelBuffer {
            data,
            width,
            height,
            stride: format.min_stride(width),
            format,
        };
        buffer.validate()?;
        Ok(buffer)
    }

    /// Create a buffer description with an explicit row stride and validate it.
    pub fn with_stride(
        data: &'a [u8],
        width: u32,
        height: u32,
        stride: usize,
        format: PixelFormat,
    ) -> Result<Self> {
        let buffer = PixelBuffer {
            data,
            width,
            height,
            stride,
            format,
        };
        buffer.validate()?;
        Ok(buffer)
    }

    /// Check that `data` is large enough for the declared geometry.
    pub fn validate(&self) -> Result<()> {
        if self.width == 0 || self.height == 0 {
            return Err(Error::InvalidArgument("zero-sized image".into()));
        }
        let min_stride = self.format.min_stride(self.width);
        if self.stride < min_stride {
            return Err(Error::InvalidArgument(format!(
                "stride {} is smaller than the minimum {} for width {}",
                self.stride, min_stride, self.width
            )));
        }
        let expected = self.stride * (self.height as usize - 1) + min_stride;
        if self.data.len() < expected {
            return Err(Error::SizeMismatch {
                expected,
                actual: self.data.len(),
            });
        }
        Ok(())
    }

    /// Iterate over the rows of the image, each `width * bytes_per_pixel` long.
    pub fn rows(&self) -> impl Iterator<Item = &'a [u8]> + '_ {
        let row_len = self.format.min_stride(self.width);
        (0..self.height as usize).map(move |y| {
            let start = y * self.stride;
            &self.data[start..start + row_len]
        })
    }

    /// Copy the image into `dst` using `dst_stride` bytes per row.
    ///
    /// `dst` must be at least `dst_stride * height` bytes long.
    pub fn copy_to(&self, dst: &mut [u8], dst_stride: usize) -> Result<()> {
        let row_len = self.format.min_stride(self.width);
        if dst_stride < row_len {
            return Err(Error::InvalidArgument(format!(
                "destination stride {dst_stride} is smaller than row length {row_len}"
            )));
        }
        let required = dst_stride * (self.height as usize - 1) + row_len;
        if dst.len() < required {
            return Err(Error::SizeMismatch {
                expected: required,
                actual: dst.len(),
            });
        }
        for (y, row) in self.rows().enumerate() {
            let start = y * dst_stride;
            dst[start..start + row_len].copy_from_slice(row);
        }
        Ok(())
    }
}

/// Copy `height` rows of `row_len` bytes from `src` (rows `src_stride` apart)
/// into `dst` (rows `dst_stride` apart).
///
/// Used by backends when reading back from a mapped GPU staging texture or an
/// IOSurface whose row pitch differs from the tightly packed layout.
pub fn copy_rows(
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    row_len: usize,
    height: usize,
) -> Result<()> {
    if height == 0 {
        return Ok(());
    }
    let src_required = src_stride * (height - 1) + row_len;
    if src.len() < src_required {
        return Err(Error::SizeMismatch {
            expected: src_required,
            actual: src.len(),
        });
    }
    let dst_required = dst_stride * (height - 1) + row_len;
    if dst.len() < dst_required {
        return Err(Error::SizeMismatch {
            expected: dst_required,
            actual: dst.len(),
        });
    }
    if src_stride == row_len && dst_stride == row_len {
        dst[..row_len * height].copy_from_slice(&src[..row_len * height]);
        return Ok(());
    }
    for y in 0..height {
        let s = y * src_stride;
        let d = y * dst_stride;
        dst[d..d + row_len].copy_from_slice(&src[s..s + row_len]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_buffer_validates_length() {
        let data = vec![0u8; 4 * 4 * 4];
        assert!(PixelBuffer::packed(&data, 4, 4, PixelFormat::Bgra8Unorm).is_ok());
        assert!(matches!(
            PixelBuffer::packed(&data, 5, 4, PixelFormat::Bgra8Unorm),
            Err(Error::SizeMismatch { .. })
        ));
        assert!(matches!(
            PixelBuffer::packed(&data, 0, 4, PixelFormat::Bgra8Unorm),
            Err(Error::InvalidArgument(_))
        ));
    }

    #[test]
    fn strided_copy() {
        // 2x2 RGBA8 image with a 12 byte stride (4 bytes padding per row).
        let mut src = vec![0u8; 12 * 2];
        for (i, b) in src.iter_mut().enumerate() {
            *b = i as u8;
        }
        let buffer = PixelBuffer::with_stride(&src, 2, 2, 12, PixelFormat::Rgba8Unorm).unwrap();
        let mut dst = vec![0u8; 16];
        buffer.copy_to(&mut dst, 8).unwrap();
        assert_eq!(&dst[..8], &src[..8]);
        assert_eq!(&dst[8..], &src[12..20]);
    }

    #[test]
    fn copy_rows_fast_path_and_slow_path() {
        let src: Vec<u8> = (0..16).collect();
        let mut dst = vec![0u8; 16];
        copy_rows(&src, 8, &mut dst, 8, 8, 2).unwrap();
        assert_eq!(src, dst);

        let mut dst = vec![0u8; 20];
        copy_rows(&src, 8, &mut dst, 10, 8, 2).unwrap();
        assert_eq!(&dst[..8], &src[..8]);
        assert_eq!(&dst[10..18], &src[8..16]);
    }
}
