//! Syphon client: connects to a server and reads its IOSurface.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{MTLDevice, MTLTexture};
use sp2_core::{Error, FrameInfo, PixelFormat, Result, SenderInfo};

use crate::cf::{new_uuid_string, Payload};
use crate::constants::{ClientMessage, ServerMessage};
use crate::description::ServerDescription;
use crate::iosurface::Surface;
use crate::messaging::{Message, MessageReceiver, MessageSender};

struct Shared {
    surface_id: AtomicU32,
    new_frame: AtomicBool,
    retired: AtomicBool,
}

/// A Syphon compatible client connected to one server.
///
/// The client registers itself for info and frame messages, tracks the
/// server's `IOSurfaceID` and exposes the surface both as CPU pixels and as
/// a Metal texture.
pub struct SyphonClient {
    uuid: String,
    server: ServerDescription,
    info: SenderInfo,
    shared: Arc<Shared>,
    sender: MessageSender,
    surface: Option<Surface>,
    last_seed: Option<u32>,
    frame_id: u64,
    frame_new: bool,
    _receiver: MessageReceiver,
}

impl std::fmt::Debug for SyphonClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyphonClient")
            .field("uuid", &self.uuid)
            .field("server", &self.server.uuid)
            .field("surface", &self.surface)
            .finish_non_exhaustive()
    }
}

impl SyphonClient {
    /// Connect to the server described by `server`.
    pub fn connect(server: &ServerDescription) -> Result<Self> {
        if !server.supports_iosurface() {
            return Err(Error::Unsupported("server does not offer an IOSurface"));
        }
        let uuid = new_uuid_string()?;
        let shared = Arc::new(Shared {
            surface_id: AtomicU32::new(0),
            new_frame: AtomicBool::new(false),
            retired: AtomicBool::new(false),
        });
        let handler_shared = shared.clone();
        let receiver = MessageReceiver::new(
            &uuid,
            Arc::new(move |message| handle_server_message(&handler_shared, message)),
        )?;

        let sender = MessageSender::new(&server.uuid);
        let payload = Payload::Text(uuid.clone());
        sender
            .send(ClientMessage::AddClientForInfo as i32, &payload)
            .map_err(|_| Error::NotFound(server.uuid.clone()))?;
        sender
            .send(ClientMessage::AddClientForFrames as i32, &payload)
            .map_err(|_| Error::NotFound(server.uuid.clone()))?;

        log::info!(
            "connected to Syphon server {:?} ({})",
            server.name,
            server.uuid
        );
        Ok(SyphonClient {
            uuid,
            info: server.to_sender_info(0, 0),
            server: server.clone(),
            shared,
            sender,
            surface: None,
            last_seed: None,
            frame_id: 0,
            frame_new: false,
            _receiver: receiver,
        })
    }

    /// Description of the connected server.
    pub fn server(&self) -> &ServerDescription {
        &self.server
    }

    /// Unified description of the server including the current surface size.
    pub fn sender_info(&self) -> &SenderInfo {
        &self.info
    }

    /// Whether the server announced that it is going away.
    pub fn is_retired(&self) -> bool {
        self.shared.retired.load(Ordering::SeqCst)
    }

    /// Whether the client currently holds a valid surface.
    pub fn is_connected(&self) -> bool {
        !self.is_retired() && self.surface.is_some()
    }

