//! Updates that apply on their own (C9; docs: protocol upgrades "Updates that apply on
//! their own"): the system leader's pass, from `reconcile`, at most every
//! [`AUTO_UPDATE_EVERY`].
//!
//! Each pass finds the apps an update would apply to on its own now
//! (`SystemState::auto_update_candidates`: a `noDestinations` policy, the newer releases
//! whose hold passed since they were logged, newest first),
//! classifies each move as any upgrade does, newest first, and runs the first green one
//! of each app (`api::upgrades::auto_upgrade`: a restore point first, then
//! `UpgradeApp { auto }`, which apply judges again). The pass's time is the one the
//! upgrade logs (`types::update_now`); nothing judges a node's clock.
//!
//! What the leader keeps between passes is in memory only ([`AutoUpdateMemory`]): when to
//! try again a move that failed before it ran (a replica down, no snapshot yet), with a
//! backoff, and which moves it found not green, for a while. Neither is replicated or
//! judged at apply: a new leader starts without them and only does the work again.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;

use calimero_bundle::update::Class;
use tracing::{info, warn};

use super::Node;
use crate::system::cluster_version::{self as cv, Feature};
use crate::system::PackageRef;

/// How often the system leader looks for updates to apply on their own.
pub const AUTO_UPDATE_EVERY: Duration = Duration::from_secs(5);

/// The most apps one pass upgrades (tries to: an app it skips, red or waiting out its
/// backoff, does not count); the rest wait for the next.
pub const MAX_AUTO_UPDATES_PER_PASS: usize = 16;

/// The most moves one pass classifies, so a pass stays bounded however many releases the
/// candidates list; a move found not green is remembered ([`NOT_GREEN_KEPT_SECS`]), so the
/// next pass reaches the apps after it.
pub const MAX_AUTO_UPDATE_CLASSES_PER_PASS: usize = 64;

/// How long after a move first failed before it ran the leader tries it again (update
/// clock seconds); each further failure doubles it, up to [`AUTO_UPDATE_RETRY_MAX_SECS`].
pub const AUTO_UPDATE_RETRY_FIRST_SECS: u64 = 60;

/// The longest the leader waits between tries of a move that keeps failing before it ran.
pub const AUTO_UPDATE_RETRY_MAX_SECS: u64 = 3600;

/// How long the leader takes its finding that a move is not green as standing before it
/// classifies it again (the unit's members may move meanwhile).
pub const NOT_GREEN_KEPT_SECS: u64 = 3600;

/// The most moves the leader remembers of each kind (retries, verdicts); past it, a new
/// one is not remembered and is only worked out again.
pub const MAX_AUTO_UPDATE_MEMORY: usize = 4096;

/// A move: the app, and the release version it would move to.
type Move = (String, String);

/// What the system leader keeps between passes, in memory ([`Node::auto_update_once`]).
#[derive(Debug, Default)]
pub struct AutoUpdateMemory {
    /// Moves that failed before the upgrade ran: failures so far, and the update clock
    /// time before which the move is not tried again.
    retry: HashMap<Move, (u32, u64)>,
    /// Moves found not green, from the bundle the app ran then, until when that stands.
    not_green: HashMap<Move, (String, u64)>,
}

impl AutoUpdateMemory {
    /// Forget what no longer concerns a candidate move, and verdicts past their time.
    fn keep_only(&mut self, moves: &[(String, Vec<PackageRef>)], now: u64) {
        let live = |m: &Move| {
            moves
                .iter()
                .any(|(app, to)| *app == m.0 && to.iter().any(|r| r.version == m.1))
        };
        self.retry.retain(|m, _| live(m));
        self.not_green
            .retain(|m, (_, until)| live(m) && now < *until);
    }

    /// Whether the move waits out its backoff at `now`.
    fn waits(&self, m: &Move, now: u64) -> bool {
        self.retry.get(m).is_some_and(|(_, at)| now < *at)
    }

    /// Whether the move was found not green while the app ran `bundle`.
    fn not_green(&self, m: &Move, bundle: &str) -> bool {
        self.not_green.get(m).is_some_and(|(b, _)| b == bundle)
    }

    fn remember_not_green(&mut self, m: Move, bundle: &str, now: u64) {
        if self.not_green.len() < MAX_AUTO_UPDATE_MEMORY || self.not_green.contains_key(&m) {
            let _prev = self.not_green.insert(
                m,
                (bundle.to_owned(), now.saturating_add(NOT_GREEN_KEPT_SECS)),
            );
        }
    }

