//! `CFMessagePort` based messaging between servers and clients.
//!
//! Every server and every client owns a local port named after its UUID. The
//! local port is serviced by a dedicated thread running a `CFRunLoop`, so
//! messages are delivered regardless of what the application's main thread is
//! doing. Payloads are `NSKeyedArchiver` archives of an `NSString` or
//! `NSNumber` (secure coding), exactly like the reference framework.

use std::ffi::c_void;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use objc2_core_foundation::{
    kCFRunLoopDefaultMode, CFData, CFMessagePort, CFMessagePortContext, CFRetained, CFRunLoop,
    CFString,
};
use sp2_core::{Error, Result};

use crate::cf::{archive, cf_string, unarchive, Payload, SendCf};
use crate::constants::MESSAGE_SEND_TIMEOUT_SECS;

/// A message received on a local port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// `CFMessagePort` message id (see [`crate::constants`]).
    pub id: i32,
    /// Decoded payload.
    pub payload: Payload,
}

/// Handler invoked on the messaging thread for every incoming message.
pub type Handler = Arc<dyn Fn(Message) + Send + Sync + 'static>;

struct PortState {
    handler: Handler,
}

unsafe extern "C-unwind" fn port_callback(
    _port: *mut CFMessagePort,
    msgid: i32,
    data: *const CFData,
    info: *mut c_void,
) -> *const CFData {
    if info.is_null() {
        return std::ptr::null();
    }
    // SAFETY: `info` points to the `PortState` owned by the `MessageReceiver`,
    // which outlives the port (the port is invalidated and the thread joined
    // before the state is dropped).
    let state = unsafe { &*(info as *const PortState) };
    let payload = if data.is_null() {
        Payload::None
    } else {
        // SAFETY: `data` is valid for the duration of the callback.
        let bytes = unsafe { &*data }.to_vec();
        unarchive(&bytes)
    };
    (state.handler)(Message { id: msgid, payload });
    std::ptr::null()
}

/// A local `CFMessagePort` receiving messages for a UUID.
pub struct MessageReceiver {
    name: String,
    port: SendCf<CFMessagePort>,
    run_loop: SendCf<CFRunLoop>,
    thread: Option<JoinHandle<()>>,
    // Boxed so that the address handed to Core Foundation stays stable.
    _state: Box<PortState>,
}

impl std::fmt::Debug for MessageReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MessageReceiver")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl MessageReceiver {
    /// Create the local port `name` and start servicing it.
    pub fn new(name: &str, handler: Handler) -> Result<Self> {
        let state = Box::new(PortState { handler });
        struct SendPtr(*mut c_void);
        // SAFETY: the pointer is only dereferenced by the port callback while
        // the owning `MessageReceiver` (and thus the `PortState`) is alive.
        unsafe impl Send for SendPtr {}
        let info = SendPtr(&*state as *const PortState as *mut c_void);
        let port_name = name.to_owned();

        let (tx, rx) = mpsc::channel::<Result<(SendCf<CFMessagePort>, SendCf<CFRunLoop>)>>();
        let thread = std::thread::Builder::new()
            .name(format!("sp2-syphon messaging {name}"))
            .spawn(move || {
                let info = info;
                let created = create_local_port(&port_name, info.0);
                let port = match created {
                    Ok(port) => port,
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        return;
                    }
                };
                let Some(run_loop) = CFRunLoop::current() else {
                    let _ = tx.send(Err(Error::backend("CFRunLoopGetCurrent failed")));
                    return;
                };
                let Some(source) = CFMessagePort::new_run_loop_source(None, Some(&port), 0) else {
                    let _ = tx.send(Err(Error::backend(
                        "CFMessagePortCreateRunLoopSource failed",
                    )));
                    return;
                };
                // SAFETY: reading a Core Foundation constant.
                let mode = unsafe { kCFRunLoopDefaultMode };
                run_loop.add_source(Some(&source), mode);
                let _ = tx.send(Ok((SendCf(port.clone()), SendCf(run_loop.clone()))));
                CFRunLoop::run();
                source.invalidate();
            })
            .map_err(|e| Error::backend(format!("failed to spawn messaging thread: {e}")))?;

        let (port, run_loop) = match rx.recv() {
            Ok(Ok(handles)) => handles,
            Ok(Err(e)) => {
                let _ = thread.join();
                return Err(e);
            }
            Err(_) => {
                let _ = thread.join();
                return Err(Error::backend("messaging thread exited unexpectedly"));
            }
        };

        Ok(MessageReceiver {
            name: name.to_owned(),
            port,
            run_loop,
            thread: Some(thread),
            _state: state,
        })
    }

    /// Port name (the owner's UUID string).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether the port is still valid.
    pub fn is_valid(&self) -> bool {
        self.port.0.is_valid()
    }
}

