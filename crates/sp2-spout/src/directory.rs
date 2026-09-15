//! Enumeration of Spout senders.

use sp2_core::{DirectoryBackend, Error, Result, SenderInfo};

use crate::layout::SharedTextureInfo;
use crate::sender_names::SenderNames;

/// Lists Spout senders registered on the system.
#[derive(Debug)]
pub struct SpoutDirectory {
    names: SenderNames,
}

impl SpoutDirectory {
    /// Open the sender registry.
    pub fn new() -> Result<Self> {
        Ok(SpoutDirectory {
            names: SenderNames::new()?,
        })
    }

    /// Registered sender names (stale entries removed first).
    pub fn sender_names(&mut self) -> Result<Vec<String>> {
        self.names.clean()?;
        self.names.names()
    }

    /// Raw information map of a sender.
    pub fn raw_info(&self, name: &str) -> Result<Option<SharedTextureInfo>> {
        SenderNames::info(name)
    }

    /// Capacity of the sender table (`MaxSenders`).
    pub fn max_senders(&self) -> usize {
        self.names.max_senders()
    }
}

fn to_sender_info(name: &str, raw: &SharedTextureInfo) -> SenderInfo {
    SenderInfo {
        id: name.to_owned(),
        name: name.to_owned(),
        app_name: (!raw.description.is_empty()).then(|| raw.description.clone()),
        width: raw.width,
        height: raw.height,
        format: raw.pixel_format(),
    }
}

impl DirectoryBackend for SpoutDirectory {
    fn senders(&mut self) -> Result<Vec<SenderInfo>> {
        let mut senders = Vec::new();
        for name in self.sender_names()? {
            if let Some(raw) = SenderNames::info(&name)? {
                senders.push(to_sender_info(&name, &raw));
            }
        }
        Ok(senders)
    }

    fn find(&mut self, name_or_id: &str) -> Result<Option<SenderInfo>> {
        Ok(SenderNames::info(name_or_id)?.map(|raw| to_sender_info(name_or_id, &raw)))
    }

    fn active_sender(&mut self) -> Result<Option<SenderInfo>> {
        if let Some(active) = self.names.active_sender()? {
            if let Some(raw) = SenderNames::info(&active)? {
                return Ok(Some(to_sender_info(&active, &raw)));
            }
        }
        Ok(self.senders()?.into_iter().next())
    }

    fn set_active_sender(&mut self, id: &str) -> Result<()> {
        if SenderNames::info(id)?.is_none() {
            return Err(Error::NotFound(id.to_owned()));
        }
        self.names.set_active_sender(id)
    }
}
