//! A component app's actions (D12, D17, D22, D27, D40): durable runs outside the log.
//!
//! - **Started** (D17): a client's call of an `#[app::action]` method, or `start-action`
//!   in a call or a timer, puts the action on the entry's outcome (`Outcome::actions`); when
//!   the entry applies, the action is recorded as pending (host rows `rows::CACT ‖ id`,
//!   `ActionsHead::pending`), with its caller and start time.
//! - **Run**: the app's leader runs pending actions in a bounded worker pool
//!   (`Node::run_actions`), outside the serial log, as `kind = action` with the starter's
//!   caller (D13), a 10 s budget, storage off, views and calls through the node, and
//!   outbound HTTP through the outbound gate (S9). A new leader resumes every pending action
//!   with the same key.
//! - **Ended** (D22): `componentActionEnd` records each attempt that timed out (retried
//!   with backoff, [`MAX_ATTEMPTS`] in all) and the final end: done, or failed with a
//!   reason, which also sends an `ActionFailed` event to the starter and runs the app's
//!   `onActionFailed`, if it has one. An app error, a fault or out-of-fuel fails at once.

use std::collections::BTreeSet;

use calimero_log::StateTx;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::app::rows;
use crate::types::Caller;

/// Pending actions one app keeps; an action started past it fails at once.
pub const MAX_PENDING_ACTIONS: usize = 1_000;
/// Attempts an action gets before it fails for good (D22).
pub const MAX_ATTEMPTS: u32 = 5;
/// Actions one node runs at once, across its apps.
pub const MAX_ACTION_WORKERS: usize = 8;
/// The first retry's wait; each later one doubles.
pub const RETRY_BASE_MS: u64 = 1_000;
/// Bytes of a failed action's reason.
pub const MAX_ACTION_REASON_BYTES: usize = 512;

/// An action as a run's outcome carries it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActionOut {
    pub id: String,
    pub method: String,
    pub args: Value,
}

/// A pending action, as the host keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionRecord {
    pub id: String,
    pub method: String,
    pub args: Value,
    /// Who started it: its runs and calls act as them (D13).
    pub caller: Caller,
    /// The entry timestamp it was started at: its monotonic clock (D19).
    pub started_ms: u64,
    /// Attempts that timed out so far.
    #[serde(default)]
    pub attempts: u32,
}

/// The app's pending actions, in its `ComponentHead`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionsHead {
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub pending: BTreeSet<String>,
}

/// The row of action `id`.
#[must_use]
pub fn action_key(id: &str) -> Vec<u8> {
    let mut k = vec![rows::CACT];
    k.extend_from_slice(id.as_bytes());
    k
}

impl ActionsHead {
    /// Record the actions an applied entry started. One past [`MAX_PENDING_ACTIONS`] (or a
    /// repeated id) is returned instead, to fail at once.
    pub fn start(
        &mut self,
        tx: &mut StateTx<'_>,
        actions: &[ActionOut],
        caller: &Caller,
        ts_ms: u64,
    ) -> Vec<ActionOut> {
        let mut refused = Vec::new();
        for a in actions {
            if self.pending.len() >= MAX_PENDING_ACTIONS || self.pending.contains(&a.id) {
                refused.push(a.clone());
                continue;
            }
            let record = ActionRecord {
                id: a.id.clone(),
                method: a.method.clone(),
                args: a.args.clone(),
                caller: caller.clone(),
                started_ms: ts_ms,
                attempts: 0,
            };
            match serde_json::to_vec(&record) {
                Ok(v) => tx.put_hidden(rows::CACT, &action_key(&a.id), &v),
                Err(e) => tx.fail(e),
            }
            let _new = self.pending.insert(a.id.clone());
        }
        refused
    }

    /// An attempt of `record` timed out: count it.
    pub fn retried(&mut self, tx: &mut StateTx<'_>, mut record: ActionRecord) {
        record.attempts = record.attempts.saturating_add(1);
        match serde_json::to_vec(&record) {
            Ok(v) => tx.put_hidden(rows::CACT, &action_key(&record.id), &v),
            Err(e) => tx.fail(e),
        }
    }

    /// The action ended for good: no longer pending.
    pub fn ended(&mut self, tx: &mut StateTx<'_>, id: &str) -> bool {
        tx.delete_hidden(rows::CACT, &action_key(id));
        self.pending.remove(id)
    }
}

/// A failure reason, cut to [`MAX_ACTION_REASON_BYTES`].
#[must_use]
pub fn reason(text: &str) -> String {
    let mut out = text.to_owned();
    if out.len() > MAX_ACTION_REASON_BYTES {
        let mut cut = MAX_ACTION_REASON_BYTES;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
    }
    out
}

/// The wait before attempt `attempts + 1`: [`RETRY_BASE_MS`] doubling.
#[must_use]
pub fn retry_after_ms(attempts: u32) -> u64 {
    RETRY_BASE_MS.saturating_mul(1_u64 << attempts.min(10))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_back_off_and_reasons_are_bounded() {
        assert_eq!(retry_after_ms(0), RETRY_BASE_MS);
        assert_eq!(retry_after_ms(3), 8 * RETRY_BASE_MS);
        assert_eq!(retry_after_ms(40), retry_after_ms(10));
        assert_eq!(reason(&"é".repeat(400)).len(), MAX_ACTION_REASON_BYTES);
        assert_eq!(action_key("a.1.2.0")[0], rows::CACT);
    }
}