fn create_local_port(name: &str, info: *mut c_void) -> Result<CFRetained<CFMessagePort>> {
    let mut context = CFMessagePortContext {
        version: 0,
        info,
        retain: None,
        release: None,
        copyDescription: None,
    };
    let mut should_free_info: u8 = 0;
    let cf_name = cf_string(name);
    // SAFETY: `port_callback` matches the callback ABI, `context` and
    // `should_free_info` are valid for the call and `info` outlives the port.
    unsafe {
        CFMessagePort::new_local(
            None,
            Some(&cf_name),
            Some(port_callback),
            &mut context,
            &mut should_free_info,
        )
    }
    .ok_or_else(|| {
        Error::backend(format!(
            "CFMessagePortCreateLocal({name}) failed; name in use?"
        ))
    })
}

impl Drop for MessageReceiver {
    fn drop(&mut self) {
        self.port.0.invalidate();
        self.run_loop.0.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A remote `CFMessagePort` used to send messages to a UUID.
pub struct MessageSender {
    name: String,
    port: Mutex<Option<SendCf<CFMessagePort>>>,
}

impl std::fmt::Debug for MessageSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MessageSender")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl MessageSender {
    /// Create a sender for the remote port `name`. The connection is
    /// established lazily on first send.
    pub fn new(name: &str) -> Self {
        MessageSender {
            name: name.to_owned(),
            port: Mutex::new(None),
        }
    }

    /// Remote port name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether the remote port can currently be reached.
    pub fn is_reachable(&self) -> bool {
        self.connect().is_ok()
    }

    fn connect(&self) -> Result<CFRetained<CFMessagePort>> {
        let mut guard = self
            .port
            .lock()
            .map_err(|_| Error::backend("poisoned lock"))?;
        if let Some(port) = guard.as_ref() {
            if port.0.is_valid() {
                return Ok(port.0.clone());
            }
            *guard = None;
        }
        let name: CFRetained<CFString> = cf_string(&self.name);
        let port = CFMessagePort::new_remote(None, Some(&name))
            .ok_or_else(|| Error::NotFound(self.name.clone()))?;
        *guard = Some(SendCf(port.clone()));
        Ok(port)
    }

    /// Send a message without waiting for a reply.
    pub fn send(&self, id: i32, payload: &Payload) -> Result<()> {
        let port = self.connect()?;
        let bytes = archive(payload)?;
        let data = bytes.as_deref().map(CFData::from_bytes);
        // SAFETY: `port` is a valid remote port; no reply is requested so the
        // return data pointer may be null.
        let status = unsafe {
            port.send_request(
                id,
                data.as_deref(),
                MESSAGE_SEND_TIMEOUT_SECS,
                0.0,
                None,
                std::ptr::null_mut(),
            )
        };
        if status != 0 {
            if let Ok(mut guard) = self.port.lock() {
                *guard = None;
            }
            return Err(Error::Os {
                code: status as i64,
                message: format!("CFMessagePortSendRequest({}) failed", self.name),
            });
        }
        Ok(())
    }
}
