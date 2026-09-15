//! Run loop pumping.
//!
//! Distributed notifications (server discovery) are delivered to observers on
//! the **main thread's** run loop. Applications that do not run a Cocoa event
//! loop must call [`poll`] from the main thread regularly, typically once per
//! frame, otherwise [`crate::SyphonDirectory`] never sees servers and
//! [`crate::SyphonServer`] never answers announce requests.

use std::time::Duration;

use objc2_core_foundation::{kCFRunLoopDefaultMode, CFRunLoop};

/// Run the current thread's run loop for at most `timeout`, processing
/// pending notifications and sources.
pub fn poll(timeout: Duration) {
    // SAFETY: reading a Core Foundation constant.
    let mode = unsafe { kCFRunLoopDefaultMode };
    CFRunLoop::run_in_mode(mode, timeout.as_secs_f64(), false);
}
