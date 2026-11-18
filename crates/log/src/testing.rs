//! Hooks for tests that need a crash, a slow apply or a held snapshot build at
//! an exact moment (feature `testing`; never in a release build).

use std::path::{Path, PathBuf};

use crate::types::GroupId;

/// Hold back every apply of `group` on the node whose data directory is `dir`
/// (what it passed to [`crate::Engine::open`]), from its next start or now,
/// until [`release_apply`]: entries still append and commit, nothing applies.
/// With [`crate::Engine::lose_unsynced_writes`] before a stop, a test restarts a
/// leader whose own log runs past its applied state, and decides when it
/// catches up.
pub fn hold_apply(dir: &Path, group: GroupId) {
    if let Ok(mut h) = crate::store::HELD.lock() {
        h.push((dir.to_owned(), group));
    }
}

/// Let `group`'s applies on the node at `dir` run again.
pub fn release_apply(dir: &Path, group: GroupId) {
    if let Ok(mut h) = crate::store::HELD.lock() {
        h.retain(|(d, g)| !(*g == group && d == dir));
    }
}

/// Make the next `n` applied batches of `group` on the node at `dir` fail to
/// commit, as a disk that answers EIO or ENOSPC would: each stops the group's
/// Raft core on that batch.
pub fn fail_applies(dir: &Path, group: GroupId, n: u32) {
    if n == 0 {
        return;
    }
    if let Ok(mut f) = crate::store::FAILING.lock() {
        f.push((dir.to_owned(), group, n));
    }
}

/// Make closing the store of the node at `dir` wait `delay` before RocksDB
/// closes, as a thread descheduled in the middle of dropping the last handle
/// would.
pub fn slow_close(dir: &Path, delay: std::time::Duration) {
    if let Ok(mut d) = crate::store::CLOSE_DELAYS.lock() {
        d.push((dir.to_owned(), delay));
    }
}

/// Make every read of `group`'s state rows on the node at `dir` fail, as a
/// disk answering EIO would, until [`heal_state_reads`].
pub fn fail_state_reads(dir: &Path, group: GroupId) {
    if let Ok(mut f) = crate::store::FAILING_READS.lock() {
        f.push((dir.to_owned(), group));
    }
}

/// Let `group`'s state reads on the node at `dir` succeed again.
pub fn heal_state_reads(dir: &Path, group: GroupId) {
    if let Ok(mut f) = crate::store::FAILING_READS.lock() {
        f.retain(|(d, g)| !(*g == group && d == dir));
    }
}

/// Where a snapshot build of a group can be held ([`hold_build`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildPoint {
    /// Before it reads the applied state the snapshot is of.
    Read,
    /// After its file is written, before it records the file as the snapshot
    /// the group serves.
    Record,
}

/// A build held by [`hold_build`].
pub(crate) struct HeldBuild {
    pub(crate) dir: PathBuf,
    pub(crate) group: GroupId,
    pub(crate) at: BuildPoint,
    /// A build has reached `at` and waits there.
    pub(crate) waiting: bool,
}

/// Hold every snapshot build of `group` on the node at `dir` when it reaches
/// `at`, until [`release_build`]; [`build_waiting`] says when one has, so a
/// test decides what an install or a replace does meanwhile. The build waits
/// on its own thread.
pub fn hold_build(dir: &Path, group: GroupId, at: BuildPoint) {
    if let Ok(mut h) = crate::store::HELD_BUILDS.lock() {
        h.push(HeldBuild {
            dir: dir.to_owned(),
            group,
            at,
            waiting: false,
        });
    }
}

/// Whether a build of `group` on the node at `dir` waits where
/// [`hold_build`] holds it.
#[must_use]
pub fn build_waiting(dir: &Path, group: GroupId) -> bool {
    crate::store::HELD_BUILDS
        .lock()
        .map(|h| {
            h.iter()
                .any(|b| b.waiting && b.group == group && b.dir == dir)
        })
        .unwrap_or(false)
}

/// Let `group`'s builds on the node at `dir` run on.
pub fn release_build(dir: &Path, group: GroupId) {
    if let Ok(mut h) = crate::store::HELD_BUILDS.lock() {
        h.retain(|b| !(b.group == group && b.dir == dir));
    }
}

/// Make every synced write of `group` on the node at `dir` (a log append, a
/// vote, a purge) take `delay` longer, as a slow or contended disk's fsync
/// would, until [`heal_synced_writes`]. The write blocks the thread it runs on,
/// before it reaches RocksDB, and [`synced_write_waiting`] names it
/// meanwhile; a long `delay` holds each write until the heal.
pub fn slow_synced_writes(dir: &Path, group: GroupId, delay: std::time::Duration) {
    if let Ok(mut s) = crate::store::SLOW_SYNCS.lock() {
        s.push((dir.to_owned(), group, delay));
    }
}

/// Let `group`'s synced writes on the node at `dir` run at the disk's speed
/// again, a write slowed now included.
pub fn heal_synced_writes(dir: &Path, group: GroupId) {
    if let Ok(mut s) = crate::store::SLOW_SYNCS.lock() {
        s.retain(|(d, g, _)| !(*g == group && d == dir));
    }
}

/// The synced write of `group` on the node at `dir` that waits now where
/// [`slow_synced_writes`] slows it, by a number no other slowed write takes;
/// `None` while none waits.
#[must_use]
pub fn synced_write_waiting(dir: &Path, group: GroupId) -> Option<u64> {
    crate::store::SYNCS_WAITING.lock().ok().and_then(|w| {
        w.iter()
            .find(|(d, g, _)| *g == group && d == dir)
            .map(|(_, _, n)| *n)
    })
}
