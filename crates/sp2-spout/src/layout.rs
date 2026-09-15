//! Wire formats of the Spout2 shared memory maps.
//!
//! Everything in this module is plain byte manipulation with no OS
//! dependencies, so it is compiled and unit tested on every platform.

use sp2_core::PixelFormat;

/// Length of one sender name slot including the NUL terminator.
pub const MAX_SENDER_NAME_LEN: usize = 256;

/// Default number of sender name slots when the registry has no `MaxSenders`.
pub const DEFAULT_MAX_SENDERS: usize = 64;

/// Size in bytes of the per-sender `SharedTextureInfo` memory map.
pub const SHARED_TEXTURE_INFO_LEN: usize = 280;

/// Name of the memory map holding the sender name table.
pub const SENDER_NAMES_MAP: &str = "SpoutSenderNames";

/// Name of the memory map holding the active sender name.
pub const ACTIVE_SENDER_MAP: &str = "ActiveSenderName";

/// `partnerId` flag: the sender shares via CPU memory rather than a texture.
pub const PARTNER_CPU_SHARE: u32 = 0x8000_0000;

/// `partnerId` flag: the sender texture is compatible with GL/DX interop.
pub const PARTNER_GLDX_COMPATIBLE: u32 = 0x4000_0000;

const DESCRIPTION_LEN: usize = 256;

/// Contents of a Spout sender information map (`<sender name>`, 280 bytes).
///
/// All fields are little-endian `u32` so that 32-bit and 64-bit processes
/// agree on the layout.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SharedTextureInfo {
    /// D3D11 shared (KMT) handle, truncated to 32 bits by the sender.
    pub share_handle: u32,
    /// Texture width.
    pub width: u32,
    /// Texture height.
    pub height: u32,
    /// `DXGI_FORMAT` value (0 for legacy DirectX 9 senders).
    pub format: u32,
    /// Unused, always 0.
    pub usage: u32,
    /// Executable path of the sender, NUL padded ANSI.
    pub description: String,
    /// Flag bits, see [`PARTNER_CPU_SHARE`] and [`PARTNER_GLDX_COMPATIBLE`].
    pub partner_id: u32,
}

impl SharedTextureInfo {
    /// Decode from the raw 280-byte map. Returns `None` if `bytes` is too short.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < SHARED_TEXTURE_INFO_LEN {
            return None;
        }
        let u32_at = |offset: usize| {
            u32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ])
        };
        Some(SharedTextureInfo {
            share_handle: u32_at(0),
            width: u32_at(4),
            height: u32_at(8),
            format: u32_at(12),
            usage: u32_at(16),
            description: decode_c_string(&bytes[20..20 + DESCRIPTION_LEN]),
            partner_id: u32_at(276),
        })
    }

    /// Encode into the raw 280-byte map layout.
    pub fn encode(&self) -> [u8; SHARED_TEXTURE_INFO_LEN] {
        let mut out = [0u8; SHARED_TEXTURE_INFO_LEN];
        out[0..4].copy_from_slice(&self.share_handle.to_le_bytes());
        out[4..8].copy_from_slice(&self.width.to_le_bytes());
        out[8..12].copy_from_slice(&self.height.to_le_bytes());
        out[12..16].copy_from_slice(&self.format.to_le_bytes());
        out[16..20].copy_from_slice(&self.usage.to_le_bytes());
        encode_c_string(&self.description, &mut out[20..20 + DESCRIPTION_LEN]);
        out[276..280].copy_from_slice(&self.partner_id.to_le_bytes());
        out
    }

    /// Whether the map describes a usable texture (non-zero size and handle).
    pub fn is_valid(&self) -> bool {
        self.width > 0 && self.height > 0 && self.share_handle != 0
    }

    /// Pixel format, treating unknown values as BGRA8 like the reference SDK.
    pub fn pixel_format(&self) -> PixelFormat {
        PixelFormat::from_dxgi_format(self.format).unwrap_or(PixelFormat::Bgra8Unorm)
    }

    /// Whether the sender shares through CPU memory instead of a texture.
    pub fn is_cpu_share(&self) -> bool {
        self.partner_id & PARTNER_CPU_SHARE != 0
    }

    /// Whether the sender flagged its texture as GL/DX interop compatible.
    pub fn is_gldx_compatible(&self) -> bool {
        self.partner_id & PARTNER_GLDX_COMPATIBLE != 0
    }

    /// Reconstruct the pointer-sized handle from the truncated 32-bit value.
    ///
    /// Mirrors `LongToHandle`: the value is sign extended, which is how the
    /// reference SDK recovers handles on 64-bit systems.
    pub fn share_handle_ptr(&self) -> isize {
        self.share_handle as i32 as isize
    }

    /// Truncate a pointer-sized handle to the 32-bit representation stored in
    /// the map (`HandleToLong`).
    pub fn truncate_handle(handle: isize) -> u32 {
        handle as u32
    }
}

/// Decode a NUL terminated (or NUL padded) ANSI string.
pub fn decode_c_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// Write `s` into `dst` as a NUL terminated string, truncating if needed and
/// zero filling the remainder.
pub fn encode_c_string(s: &str, dst: &mut [u8]) {
    dst.fill(0);
    if dst.is_empty() {
        return;
    }
    let max = dst.len() - 1;
    let bytes = s.as_bytes();
    let len = bytes.len().min(max);
    dst[..len].copy_from_slice(&bytes[..len]);
}

