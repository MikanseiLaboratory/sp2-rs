use crate::{Error, PixelFormat, Result};

/// Options used to create a [`crate::Sender`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SenderOptions {
    /// Sender name shown to receivers. Spout uniquifies duplicates
    /// (`name_1`, `name_2`, ...); Syphon allows duplicates and distinguishes
    /// servers by UUID.
    pub name: String,
    /// Initial texture width.
    pub width: u32,
    /// Initial texture height.
    pub height: u32,
    /// Pixel format of the shared texture.
    pub format: PixelFormat,
    /// Do not announce the sender in the system directory. Only honoured by
    /// Syphon (`SyphonServerOptionIsPrivate`); ignored by Spout.
    pub private: bool,
    /// Register as the active Spout sender when created. Ignored by Syphon.
    pub set_active: bool,
}

impl SenderOptions {
    /// Options with the default BGRA8 format.
    pub fn new(name: impl Into<String>, width: u32, height: u32) -> Self {
        SenderOptions {
            name: name.into(),
            width,
            height,
            format: PixelFormat::Bgra8Unorm,
            private: false,
            set_active: false,
        }
    }

    /// Set the pixel format.
    pub fn format(mut self, format: PixelFormat) -> Self {
        self.format = format;
        self
    }

    /// Hide the sender from the system directory (Syphon only).
    pub fn private(mut self, private: bool) -> Self {
        self.private = private;
        self
    }

    /// Make this the active sender on creation (Spout only).
    pub fn set_active(mut self, set_active: bool) -> Self {
        self.set_active = set_active;
        self
    }

    /// Validate the options.
    pub fn validate(&self) -> Result<()> {
        if self.name.is_empty() {
            return Err(Error::InvalidArgument(
                "sender name must not be empty".into(),
            ));
        }
        if self.name.len() >= 256 {
            return Err(Error::InvalidArgument(
                "sender name must be shorter than 256 bytes".into(),
            ));
        }
        if self.width == 0 || self.height == 0 {
            return Err(Error::InvalidArgument(
                "sender size must not be zero".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation() {
        assert!(SenderOptions::new("ok", 1, 1).validate().is_ok());
        assert!(SenderOptions::new("", 1, 1).validate().is_err());
        assert!(SenderOptions::new("x", 0, 1).validate().is_err());
        assert!(SenderOptions::new("a".repeat(256), 1, 1)
            .validate()
            .is_err());
    }
}