    /// Follow surface changes announced by the server.
    ///
    /// Returns `true` when a surface is available.
    pub fn update(&mut self) -> Result<bool> {
        if self.is_retired() {
            self.surface = None;
            return Ok(false);
        }
        let id = self.shared.surface_id.load(Ordering::SeqCst);
        if id == 0 {
            return Ok(self.surface.is_some());
        }
        let current = self.surface.as_ref().map(Surface::id);
        if current != Some(id) {
            match Surface::lookup(id) {
                Some(surface) => {
                    log::info!(
                        "Syphon surface {} is {}x{}",
                        id,
                        surface.width(),
                        surface.height()
                    );
                    self.info.width = surface.width();
                    self.info.height = surface.height();
                    self.info.format = PixelFormat::Bgra8Unorm;
                    self.surface = Some(surface);
                    self.last_seed = None;
                    self.shared.new_frame.store(true, Ordering::SeqCst);
                }
                None => {
                    log::warn!("IOSurfaceLookup({id}) failed");
                    self.surface = None;
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// Whether a new frame arrived since the last receive.
    ///
    /// Combines the `NewFrame` message with the surface seed so that frames
    /// are detected even when messages are coalesced.
    pub fn has_new_frame(&self) -> bool {
        if self.shared.new_frame.load(Ordering::SeqCst) {
            return true;
        }
        match (&self.surface, self.last_seed) {
            (Some(surface), Some(seed)) => surface.seed() != seed,
            (Some(_), None) => true,
            _ => false,
        }
    }

    /// The server's surface, if connected.
    pub fn surface(&self) -> Option<&Surface> {
        self.surface.as_ref()
    }

    /// Metal texture backed by the server's surface (zero copy).
    pub fn metal_texture(
        &self,
        device: &ProtocolObject<dyn MTLDevice>,
    ) -> Result<Retained<ProtocolObject<dyn MTLTexture>>> {
        self.surface
            .as_ref()
            .ok_or(Error::NotConnected)?
            .metal_texture(device)
    }

    /// Mark the current frame as consumed (for GPU consumers that read the
    /// surface directly) and return its [`FrameInfo`], or `None` if no new
    /// frame is available.
    pub fn acknowledge_frame(&mut self) -> Result<Option<FrameInfo>> {
        if !self.update()? || !self.has_new_frame() {
            self.frame_new = false;
            return Ok(None);
        }
        let surface = self.surface.as_ref().expect("checked by update");
        self.shared.new_frame.store(false, Ordering::SeqCst);
        self.last_seed = Some(surface.seed());
        self.frame_id = self.frame_id.wrapping_add(1);
        self.frame_new = true;
        Ok(Some(self.frame_info()))
    }

    /// Copy the latest frame into `out` if it is new.
    pub fn receive_pixels(&mut self, out: &mut Vec<u8>) -> Result<Option<FrameInfo>> {
        if !self.update()? || !self.has_new_frame() {
            self.frame_new = false;
            return Ok(None);
        }
        let surface = self.surface.as_ref().expect("checked by update");
        self.shared.new_frame.store(false, Ordering::SeqCst);
        let seed = surface.read_pixels(out)?;
        self.last_seed = Some(seed);
        self.frame_id = self.frame_id.wrapping_add(1);
        self.frame_new = true;
        Ok(Some(self.frame_info()))
    }

    /// Whether the last receive produced a new frame.
    pub fn is_frame_new(&self) -> bool {
        self.frame_new
    }

    /// Number of frames received so far.
    pub fn frame_id(&self) -> u64 {
        self.frame_id
    }

    fn frame_info(&self) -> FrameInfo {
        FrameInfo {
            width: self.info.width,
            height: self.info.height,
            format: PixelFormat::Bgra8Unorm,
            frame_id: self.frame_id,
        }
    }
}

fn handle_server_message(shared: &Shared, message: Message) {
    match ServerMessage::from_id(message.id) {
        Some(ServerMessage::NewFrame) => shared.new_frame.store(true, Ordering::SeqCst),
        Some(ServerMessage::UpdateSurfaceId) => {
            if let Some(id) = message.payload.as_number() {
                shared.surface_id.store(id as u32, Ordering::SeqCst);
                shared.new_frame.store(true, Ordering::SeqCst);
            }
        }
        Some(ServerMessage::RetireServer) => shared.retired.store(true, Ordering::SeqCst),
        Some(ServerMessage::UpdateServerName) => {}
        None => log::debug!("unknown Syphon server message {}", message.id),
    }
}

impl Drop for SyphonClient {
    fn drop(&mut self) {
        if !self.is_retired() {
            let payload = Payload::Text(self.uuid.clone());
            let _ = self
                .sender
                .send(ClientMessage::RemoveClientForFrames as i32, &payload);
            let _ = self
                .sender
                .send(ClientMessage::RemoveClientForInfo as i32, &payload);
        }
    }
}
