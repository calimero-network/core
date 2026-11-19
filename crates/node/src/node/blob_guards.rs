//! What keeps a deletion of a blob's bytes from destroying bytes stored again meanwhile: a
//! logged entry pins the ids it stores until it commits or fails, and a deletion that was
//! between its checks and its delete when a put of the same id landed gives up.
//!
//! A put and a delete of one id take turns on that id's own lock, held across the disk write;
//! the map of ids is locked only to read and change counts, never across a write, so a pin or a
//! check on an async worker never waits for another id's fsync.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::oneshot;

/// How long a pin taken for a peer lasts if the peer never releases it.
const LEASE_TTL: Duration = Duration::from_secs(60);
pub(crate) const MAX_LEASE_IDS: usize = 16; // ids one peer pin may name
const MAX_LEASES: usize = 1024; // peer pins open at once on this node

#[derive(Default)]
struct Slot {
    pins: u32,
    deletions: u32,
    raced: bool,
    /// Taken by a deletion's delete and by a put racing it, across the disk write.
    turn: Arc<Mutex<()>>,
}

#[derive(Default)]
pub(crate) struct BlobGuards {
    slots: Mutex<HashMap<String, Slot>>,
    leases: Mutex<HashMap<u64, (String, oneshot::Sender<()>)>>,
    next_lease: AtomicU64,
}

/// Ids a logged entry stored and has not committed or failed yet; unpinned on drop.
pub(crate) struct Pins {
    guards: Arc<BlobGuards>,
    ids: Vec<String>,
}

/// A deletion of one id, from before its held checks until it is dropped.
pub(crate) struct Deletion {
    guards: Arc<BlobGuards>,
    id: String,
}

impl BlobGuards {
    fn slots(&self) -> MutexGuard<'_, HashMap<String, Slot>> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn pin(self: &Arc<Self>, ids: Vec<String>) -> Pins {
        let mut slots = self.slots();
        for id in &ids {
            slots.entry(id.clone()).or_default().pins += 1;
        }
        Pins {
            guards: Arc::clone(self),
            ids,
        }
    }

    /// Pins `ids` for a peer until [`Self::release_lease`] or `LEASE_TTL`, whichever is first.
    /// A pin of `ids` for a peer storing `app`'s bytes; `None` when too many are open.
    pub(crate) fn lease(self: &Arc<Self>, app: &str, ids: Vec<String>) -> Option<u64> {
        self.lease_for(app, ids, LEASE_TTL)
    }

    fn lease_for(self: &Arc<Self>, app: &str, ids: Vec<String>, ttl: Duration) -> Option<u64> {
        let token = self.next_lease.fetch_add(1, Ordering::Relaxed);
        let (held, released) = oneshot::channel();
        {
            let mut leases = self.leases();
            if leases.len() >= MAX_LEASES {
                return None;
            }
            drop(leases.insert(token, (app.to_owned(), held)));
        }
        let pins = self.pin(ids);
        let guards = Arc::clone(self);
        drop(tokio::spawn(async move {
            let _ended = tokio::time::timeout(ttl, released).await;
            drop(guards.leases().remove(&token));
            drop(pins);
        }));
        Some(token)
    }

    /// Ends `app`'s lease `token`; a token of another app is left alone.
    pub(crate) fn release_lease(&self, app: &str, token: u64) {
        let mut leases = self.leases();
        if leases.get(&token).is_some_and(|(a, _)| a == app) {
            drop(leases.remove(&token));
        }
    }

    fn leases(&self) -> MutexGuard<'_, HashMap<u64, (String, oneshot::Sender<()>)>> {
        self.leases.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn is_pinned(&self, id: &str) -> bool {
        self.slots().get(id).is_some_and(|s| s.pins > 0)
    }

    pub(crate) fn begin_deletion(self: &Arc<Self>, id: &str) -> Deletion {
        self.slots().entry(id.to_owned()).or_default().deletions += 1;
        Deletion {
            guards: Arc::clone(self),
            id: id.to_owned(),
        }
    }

    /// Runs the put of `id`, and tells the deletions of `id` in flight that bytes came. Blocks
    /// while a deletion of `id` deletes: run it off the async workers (`node::disk`).
    pub(crate) fn put<T>(&self, id: &str, store: impl FnOnce() -> T) -> T {
        let turn = (self.slots().get(id))
            .filter(|s| s.deletions > 0)
            .map(|s| Arc::clone(&s.turn));
        let _turn = turn.as_deref().map(take_turn);
        if let Some(slot) = self.slots().get_mut(id).filter(|s| s.deletions > 0) {
            slot.raced = true;
        }
        store()
    }

    fn release(&self, id: &str, pins: u32, deletions: u32) {
        let mut slots = self.slots();
        let Some(slot) = slots.get_mut(id) else {
            return;
        };
        slot.pins -= pins;
        slot.deletions -= deletions;
        if slot.deletions == 0 {
            slot.raced = false;
        }
        if slot.pins == 0 && slot.deletions == 0 {
            let _gone = slots.remove(id);
        }
    }
}

