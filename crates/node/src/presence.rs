//! Ephemeral presence (the awareness channel, `presence-intents`): what each
//! member's device says about itself right now (online, a cursor, a
//! selection), for the app's frontends. Never persisted and never in the log.
//!
//! Each node keeps every app's presence in memory and passes each update to
//! every other node (`internal/presence`), so a client on any node sees the
//! whole cluster. One entry per (account, device), the
//! highest `seq` wins (a retract is an entry with no state, so an older set
//! cannot bring it back), an entry not refreshed within [`PRESENCE_TTL_MS`]
//! is swept, at most [`MAX_AUTHORS_PER_APP`] per app (a new author evicts the
//! stalest). Authors are authenticated by the node that took the update.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// An entry not refreshed for this long is gone.
pub const PRESENCE_TTL_MS: u64 = 7_000;

/// The most authors one app's presence holds.
pub const MAX_AUTHORS_PER_APP: usize = 512;

/// The most bytes of state one entry carries.
pub const MAX_STATE_BYTES: usize = 4096;

/// One author's update, as nodes pass it on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Update {
    pub app: String,
    pub account: String,
    /// The passkey (or API key) the update came through: one account's
    /// devices are separate entries.
    pub device: String,
    pub seq: u64,
    /// Hex; `None` retracts.
    pub state: Option<String>,
}

/// What [`Presence::apply`] did with an update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    /// It changed an entry, and the app's streams were told.
    Changed,
    /// A heartbeat: the entry stays alive, nothing to report.
    Refreshed,
    /// Older than what is here, or too large.
    Ignored,
}

impl Applied {
    /// Whether the other nodes need it too.
    #[must_use]
    pub const fn spreads(self) -> bool {
        !matches!(self, Self::Ignored)
    }
}

/// A change a stream reports: an author's new state, or `None` when they left.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diff {
    pub account: String,
    pub device: String,
    pub state: Option<String>,
}

#[derive(Clone, Debug)]
struct Slot {
    seq: u64,
    state: Option<String>,
    at_ms: u64,
}

struct AppPresence {
    entries: BTreeMap<(String, String), Slot>,
    tx: broadcast::Sender<Diff>,
}

impl AppPresence {
    fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            tx: broadcast::channel(256).0,
        }
    }
}

/// Every app's presence on this node.
#[derive(Default)]
pub struct Presence {
    apps: Mutex<HashMap<String, AppPresence>>,
}

impl Presence {
    /// Apply `u` received at `now_ms`.
    pub fn apply(&self, u: &Update, now_ms: u64) -> Applied {
        if u.state
            .as_ref()
            .is_some_and(|s| s.len() > 2 * MAX_STATE_BYTES)
        {
            return Applied::Ignored;
        }
        let mut apps = self
            .apps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let app = apps.entry(u.app.clone()).or_insert_with(AppPresence::new);
        let key = (u.account.clone(), u.device.clone());
        match app.entries.get_mut(&key) {
            Some(slot) if u.seq < slot.seq => return Applied::Ignored,
            Some(slot) if u.seq == slot.seq => {
                // A heartbeat: the same statement again keeps it alive.
                if slot.state == u.state {
                    slot.at_ms = now_ms;
                    return Applied::Refreshed;
                }
                return Applied::Ignored;
            }
            Some(_) => {}
            None => {
                if app.entries.len() >= MAX_AUTHORS_PER_APP {
                    let stalest = app
                        .entries
                        .iter()
                        .min_by_key(|(_, s)| s.at_ms)
                        .map(|(k, _)| k.clone());
                    if let Some(k) = stalest {
                        if let Some(gone) = app.entries.remove(&k) {
                            if gone.state.is_some() {
                                let _n = app.tx.send(Diff {
                                    account: k.0,
                                    device: k.1,
                                    state: None,
                                });
                            }
                        }
                    }
                }
            }
        }
        let _prev = app.entries.insert(
            key,
            Slot {
                seq: u.seq,
                state: u.state.clone(),
                at_ms: now_ms,
            },
        );
        let _n = app.tx.send(Diff {
            account: u.account.clone(),
            device: u.device.clone(),
            state: u.state.clone(),
        });
        Applied::Changed
    }

    /// Drop every entry not refreshed since `now_ms - PRESENCE_TTL_MS`, and
    /// every app left with none and no listener.
    pub fn sweep(&self, now_ms: u64) {
        let mut apps = self
            .apps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for app in apps.values_mut() {
            let expired: Vec<(String, String)> = app
                .entries
                .iter()
                .filter(|(_, s)| {
                    s.at_ms + PRESENCE_TTL_MS <= now_ms || s.at_ms > now_ms + PRESENCE_TTL_MS
                })
                .map(|(k, _)| k.clone())
                .collect();
            for k in expired {
                if let Some(gone) = app.entries.remove(&k) {
                    if gone.state.is_some() {
                        let _n = app.tx.send(Diff {
                            account: k.0,
                            device: k.1,
                            state: None,
                        });
                    }
                }
            }
        }
        apps.retain(|_, a| !a.entries.is_empty() || a.tx.receiver_count() > 0);
    }

