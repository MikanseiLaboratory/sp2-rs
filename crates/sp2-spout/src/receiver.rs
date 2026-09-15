//! Spout receiver: opens a sender's shared texture and follows its updates.

use sp2_core::{Error, FrameInfo, PixelFormat, ReceiverBackend, Result, SenderInfo};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;

use crate::d3d11::{texture_desc, Device};
use crate::frame_count::{AccessMutex, FrameCounter};
use crate::layout::SharedTextureInfo;
use crate::sender_names::SenderNames;

struct Connection {
    info: SenderInfo,
    raw: SharedTextureInfo,
    texture: ID3D11Texture2D,
    staging: Option<ID3D11Texture2D>,
    frame: FrameCounter,
    access: AccessMutex,
}

/// A Spout 2.007 compatible receiver.
///
/// The receiver is polled: [`SpoutReceiver::update`] (called by every receive
/// method) re-reads the sender information map, connects to the sender if
/// necessary and re-opens the shared texture when its handle or size changed.
pub struct SpoutReceiver {
    device: Device,
    names: SenderNames,
    target: Option<String>,
    connection: Option<Connection>,
    updated: bool,
    frame_new: bool,
    failed_handle: Option<u32>,
}

impl std::fmt::Debug for SpoutReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpoutReceiver")
            .field("target", &self.target)
            .field("connected", &self.connection.as_ref().map(|c| &c.info))
            .finish_non_exhaustive()
    }
}

impl SpoutReceiver {
    /// Create a receiver on a new D3D11 device following `target`
    /// (`None` = the active sender).
    pub fn new(target: Option<&str>) -> Result<Self> {
        Self::with_device(Device::new()?, target)
    }

    /// Create a receiver whose textures live on `device`.
    pub fn with_device(device: Device, target: Option<&str>) -> Result<Self> {
        Ok(SpoutReceiver {
            device,
            names: SenderNames::new()?,
            target: target.map(str::to_owned),
            connection: None,
            updated: false,
            frame_new: false,
            failed_handle: None,
        })
    }

    /// Change the sender to follow (`None` = active sender).
    pub fn set_target(&mut self, target: Option<&str>) {
        let target = target.map(str::to_owned);
        if target != self.target {
            self.target = target;
            self.disconnect();
        }
    }

    /// The device the shared texture is opened on.
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// The sender's shared texture, if connected.
    ///
    /// Reads from the texture should happen while holding the access mutex
    /// (see [`SpoutReceiver::receive_texture`]).
    pub fn shared_texture(&self) -> Option<&ID3D11Texture2D> {
        self.connection.as_ref().map(|c| &c.texture)
    }

    /// The legacy shared handle currently opened, if connected.
    pub fn share_handle(&self) -> Option<HANDLE> {
        self.connection
            .as_ref()
            .map(|c| HANDLE(c.raw.share_handle_ptr() as *mut _))
    }

    /// Whether the last [`SpoutReceiver::update`] connected to a sender or
    /// changed texture (size / handle). Applications recreate their receiving
    /// resources when this is set.
    pub fn is_updated(&self) -> bool {
        self.updated
    }

    /// Estimated frame rate of the sender based on received frames.
    pub fn frame_rate(&self) -> f64 {
        self.connection
            .as_ref()
            .map(|c| c.frame.fps())
            .unwrap_or(0.0)
    }

    /// Re-read the sender information and (re)connect as needed.
    ///
    /// Returns `true` when a sender is connected after the call.
    pub fn update(&mut self) -> Result<bool> {
        self.updated = false;

        let Some(name) = self.resolve_name()? else {
            self.disconnect();
            return Ok(false);
        };
        let Some(raw) = SenderNames::info(&name)? else {
            if self.connection.is_some() {
                log::info!("Spout sender {name:?} closed");
            }
            self.disconnect();
            return Ok(false);
        };
        if !raw.is_valid() {
            // Sender exists but has not published a texture yet.
            return Ok(false);
        }
        if raw.is_cpu_share() {
            // CPU share senders publish pixels in a memory map instead of a
            // texture; not supported by this receiver.
            if self.failed_handle != Some(raw.share_handle) {
                log::warn!("Spout sender {name:?} uses CPU sharing, which is not supported");
                self.failed_handle = Some(raw.share_handle);
            }
            self.disconnect();
            return Ok(false);
        }

        if let Some(conn) = &self.connection {
            let same = conn.info.name == name
                && conn.raw.share_handle == raw.share_handle
                && conn.raw.width == raw.width
                && conn.raw.height == raw.height
                && conn.raw.format == raw.format;
            if same {
                return Ok(true);
            }
        }

        if self.failed_handle == Some(raw.share_handle) {
            return Ok(false);
        }

        let handle = HANDLE(raw.share_handle_ptr() as *mut _);
        let texture = match self.device.open_shared_texture(handle) {
            Ok(texture) => texture,
            Err(e) => {
                log::warn!("failed to open shared texture of {name:?}: {e}");
                self.failed_handle = Some(raw.share_handle);
                self.disconnect();
                return Ok(false);
            }
        };

        // Prefer the actual texture description over the info map; some
        // senders write 0 as the format.
        let desc = texture_desc(&texture);
        let format = PixelFormat::from_dxgi_format(desc.Format.0 as u32)
            .unwrap_or_else(|| raw.pixel_format());

        let (frame, access) = match self.connection.take() {
            Some(conn) if conn.info.name == name => (conn.frame, conn.access),
            _ => (FrameCounter::new(&name)?, AccessMutex::new(&name)?),
        };
        let mut frame = frame;
        frame.reset();

        let info = SenderInfo {
            id: name.clone(),
            name: name.clone(),
            app_name: (!raw.description.is_empty()).then(|| raw.description.clone()),
            width: desc.Width,
            height: desc.Height,
            format,
        };
        log::info!(
            "connected to Spout sender {:?} {}x{} {:?}",
            name,
            info.width,
            info.height,
            format
        );
        self.connection = Some(Connection {
            info,
            raw,
            texture,
            staging: None,
            frame,
            access,
        });
        self.updated = true;
        self.failed_handle = None;
        Ok(true)
    }

