//! View subscriptions (PoC): which views the clients connected to this node watch, what
//! each one read, and which have changed since their last notice.
//!
//! A watch belongs to a watch stream (`GET /apps/{app}/events?watch=true`), held by the
//! node that serves that stream: a replica of the app, since every replica applies every
//! entry's diff ([`crate::app::Changed`]). The stream's task (`api::watches`) marks the
//! watches a change touches, and once per [`NOTICE_INTERVAL`] sends one `view-changed`
//! notice `{ sub, index }` per marked watch, then runs the view again as its caller to
//! learn what it reads now. Nothing here holds a view's result, a row or an argument
//! beyond the watch's own `args`, which never leave the node.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use calimero_app_host::ReadSet;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, Notify};

use crate::app::Changed;
use crate::types::Caller;

/// Watches one stream holds at most.
pub const MAX_WATCHES_PER_STREAM: usize = 64;
/// Watches one node holds at most, over every stream.
pub const MAX_WATCHES_PER_NODE: usize = 2_048;
/// Bytes of one watch's view arguments (they are kept for as long as the watch).
pub const MAX_WATCH_ARGS_BYTES: usize = 4 * 1024;
/// Notices one watch gets at most: one per interval, naming the newest change.
pub const NOTICE_INTERVAL: Duration = Duration::from_secs(1);
/// How long a stream's watches outlive its connection, for the client to resume it.
pub const RESUME_GRACE: Duration = Duration::from_secs(30);
/// Frames a stream's connection buffers before notices wait for the next interval.
pub const STREAM_BUFFER: usize = 256;

/// Who opened a stream, and what makes their credential still good: re-checked before
/// every notice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Holder {
    pub caller: Caller,
    /// The session (revocation), when signed in with one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The passkey the session was opened with (removing it ends the session).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passkey: Option<String>,
}

/// What a stream sends besides its first frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    /// `event: view-changed`, `id: <index>:<seq>`, `data: {"sub", "index"}`.
    Changed { sub: String, index: u64, seq: u64 },
    /// `event: watch-ended`, `data: {"sub"}`: the watch is gone (access lost, or its
    /// view no longer reads a bounded set).
    Ended { sub: String },
}

/// One watch: a view, its arguments and what it read last.
#[derive(Clone, Debug)]
pub struct Watch {
    pub view: String,
    pub args: Value,
    read_set: ReadSet,
    /// The app's last entry the read set was taken at: older changes are in it.
    since: u64,
    /// The newest change that touched it and has not been noticed yet.
    dirty: Option<u64>,
    /// The newest change that touched it (noticed or not), for a resumed stream.
    touched: u64,
}

impl Watch {
    /// A watch of `view(args)` whose read set was taken at the app's entry `since`.
    #[must_use]
    pub fn new(view: String, args: Value, read_set: ReadSet, since: u64) -> Self {
        Self {
            view,
            args,
            read_set,
            since,
            dirty: None,
            touched: since,
        }
    }
}

/// A watch that is due a notice.
#[derive(Clone, Debug)]
pub struct Due {
    pub sub: String,
    pub view: String,
    pub args: Value,
    pub index: u64,
}

/// Why a watch was not added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Full {
    Stream,
    Node,
}

struct StreamState {
    watches: BTreeMap<String, Watch>,
    /// The connection while a client holds the stream.
    out: Option<mpsc::Sender<Notice>>,
    /// When the last connection went.
    detached_at: Option<Instant>,
    /// The last notice's sequence number on this stream.
    seq: u64,
}

/// One watch stream, served by this node.
pub struct WatchStream {
    pub id: String,
    pub app: String,
    pub holder: Holder,
    state: Mutex<StreamState>,
    /// Asks the stream's task to send what is due now (a client resumed).
    pub wake: Notify,
}

