//! Server discovery through `NSDistributedNotificationCenter`.

use std::collections::HashMap;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2_foundation::{NSDictionary, NSDistributedNotificationCenter, NSNotification, NSString};
use sp2_core::{DirectoryBackend, Result, SenderInfo};

use crate::cf::{description_to_dictionary, dictionary_to_description};
use crate::constants::{
    ANNOUNCE_TIMEOUT_SECS, NOTIFICATION_ANNOUNCE, NOTIFICATION_ANNOUNCE_REQUEST,
    NOTIFICATION_RETIRE, NOTIFICATION_UPDATE,
};
use crate::description::ServerDescription;

/// Post a server notification (`ServerAnnounce`, `ServerUpdate`, `ServerRetire`).
pub(crate) fn post_server_notification(name: &str, desc: &ServerDescription) {
    let center = NSDistributedNotificationCenter::defaultCenter();
    let name = NSString::from_str(name);
    let object = NSString::from_str(&desc.uuid);
    let user_info = description_to_dictionary(desc);
    // SAFETY: all arguments are valid Foundation objects.
    unsafe {
        center.postNotificationName_object_userInfo_deliverImmediately(
            &name,
            Some(&object),
            Some(&user_info),
            true,
        );
    }
}

/// Ask every running server to announce itself.
pub(crate) fn post_announce_request() {
    let center = NSDistributedNotificationCenter::defaultCenter();
    let name = NSString::from_str(NOTIFICATION_ANNOUNCE_REQUEST);
    // SAFETY: all arguments are valid Foundation objects.
    unsafe {
        center.postNotificationName_object_userInfo_deliverImmediately(&name, None, None, true);
    }
}

/// Observer token for a distributed notification; removed on drop.
pub(crate) struct Observer {
    token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

impl Observer {
    /// Observe `name` and call `callback` (on the main thread) for every
    /// notification.
    pub(crate) fn new(
        name: &str,
        callback: impl Fn(&NSNotification) + Send + Sync + 'static,
    ) -> Self {
        let center = NSDistributedNotificationCenter::defaultCenter();
        let name = NSString::from_str(name);
        let block = RcBlock::new(move |notification: NonNull<NSNotification>| {
            // SAFETY: Foundation passes a valid notification for the duration
            // of the block call.
            callback(unsafe { notification.as_ref() });
        });
        // SAFETY: the block is `Send`; a nil queue delivers on the posting
        // thread (the main thread for distributed notifications).
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(Some(&name), None, None, &block)
        };
        Observer { token }
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        let center = NSDistributedNotificationCenter::defaultCenter();
        // SAFETY: `token` was returned by addObserverForName:... on this center.
        unsafe { center.removeObserver(self.token.as_ref()) };
    }
}

// SAFETY: the token is only used to remove the observer, which the
// notification center allows from any thread.
unsafe impl Send for Observer {}
unsafe impl Sync for Observer {}

#[derive(Default)]
struct State {
    servers: Vec<ServerDescription>,
    /// Servers seen since the last announce request.
    responded: HashMap<String, ()>,
    pending_request: Option<Instant>,
}

/// Directory of Syphon servers on this machine.
///
/// Observers are registered on construction and an announce request is sent
/// so that running servers report themselves. Notifications are delivered on
/// the main thread's run loop: call [`crate::run_loop::poll`] regularly from
/// the main thread. Servers that do not answer an announce request within six
/// seconds are dropped, matching the reference implementation.
pub struct SyphonDirectory {
    state: Arc<Mutex<State>>,
    _observers: Vec<Observer>,
}

impl std::fmt::Debug for SyphonDirectory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyphonDirectory").finish_non_exhaustive()
    }
}

impl SyphonDirectory {
    /// Start observing servers and request announcements.
    pub fn new() -> Result<Self> {
        let state = Arc::new(Mutex::new(State::default()));

        let announce = {
            let state = state.clone();
            Observer::new(NOTIFICATION_ANNOUNCE, move |n| handle_announce(&state, n))
        };
        let update = {
            let state = state.clone();
            Observer::new(NOTIFICATION_UPDATE, move |n| handle_announce(&state, n))
        };
        let retire = {
            let state = state.clone();
            Observer::new(NOTIFICATION_RETIRE, move |n| handle_retire(&state, n))
        };

        let directory = SyphonDirectory {
            state,
            _observers: vec![announce, update, retire],
        };
        directory.request_announcements();
        Ok(directory)
    }

    /// Send an announce request and start the timeout after which servers
    /// that did not respond are removed.
    pub fn request_announcements(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.responded.clear();
            state.pending_request = Some(Instant::now());
        }
        post_announce_request();
    }

    /// Current list of known servers (does not pump the run loop).
    pub fn servers(&self) -> Vec<ServerDescription> {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return Vec::new(),
        };
        if let Some(requested) = state.pending_request {
            if requested.elapsed() >= Duration::from_secs_f64(ANNOUNCE_TIMEOUT_SECS) {
                let responded = std::mem::take(&mut state.responded);
                state.servers.retain(|s| responded.contains_key(&s.uuid));
                state.pending_request = None;
            }
        }
        state.servers.clone()
    }

    /// Find a server by UUID or name.
    pub fn find_server(&self, name_or_uuid: &str) -> Option<ServerDescription> {
        self.servers().into_iter().find(|s| s.matches(name_or_uuid))
    }
}

fn handle_announce(state: &Arc<Mutex<State>>, notification: &NSNotification) {
    let Some(user_info) = notification.userInfo() else {
        return;
    };
    let user_info: &NSDictionary = &user_info;
    let Some(desc) = dictionary_to_description(user_info) else {
        return;
    };
    if let Ok(mut state) = state.lock() {
        state.responded.insert(desc.uuid.clone(), ());
        match state.servers.iter_mut().find(|s| s.uuid == desc.uuid) {
            Some(existing) => *existing = desc,
            None => {
                log::debug!(
                    "Syphon server appeared: {:?} ({})",
                    desc.name,
                    desc.app_name
                );
                state.servers.push(desc);
            }
        }
    }
}

fn handle_retire(state: &Arc<Mutex<State>>, notification: &NSNotification) {
    let uuid = notification
        .userInfo()
        .and_then(|info| {
            let info: &NSDictionary = &info;
            dictionary_to_description(info)
        })
        .map(|d| d.uuid)
        .or_else(|| {
            notification
                .object()
                .and_then(|o| o.downcast_ref::<NSString>().map(|s| s.to_string()))
        });
    let Some(uuid) = uuid else {
        return;
    };
    if let Ok(mut state) = state.lock() {
        let before = state.servers.len();
        state.servers.retain(|s| s.uuid != uuid);
        state.responded.remove(&uuid);
        if state.servers.len() != before {
            log::debug!("Syphon server retired: {uuid}");
        }
    }
}

impl DirectoryBackend for SyphonDirectory {
    fn senders(&mut self) -> Result<Vec<SenderInfo>> {
        Ok(SyphonDirectory::servers(self)
            .iter()
            .filter(|s| s.supports_iosurface())
            .map(|s| s.to_sender_info(0, 0))
            .collect())
    }

    fn find(&mut self, name_or_id: &str) -> Result<Option<SenderInfo>> {
        Ok(self.find_server(name_or_id).map(|s| s.to_sender_info(0, 0)))
    }

    fn active_sender(&mut self) -> Result<Option<SenderInfo>> {
        Ok(self.senders()?.into_iter().next())
    }

    fn set_active_sender(&mut self, _id: &str) -> Result<()> {
        // Syphon has no notion of an active server.
        Ok(())
    }
}
