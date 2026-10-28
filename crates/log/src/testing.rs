//! Hooks for tests that need a crash or a slow apply at an exact moment
//! (feature `testing`; never in a release build).

use std::path::Path;

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