    /// Who is present in `app` now, and a receiver for what changes after.
    pub fn subscribe(&self, app: &str) -> (Vec<Diff>, broadcast::Receiver<Diff>) {
        let mut apps = self
            .apps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let a = apps.entry(app.to_owned()).or_insert_with(AppPresence::new);
        (snapshot_of(a), a.tx.subscribe())
    }

    /// Who is present in `app` now.
    pub fn snapshot(&self, app: &str) -> Vec<Diff> {
        let apps = self
            .apps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        apps.get(app).map(snapshot_of).unwrap_or_default()
    }
}

fn snapshot_of(a: &AppPresence) -> Vec<Diff> {
    a.entries
        .iter()
        .filter(|(_, s)| s.state.is_some())
        .map(|((account, device), s)| Diff {
            account: account.clone(),
            device: device.clone(),
            state: s.state.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(account: &str, device: &str, seq: u64, state: Option<&str>) -> Update {
        Update {
            app: "app".into(),
            account: account.into(),
            device: device.into(),
            seq,
            state: state.map(str::to_owned),
        }
    }

    #[test]
    fn the_highest_seq_wins_and_a_retract_does_not_come_back() {
        let p = Presence::default();
        assert_eq!(Applied::Changed, p.apply(&up("a", "d", 2, Some("aa")), 0));
        assert_eq!(
            Applied::Ignored,
            p.apply(&up("a", "d", 1, Some("bb")), 1),
            "an older set"
        );
        assert_eq!(p.snapshot("app")[0].state.as_deref(), Some("aa"));
        assert_eq!(Applied::Changed, p.apply(&up("a", "d", 3, None), 2));
        assert!(p.snapshot("app").is_empty());
        assert_eq!(
            Applied::Ignored,
            p.apply(&up("a", "d", 2, Some("aa")), 3),
            "a replay of the old set"
        );
        assert!(p.snapshot("app").is_empty());
    }

    #[test]
    fn two_devices_of_one_account_are_two_entries() {
        let p = Presence::default();
        assert_eq!(
            Applied::Changed,
            p.apply(&up("a", "laptop", 1, Some("01")), 0)
        );
        assert_eq!(
            Applied::Changed,
            p.apply(&up("a", "phone", 1, Some("02")), 0)
        );
        assert_eq!(p.snapshot("app").len(), 2);
    }

    #[test]
    fn entries_expire_unless_refreshed() {
        let p = Presence::default();
        let (_now, mut rx) = p.subscribe("app");
        assert_eq!(Applied::Changed, p.apply(&up("a", "d", 1, Some("01")), 0));
        assert_eq!(Applied::Changed, p.apply(&up("b", "d", 1, Some("02")), 0));
        // A heartbeat (same seq, same state) keeps b alive.
        assert_eq!(
            Applied::Refreshed,
            p.apply(&up("b", "d", 1, Some("02")), 5_000)
        );
        p.sweep(PRESENCE_TTL_MS + 1);
        let left: Vec<String> = p.snapshot("app").into_iter().map(|d| d.account).collect();
        assert_eq!(left, vec!["b".to_owned()]);
        let mut seen = Vec::new();
        while let Ok(d) = rx.try_recv() {
            seen.push((d.account, d.state));
        }
        assert!(seen.contains(&("a".to_owned(), None)), "{seen:?}");
    }

    #[test]
    fn a_full_app_evicts_the_stalest_author() {
        let p = Presence::default();
        for i in 0..MAX_AUTHORS_PER_APP {
            assert_eq!(
                Applied::Changed,
                p.apply(&up(&format!("a{i}"), "d", 1, Some("00")), i as u64)
            );
        }
        assert_eq!(
            Applied::Changed,
            p.apply(&up("new", "d", 1, Some("00")), 10_000)
        );
        let snap = p.snapshot("app");
        assert_eq!(snap.len(), MAX_AUTHORS_PER_APP);
        assert!(!snap.iter().any(|d| d.account == "a0"), "the stalest went");
        assert!(snap.iter().any(|d| d.account == "new"));
    }

    #[test]
    fn oversized_state_is_refused() {
        let p = Presence::default();
        let big = "00".repeat(MAX_STATE_BYTES + 1);
        assert_eq!(Applied::Ignored, p.apply(&up("a", "d", 1, Some(&big)), 0));
    }
}