impl WatchStream {
    fn lock(&self) -> std::sync::MutexGuard<'_, StreamState> {
        // SAFETY: guaranteed by every critical section below: none panics while holding it.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Takes a client's connection; `resume_after` (the index of its `Last-Event-ID`)
    /// marks every watch touched after it, so what it missed is sent again.
    pub fn attach(&self, out: mpsc::Sender<Notice>, resume_after: Option<u64>) {
        let mut st = self.lock();
        st.out = Some(out);
        st.detached_at = None;
        if let Some(after) = resume_after {
            for w in st.watches.values_mut() {
                if w.touched > after {
                    w.dirty = Some(w.dirty.map_or(w.touched, |d| d.max(w.touched)));
                }
            }
        }
        drop(st);
        self.wake.notify_one();
    }

    /// The client's connection `out` went; the watches stay for [`RESUME_GRACE`]. A
    /// connection that a resumed one replaced changes nothing.
    pub fn detach(&self, out: &mpsc::Sender<Notice>) {
        let mut st = self.lock();
        if st.out.as_ref().is_some_and(|o| o.same_channel(out)) {
            st.out = None;
            st.detached_at = Some(Instant::now());
        }
    }

    /// Whether the stream has been without a client longer than [`RESUME_GRACE`].
    #[must_use]
    pub fn expired(&self) -> bool {
        self.lock()
            .detached_at
            .is_some_and(|at| at.elapsed() > RESUME_GRACE)
    }