    /// The move failed before it ran: try it again after the backoff.
    fn failed(&mut self, m: Move, now: u64) {
        if self.retry.len() >= MAX_AUTO_UPDATE_MEMORY && !self.retry.contains_key(&m) {
            return;
        }
        let failures = self.retry.get(&m).map_or(0, |(n, _)| *n).saturating_add(1);
        let _prev = self
            .retry
            .insert(m, (failures, now.saturating_add(retry_delay(failures))));
    }

    fn applied(&mut self, m: &Move) {
        let _gone = self.retry.remove(m);
    }
}

/// The wait before the next try of a move that failed `failures` times before it ran.
fn retry_delay(failures: u32) -> u64 {
    let doublings = failures.saturating_sub(1).min(16);
    AUTO_UPDATE_RETRY_FIRST_SECS
        .saturating_mul(1 << doublings)
        .min(AUTO_UPDATE_RETRY_MAX_SECS)
}

impl Node {
    /// On the system leader: start a pass ([`Self::auto_update_once`]) in the
    /// background unless one runs or ran within [`AUTO_UPDATE_EVERY`].
    pub(super) fn auto_update(&self) {
        let now = tokio::time::Instant::now();
        {
            let Ok(mut due) = self.auto_update_due.lock() else {
                return;
            };
            if due.is_some_and(|d| now < d) {
                return;
            }
            *due = Some(now + AUTO_UPDATE_EVERY);
        }
        if self.auto_updating.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(me) = self.shared() else {
            self.auto_updating.store(false, Ordering::SeqCst);
            return;
        };
        drop(tokio::spawn(async move {
            let applied = me.auto_update_once().await;
            if applied > 0 {
                info!(applied, "updates applied on their own");
            }
            me.auto_updating.store(false, Ordering::SeqCst);
        }));
    }

