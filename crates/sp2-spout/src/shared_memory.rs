//! Named, page-file backed memory maps with the `<name>_mutex` lock used by
//! every Spout shared memory object.

use std::ffi::CString;
use std::ptr::NonNull;
use std::time::Duration;

use sp2_core::{Error, Result};
use windows::core::PCSTR;
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, INVALID_HANDLE_VALUE, WAIT_ABANDONED,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::System::Memory::{
    CreateFileMappingA, MapViewOfFile, OpenFileMappingA, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
    MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};
use windows::Win32::System::Threading::{CreateMutexA, ReleaseMutex, WaitForSingleObject};

/// Timeout used by the reference SDK when locking a map mutex.
pub const LOCK_TIMEOUT: Duration = Duration::from_millis(67);

/// Convert a `windows` error into the crate error type.
pub(crate) fn os_error(context: &str, err: windows::core::Error) -> Error {
    Error::Os {
        code: err.code().0 as i64,
        message: format!("{context}: {}", err.message()),
    }
}

/// Convert a `&str` into a NUL terminated ANSI string for Win32 `A` APIs.
pub(crate) fn cstring(name: &str) -> Result<CString> {
    CString::new(name).map_err(|_| Error::InvalidArgument(format!("name contains NUL: {name:?}")))
}

/// Owned kernel handle closed on drop.
#[derive(Debug)]
pub(crate) struct OwnedHandle(pub HANDLE);

impl OwnedHandle {
    pub fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: the handle was returned by a Win32 creation function and
            // is owned exclusively by this value.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

// SAFETY: kernel handles may be used from any thread.
unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

/// Named mutex (`<name>_mutex`, `<name>_SpoutAccessMutex`, ...).
#[derive(Debug)]
pub struct NamedMutex {
    handle: OwnedHandle,
    name: String,
}

impl NamedMutex {
    /// Create or open a named mutex.
    pub fn create(name: &str) -> Result<Self> {
        let cname = cstring(name)?;
        // SAFETY: `cname` is a valid NUL terminated string that outlives the call.
        let handle = unsafe { CreateMutexA(None, false, PCSTR(cname.as_ptr() as *const u8)) }
            .map_err(|e| os_error(&format!("CreateMutexA({name})"), e))?;
        Ok(NamedMutex {
            handle: OwnedHandle(handle),
            name: name.to_owned(),
        })
    }

    /// Mutex name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Acquire the mutex, waiting at most `timeout`.
    ///
    /// Returns `Ok(true)` when acquired (including abandoned mutexes, which the
    /// reference SDK also treats as acquired), `Ok(false)` on timeout.
    pub fn try_lock(&self, timeout: Duration) -> Result<bool> {
        let ms = timeout.as_millis().min(u32::MAX as u128) as u32;
        // SAFETY: the handle is a valid mutex handle owned by `self`.
        let result = unsafe { WaitForSingleObject(self.handle.raw(), ms) };
        match result {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            other => Err(Error::Os {
                code: other.0 as i64,
                message: format!("WaitForSingleObject({}) failed", self.name),
            }),
        }
    }

    /// Release the mutex previously acquired with [`NamedMutex::try_lock`].
    pub fn unlock(&self) {
        // SAFETY: the handle is a valid mutex handle owned by `self`; releasing
        // a mutex we do not own fails harmlessly.
        unsafe {
            let _ = ReleaseMutex(self.handle.raw());
        }
    }

    /// Acquire the mutex and return a guard, or `Err(Error::Timeout)`.
    pub fn lock(&self, timeout: Duration) -> Result<MutexGuard<'_>> {
        if self.try_lock(timeout)? {
            Ok(MutexGuard { mutex: self })
        } else {
            Err(Error::Timeout)
        }
    }
}

/// RAII guard releasing a [`NamedMutex`] on drop.
pub struct MutexGuard<'a> {
    mutex: &'a NamedMutex,
}

impl Drop for MutexGuard<'_> {
    fn drop(&mut self) {
        self.mutex.unlock();
    }
}

/// A named shared memory map protected by `<name>_mutex`.
#[derive(Debug)]
pub struct SharedMemory {
    name: String,
    _mapping: OwnedHandle,
    view: NonNull<u8>,
    size: usize,
    mutex: NamedMutex,
    created: bool,
}

// SAFETY: access to the mapped view is serialized through the named mutex and
// the mapping handle can be used from any thread.
unsafe impl Send for SharedMemory {}
unsafe impl Sync for SharedMemory {}

