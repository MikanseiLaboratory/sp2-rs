//! Platform-independent building blocks shared by every `sp2-rs` crate.
//!
//! This crate defines:
//!
//! * [`PixelFormat`] with conversions to the native format identifiers used by
//!   Spout2 (`DXGI_FORMAT`) and Syphon (IOSurface / Metal pixel formats).
//! * [`SenderInfo`] / [`FrameInfo`], the descriptions exchanged between senders
//!   and receivers.
//! * The [`SenderBackend`], [`ReceiverBackend`] and [`DirectoryBackend`] traits
//!   implemented by the Windows (`sp2-spout`) and macOS (`sp2-syphon`)
//!   backends, and re-exported through the `sp2` umbrella crate.
//! * [`Error`], the common error type.
//!
//! The crate has no OS dependencies, so it compiles and is unit tested on every
//! platform.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod error;
mod format;
mod info;
mod traits;

pub use error::{Error, Result};
pub use format::PixelFormat;
pub use info::{copy_rows, FrameInfo, PixelBuffer, SenderInfo};
pub use traits::{DirectoryBackend, ReceiverBackend, SenderBackend, TransferPath};
