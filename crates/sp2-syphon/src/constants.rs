//! Protocol constants shared with the reference Syphon framework.
//!
//! The string values are part of the wire protocol: they are the distributed
//! notification names, dictionary keys and `CFMessagePort` message ids that
//! existing Syphon servers and clients exchange.

/// Prefix of every Syphon identifier (`info.v002.Syphon`).
pub const IDENTIFIER: &str = "info.v002.Syphon";

/// Distributed notification: a directory asks every server to announce itself.
pub const NOTIFICATION_ANNOUNCE_REQUEST: &str = "info.v002.Syphon.ServerAnnounceRequest";
/// Distributed notification: a server announces itself (`userInfo` = description).
pub const NOTIFICATION_ANNOUNCE: &str = "info.v002.Syphon.ServerAnnounce";
/// Distributed notification: a server changed its description (e.g. name).
pub const NOTIFICATION_UPDATE: &str = "info.v002.Syphon.ServerUpdate";
/// Distributed notification: a server stopped.
pub const NOTIFICATION_RETIRE: &str = "info.v002.Syphon.ServerRetire";

/// Description key: dictionary format version (`u32`).
pub const KEY_DICTIONARY_VERSION: &str = "SyphonServerDescriptionDictionaryVersionKey";
/// Description key: server UUID string (`info.v002.Syphon.<UUID>`).
pub const KEY_UUID: &str = "SyphonServerDescriptionUUIDKey";
/// Description key: user visible server name.
pub const KEY_NAME: &str = "SyphonServerDescriptionNameKey";
/// Description key: name of the hosting application.
pub const KEY_APP_NAME: &str = "SyphonServerDescriptionAppNameKey";
/// Description key: array of surface descriptions.
pub const KEY_SURFACES: &str = "SyphonServerDescriptionSurfacesKey";
/// Description key: application icon (added by directories, never sent).
pub const KEY_ICON: &str = "SyphonServerDescriptionIconKey";

/// Surface description key.
pub const KEY_SURFACE_TYPE: &str = "SyphonSurfaceType";
/// Surface description value for IOSurface backed servers.
pub const SURFACE_TYPE_IOSURFACE: &str = "SyphonSurfaceTypeIOSurface";

/// Current description dictionary version.
pub const DICTIONARY_VERSION: u32 = 0;

/// Seconds a directory waits for announce responses before dropping servers
/// that did not answer.
pub const ANNOUNCE_TIMEOUT_SECS: f64 = 6.0;

/// Send timeout used for `CFMessagePortSendRequest`, in seconds.
pub const MESSAGE_SEND_TIMEOUT_SECS: f64 = 60.0;

/// Messages a client sends to a server (`CFMessagePort` msgid).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ClientMessage {
    /// Payload: client UUID. Receive `UpdateSurfaceID` / `RetireServer`.
    AddClientForInfo = 0,
    /// Payload: client UUID. Receive `NewFrame`.
    AddClientForFrames = 1,
    /// Payload: client UUID.
    RemoveClientForInfo = 2,
    /// Payload: client UUID.
    RemoveClientForFrames = 3,
}

impl ClientMessage {
    /// Decode a message id.
    pub fn from_id(id: i32) -> Option<Self> {
        Some(match id {
            0 => ClientMessage::AddClientForInfo,
            1 => ClientMessage::AddClientForFrames,
            2 => ClientMessage::RemoveClientForInfo,
            3 => ClientMessage::RemoveClientForFrames,
            _ => return None,
        })
    }
}

/// Messages a server sends to a client (`CFMessagePort` msgid).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ServerMessage {
    /// Payload: new name. Ignored by current clients.
    UpdateServerName = 0,
    /// No payload. A new frame is available in the surface.
    NewFrame = 1,
    /// Payload: `IOSurfaceID` (`u32`). The surface was (re)created.
    UpdateSurfaceId = 2,
    /// No payload. The server is going away.
    RetireServer = 3,
}

impl ServerMessage {
    /// Decode a message id.
    pub fn from_id(id: i32) -> Option<Self> {
        Some(match id {
            0 => ServerMessage::UpdateServerName,
            1 => ServerMessage::NewFrame,
            2 => ServerMessage::UpdateSurfaceId,
            3 => ServerMessage::RetireServer,
            _ => return None,
        })
    }
}

/// Build a Syphon UUID string from a bare UUID (`XXXXXXXX-XXXX-...`).
pub fn uuid_string(bare_uuid: &str) -> String {
    format!("{IDENTIFIER}.{bare_uuid}")
}

/// Whether `s` has the shape of a Syphon UUID string.
pub fn is_uuid_string(s: &str) -> bool {
    match s
        .strip_prefix(IDENTIFIER)
        .and_then(|rest| rest.strip_prefix('.'))
    {
        Some(uuid) => {
            uuid.len() == 36
                && uuid.bytes().enumerate().all(|(i, b)| {
                    if matches!(i, 8 | 13 | 18 | 23) {
                        b == b'-'
                    } else {
                        b.is_ascii_hexdigit()
                    }
                })
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_ids_round_trip() {
        for id in 0..4 {
            assert_eq!(ClientMessage::from_id(id).unwrap() as i32, id);
            assert_eq!(ServerMessage::from_id(id).unwrap() as i32, id);
        }
        assert!(ClientMessage::from_id(4).is_none());
        assert!(ServerMessage::from_id(-1).is_none());
    }

    #[test]
    fn uuid_strings() {
        let uuid = uuid_string("6B29FC40-CA47-1067-B31D-00DD010662DA");
        assert_eq!(
            uuid,
            "info.v002.Syphon.6B29FC40-CA47-1067-B31D-00DD010662DA"
        );
        assert!(is_uuid_string(&uuid));
        assert!(!is_uuid_string("info.v002.Syphon.ServerAnnounce"));
        assert!(!is_uuid_string("6B29FC40-CA47-1067-B31D-00DD010662DA"));
        assert!(!is_uuid_string(
            "info.v002.Syphon.6B29FC40-CA47-1067-B31D-00DD010662DG"
        ));
    }
}
