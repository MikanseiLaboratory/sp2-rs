//! Frame counting (`<name>_Count_Semaphore`) and texture access
//! synchronisation (`<name>_SpoutAccessMutex`).

use std::time::{Duration, Instant};

use sp2_core::Result;
use windows::core::PCSTR;
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::System::Threading::{CreateSemaphoreA, ReleaseSemaphore, WaitForSingleObject};

use crate::registry;
use crate::shared_memory::{cstring, os_error, MutexGuard, NamedMutex, OwnedHandle};

/// Timeout used when waiting for the texture access mutex.
pub const ACCESS_TIMEOUT: Duration = Duration::from_millis(67);

/// Semaphore based frame counter shared between a sender and its receivers.
///
/// The semaphore count itself is the frame number. The sender decrements by
/// one (`WaitForSingleObject` with zero timeout) and releases by two, for a
/// net increment of one; a receiver decrements and releases by one, reading
/// the previous count without changing it.
pub struct FrameCounter {
    semaphore: Option<OwnedHandle>,
    enabled: bool,
    frame_count: u64,
    last_count: Option<i32>,
    // Receive-side frame rate estimate.
    fps: f64,
    last_frame_time: Option<Instant>,
}

impl std::fmt::Debug for FrameCounter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameCounter")
            .field("enabled", &self.enabled)
            .field("frame_count", &self.frame_count)
            .field("last_count", &self.last_count)
            .finish()
    }
}

impl FrameCounter {
    /// Create or open the frame count semaphore for `sender_name`.
    ///
    /// Frame counting is disabled when the registry `Framecount` value is 0;
    /// in that case every frame is reported as new.
    pub fn new(sender_name: &str) -> Result<Self> {
        let enabled = registry::frame_count_enabled();
        let semaphore = if enabled {
            let name = cstring(&format!("{sender_name}_Count_Semaphore"))?;
            // SAFETY: `name` is a valid NUL terminated string.
            let handle =
                unsafe { CreateSemaphoreA(None, 1, i32::MAX, PCSTR(name.as_ptr() as *const u8)) }
                    .map_err(|e| os_error("CreateSemaphoreA", e))?;
            Some(OwnedHandle(handle))
        } else {
            None
        };
        Ok(FrameCounter {
            semaphore,
            enabled,
            frame_count: 0,
            last_count: None,
            fps: 0.0,
            last_frame_time: None,
        })
    }

    /// Whether frame counting is active.
    pub fn is_enabled(&self) -> bool {
        self.enabled && self.semaphore.is_some()
    }

    /// Frames counted locally (sent frames for a sender, received frames for a
    /// receiver).
    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }

    /// Latest semaphore value observed by a receiver (sender frame number).
    pub fn sender_frame(&self) -> u64 {
        self.last_count.map(|c| c.max(0) as u64).unwrap_or(0)
    }

    /// Estimated receive frame rate.
    pub fn fps(&self) -> f64 {
        self.fps
    }

    /// Sender side: mark a new frame by incrementing the shared count.
    pub fn set_new_frame(&mut self) {
        self.frame_count = self.frame_count.wrapping_add(1);
        let Some(sem) = &self.semaphore else {
            return;
        };
        // SAFETY: valid semaphore handle owned by `self`.
        unsafe {
            if WaitForSingleObject(sem.raw(), 0) == WAIT_OBJECT_0 {
                let mut previous = 0i32;
                if ReleaseSemaphore(sem.raw(), 2, Some(&mut previous)).is_ok() {
                    self.last_count = Some(previous.saturating_add(2));
                }
            } else {
                // Count reached zero (should not happen); restore it.
                let _ = ReleaseSemaphore(sem.raw(), 1, None);
            }
        }
    }

    /// Receiver side: check whether the sender produced a new frame since the
    /// last call. Always `true` when frame counting is disabled.
    ///
    /// Matches the official SDK: do not block when the semaphore cannot be
    /// read, and treat the value returned through `ReleaseSemaphore` as the
    /// frame count. A count of 0 means the sender is not using frame counting,
    /// so every poll is considered new.
    pub fn get_new_frame(&mut self) -> bool {
        let Some(sem) = &self.semaphore else {
            self.note_received();
            return true;
        };
        // SAFETY: valid semaphore handle owned by `self`.
        let current = unsafe {
            if WaitForSingleObject(sem.raw(), 0) != WAIT_OBJECT_0 {
                self.note_received();
                return true;
            }
            let mut previous = 0i32;
            if ReleaseSemaphore(sem.raw(), 1, Some(&mut previous)).is_err() {
                self.note_received();
                return true;
            }
            // `previous` is the count after WaitForSingleObject decremented
            // it and before ReleaseSemaphore restores it. This is exactly
            // the value used by SpoutFrameCount::GetNewFrame.
            previous
        };
        if current == 0 {
            self.last_count = Some(current);
            self.note_received();
            return true;
        }
        let is_new = self.last_count != Some(current);
        self.last_count = Some(current);
        if is_new {
            self.note_received();
        }
        is_new
    }

    fn note_received(&mut self) {
        self.frame_count = self.frame_count.wrapping_add(1);
        self.update_fps();
    }

    /// Forget the last observed count so the next frame is reported as new.
    pub fn reset(&mut self) {
        self.last_count = None;
        self.last_frame_time = None;
    }

    fn update_fps(&mut self) {
        let now = Instant::now();
        if let Some(last) = self.last_frame_time {
            let dt = now.duration_since(last).as_secs_f64();
            if dt > 0.0 {
                let instant = 1.0 / dt;
                self.fps = if self.fps == 0.0 {
                    instant
                } else {
                    self.fps * 0.9 + instant * 0.1
                };
            }
        }
        self.last_frame_time = Some(now);
    }
}

/// The per-sender texture access mutex.
#[derive(Debug)]
pub struct AccessMutex {
    mutex: NamedMutex,
}

impl AccessMutex {
    /// Create or open `<sender_name>_SpoutAccessMutex`.
    pub fn new(sender_name: &str) -> Result<Self> {
        Ok(AccessMutex {
            mutex: NamedMutex::create(&format!("{sender_name}_SpoutAccessMutex"))?,
        })
    }

    /// Wait up to [`ACCESS_TIMEOUT`] for exclusive texture access.
    ///
    /// Returns `Err(Error::Timeout)` if the other side holds the texture;
    /// callers skip the frame in that case.
    pub fn lock(&self) -> Result<MutexGuard<'_>> {
        self.mutex.lock(ACCESS_TIMEOUT)
    }
}
