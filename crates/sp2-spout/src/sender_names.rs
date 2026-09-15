//! The system-wide sender registry: `SpoutSenderNames`, `ActiveSenderName`
//! and the per-sender `SharedTextureInfo` maps.

use std::sync::Mutex;

use sp2_core::{Error, Result};

use crate::layout::{
    decode_c_string, decode_sender_names, encode_c_string, encode_sender_names, unique_sender_name,
    SharedTextureInfo, ACTIVE_SENDER_MAP, MAX_SENDER_NAME_LEN, SENDER_NAMES_MAP,
    SHARED_TEXTURE_INFO_LEN,
};
use crate::registry;
use crate::shared_memory::SharedMemory;

/// Handle to the sender name table shared by every Spout process.
///
/// Creating the table if it does not exist and keeping the mapping open for
/// the lifetime of the value mirrors the reference SDK, where the table lives
/// as long as at least one Spout process holds it open.
pub struct SenderNames {
    max_senders: usize,
    table: SharedMemory,
    active: Mutex<Option<SharedMemory>>,
}

impl std::fmt::Debug for SenderNames {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SenderNames")
            .field("max_senders", &self.max_senders)
            .finish_non_exhaustive()
    }
}

impl SenderNames {
    /// Open (or create) the sender name table using the registry `MaxSenders`.
    pub fn new() -> Result<Self> {
        let max_senders = registry::max_senders();
        let table = SharedMemory::create(SENDER_NAMES_MAP, max_senders * MAX_SENDER_NAME_LEN)?;
        Ok(SenderNames {
            max_senders,
            table,
            active: Mutex::new(None),
        })
    }

    /// Capacity of the table.
    pub fn max_senders(&self) -> usize {
        self.max_senders
    }

    /// All registered sender names in table order.
    pub fn names(&self) -> Result<Vec<String>> {
        self.table.with_lock(|bytes| decode_sender_names(bytes))
    }

    /// Whether `name` is registered.
    pub fn contains(&self, name: &str) -> Result<bool> {
        Ok(self.names()?.iter().any(|n| n == name))
    }

    /// Register a sender name, uniquifying it (`name_1`, ...) if it already
    /// exists. Returns the name actually registered.
    ///
    /// The first sender registered becomes the active sender, as does any
    /// sender registered while no valid active sender exists.
    pub fn register(&self, name: &str) -> Result<String> {
        let registered = self.table.with_lock(|bytes| {
            let mut names = decode_sender_names(bytes);
            if names.len() >= self.max_senders {
                return Err(Error::Backend(format!(
                    "sender table is full ({} senders)",
                    self.max_senders
                )));
            }
            let unique = unique_sender_name(name, &names);
            names.push(unique.clone());
            encode_sender_names(&names, bytes);
            Ok((unique, names.len()))
        })??;
        let (unique, count) = registered;
        let active_valid = match self.active_sender()? {
            Some(active) => active != unique && SharedMemory::exists(&active),
            None => false,
        };
        if count == 1 || !active_valid {
            self.set_active_sender(&unique)?;
        }
        Ok(unique)
    }

    /// Remove a sender name from the table.
    ///
    /// If the sender was active, the first remaining sender becomes active
    /// (or the active name is cleared when the table is empty).
    pub fn release(&self, name: &str) -> Result<()> {
        let remaining = self.table.with_lock(|bytes| {
            let mut names = decode_sender_names(bytes);
            let before = names.len();
            names.retain(|n| n != name);
            if names.len() != before {
                encode_sender_names(&names, bytes);
            }
            names
        })?;
        if self.active_sender()?.as_deref() == Some(name) {
            match remaining.first() {
                Some(next) => self.set_active_sender(next)?,
                None => self.set_active_sender("")?,
            }
        }
        Ok(())
    }

    /// Remove registered names whose information map no longer exists (the
    /// sender process terminated without releasing its name).
    pub fn clean(&self) -> Result<Vec<String>> {
        let removed = self.table.with_lock(|bytes| {
            let names = decode_sender_names(bytes);
            let (alive, dead): (Vec<_>, Vec<_>) =
                names.into_iter().partition(|n| SharedMemory::exists(n));
            if !dead.is_empty() {
                encode_sender_names(&alive, bytes);
            }
            dead
        })?;
        if !removed.is_empty() {
            log::debug!("removed stale Spout senders: {removed:?}");
            if let Some(active) = self.active_sender()? {
                if removed.contains(&active) {
                    let names = self.names()?;
                    self.set_active_sender(names.first().map(String::as_str).unwrap_or(""))?;
                }
            }
        }
        Ok(removed)
    }

    /// Name of the active sender, if the `ActiveSenderName` map exists and is
    /// not empty.
    pub fn active_sender(&self) -> Result<Option<String>> {
        let guard = self
            .active
            .lock()
            .map_err(|_| Error::backend("poisoned lock"))?;
        let read = |map: &SharedMemory| map.with_lock(|bytes| decode_c_string(bytes));
        let name = match guard.as_ref() {
            Some(map) => read(map)?,
            None => match SharedMemory::open(ACTIVE_SENDER_MAP, MAX_SENDER_NAME_LEN) {
                Ok(map) => read(&map)?,
                Err(Error::NotFound(_)) => return Ok(None),
                Err(e) => return Err(e),
            },
        };
        Ok(if name.is_empty() { None } else { Some(name) })
    }

    /// Set the active sender name (empty string clears it).
    pub fn set_active_sender(&self, name: &str) -> Result<()> {
        let mut guard = self
            .active
            .lock()
            .map_err(|_| Error::backend("poisoned lock"))?;
        if guard.is_none() {
            *guard = Some(SharedMemory::create(
                ACTIVE_SENDER_MAP,
                MAX_SENDER_NAME_LEN,
            )?);
        }
        let map = guard.as_ref().expect("initialised above");
        map.with_lock(|bytes| encode_c_string(name, bytes))
    }

    /// Read the information map of `name`. `None` if the sender does not exist.
    pub fn info(name: &str) -> Result<Option<SharedTextureInfo>> {
        match SharedMemory::open(name, SHARED_TEXTURE_INFO_LEN) {
            Ok(map) => {
                let info = map.with_lock(|bytes| SharedTextureInfo::decode(bytes))?;
                Ok(info)
            }
            Err(Error::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// The information map owned by a sender (`<sender name>`, 280 bytes).
#[derive(Debug)]
pub struct SenderInfoMap {
    map: SharedMemory,
}

impl SenderInfoMap {
    /// Create the information map for `name`.
    pub fn create(name: &str) -> Result<Self> {
        let map = SharedMemory::create(name, SHARED_TEXTURE_INFO_LEN)?;
        Ok(SenderInfoMap { map })
    }

    /// Overwrite the map contents.
    pub fn write(&self, info: &SharedTextureInfo) -> Result<()> {
        self.map.write(&info.encode())
    }

    /// Read the map contents.
    pub fn read(&self) -> Result<SharedTextureInfo> {
        self.map
            .with_lock(|bytes| SharedTextureInfo::decode(bytes))?
            .ok_or_else(|| Error::backend("sender info map is too small"))
    }
}
