//! Pure Rust implementation of the Syphon frame sharing protocol.
//!
//! Syphon shares IOSurfaces between processes on macOS. A server creates a
//! global BGRA8 IOSurface, announces itself with distributed notifications
//! (`info.v002.Syphon.ServerAnnounce` and friends) and talks to clients over
//! `CFMessagePort`s named after UUIDs. Clients look the surface up by its
//! `IOSurfaceID` and read it directly (CPU or Metal).
//!
//! * [`SyphonServer`] / [`SyphonClient`] / [`SyphonReceiver`] /
//!   [`SyphonDirectory`] implement the protocol and the `sp2-core` traits.
//! * [`constants`] and [`description`] hold the wire level names and are
//!   available on every platform.
//! * The remaining modules are only compiled on macOS.
//!
//! # Run loop requirement
//!
//! Distributed notifications are delivered on the **main thread's run loop**.
//! Applications without a Cocoa event loop must call [`run_loop::poll`] (or
//! `sp2::pump_events`) from the main thread regularly, otherwise servers are
//! never discovered and servers never answer announce requests. Message ports
//! are serviced on a dedicated thread and do not depend on this.

#![cfg_attr(not(target_os = "macos"), allow(unused))]
#![warn(missing_docs)]

pub mod constants;
pub mod description;

#[cfg(target_os = "macos")]
mod cf;
#[cfg(target_os = "macos")]
mod client;
#[cfg(target_os = "macos")]
mod directory;
#[cfg(target_os = "macos")]
pub mod iosurface;
#[cfg(target_os = "macos")]
pub mod messaging;
#[cfg(target_os = "macos")]
mod receiver;
#[cfg(target_os = "macos")]
pub mod run_loop;
#[cfg(target_os = "macos")]
mod server;

#[cfg(target_os = "macos")]
pub use cf::Payload;
#[cfg(target_os = "macos")]
pub use client::SyphonClient;
#[cfg(target_os = "macos")]
pub use directory::SyphonDirectory;
#[cfg(target_os = "macos")]
pub use receiver::SyphonReceiver;
#[cfg(target_os = "macos")]
pub use server::{ServerOptions, SyphonServer};

pub use description::ServerDescription;
pub use sp2_core;