    /// Copy the latest frame into `dst` (a texture on the receiver device with
    /// the sender's size and format) when it is new.
    pub fn receive_texture(&mut self, dst: &ID3D11Texture2D) -> Result<Option<FrameInfo>> {
        if !self.update()? {
            self.frame_new = false;
            return Ok(None);
        }
        let conn = self.connection.as_mut().expect("connected");
        let desc = texture_desc(dst);
        if desc.Width != conn.info.width
            || desc.Height != conn.info.height
            || desc.Format.0 as u32 != conn.info.format.dxgi_format()
        {
            return Err(Error::InvalidArgument(
                "destination texture does not match the sender size / format".into(),
            ));
        }
        let guard = match conn.access.lock() {
            Ok(guard) => guard,
            Err(Error::Timeout) => {
                self.frame_new = false;
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        if !conn.frame.get_new_frame() {
            drop(guard);
            self.frame_new = false;
            return Ok(None);
        }
        self.device.copy_texture(dst, &conn.texture);
        self.device.flush();
        drop(guard);
        self.frame_new = true;
        Ok(Some(frame_info(conn)))
    }
}

fn frame_info(conn: &Connection) -> FrameInfo {
    FrameInfo {
        width: conn.info.width,
        height: conn.info.height,
        format: conn.info.format,
        frame_id: if conn.frame.is_enabled() {
            conn.frame.sender_frame()
        } else {
            conn.frame.frame_count()
        },
    }
}

impl SpoutReceiver {
    fn resolve_name(&mut self) -> Result<Option<String>> {
        if let Some(target) = &self.target {
            return Ok(Some(target.clone()));
        }
        if let Some(active) = self.names.active_sender()? {
            if SenderNames::info(&active)?.is_some() {
                return Ok(Some(active));
            }
        }
        self.names.clean()?;
        let names = self.names.names()?;
        if let Some(first) = names.into_iter().next() {
            self.names.set_active_sender(&first)?;
            return Ok(Some(first));
        }
        Ok(None)
    }
}

impl ReceiverBackend for SpoutReceiver {
    fn sender_info(&self) -> Option<&SenderInfo> {
        self.connection.as_ref().map(|c| &c.info)
    }

    fn receive_pixels(&mut self, out: &mut Vec<u8>) -> Result<Option<FrameInfo>> {
        if !self.update()? {
            self.frame_new = false;
            return Ok(None);
        }
        let device = self.device.clone();
        let conn = self.connection.as_mut().expect("connected");
        let (width, height, format) = (conn.info.width, conn.info.height, conn.info.format);
        if conn.staging.is_none() {
            conn.staging = Some(device.create_staging_texture(width, height, format)?);
        }
        let staging = conn.staging.as_ref().expect("created above");

        let guard = match conn.access.lock() {
            Ok(guard) => guard,
            Err(Error::Timeout) => {
                self.frame_new = false;
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        if !conn.frame.get_new_frame() {
            drop(guard);
            self.frame_new = false;
            return Ok(None);
        }
        device.copy_texture(staging, &conn.texture);
        drop(guard);

        device.read_staging(staging, width, height, format, out)?;
        self.frame_new = true;
        Ok(Some(frame_info(conn)))
    }

    fn is_frame_new(&self) -> bool {
        self.frame_new
    }

    fn frame_id(&self) -> u64 {
        self.connection
            .as_ref()
            .map(frame_info)
            .map(|f| f.frame_id)
            .unwrap_or(0)
    }

    fn disconnect(&mut self) {
        self.connection = None;
        self.frame_new = false;
    }
}
