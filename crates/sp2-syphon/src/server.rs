//! Syphon server: publishes an IOSurface to clients.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{MTLDevice, MTLTexture};
use sp2_core::{Error, PixelBuffer, PixelFormat, Result, SenderBackend, SenderInfo};

use crate::cf::{new_uuid_string, process_name, Payload};
use crate::constants::{
    ClientMessage, ServerMessage, NOTIFICATION_ANNOUNCE, NOTIFICATION_ANNOUNCE_REQUEST,
    NOTIFICATION_RETIRE, NOTIFICATION_UPDATE,
};
use crate::description::ServerDescription;
use crate::directory::{post_server_notification, Observer};
use crate::iosurface::Surface;
use crate::messaging::{Message, MessageReceiver, MessageSender};

/// Options for [`SyphonServer::new`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerOptions {
    /// Do not broadcast the server (`SyphonServerOptionIsPrivate`). Clients
    /// that know the UUID can still connect.
    pub private: bool,
}

#[derive(Default)]
struct Clients {
    info: HashMap<String, MessageSender>,
    frames: HashMap<String, MessageSender>,
}

struct Shared {
    clients: Mutex<Clients>,
    surface_id: AtomicU32,
}

/// A Syphon compatible server.
///
/// The server owns a local `CFMessagePort` named after its UUID, a global
/// BGRA8 IOSurface and the list of connected clients. It announces itself
/// through distributed notifications (unless private) and answers announce
/// requests, which requires the main thread run loop to be pumped (see
/// [`crate::run_loop::poll`]).
pub struct SyphonServer {
    desc: ServerDescription,
    options: ServerOptions,
    shared: Arc<Shared>,
    surface: Surface,
    frame_count: u64,
    _receiver: MessageReceiver,
    _announce_observer: Observer,
}

impl std::fmt::Debug for SyphonServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyphonServer")
            .field("uuid", &self.desc.uuid)
            .field("name", &self.desc.name)
            .field("surface", &self.surface)
            .finish_non_exhaustive()
    }
}

impl SyphonServer {
    /// Create a server named `name` with an initial `width` x `height` surface.
    pub fn new(name: &str, width: u32, height: u32, options: ServerOptions) -> Result<Self> {
        let uuid = new_uuid_string()?;
        let desc = ServerDescription::new(uuid.clone(), name, process_name());
        let surface = Surface::create(width, height)?;

        let shared = Arc::new(Shared {
            clients: Mutex::new(Clients::default()),
            surface_id: AtomicU32::new(surface.id()),
        });

        let handler_shared = shared.clone();
        let receiver = MessageReceiver::new(
            &uuid,
            Arc::new(move |message| handle_client_message(&handler_shared, message)),
        )?;

        let announce_desc = desc.clone();
        let private = options.private;
        let announce_observer = Observer::new(NOTIFICATION_ANNOUNCE_REQUEST, move |_| {
            if !private {
                post_server_notification(NOTIFICATION_ANNOUNCE, &announce_desc);
            }
        });

        let server = SyphonServer {
            desc,
            options,
            shared,
            surface,
            frame_count: 0,
            _receiver: receiver,
            _announce_observer: announce_observer,
        };
        if !server.options.private {
            post_server_notification(NOTIFICATION_ANNOUNCE, &server.desc);
        }
        log::info!(
            "created Syphon server {:?} ({}) {}x{}",
            server.desc.name,
            server.desc.uuid,
            width,
            height
        );
        Ok(server)
    }

    /// The published description.
    pub fn description(&self) -> &ServerDescription {
        &self.desc
    }

    /// Server UUID string (`info.v002.Syphon.<UUID>`).
    pub fn uuid(&self) -> &str {
        &self.desc.uuid
    }

    /// Rename the server and notify directories.
    pub fn set_name(&mut self, name: &str) {
        if self.desc.name == name {
            return;
        }
        self.desc.name = name.to_owned();
        if !self.options.private {
            post_server_notification(NOTIFICATION_UPDATE, &self.desc);
        }
        // Current clients ignore UpdateServerName; still send it for parity.
        let payload = Payload::Text(name.to_owned());
        self.broadcast(ServerMessage::UpdateServerName, &payload, false);
    }

    /// Whether any client registered for frames.
    pub fn has_clients(&self) -> bool {
        self.shared
            .clients
            .lock()
            .map(|c| !c.frames.is_empty())
            .unwrap_or(false)
    }

    /// The IOSurface currently published.
    pub fn surface(&self) -> &Surface {
        &self.surface
    }

