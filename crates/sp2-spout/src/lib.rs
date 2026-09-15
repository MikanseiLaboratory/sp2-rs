//! Pure Rust implementation of the Spout2 (2.007) texture sharing protocol.
//!
//! Spout shares Direct3D 11 textures between processes. A sender creates a
//! `D3D11_RESOURCE_MISC_SHARED` texture, registers its name in the
//! `SpoutSenderNames` memory map and publishes the shared handle and geometry
//! in a 280-byte `<name>` map. Receivers open the handle with
//! `ID3D11Device::OpenSharedResource`, synchronise through the
//! `<name>_SpoutAccessMutex` mutex and detect new frames with the
//! `<name>_Count_Semaphore` semaphore.
//!
//! * [`SpoutSender`] / [`SpoutReceiver`] / [`SpoutDirectory`] implement the
//!   `sp2-core` traits and are what the `sp2` umbrella crate wraps on Windows.
//! * [`layout`] contains the wire formats and is available on every platform.
//! * The remaining modules are only compiled on Windows.
//!
//! Interoperability notes:
//!
//! * Legacy (KMT) shared handles are used, because existing receivers open
//!   textures with `OpenSharedResource`. The handle is stored truncated to 32
//!   bits exactly like the reference SDK.
//! * Every write to the shared texture is followed by `Flush()` so that other
//!   devices observe it.
//! * CPU-share senders (`partnerId & 0x80000000`) are detected but not
//!   supported.

#![cfg_attr(not(windows), allow(unused))]
#![warn(missing_docs)]

pub mod layout;

#[cfg(windows)]
pub mod d3d11;
#[cfg(windows)]
mod directory;
#[cfg(windows)]
pub mod frame_count;
#[cfg(windows)]
mod receiver;
#[cfg(windows)]
pub mod registry;
#[cfg(windows)]
mod sender;
#[cfg(windows)]
pub mod sender_names;
#[cfg(windows)]
pub mod shared_memory;

#[cfg(windows)]
pub use directory::SpoutDirectory;
#[cfg(windows)]
pub use receiver::SpoutReceiver;
#[cfg(windows)]
pub use sender::SpoutSender;

pub use sp2_core;
