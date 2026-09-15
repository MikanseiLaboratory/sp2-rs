//! Compile-time selection of the platform backend.
//!
//! Each backend module exposes the same surface:
//!
//! * `Sender::new(&SenderOptions) -> Result<Sender>` implementing `SenderBackend`
//! * `Receiver::new(Option<&str>) -> Result<Receiver>` with `set_target`, implementing `ReceiverBackend`
//! * `Directory::new() -> Result<Directory>` implementing `DirectoryBackend`
//! * `pump_events(Duration)`
//! * `BACKEND: &str`
//!
//! The selected module is re-exported as [`imp`] and its types are reachable
//! through `Sender::platform()` and friends.

pub mod unsupported;

/// The backend compiled for this target.
pub use unsupported as imp;
