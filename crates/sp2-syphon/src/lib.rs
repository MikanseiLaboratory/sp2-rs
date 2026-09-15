//! Pure Rust implementation of the Syphon frame sharing protocol.
//!
//! The crate only has content on macOS. On other platforms it is empty so
//! that workspace-wide builds succeed.

#![cfg_attr(not(target_os = "macos"), allow(unused))]
