//! A component app's xcalls (D13, D25, D29): the outbox a run fills, the order and
//! deduplication of delivery, and the dead letters.
//!
//! - **Outbox**: an `Ok` entry's xcalls (`Outcome::xcalls`) are appended, in order, to the
//!   source app's outbox: host rows `rows::COUT ‖ seq` (D39), `ComponentHead::outbox`
//!   naming the first and next sequence numbers. A failed run sends nothing.
//! - **Delivery**: the source's leader delivers the outbox head, one message at a time, so
//!   messages leave each source in order (`Node::deliver_xcalls`). The target runs the
//!   method as `kind = xcall-deliver` with `Caller::App(source)`; its ABI must allow the
//!   source (`allowApps`, D13). Only within one organization, or between two personal apps
//!   of one owner, as wasmer's xcall.
//! - **Deduplication** (D25): the target keeps, per source app, the highest sequence number
//!   it applied (`ComponentHead::watermarks`); a message at or below it is answered as
//!   delivered without running, so a leader change on either side never runs one twice.
//! - **Retry and dead letters** (D29): a timeout or an unreachable target is retried with
//!   backoff for up to [`XCALL_WINDOW_MS`] from the send; then, or at once for a deleted
//!   target, one outside the organization, or one that is not a component, the message
//!   goes to the source's dead letters (`rows::CDEAD`, the newest [`MAX_DEAD_LETTERS`]) and
//!   the source's `onXcallFailed`, if its ABI has one that allows the system, runs.
//!   Anything the target answered, an app error or a trap included, is a final answer.

use std::collections::BTreeMap;

use calimero_log::StateTx;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::app::rows;

/// Messages one app's outbox holds; an xcall past it goes to the dead letters at once.
pub const MAX_OUTBOX: u64 = 10_000;
/// Dead letters one app keeps (the oldest go first).
pub const MAX_DEAD_LETTERS: u64 = 1_000;
/// How long a message is retried, from its send (app time), before it is dead-lettered.
pub const XCALL_WINDOW_MS: u64 = 60 * 60 * 1_000;
/// Source apps one target keeps a watermark for; past it the lowest watermark is dropped
/// (its source's next message then runs even if it was delivered before).
pub const MAX_WATERMARKS: usize = 10_000;
/// The longest wait between two attempts at one message.
pub const MAX_BACKOFF_MS: u64 = 30_000;
/// Messages one round of the leader's poll delivers at most.
pub const DELIVER_BATCH: usize = 16;
/// Bytes of a dead letter's reason.
pub const MAX_REASON_BYTES: usize = 512;

/// An xcall as a run's outcome carries it (D25).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct XcallOut {
    pub app: String,
    pub method: String,
    pub args: Value,
}

/// A message waiting in the outbox.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outgoing {
    pub seq: u64,
    pub app: String,
    pub method: String,
    pub args: Value,
    /// The entry timestamp it was sent at (app time).
    pub sent_ms: u64,
}

/// A message that was never delivered (D29).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeadLetter {
    pub seq: u64,
    pub app: String,
    pub method: String,
    pub reason: String,
    /// The entry timestamp it was dead-lettered at.
    pub at_ms: u64,
}

/// Which message a delivery is, carried by the target's entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct XcallFrom {
    pub source: String,
    pub seq: u64,
}

/// An app's outbox, dead letters and watermarks, kept in its `ComponentHead`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct XcallHead {
    /// The first sequence number still in the outbox, and the next one to assign.
    #[serde(default)]
    pub first: u64,
    #[serde(default)]
    pub next: u64,
    /// The dead letters kept: `[dead_first, dead_next)`.
    #[serde(default)]
    pub dead_first: u64,
    #[serde(default)]
    pub dead_next: u64,
    /// Per source app, the highest sequence number applied here (D25).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub watermarks: BTreeMap<String, u64>,
}

fn row(tag: u8, seq: u64) -> Vec<u8> {
    let mut k = vec![tag];
    k.extend_from_slice(&seq.to_be_bytes());
    k
}

impl XcallHead {
    /// Messages waiting.
    #[must_use]
    pub fn pending(&self) -> u64 {
        self.next - self.first
    }

