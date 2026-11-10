//! Sweeping the segments of sealed files that no app references.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tracing::warn;

use super::Node;

/// How long a segment no app references stays before it is deleted: an upload's or a push's
/// segments reach a node before the entry that lists them.
pub const SEGMENT_GRACE: Duration = Duration::from_secs(24 * 60 * 60);
/// How often every node sweeps its segments.
pub const SWEEP_EVERY: Duration = Duration::from_secs(10 * 60);

/// When each segment was first seen listed by no app; in memory only.
#[derive(Default)]
pub(super) struct Sweep {
    unreferenced: HashMap<String, Instant>,
}

/// Segments an upload session or a pending entry holds, by the number of holders.
pub(super) type Pinned = Arc<Mutex<HashMap<String, usize>>>;

/// Keeps segments out of the sweep and the delete route until it drops.
pub struct SegmentPin {
    pinned: Pinned,
    hashes: Vec<String>,
}

impl Drop for SegmentPin {
    fn drop(&mut self) {
        let mut pinned = self
            .pinned
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for hash in &self.hashes {
            if let Some(n) = pinned.get_mut(hash) {
                *n -= 1;
                if *n == 0 {
                    let _gone = pinned.remove(hash);
                }
            }
        }
    }
}

impl Node {
    /// Holds `hashes` out of the sweep and the delete route until the returned pin drops.
    pub fn pin_segments(&self, hashes: &[String]) -> SegmentPin {
        let mut pinned = self
            .pinned_segments
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for hash in hashes {
            *pinned.entry(hash.clone()).or_default() += 1;
        }
        SegmentPin {
            pinned: Arc::clone(&self.pinned_segments),
            hashes: hashes.to_vec(),
        }
    }

    /// [`Self::sweep_segments_at`] now.
    pub fn sweep_segments(&self) {
        self.sweep_segments_at(Instant::now());
    }

    /// Deletes the stored segments no app hosted here lists and nothing pins, once unlisted for
    /// [`SEGMENT_GRACE`] since first seen so; a pin restarts that time.
    pub fn sweep_segments_at(&self, now: Instant) {
        let stored = match self.engine.list_segments() {
            Ok(stored) => stored,
            Err(e) => {
                warn!(error = %e, "the stored segments could not be listed");
                return;
            }
        };
        if self.apps.is_poisoned() {
            warn!("the hosted apps could not be read: segments not swept");
            return;
        }
        let mut seen = self
            .segment_sweep
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let present: HashSet<&String> = stored.iter().collect();
        seen.unreferenced.retain(|h, _| present.contains(h));
        for hash in &stored {
            let pinned = (self.pinned_segments.lock())
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key(hash);
            if pinned || self.holds_segment(hash) {
                let _gone = seen.unreferenced.remove(hash);
                continue;
            }
            let first = *seen.unreferenced.entry(hash.clone()).or_insert(now);
            if now.saturating_duration_since(first) < SEGMENT_GRACE {
                continue;
            }
            match self.engine.delete_segment(hash) {
                Ok(()) => {
                    let _gone = seen.unreferenced.remove(hash);
                }
                Err(e) => warn!(segment = %hash, error = %e, "segment not deleted"),
            }
        }
    }
}