    /// Metal texture backed by the surface, for rendering into it directly.
    ///
    /// Call [`SyphonServer::publish`] after the GPU work that writes the
    /// texture has completed.
    pub fn metal_texture(
        &self,
        device: &ProtocolObject<dyn MTLDevice>,
    ) -> Result<Retained<ProtocolObject<dyn MTLTexture>>> {
        self.surface.metal_texture(device)
    }

    /// Recreate the surface if the size differs, notifying info clients.
    pub fn ensure_surface(&mut self, width: u32, height: u32) -> Result<()> {
        if self.surface.width() == width && self.surface.height() == height {
            return Ok(());
        }
        let surface = Surface::create(width, height)?;
        self.shared.surface_id.store(surface.id(), Ordering::SeqCst);
        self.surface = surface;
        let payload = Payload::Number(self.surface.id() as i64);
        self.broadcast(ServerMessage::UpdateSurfaceId, &payload, false);
        log::info!(
            "Syphon server {:?} surface resized to {}x{}",
            self.desc.name,
            width,
            height
        );
        Ok(())
    }

    /// Notify frame clients that the surface holds a new frame.
    pub fn publish(&mut self) {
        self.frame_count = self.frame_count.wrapping_add(1);
        self.broadcast(ServerMessage::NewFrame, &Payload::None, true);
    }

    fn broadcast(&self, message: ServerMessage, payload: &Payload, frame_clients: bool) {
        let Ok(mut clients) = self.shared.clients.lock() else {
            return;
        };
        let targets = if frame_clients {
            &mut clients.frames
        } else {
            &mut clients.info
        };
        targets.retain(|uuid, sender| match sender.send(message as i32, payload) {
            Ok(()) => true,
            Err(e) => {
                log::debug!("dropping Syphon client {uuid}: {e}");
                false
            }
        });
    }
}

fn handle_client_message(shared: &Shared, message: Message) {
    let Some(kind) = ClientMessage::from_id(message.id) else {
        log::debug!("unknown Syphon client message {}", message.id);
        return;
    };
    let Some(uuid) = message.payload.as_text() else {
        log::debug!("Syphon client message {kind:?} without client UUID");
        return;
    };
    let Ok(mut clients) = shared.clients.lock() else {
        return;
    };
    match kind {
        ClientMessage::AddClientForInfo => {
            let sender = MessageSender::new(uuid);
            let surface_id = shared.surface_id.load(Ordering::SeqCst);
            if surface_id != 0 {
                let _ = sender.send(
                    ServerMessage::UpdateSurfaceId as i32,
                    &Payload::Number(surface_id as i64),
                );
            }
            clients.info.insert(uuid.to_owned(), sender);
        }
        ClientMessage::AddClientForFrames => {
            let sender = MessageSender::new(uuid);
            if shared.surface_id.load(Ordering::SeqCst) != 0 {
                let _ = sender.send(ServerMessage::NewFrame as i32, &Payload::None);
            }
            clients.frames.insert(uuid.to_owned(), sender);
        }
        ClientMessage::RemoveClientForInfo => {
            clients.info.remove(uuid);
        }
        ClientMessage::RemoveClientForFrames => {
            clients.frames.remove(uuid);
        }
    }
}

impl SenderBackend for SyphonServer {
    fn name(&self) -> &str {
        &self.desc.name
    }

    fn id(&self) -> &str {
        &self.desc.uuid
    }

    fn width(&self) -> u32 {
        self.surface.width()
    }

    fn height(&self) -> u32 {
        self.surface.height()
    }

    fn format(&self) -> PixelFormat {
        PixelFormat::Bgra8Unorm
    }

    fn info(&self) -> SenderInfo {
        self.desc
            .to_sender_info(self.surface.width(), self.surface.height())
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        self.ensure_surface(width, height)
    }

    fn send_pixels(&mut self, pixels: PixelBuffer<'_>) -> Result<()> {
        pixels.validate()?;
        if !pixels.format.is_syphon_compatible() {
            return Err(Error::InvalidFormat(pixels.format));
        }
        self.ensure_surface(pixels.width, pixels.height)?;
        self.surface.write_pixels(pixels.data, pixels.stride)?;
        self.publish();
        Ok(())
    }

    fn frame_count(&self) -> u64 {
        self.frame_count
    }

    fn has_receivers(&self) -> Option<bool> {
        Some(self.has_clients())
    }
}

impl Drop for SyphonServer {
    fn drop(&mut self) {
        self.broadcast(ServerMessage::RetireServer, &Payload::None, false);
        if !self.options.private {
            post_server_notification(NOTIFICATION_RETIRE, &self.desc);
        }
    }
}
