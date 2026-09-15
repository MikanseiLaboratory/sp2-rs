//! Server description as exchanged through distributed notifications.

use sp2_core::{PixelFormat, SenderInfo};

use crate::constants::{DICTIONARY_VERSION, SURFACE_TYPE_IOSURFACE};

/// Description of a Syphon server.
///
/// This is the platform independent view of the
/// `SyphonServerDescription*` dictionary. Servers publish it in the
/// `ServerAnnounce` / `ServerUpdate` / `ServerRetire` notifications and
/// directories collect them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerDescription {
    /// Dictionary version (`SyphonServerDescriptionDictionaryVersionKey`).
    pub version: u32,
    /// `info.v002.Syphon.<UUID>`; also the server's `CFMessagePort` name.
    pub uuid: String,
    /// User visible server name (may be empty).
    pub name: String,
    /// Hosting application name.
    pub app_name: String,
    /// Surface types offered by the server (`SyphonSurfaceType` values).
    pub surfaces: Vec<String>,
}

impl ServerDescription {
    /// Description for a new IOSurface server.
    pub fn new(
        uuid: impl Into<String>,
        name: impl Into<String>,
        app_name: impl Into<String>,
    ) -> Self {
        ServerDescription {
            version: DICTIONARY_VERSION,
            uuid: uuid.into(),
            name: name.into(),
            app_name: app_name.into(),
            surfaces: vec![SURFACE_TYPE_IOSURFACE.to_owned()],
        }
    }

    /// Whether the server shares through IOSurface (the only supported type).
    pub fn supports_iosurface(&self) -> bool {
        self.surfaces.iter().any(|s| s == SURFACE_TYPE_IOSURFACE)
    }

    /// Whether `needle` matches this server's UUID or name.
    pub fn matches(&self, needle: &str) -> bool {
        self.uuid == needle || self.name == needle
    }

    /// Convert to the unified [`SenderInfo`] with the given geometry.
    ///
    /// Syphon descriptions do not carry the frame size; it is only known once
    /// a client connected and looked up the surface, so `width` / `height`
    /// are passed in (0 when unknown).
    pub fn to_sender_info(&self, width: u32, height: u32) -> SenderInfo {
        SenderInfo {
            id: self.uuid.clone(),
            name: if self.name.is_empty() {
                self.app_name.clone()
            } else {
                self.name.clone()
            },
            app_name: (!self.app_name.is_empty()).then(|| self.app_name.clone()),
            width,
            height,
            format: PixelFormat::Bgra8Unorm,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_defaults() {
        let d = ServerDescription::new("info.v002.Syphon.X", "Main", "App");
        assert_eq!(d.version, DICTIONARY_VERSION);
        assert!(d.supports_iosurface());
        assert!(d.matches("Main"));
        assert!(d.matches("info.v002.Syphon.X"));
        assert!(!d.matches("Other"));
    }

    #[test]
    fn sender_info_falls_back_to_app_name() {
        let d = ServerDescription::new("u", "", "Simple Server");
        let info = d.to_sender_info(0, 0);
        assert_eq!(info.name, "Simple Server");
        assert_eq!(info.app_name.as_deref(), Some("Simple Server"));
        assert_eq!(info.format, PixelFormat::Bgra8Unorm);

        let d = ServerDescription {
            surfaces: vec!["Other".into()],
            ..ServerDescription::new("u", "n", "a")
        };
        assert!(!d.supports_iosurface());
    }
}