    /// The connection, if a client holds the stream.
    #[must_use]
    pub fn out(&self) -> Option<mpsc::Sender<Notice>> {
        self.lock().out.clone()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().watches.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Marks every watch `change` touches (an entry after the one its read set was taken
    /// at whose diff lands in it; `keys: None`, everything).
    pub fn mark(&self, change: &Changed) {
        let mut st = self.lock();
        for w in st.watches.values_mut() {
            if change.index <= w.since {
                continue;
            }
            let hit = match &change.keys {
                None => true,
                Some(keys) => keys.iter().any(|k| w.read_set.touches_key(k)),
            };
            if hit {
                w.dirty = Some(w.dirty.map_or(change.index, |d| d.max(change.index)));
                w.touched = w.touched.max(change.index);
            }
        }
    }

    /// Takes the watches due a notice, clearing their marks.
    #[must_use]
    pub fn take_due(&self) -> Vec<Due> {
        let mut st = self.lock();
        st.watches
            .iter_mut()
            .filter_map(|(sub, w)| {
                let index = w.dirty.take()?;
                Some(Due {
                    sub: sub.clone(),
                    view: w.view.clone(),
                    args: w.args.clone(),
                    index,
                })
            })
            .collect()
    }

    /// Puts a mark back (the notice could not be sent).
    pub fn remark(&self, sub: &str, index: u64) {
        if let Some(w) = self.lock().watches.get_mut(sub) {
            w.dirty = Some(w.dirty.map_or(index, |d| d.max(index)));
        }
    }

    /// The next notice sequence number.
    pub fn next_seq(&self) -> u64 {
        let mut st = self.lock();
        st.seq += 1;
        st.seq
    }

    /// Replaces a watch's read set with one taken at `since`; `now` is the app's last entry
    /// once it is in place: anything applied in between may have been checked against the
    /// old set, so it counts as a change.
    pub fn refresh(&self, sub: &str, read_set: ReadSet, since: u64, now: impl FnOnce() -> u64) {
        {
            let mut st = self.lock();
            let Some(w) = st.watches.get_mut(sub) else {
                return;
            };
            w.read_set = read_set;
            w.since = w.since.max(since);
        }
        self.caught_up(sub, now());
    }

    /// Marks watch `sub` if the app's last entry `now` (read once the watch's read set is in
    /// place) is past the one its read set was taken at.
    fn caught_up(&self, sub: &str, now: u64) {
        let mut st = self.lock();
        let Some(w) = st.watches.get_mut(sub) else {
            return;
        };
        if now > w.since {
            w.dirty = Some(w.dirty.map_or(now, |d| d.max(now)));
            w.touched = w.touched.max(now);
        }
    }

    fn insert(&self, sub: String, watch: Watch) -> Result<(), Full> {
        let mut st = self.lock();
        if st.watches.len() >= MAX_WATCHES_PER_STREAM {
            return Err(Full::Stream);
        }
        let _prev = st.watches.insert(sub, watch);
        Ok(())
    }

    fn remove(&self, sub: &str) -> bool {
        self.lock().watches.remove(sub).is_some()
    }
}

/// Every watch stream this node serves.
#[derive(Default)]
pub struct Watches {
    streams: Mutex<HashMap<String, Arc<WatchStream>>>,
    /// Watch id to its stream.
    subs: Mutex<HashMap<String, String>>,
    total: AtomicUsize,
}

impl Watches {
    fn streams(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<WatchStream>>> {
        // SAFETY: guaranteed by every critical section below: none panics while holding it.
        self.streams
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn subs(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        // SAFETY: as for `streams`.
        self.subs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A new stream, or `None` past [`crate::api::MAX_EVENT_STREAMS`] streams held.
    #[must_use]
    pub fn open(&self, id: String, app: String, holder: Holder) -> Option<Arc<WatchStream>> {
        let mut streams = self.streams();
        if streams.len() >= crate::api::MAX_EVENT_STREAMS {
            return None;
        }
        let stream = Arc::new(WatchStream {
            id: id.clone(),
            app,
            holder,
            state: Mutex::new(StreamState {
                watches: BTreeMap::new(),
                out: None,
                detached_at: None,
                seq: 0,
            }),
            wake: Notify::new(),
        });
        let _prev = streams.insert(id, Arc::clone(&stream));
        Some(stream)
    }

    #[must_use]
    pub fn stream(&self, id: &str) -> Option<Arc<WatchStream>> {
        self.streams().get(id).cloned()
    }

    /// The stream that holds watch `sub`.
    #[must_use]
    pub fn stream_of(&self, sub: &str) -> Option<Arc<WatchStream>> {
        let id = self.subs().get(sub).cloned()?;
        self.stream(&id)
    }

    /// Forgets a stream and its watches.
    pub fn close(&self, id: &str) {
        let Some(stream) = self.streams().remove(id) else {
            return;
        };
        let gone: Vec<String> = stream.lock().watches.keys().cloned().collect();
        let mut subs = self.subs();
        for sub in &gone {
            let _ = subs.remove(sub);
        }
        let _ = self.total.fetch_sub(gone.len(), Ordering::SeqCst);
    }

    /// Adds `watch` as `sub`; `now` as in [`WatchStream::refresh`].
    pub fn add(
        &self,
        stream: &WatchStream,
        sub: String,
        watch: Watch,
        now: impl FnOnce() -> u64,
    ) -> Result<(), Full> {
        if self.total.fetch_add(1, Ordering::SeqCst) >= MAX_WATCHES_PER_NODE {
            let _ = self.total.fetch_sub(1, Ordering::SeqCst);
            return Err(Full::Node);
        }
        if let Err(full) = stream.insert(sub.clone(), watch) {
            let _ = self.total.fetch_sub(1, Ordering::SeqCst);
            return Err(full);
        }
        let _prev = self.subs().insert(sub.clone(), stream.id.clone());
        stream.caught_up(&sub, now());
        Ok(())
    }

    /// Removes a watch; `false` if the stream holds no such watch.
    pub fn remove(&self, stream: &WatchStream, sub: &str) -> bool {
        if !stream.remove(sub) {
            return false;
        }
        let _ = self.subs().remove(sub);
        let _ = self.total.fetch_sub(1, Ordering::SeqCst);
        true
    }

    /// Watches held on this node.
    #[must_use]
    pub fn total(&self) -> usize {
        self.total.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_set_of(state: &[(&[u8], &[u8])], key: &[u8]) -> ReadSet {
        use calimero_app_host::{
            Directory, HostCore, Invocation, InvocationKind, OrgRole, Status, Usage,
        };
        struct Nobody;
        impl Directory for Nobody {
            fn org(&self) -> Option<String> {
                None
            }
            fn app_owner(&self) -> String {
                String::new()
            }
            fn org_role(&self, _: &str) -> Option<OrgRole> {
                None
            }
            fn in_team(&self, _: &str, _: &str) -> bool {
                false
            }
            fn team_exists(&self, _: &str) -> bool {
                false
            }
            fn orgs_of(&self, _: &str) -> Vec<(String, OrgRole)> {
                Vec::new()
            }
        }
        let state: BTreeMap<Vec<u8>, Vec<u8>> =
            state.iter().map(|(k, v)| (stored(k), v.to_vec())).collect();
        let inv = Invocation {
            kind: InvocationKind::View,
            method: "m".into(),
            args: Vec::new(),
            caller: calimero_app_host::Caller::Public,
            entry_ts_ms: 0,
            id_term: 0,
            id_seq: 0,
            seed: [0; 32],
            timer: None,
            compute: None,
        };
        let mut host = HostCore::new("a", Arc::new(state), 0, inv, Arc::new(Nobody))
            .unwrap()
            .with_read_set();
        let _ = host.storage_get(key).unwrap();
        host.finish(Status::Ok, Vec::new(), Usage::default())
            .read_set
            .unwrap()
    }

    fn stored(key: &[u8]) -> Vec<u8> {
        calimero_app_host::stored_key(calimero_app_host::GENERATION, key)
    }

    fn change(index: u64, key: &[u8]) -> Changed {
        Changed {
            index,
            keys: Some(vec![stored(key)]),
        }
    }

    fn holder() -> Holder {
        Holder {
            caller: Caller::Public,
            session: None,
            passkey: None,
        }
    }

    #[test]
    fn changes_mark_only_the_watches_they_touch_and_coalesce() {
        let watches = Watches::default();
        let stream = watches.open("s".into(), "app".into(), holder()).unwrap();
        let rs = read_set_of(&[(b"a", b"1")], b"a");
        watches
            .add(
                &stream,
                "w1".into(),
                Watch::new("v".into(), Value::Null, rs, 5),
                || 5,
            )
            .unwrap();
        // At or before the index the set was taken at: already in what the view read.
        stream.mark(&change(5, b"a"));
        assert!(stream.take_due().is_empty());
        stream.mark(&change(6, b"b"));
        assert!(stream.take_due().is_empty());
        stream.mark(&change(7, b"a"));
        stream.mark(&change(9, b"a"));
        let due = stream.take_due();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].index, 9);
        assert!(stream.take_due().is_empty());
        // A resumed client is sent what it missed after its last id.
        let (tx, _rx) = mpsc::channel(1);
        stream.attach(tx, Some(8));
        assert_eq!(stream.take_due()[0].index, 9);
        stream.attach(mpsc::channel(1).0, Some(9));
        assert!(stream.take_due().is_empty());
    }

    #[test]
    fn a_change_applied_while_a_watch_is_added_is_not_lost() {
        let watches = Watches::default();
        let stream = watches.open("s".into(), "app".into(), holder()).unwrap();
        let rs = read_set_of(&[], b"a");
        // The task saw entry 6 before the watch was in place; the app is at 6 now.
        watches
            .add(
                &stream,
                "w".into(),
                Watch::new("v".into(), Value::Null, rs, 5),
                || 6,
            )
            .unwrap();
        assert_eq!(stream.take_due()[0].index, 6);
    }

    #[test]
    fn watches_are_bounded_per_stream_and_freed_with_it() {
        let watches = Watches::default();
        let stream = watches.open("s".into(), "app".into(), holder()).unwrap();
        for i in 0..MAX_WATCHES_PER_STREAM {
            let watch = Watch::new("v".into(), Value::Null, ReadSet::default(), 0);
            watches.add(&stream, format!("w{i}"), watch, || 0).unwrap();
        }
        let watch = Watch::new("v".into(), Value::Null, ReadSet::default(), 0);
        let over = watches.add(&stream, "x".into(), watch, || 0);
        assert_eq!(over, Err(Full::Stream));
        assert_eq!(watches.total(), MAX_WATCHES_PER_STREAM);
        assert!(watches.stream_of("w3").is_some());
        watches.close("s");
        assert_eq!(watches.total(), 0);
        assert!(watches.stream_of("w3").is_none());
    }
}