/// Read the sender name table (`SpoutSenderNames`).
///
/// The table is an array of 256-byte NUL terminated slots; reading stops at
/// the first empty slot. Names that contain invalid UTF-8 are decoded lossily.
pub fn decode_sender_names(bytes: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    for slot in bytes.chunks(MAX_SENDER_NAME_LEN) {
        if slot.is_empty() || slot[0] == 0 {
            break;
        }
        names.push(decode_c_string(slot));
    }
    names
}

/// Write the sender name table into `dst`, zero filling unused slots.
///
/// Names beyond the capacity of `dst` are dropped.
pub fn encode_sender_names<S: AsRef<str>>(names: &[S], dst: &mut [u8]) {
    dst.fill(0);
    for (name, slot) in names.iter().zip(dst.chunks_mut(MAX_SENDER_NAME_LEN)) {
        if slot.len() < MAX_SENDER_NAME_LEN {
            break;
        }
        encode_c_string(name.as_ref(), slot);
    }
}

/// Produce a name that does not collide with `existing`, appending `_1`,
/// `_2`, ... like the reference SDK does for duplicate sender names.
pub fn unique_sender_name<S: AsRef<str>>(name: &str, existing: &[S]) -> String {
    let taken = |candidate: &str| existing.iter().any(|n| n.as_ref() == candidate);
    if !taken(name) {
        return name.to_owned();
    }
    let mut index = 1u32;
    loop {
        let suffix = format!("_{index}");
        let max_base = MAX_SENDER_NAME_LEN - 1 - suffix.len();
        let base: String = name.chars().take(max_base).collect();
        let candidate = format!("{base}{suffix}");
        if !taken(&candidate) {
            return candidate;
        }
        index += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_texture_info_round_trip() {
        let info = SharedTextureInfo {
            share_handle: 0x4000_1234,
            width: 1920,
            height: 1080,
            format: 87,
            usage: 0,
            description: "C:\\Apps\\sender.exe".into(),
            partner_id: PARTNER_GLDX_COMPATIBLE,
        };
        let bytes = info.encode();
        assert_eq!(bytes.len(), SHARED_TEXTURE_INFO_LEN);
        assert_eq!(&bytes[0..4], &0x4000_1234u32.to_le_bytes());
        assert_eq!(&bytes[4..8], &1920u32.to_le_bytes());
        assert_eq!(&bytes[276..280], &PARTNER_GLDX_COMPATIBLE.to_le_bytes());
        assert_eq!(SharedTextureInfo::decode(&bytes), Some(info));
        assert_eq!(SharedTextureInfo::decode(&bytes[..100]), None);
    }

    #[test]
    fn description_is_truncated_and_nul_terminated() {
        let info = SharedTextureInfo {
            description: "x".repeat(400),
            ..Default::default()
        };
        let bytes = info.encode();
        assert_eq!(bytes[20 + 255], 0);
        let decoded = SharedTextureInfo::decode(&bytes).unwrap();
        assert_eq!(decoded.description.len(), 255);
    }

    #[test]
    fn handle_truncation_matches_handletolong() {
        let handle: isize = 0x4000_1234;
        let truncated = SharedTextureInfo::truncate_handle(handle);
        let info = SharedTextureInfo {
            share_handle: truncated,
            ..Default::default()
        };
        assert_eq!(info.share_handle_ptr(), handle);

        // High bit set: sign extension must reproduce what LongToHandle does.
        let info = SharedTextureInfo {
            share_handle: 0x8000_0001,
            ..Default::default()
        };
        assert_eq!(info.share_handle_ptr(), 0x8000_0001u32 as i32 as isize);
    }

    #[test]
    fn pixel_format_fallback() {
        let mut info = SharedTextureInfo::default();
        assert_eq!(info.pixel_format(), PixelFormat::Bgra8Unorm);
        info.format = 10;
        assert_eq!(info.pixel_format(), PixelFormat::Rgba16Float);
        info.format = 12345;
        assert_eq!(info.pixel_format(), PixelFormat::Bgra8Unorm);
    }

    #[test]
    fn sender_names_round_trip() {
        let names = ["Spout Demo Sender", "OBS", "Resolume Arena - Output"];
        let mut buf = vec![0xffu8; MAX_SENDER_NAME_LEN * 4];
        encode_sender_names(&names, &mut buf);
        assert_eq!(&buf[..17], b"Spout Demo Sender");
        assert_eq!(buf[17], 0);
        assert_eq!(buf[MAX_SENDER_NAME_LEN * 3], 0);
        assert_eq!(decode_sender_names(&buf), names);
    }

    #[test]
    fn sender_names_stop_at_empty_slot() {
        let mut buf = vec![0u8; MAX_SENDER_NAME_LEN * 3];
        encode_c_string("first", &mut buf[..MAX_SENDER_NAME_LEN]);
        encode_c_string("third", &mut buf[MAX_SENDER_NAME_LEN * 2..]);
        assert_eq!(decode_sender_names(&buf), ["first"]);
        assert!(decode_sender_names(&[]).is_empty());
    }

    #[test]
    fn sender_names_drop_beyond_capacity() {
        let names = ["a", "b", "c"];
        let mut buf = vec![0u8; MAX_SENDER_NAME_LEN * 2];
        encode_sender_names(&names, &mut buf);
        assert_eq!(decode_sender_names(&buf), ["a", "b"]);
    }

    #[test]
    fn unique_names() {
        let existing = ["Sender", "Sender_1"];
        assert_eq!(unique_sender_name("Other", &existing), "Other");
        assert_eq!(unique_sender_name("Sender", &existing), "Sender_2");
        let long = "n".repeat(255);
        let unique = unique_sender_name(&long, std::slice::from_ref(&long));
        assert!(unique.len() < MAX_SENDER_NAME_LEN);
        assert!(unique.ends_with("_1"));
    }
}
