//! CPU fallback: GPU read-back and upload through a staging buffer.
//!
//! Used when the wgpu backend has no interop path to the platform shared
//! texture (for example OpenGL, or a Vulkan driver without
//! `VK_KHR_external_memory_win32`). The frame takes a round trip through host
//! memory, which is reported as [`sp2_core::TransferPath::CpuCopy`].

use std::sync::mpsc;

use sp2_core::{copy_rows, Error, PixelFormat, Result};

use crate::format::require_pixel_format;

/// Reusable staging buffer for reading a texture back to the CPU.
#[derive(Default)]
pub struct Readback {
    buffer: Option<wgpu::Buffer>,
    capacity: u64,
}

impl Readback {
    /// Copy mip level 0 of `src` into `out`, tightly packed.
    ///
    /// Blocks until the copy has completed. Returns the frame geometry.
    pub fn read(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        src: &wgpu::Texture,
        out: &mut Vec<u8>,
    ) -> Result<(u32, u32, PixelFormat)> {
        let format = require_pixel_format(src.format())?;
        let width = src.width();
        let height = src.height();
        let row_len = format.min_stride(width);
        let padded_stride = padded_bytes_per_row(row_len);
        let size = padded_stride as u64 * height as u64;

        let buffer = self.ensure_buffer(device, size);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("sp2 readback"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: src,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_stride as u32),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);

        let slice = buffer.slice(..size);
        let (tx, rx) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        wait_for_gpu(device)?;
        match rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(Error::backend(format!("buffer map failed: {e}"))),
            Err(_) => return Err(Error::backend("buffer map callback was dropped")),
        }

        {
            let mapped = slice
                .get_mapped_range()
                .map_err(|e| Error::backend(format!("get_mapped_range failed: {e}")))?;
            out.resize(row_len * height as usize, 0);
            copy_rows(
                &mapped,
                padded_stride,
                out,
                row_len,
                row_len,
                height as usize,
            )?;
        }
        buffer.unmap();
        Ok((width, height, format))
    }

    fn ensure_buffer(&mut self, device: &wgpu::Device, size: u64) -> &wgpu::Buffer {
        if self.buffer.is_none() || self.capacity < size {
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("sp2 readback staging"),
                size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }));
            self.capacity = size;
        }
        self.buffer.as_ref().expect("created above")
    }
}

/// Upload tightly packed `pixels` into mip level 0 of `dst`.
pub fn upload(
    queue: &wgpu::Queue,
    dst: &wgpu::Texture,
    pixels: &[u8],
    width: u32,
    height: u32,
    format: PixelFormat,
) -> Result<()> {
    let expected = format.min_buffer_len(width, height);
    if pixels.len() < expected {
        return Err(Error::SizeMismatch {
            expected,
            actual: pixels.len(),
        });
    }
    if dst.width() != width || dst.height() != height {
        return Err(Error::InvalidArgument(
            "destination texture size does not match the frame".into(),
        ));
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: dst,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &pixels[..expected],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(format.min_stride(width) as u32),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    Ok(())
}

/// Round `row_len` up to `COPY_BYTES_PER_ROW_ALIGNMENT`.
pub fn padded_bytes_per_row(row_len: usize) -> usize {
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
    row_len.div_ceil(align) * align
}

/// Block until all submitted GPU work has completed.
pub fn wait_for_gpu(device: &wgpu::Device) -> Result<()> {
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map(|_| ())
        .map_err(|e| Error::backend(format!("device poll failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_padding() {
        assert_eq!(padded_bytes_per_row(0), 0);
        assert_eq!(padded_bytes_per_row(1), 256);
        assert_eq!(padded_bytes_per_row(256), 256);
        assert_eq!(padded_bytes_per_row(640 * 4), 2560);
        assert_eq!(padded_bytes_per_row(641 * 4), 2816);
    }
}