    /// One pass: apply each green update whose hold allows it. Answers how many applied. Nothing before the logged cluster version
    /// brings automatic updates and restore points.
    pub async fn auto_update_once(&self) -> usize {
        // One pass at a time on this node, the timed one ([`Self::auto_update`]) or one
        // called beside it: two at once would classify and upgrade the same app together,
        // and the one that lost would note the other's move as a failed upgrade. A pass
        // called while another runs waits for it, then judges the state it left.
        let _turn = self.auto_update_turn.lock().await;
        #[cfg(feature = "testing")]
        let _counted = crate::testing::AutoUpdatePass::start();
        // Only the system leader's pass moves anything: a node that lost the leadership
        // (a failover while its pass waited or ran) would race the new leader's pass.
        if !self.leads_system() {
            return 0;
        }
        let on = self.sys(|s| {
            cv::check(
                s.cluster_version.as_deref(),
                &[
                    Feature::UpgradeClasses,
                    Feature::AutoUpdates,
                    Feature::RestorePoints,
                ],
            )
            .is_ok()
        });
        if !on {
            return 0;
        }
        let now = crate::types::update_now();
        let candidates = self.sys(|s| s.auto_update_candidates(now));
        self.with_auto_update_memory(|m| {
            let moves: Vec<(String, Vec<PackageRef>)> = candidates
                .iter()
                .map(|(a, to)| (a.app_id.clone(), to.clone()))
                .collect();
            m.keep_only(&moves, now);
        });
        let (mut tried, mut classified, mut applied) = (0, 0, 0);
        'apps: for (app, releases) in candidates {
            if tried >= MAX_AUTO_UPDATES_PER_PASS {
                break;
            }
            // Newest first: the first green one is the app's move this pass; one the
            // leader finds red falls through to the next, down to the release it runs.
            for to in releases {
                let m: Move = (app.app_id.clone(), to.version.clone());
                if self.with_auto_update_memory(|mem| mem.not_green(&m, &app.bundle)) == Some(true)
                {
                    continue;
                }
                if self.with_auto_update_memory(|mem| mem.waits(&m, now)) == Some(true) {
                    continue 'apps;
                }
                if classified >= MAX_AUTO_UPDATE_CLASSES_PER_PASS {
                    break 'apps;
                }
                classified += 1;
                let verdict =
                    crate::api::upgrades::move_verdict(self, &app.app_id, Some(&to)).await;
                match &verdict {
                    // A green move that drops a method waits for a person too: another app
                    // may call it, and no record says which (plain xcalls declare nothing).
                    Some(c) if c.class == Class::Green && c.removed_methods.is_empty() => {}
                    // Red: remembered, so later passes reach the apps after it.
                    Some(_) => {
                        let _r = self.with_auto_update_memory(|mem| {
                            mem.remember_not_green(m, &app.bundle, now);
                        });
                        continue;
                    }
                    // Not classified (a release not readable now): skipped, judged again
                    // next pass.
                    None => continue,
                }
                // Each upgrade takes a while: stop as soon as this node no longer leads.
                if !self.leads_system() {
                    warn!("no longer the system leader: the pass stops");
                    break 'apps;
                }
                tried += 1;
                match crate::api::upgrades::auto_upgrade(self, &app, &to).await {
                    Ok(_) => {
                        applied += 1;
                        let _r = self.with_auto_update_memory(|mem| mem.applied(&m));
                        info!(app = %app.app_id, version = %to.version, "an update applied on its own");
                    }
                    Err(e) => {
                        // A failure of the release itself is noted on the app, which
                        // leaves the candidates; any other is tried again after a backoff.
                        let _r = self.with_auto_update_memory(|mem| mem.failed(m, now));
                        warn!(app = %app.app_id, version = %to.version, error = %e.message, "an update did not apply on its own");
                    }
                }
                continue 'apps;
            }
        }
        applied
    }

    /// Whether this node leads the system group now.
    fn leads_system(&self) -> bool {
        self.system.status().is_leader
    }

    /// Run `f` on the leader's [`AutoUpdateMemory`]; `None` if its lock was poisoned.
    fn with_auto_update_memory<T>(&self, f: impl FnOnce(&mut AutoUpdateMemory) -> T) -> Option<T> {
        self.auto_update_memory.lock().ok().map(|mut m| f(&mut m))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to(version: &str) -> PackageRef {
        PackageRef {
            package: "p".into(),
            version: version.into(),
            service: None,
            compute: false,
        }
    }

    #[test]
    fn a_move_that_failed_before_it_ran_waits_a_doubling_bounded_backoff() {
        let mut mem = AutoUpdateMemory::default();
        let m: Move = ("a".into(), "1.1.0".into());
        mem.failed(m.clone(), 1000);
        assert!(mem.waits(&m, 1000 + AUTO_UPDATE_RETRY_FIRST_SECS - 1));
        assert!(!mem.waits(&m, 1000 + AUTO_UPDATE_RETRY_FIRST_SECS));
        mem.failed(m.clone(), 2000);
        assert!(mem.waits(&m, 2000 + 2 * AUTO_UPDATE_RETRY_FIRST_SECS - 1));
        for _ in 0..40 {
            mem.failed(m.clone(), 3000);
        }
        assert!(!mem.waits(&m, 3000 + AUTO_UPDATE_RETRY_MAX_SECS), "capped");
        mem.applied(&m);
        assert!(!mem.waits(&m, 3000));
    }

    #[test]
    fn the_memory_keeps_only_live_moves_and_is_bounded() {
        let mut mem = AutoUpdateMemory::default();
        let m: Move = ("a".into(), "1.1.0".into());
        mem.remember_not_green(m.clone(), "b1", 0);
        assert!(mem.not_green(&m, "b1"));
        assert!(!mem.not_green(&m, "b2"), "judged from another bundle");
        mem.keep_only(&[("a".into(), vec![to("1.1.0")])], NOT_GREEN_KEPT_SECS - 1);
        assert!(mem.not_green(&m, "b1"));
        mem.keep_only(&[("a".into(), vec![to("1.1.0")])], NOT_GREEN_KEPT_SECS);
        assert!(!mem.not_green(&m, "b1"), "past its time");
        mem.failed(m.clone(), 0);
        mem.keep_only(&[], 0);
        assert!(!mem.waits(&m, 0), "no longer a candidate");
        for i in 0..MAX_AUTO_UPDATE_MEMORY + 10 {
            mem.failed((format!("a{i}"), "1".into()), 0);
            mem.remember_not_green((format!("a{i}"), "1".into()), "b", 0);
        }
        assert_eq!(mem.retry.len(), MAX_AUTO_UPDATE_MEMORY);
        assert_eq!(mem.not_green.len(), MAX_AUTO_UPDATE_MEMORY);
    }
}