impl SharedMemory {
    /// Create a map of `size` bytes, or open the existing map with that name.
    ///
    /// When the map already existed its original size is kept by the OS;
    /// [`SharedMemory::was_created`] reports which case occurred.
    pub fn create(name: &str, size: usize) -> Result<Self> {
        if size == 0 || size > u32::MAX as usize {
            return Err(Error::InvalidArgument(format!("invalid map size {size}")));
        }
        let cname = cstring(name)?;
        // SAFETY: arguments are valid; INVALID_HANDLE_VALUE requests a page-file
        // backed mapping.
        let mapping = unsafe {
            CreateFileMappingA(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                0,
                size as u32,
                PCSTR(cname.as_ptr() as *const u8),
            )
        }
        .map_err(|e| os_error(&format!("CreateFileMappingA({name})"), e))?;
        // SAFETY: GetLastError is meaningful right after a successful
        // CreateFileMappingA and reports ERROR_ALREADY_EXISTS for reopened maps.
        let created = unsafe { GetLastError() } != ERROR_ALREADY_EXISTS;
        Self::map(name, OwnedHandle(mapping), size, created)
    }

    /// Open an existing map. Fails with [`Error::NotFound`] if it does not exist.
    pub fn open(name: &str, size: usize) -> Result<Self> {
        let cname = cstring(name)?;
        // SAFETY: arguments are valid.
        let mapping = unsafe {
            OpenFileMappingA(
                FILE_MAP_ALL_ACCESS.0,
                false,
                PCSTR(cname.as_ptr() as *const u8),
            )
        }
        .map_err(|_| Error::NotFound(name.to_owned()))?;
        Self::map(name, OwnedHandle(mapping), size, false)
    }

    /// Whether a map with this name currently exists.
    pub fn exists(name: &str) -> bool {
        let Ok(cname) = cstring(name) else {
            return false;
        };
        // SAFETY: arguments are valid; the handle is closed immediately.
        match unsafe {
            OpenFileMappingA(
                FILE_MAP_ALL_ACCESS.0,
                false,
                PCSTR(cname.as_ptr() as *const u8),
            )
        } {
            Ok(handle) => {
                drop(OwnedHandle(handle));
                true
            }
            Err(_) => false,
        }
    }

    fn map(name: &str, mapping: OwnedHandle, size: usize, created: bool) -> Result<Self> {
        // SAFETY: `mapping` is a valid file mapping handle.
        let view: MEMORY_MAPPED_VIEW_ADDRESS =
            unsafe { MapViewOfFile(mapping.raw(), FILE_MAP_ALL_ACCESS, 0, 0, size) };
        let view = NonNull::new(view.Value as *mut u8).ok_or_else(|| {
            // SAFETY: called right after the failing MapViewOfFile.
            let err = unsafe { GetLastError() };
            Error::Os {
                code: err.0 as i64,
                message: format!("MapViewOfFile({name}) failed"),
            }
        })?;
        let mutex = NamedMutex::create(&format!("{name}_mutex"))?;
        Ok(SharedMemory {
            name: name.to_owned(),
            _mapping: mapping,
            view,
            size,
            mutex,
            created,
        })
    }

    /// Map name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Mapped size in bytes.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Whether [`SharedMemory::create`] created a new map rather than opening
    /// an existing one.
    pub fn was_created(&self) -> bool {
        self.created
    }

    /// Lock the map and run `f` with a view of its contents.
    pub fn with_lock<R>(&self, f: impl FnOnce(&mut [u8]) -> R) -> Result<R> {
        let _guard = self.mutex.lock(LOCK_TIMEOUT)?;
        // SAFETY: the view is valid for `size` bytes for the lifetime of
        // `self`, and the named mutex serializes access across processes.
        let slice = unsafe { std::slice::from_raw_parts_mut(self.view.as_ptr(), self.size) };
        Ok(f(slice))
    }

    /// Lock the map and copy its contents into a new vector.
    pub fn read(&self) -> Result<Vec<u8>> {
        self.with_lock(|bytes| bytes.to_vec())
    }

    /// Lock the map and overwrite the beginning of it with `data`.
    pub fn write(&self, data: &[u8]) -> Result<()> {
        if data.len() > self.size {
            return Err(Error::SizeMismatch {
                expected: self.size,
                actual: data.len(),
            });
        }
        self.with_lock(|bytes| bytes[..data.len()].copy_from_slice(data))
    }
}

impl Drop for SharedMemory {
    fn drop(&mut self) {
        // SAFETY: `view` was returned by MapViewOfFile and is unmapped once.
        unsafe {
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view.as_ptr() as *mut _,
            });
        }
    }
}
