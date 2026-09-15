//! Auto-connecting receiver built on [`SyphonDirectory`] and [`SyphonClient`].

use sp2_core::{FrameInfo, ReceiverBackend, Result, SenderInfo};

use crate::client::SyphonClient;
use crate::directory::SyphonDirectory;

/// A receiver that follows a server by UUID or name (or the first server
/// available) and reconnects when it disappears.
///
/// Server discovery relies on distributed notifications, so the main thread
/// must pump its run loop ([`crate::run_loop::poll`]) for the receiver to
/// find servers.
pub struct SyphonReceiver {
    directory: SyphonDirectory,
    target: Option<String>,
    client: Option<SyphonClient>,
    frame_new: bool,
}

impl std::fmt::Debug for SyphonReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyphonReceiver")
            .field("target", &self.target)
            .field("client", &self.client)
            .finish_non_exhaustive()
    }
}

impl SyphonReceiver {
    /// Create a receiver following `target` (UUID or name; `None` = first server).
    pub fn new(target: Option<&str>) -> Result<Self> {
        Ok(SyphonReceiver {
            directory: SyphonDirectory::new()?,
            target: target.map(str::to_owned),
            client: None,
            frame_new: false,
        })
    }

    /// Change the server to follow.
    pub fn set_target(&mut self, target: Option<&str>) {
        let target = target.map(str::to_owned);
        if target != self.target {
            self.target = target;
            self.disconnect();
        }
    }

    /// The directory used for discovery.
    pub fn directory(&self) -> &SyphonDirectory {
        &self.directory
    }

    /// The connected client, if any.
    pub fn client(&self) -> Option<&SyphonClient> {
        self.client.as_ref()
    }

    /// Mutable access to the connected client.
    pub fn client_mut(&mut self) -> Option<&mut SyphonClient> {
        self.client.as_mut()
    }

    /// Connect if necessary and follow surface changes.
    ///
    /// Returns `true` when a surface is available.
    pub fn update(&mut self) -> Result<bool> {
        if let Some(client) = &self.client {
            if client.is_retired() {
                log::info!("Syphon server {:?} retired", client.server().name);
                self.client = None;
                self.directory.request_announcements();
            }
        }
        if self.client.is_none() {
            let servers = self.directory.servers();
            let server = match &self.target {
                Some(target) => servers.iter().find(|s| s.matches(target)),
                None => servers.first(),
            };
            let Some(server) = server else {
                return Ok(false);
            };
            if !server.supports_iosurface() {
                return Ok(false);
            }
            match SyphonClient::connect(server) {
                Ok(client) => self.client = Some(client),
                Err(e) => {
                    log::warn!("failed to connect to Syphon server {:?}: {e}", server.name);
                    return Ok(false);
                }
            }
        }
        match self.client.as_mut() {
            Some(client) => client.update(),
            None => Ok(false),
        }
    }
}

impl ReceiverBackend for SyphonReceiver {
    fn sender_info(&self) -> Option<&SenderInfo> {
        self.client
            .as_ref()
            .filter(|c| c.is_connected())
            .map(SyphonClient::sender_info)
    }

    fn receive_pixels(&mut self, out: &mut Vec<u8>) -> Result<Option<FrameInfo>> {
        if !self.update()? {
            self.frame_new = false;
            return Ok(None);
        }
        let client = self.client.as_mut().expect("connected");
        let frame = client.receive_pixels(out)?;
        self.frame_new = frame.is_some();
        Ok(frame)
    }

    fn is_frame_new(&self) -> bool {
        self.frame_new
    }

    fn frame_id(&self) -> u64 {
        self.client
            .as_ref()
            .map(SyphonClient::frame_id)
            .unwrap_or(0)
    }

    fn disconnect(&mut self) {
        self.client = None;
        self.frame_new = false;
    }
}