impl Deletion {
    /// Runs `delete` unless a put of the id landed since this deletion began: `None` then.
    /// Blocks while a put of the id racing it writes: run it off the async workers
    /// (`node::disk`).
    pub(crate) fn delete<T>(&self, delete: impl FnOnce() -> T) -> Option<T> {
        // This deletion keeps the id's slot.
        let turn = (self.guards.slots().get(&self.id)).map(|s| Arc::clone(&s.turn));
        let _turn = turn.as_deref().map(take_turn);
        let raced = (self.guards.slots().get(&self.id)).is_some_and(|s| s.raced);
        (!raced).then(delete)
    }
}

fn take_turn(turn: &Mutex<()>) -> MutexGuard<'_, ()> {
    turn.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Drop for Pins {
    fn drop(&mut self) {
        for id in &self.ids {
            self.guards.release(id, 1, 0);
        }
    }
}

impl Drop for Deletion {
    fn drop(&mut self) {
        self.guards.release(&self.id, 0, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_pinned_until_every_pin_on_it_drops() {
        let guards = Arc::new(BlobGuards::default());
        let a = guards.pin(vec!["x".into()]);
        let b = guards.pin(vec!["x".into(), "y".into()]);
        drop(a);
        assert!(guards.is_pinned("x") && guards.is_pinned("y"));
        drop(b);
        assert!(!guards.is_pinned("x") && !guards.is_pinned("y"));
        assert!(guards.slots().is_empty(), "no slot is kept for nothing");
    }

    #[tokio::test]
    async fn a_lease_pins_until_released() {
        let guards = Arc::new(BlobGuards::default());
        let token = guards.lease("a", vec!["x".into()]).unwrap();
        assert!(guards.is_pinned("x"));
        guards.release_lease("a", token);
        for _ in 0..100 {
            if !guards.is_pinned("x") {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("the pin outlived its release");
    }

    #[tokio::test]
    async fn a_lease_nobody_releases_expires() {
        let guards = Arc::new(BlobGuards::default());
        let _token = guards.lease_for("a", vec!["x".into()], Duration::from_millis(20));
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(!guards.is_pinned("x"));
    }

    #[tokio::test]
    async fn another_apps_release_leaves_the_pin() {
        let guards = Arc::new(BlobGuards::default());
        let token = guards.lease("a", vec!["x".into()]).unwrap();
        guards.release_lease("b", token);
        tokio::task::yield_now().await;
        assert!(guards.is_pinned("x"));
    }

    #[tokio::test]
    async fn no_lease_is_taken_past_the_cap() {
        let guards = Arc::new(BlobGuards::default());
        for _ in 0..MAX_LEASES {
            assert!(guards.lease("a", vec!["x".into()]).is_some());
        }
        assert!(guards.lease("a", vec!["y".into()]).is_none());
        assert!(!guards.is_pinned("y"), "a refused lease pins nothing");
    }

    #[test]
    fn a_put_after_a_deletion_began_stops_its_delete() {
        let guards = Arc::new(BlobGuards::default());
        let deletion = guards.begin_deletion("x");
        guards.put("x", || ());
        assert_eq!(deletion.delete(|| "deleted"), None);
    }

    #[test]
    fn a_put_before_a_deletion_began_does_not_stop_it() {
        let guards = Arc::new(BlobGuards::default());
        guards.put("x", || ());
        let deletion = guards.begin_deletion("x");
        assert_eq!(deletion.delete(|| "deleted"), Some("deleted"));
    }

    #[test]
    fn a_put_of_another_id_does_not_stop_a_delete() {
        let guards = Arc::new(BlobGuards::default());
        let deletion = guards.begin_deletion("x");
        guards.put("y", || ());
        assert_eq!(deletion.delete(|| "deleted"), Some("deleted"));
    }

    /// A put waits for a delete of the same id that is writing, then stops none: its bytes land
    /// after the delete. Meanwhile the map of ids is free, as a pin on an async worker needs.
    #[test]
    fn a_put_waits_for_a_delete_of_its_id_without_holding_the_others() {
        let guards = Arc::new(BlobGuards::default());
        let deletion = guards.begin_deletion("x");
        let (deleting, writing) = std::sync::mpsc::channel();
        let (finish, finished) = std::sync::mpsc::channel::<()>();
        std::thread::scope(|s| {
            let deletion = &deletion;
            let deleter = s.spawn(move || {
                deletion.delete(|| {
                    deleting.send(()).unwrap();
                    finished.recv().unwrap();
                })
            });
            writing.recv().unwrap();
            let putter = s.spawn(|| guards.put("x", || "stored"));
            // The map is not held by the delete's write: other ids are pinned meanwhile.
            drop(guards.pin(vec!["y".into()]));
            assert!(!guards.is_pinned("x"));
            std::thread::sleep(Duration::from_millis(50));
            assert!(!putter.is_finished(), "the put did not wait for the delete");
            finish.send(()).unwrap();
            assert_eq!(deleter.join().unwrap(), Some(()));
            assert_eq!(putter.join().unwrap(), "stored");
        });
        drop(deletion);
        assert!(guards.slots().is_empty());
    }

    #[test]
    fn a_race_is_forgotten_once_the_deletions_are_over() {
        let guards = Arc::new(BlobGuards::default());
        let first = guards.begin_deletion("x");
        guards.put("x", || ());
        drop(first);
        let second = guards.begin_deletion("x");
        assert_eq!(second.delete(|| "deleted"), Some("deleted"));
    }
}
