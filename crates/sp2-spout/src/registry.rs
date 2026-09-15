//! Spout settings stored under `HKEY_CURRENT_USER\Software\Leading Edge\Spout`.

use sp2_core::Result;
use windows::core::PCSTR;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExA, RegOpenKeyExA, RegQueryValueExA, RegSetValueExA, HKEY,
    HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE,
};

use crate::layout::DEFAULT_MAX_SENDERS;
use crate::shared_memory::cstring;

/// Registry sub key used by the Spout SDK and its tools.
pub const SPOUT_KEY: &str = "Software\\Leading Edge\\Spout";

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the key was opened by this value.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

/// Read a DWORD value from the Spout key. `None` if the key or value is absent.
pub fn read_dword(value_name: &str) -> Option<u32> {
    let sub_key = cstring(SPOUT_KEY).ok()?;
    let value = cstring(value_name).ok()?;
    let mut hkey = HKEY::default();
    // SAFETY: arguments are valid NUL terminated strings and an out pointer.
    let status = unsafe {
        RegOpenKeyExA(
            HKEY_CURRENT_USER,
            PCSTR(sub_key.as_ptr() as *const u8),
            None,
            KEY_READ,
            &mut hkey,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let key = Key(hkey);
    let mut data = [0u8; 4];
    let mut len = data.len() as u32;
    // SAFETY: `data` is a 4-byte buffer and `len` its size.
    let status = unsafe {
        RegQueryValueExA(
            key.0,
            PCSTR(value.as_ptr() as *const u8),
            None,
            None,
            Some(data.as_mut_ptr()),
            Some(&mut len),
        )
    };
    if status != ERROR_SUCCESS || len != 4 {
        return None;
    }
    Some(u32::from_le_bytes(data))
}

/// Write a DWORD value to the Spout key, creating the key if needed.
pub fn write_dword(value_name: &str, value: u32) -> Result<()> {
    let sub_key = cstring(SPOUT_KEY)?;
    let name = cstring(value_name)?;
    let mut hkey = HKEY::default();
    // SAFETY: arguments are valid.
    let status = unsafe {
        RegCreateKeyExA(
            HKEY_CURRENT_USER,
            PCSTR(sub_key.as_ptr() as *const u8),
            None,
            PCSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(sp2_core::Error::os(status.0, "RegCreateKeyExA(Spout)"));
    }
    let key = Key(hkey);
    // SAFETY: arguments are valid.
    let status = unsafe {
        RegSetValueExA(
            key.0,
            PCSTR(name.as_ptr() as *const u8),
            None,
            REG_DWORD,
            Some(&value.to_le_bytes()),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(sp2_core::Error::os(
            status.0,
            format!("RegSetValueExA({value_name})"),
        ));
    }
    Ok(())
}

/// `MaxSenders`: capacity of the sender name table (default 64).
pub fn max_senders() -> usize {
    match read_dword("MaxSenders") {
        Some(n) if n > 0 => n as usize,
        _ => DEFAULT_MAX_SENDERS,
    }
}

/// `Framecount`: whether frame counting is enabled (default true).
pub fn frame_count_enabled() -> bool {
    read_dword("Framecount").map(|v| v != 0).unwrap_or(true)
}

/// `CPU`: whether CPU sharing mode is forced (default false).
pub fn cpu_share_forced() -> bool {
    read_dword("CPU").map(|v| v != 0).unwrap_or(false)
}