    /// Append an `Ok` entry's xcalls sent at `ts_ms`; one past [`MAX_OUTBOX`] is
    /// dead-lettered at once.
    pub fn send(&mut self, tx: &mut StateTx<'_>, xcalls: &[XcallOut], ts_ms: u64) {
        for x in xcalls {
            let seq = self.next;
            self.next += 1;
            if self.next - self.first > MAX_OUTBOX {
                // Never queued: it leaves the sequence at once.
                self.first = self.next;
                self.dead(tx, seq, &x.app, &x.method, "the outbox is full", ts_ms);
                continue;
            }
            let out = Outgoing {
                seq,
                app: x.app.clone(),
                method: x.method.clone(),
                args: x.args.clone(),
                sent_ms: ts_ms,
            };
            match serde_json::to_vec(&out) {
                Ok(v) => tx.put_hidden(rows::COUT, &row(rows::COUT, seq), &v),
                Err(e) => tx.fail(e),
            }
        }
    }

    /// The message at the head of the outbox: `seq` is done, delivered or (with a reason)
    /// dead-lettered. Any other `seq` changes nothing (an older leader's answer).
    pub fn done(
        &mut self,
        tx: &mut StateTx<'_>,
        head: Option<Outgoing>,
        seq: u64,
        dead: Option<&str>,
        ts_ms: u64,
    ) -> bool {
        if seq != self.first || self.first == self.next {
            return false;
        }
        tx.delete_hidden(rows::COUT, &row(rows::COUT, seq));
        self.first += 1;
        if let (Some(reason), Some(h)) = (dead, head) {
            self.dead(tx, seq, &h.app, &h.method, reason, ts_ms);
        }
        true
    }

    fn dead(
        &mut self,
        tx: &mut StateTx<'_>,
        seq: u64,
        app: &str,
        method: &str,
        reason: &str,
        at_ms: u64,
    ) {
        let mut reason = reason.to_owned();
        if reason.len() > MAX_REASON_BYTES {
            let mut cut = MAX_REASON_BYTES;
            while !reason.is_char_boundary(cut) {
                cut -= 1;
            }
            reason.truncate(cut);
        }
        let letter = DeadLetter {
            seq,
            app: app.to_owned(),
            method: method.to_owned(),
            reason,
            at_ms,
        };
        match serde_json::to_vec(&letter) {
            Ok(v) => tx.put_hidden(rows::CDEAD, &row(rows::CDEAD, self.dead_next), &v),
            Err(e) => tx.fail(e),
        }
        self.dead_next += 1;
        while self.dead_next - self.dead_first > MAX_DEAD_LETTERS {
            tx.delete_hidden(rows::CDEAD, &row(rows::CDEAD, self.dead_first));
            self.dead_first += 1;
        }
    }

    /// Record that the message `seq` from `source` was applied here.
    pub fn delivered(&mut self, from: &XcallFrom) {
        let w = self.watermarks.entry(from.source.clone()).or_default();
        *w = (*w).max(from.seq);
        while self.watermarks.len() > MAX_WATERMARKS {
            let Some(lowest) = self
                .watermarks
                .iter()
                .min_by_key(|(_, s)| **s)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            let _gone = self.watermarks.remove(&lowest);
        }
    }

    /// Whether message `seq` from `source` was applied here already (D25).
    #[must_use]
    pub fn seen(&self, from: &XcallFrom) -> bool {
        self.watermarks
            .get(&from.source)
            .is_some_and(|w| *w >= from.seq)
    }
}

/// The outbox row of `seq`, as `get_hidden` takes it.
#[must_use]
pub fn outbox_key(seq: u64) -> Vec<u8> {
    row(rows::COUT, seq)
}

/// The dead-letter row of position `n`.
#[must_use]
pub fn dead_key(n: u64) -> Vec<u8> {
    row(rows::CDEAD, n)
}

/// Exponential backoff after `attempts` failures: 200 ms doubling, at most
/// [`MAX_BACKOFF_MS`].
#[must_use]
pub fn backoff_ms(attempts: u32) -> u64 {
    200_u64
        .saturating_mul(1_u64 << attempts.min(16))
        .min(MAX_BACKOFF_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_watermark_only_moves_up() {
        let mut h = XcallHead::default();
        let from = |seq| XcallFrom {
            source: "a".into(),
            seq,
        };
        assert!(!h.seen(&from(0)));
        h.delivered(&from(3));
        h.delivered(&from(1));
        assert!(h.seen(&from(3)));
        assert!(h.seen(&from(0)));
        assert!(!h.seen(&from(4)));
    }

    #[test]
    fn backoff_grows_to_its_cap() {
        assert_eq!(backoff_ms(0), 200);
        assert_eq!(backoff_ms(1), 400);
        assert_eq!(backoff_ms(40), MAX_BACKOFF_MS);
    }
}
