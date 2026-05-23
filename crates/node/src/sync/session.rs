//! Per-context sync-session tracking and dispatch backoff.
//!
//! Owns the run-loop-internal state that decides:
//!
//! - Whether a context is eligible for a sync attempt this tick.
//! - How `SyncSessionActor` dispatch outcomes (`Full` / `Closed` / `Ok`)
//!   translate into per-context backoff.
//! - When a session's silence has crossed the wedge-watchdog grace and
//!   the loop should synthesise a failure to unstick the context.
//! - How `SyncSessionResult`s map to per-context `SyncState`
//!   transitions (success / failure / timeout / not-materialised).
//!
//! Extracted from `SyncManager::start` as Phase 3 of #2313. Replaces
//! the inline locals (`state`, `last_dispatch_attempt`,
//! `initiator_dispatched_at`, `last_full_warn`, `full_drops_in_window`,
//! `full_window_started`) and the nested `apply_session_result` /
//! free-function `dispatch_recently_attempted` / `session_dispatch_wedged`
//! helpers with a single typed `SessionTracker`.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use calimero_primitives::context::ContextId;
use tokio::time::Instant;
use tracing::{debug, info, warn};

use super::manager::PeerNotMaterialized;
use super::tracking::{SyncProtocol as TrackingSyncProtocol, SyncState};
use crate::sync_session_bridge::SyncSessionResult;

/// Window over which `record_dispatch_full` rate-limits the per-context
/// warn log. The first drop in the window emits the loud warn; further
/// drops within the window emit a debug. After the window expires the
/// counter rolls up into a single `info` line via
/// [`SessionTracker::tick_full_drops_summary`].
const MAILBOX_FULL_SUMMARY_WINDOW: Duration = Duration::from_secs(60);

/// Per-context sync-session tracking state.
///
/// Owned by [`SyncManager::start`]'s stack frame; lifetime is the
/// run-loop. Not cloneable — only one tracker exists per running
/// `SyncManager`, matching the pre-extraction inline-locals shape.
pub(super) struct SessionTracker {
    /// Per-context sync state. The primary source of truth for "is
    /// this context currently in-flight?" — `last_sync == None` means
    /// in-progress, `Some(_)` means a result has settled the session.
    state: HashMap<ContextId, SyncState>,
    /// #2319: per-context last `try_send` attempt. A `Full` / `Closed`
    /// outcome bumps this so the next interval tick skips re-dispatch
    /// until `dispatch_backoff` has elapsed.
    last_dispatch_attempt: HashMap<ContextId, Instant>,
    /// #2319 watchdog: when a dispatch succeeded. Cleared by the real
    /// `SyncSessionResult` arriving; otherwise [`Self::tick_wedge_watchdog`]
    /// synthesises a failure once `session_wedge_grace` has lapsed.
    initiator_dispatched_at: HashMap<ContextId, Instant>,
    /// #2319: when each context's mailbox-full warn was last emitted,
    /// for rate-limiting (≤1 per `MAILBOX_FULL_SUMMARY_WINDOW`).
    last_full_warn: HashMap<ContextId, Instant>,
    /// Running count of mailbox-full drops in the current rollup
    /// window. Reset by [`Self::tick_full_drops_summary`].
    full_drops_in_window: u64,
    /// Distinct contexts that had at least one drop in the current
    /// rollup window. Reset alongside `full_drops_in_window`. Kept as
    /// a separate set (rather than deriving from `last_full_warn`)
    /// because `last_full_warn`'s entries can age past the window
    /// boundary mid-window, which would make a derived count both
    /// over-count (entries from prior windows that haven't been
    /// pruned yet) and under-count (current-window entries inserted
    /// early enough that their elapsed time has crossed the window).
    /// A dedicated per-window set sidesteps both errors.
    drop_contexts_in_window: HashSet<ContextId>,
    /// Start of the current rollup window.
    full_window_started: Instant,
    /// `session_deadline * 2`. After this, an unresolved dispatch is
    /// treated as wedged and the watchdog synthesises a failure.
    session_wedge_grace: Duration,
    /// `sync_config.interval`. The minimum between consecutive
    /// dispatch attempts for a context after a `Full` / `Closed`
    /// outcome, AND the minimum between successful syncs before the
    /// next attempt is considered (subject to a `force` override).
    dispatch_backoff: Duration,
}

/// Outcome of [`SessionTracker::dispatch_decision`].
#[derive(Debug, Clone, Copy)]
pub(super) enum DispatchDecision {
    Skip(SkipReason),
    Eligible {
        is_first_sync: bool,
        /// `Some(time_since)` when the last successful sync was within
        /// `dispatch_backoff` but the caller forced through anyway
        /// (the explicit-request override). Caller should emit the
        /// "force syncing despite recency" debug log with this.
        forced_despite_recency: Option<Duration>,
    },
}

/// Reason a context is not eligible for dispatch this tick.
#[derive(Debug, Clone, Copy)]
pub(super) enum SkipReason {
    /// `SyncState.last_sync == None`. A dispatch is already in-flight
    /// (or the wedge watchdog hasn't fired yet).
    AlreadyInProgress,
    /// `last_dispatch_attempt` is within `dispatch_backoff`. Either
    /// the mailbox was `Full`/`Closed` recently, or this is a same-tick
    /// re-trigger we already throttled.
    DispatchRecentlyAttempted,
    /// `SyncState.last_sync` was successful within `minimum` ago.
    LastSyncTooRecent {
        time_since: Duration,
        minimum: Duration,
    },
}

/// Whether the caller should emit the loud "mailbox full" warn after
/// [`SessionTracker::record_dispatch_full`]. The first drop in the
/// rollup window for each context returns `EmitWarn`; further drops
/// in the same window return `EmitDebug` and roll up into the periodic
/// info summary instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FullWarnHint {
    EmitWarn,
    EmitDebug,
}

/// Rollup payload returned by [`SessionTracker::tick_full_drops_summary`]
/// when the rate-limit window has expired with non-zero drops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FullDropsRollup {
    pub(super) drops: u64,
    pub(super) contexts_affected: usize,
}

impl SessionTracker {
    /// `session_deadline` is `sync_config.session_deadline` — the
    /// per-session timeout the `SyncSessionActor` enforces.
    /// `dispatch_backoff` is `sync_config.interval` — the minimum
    /// between dispatch attempts and successful syncs.
    pub(super) fn new(session_deadline: Duration, dispatch_backoff: Duration) -> Self {
        const SESSION_WEDGE_GRACE_MULTIPLIER: u32 = 2;
        Self {
            state: HashMap::new(),
            last_dispatch_attempt: HashMap::new(),
            initiator_dispatched_at: HashMap::new(),
            last_full_warn: HashMap::new(),
            full_drops_in_window: 0,
            drop_contexts_in_window: HashSet::new(),
            full_window_started: Instant::now(),
            session_wedge_grace: session_deadline * SESSION_WEDGE_GRACE_MULTIPLIER,
            dispatch_backoff,
        }
    }

    /// `session_wedge_grace` exposed for the caller's wedge warn-log
    /// field (so all log shapes match the pre-extraction text).
    pub(super) fn session_wedge_grace(&self) -> Duration {
        self.session_wedge_grace
    }

    /// Read-only eligibility check for the dispatch loop. `force`
    /// mirrors the explicit-request override the loop applies when a
    /// caller pushed a specific context onto `ctx_sync_rx`.
    ///
    /// `force=true` bypasses [`SkipReason::DispatchRecentlyAttempted`]
    /// (the #2319 mailbox-full backoff) and
    /// [`SkipReason::LastSyncTooRecent`] (the success-interval
    /// throttle). It does NOT bypass [`SkipReason::AlreadyInProgress`]
    /// — an in-flight session is never double-dispatched even on an
    /// explicit request; the wedge watchdog is the right recovery
    /// path for genuinely stuck sessions.
    pub(super) fn dispatch_decision(&self, ctx: &ContextId, force: bool) -> DispatchDecision {
        if !force
            && dispatch_recently_attempted(&self.last_dispatch_attempt, ctx, self.dispatch_backoff)
        {
            return DispatchDecision::Skip(SkipReason::DispatchRecentlyAttempted);
        }
        match self.state.get(ctx) {
            None => DispatchDecision::Eligible {
                is_first_sync: true,
                forced_despite_recency: None,
            },
            Some(existing) => {
                let Some(last_sync) = existing.last_sync() else {
                    return DispatchDecision::Skip(SkipReason::AlreadyInProgress);
                };
                let time_since = last_sync.elapsed();
                let minimum = self.dispatch_backoff;
                if time_since < minimum {
                    if !force {
                        return DispatchDecision::Skip(SkipReason::LastSyncTooRecent {
                            time_since,
                            minimum,
                        });
                    }
                    return DispatchDecision::Eligible {
                        is_first_sync: false,
                        forced_despite_recency: Some(time_since),
                    };
                }
                DispatchDecision::Eligible {
                    is_first_sync: false,
                    forced_despite_recency: None,
                }
            }
        }
    }

    /// Record a `try_send` returning `Full`. Bumps the per-context
    /// dispatch-attempt timestamp, increments the rollup counter, and
    /// returns whether the caller should emit the loud warn this round.
    pub(super) fn record_dispatch_full(&mut self, ctx: ContextId) -> FullWarnHint {
        self.full_drops_in_window += 1;
        let _inserted = self.drop_contexts_in_window.insert(ctx);
        let _prev = self.last_dispatch_attempt.insert(ctx, Instant::now());
        let warn_now = self
            .last_full_warn
            .get(&ctx)
            .is_none_or(|t| t.elapsed() >= MAILBOX_FULL_SUMMARY_WINDOW);
        if warn_now {
            let _prev = self.last_full_warn.insert(ctx, Instant::now());
            FullWarnHint::EmitWarn
        } else {
            FullWarnHint::EmitDebug
        }
    }

    /// Record a `try_send` returning `Closed`. Same backoff as `Full`;
    /// caller always emits a warn (no rate-limiting because `Closed`
    /// is fatal-ish — the actor is gone).
    pub(super) fn record_dispatch_closed(&mut self, ctx: ContextId) {
        let _prev = self.last_dispatch_attempt.insert(ctx, Instant::now());
    }

    /// Record a successful dispatch. Inserts the wedge-watchdog timer
    /// and applies the state transition: a fresh `SyncState::new()`
    /// followed by `start()` on first sync, otherwise
    /// `existing.take_last_sync()` (the in-progress marker the
    /// watchdog watches for).
    ///
    /// Also clears any stale `last_dispatch_attempt` entry for the
    /// context. A successful dispatch supersedes any prior backoff
    /// from a `Full`/`Closed` outcome, so leaving the stale stamp in
    /// place would cause [`Self::dispatch_decision`] to return
    /// `Skip(DispatchRecentlyAttempted)` on subsequent ticks instead
    /// of the correct `Skip(AlreadyInProgress)` — same skip outcome
    /// but a misleading log line.
    pub(super) fn record_dispatch_succeeded(&mut self, ctx: ContextId, is_first_sync: bool) {
        let _prev = self.initiator_dispatched_at.insert(ctx, Instant::now());
        let _stale = self.last_dispatch_attempt.remove(&ctx);
        if is_first_sync {
            let mut new_state = SyncState::new();
            new_state.start();
            let _replaced = self.state.insert(ctx, new_state);
        } else if let Some(existing) = self.state.get_mut(&ctx) {
            let _ignored = existing.take_last_sync();
        }
    }

    /// Apply a `SyncSessionResult` from the result channel. Clears
    /// the dispatch-attempt + wedge timers for the context, then
    /// updates `SyncState`. Logs preserved verbatim from the
    /// pre-extraction `apply_session_result` body so log-grep-based
    /// regression detection still works.
    ///
    /// Defensive: emits a `warn!` if no `SyncState` entry exists for
    /// the result's `context_id`. This should never happen in
    /// practice — `record_dispatch_succeeded` always inserts an
    /// in-progress entry before a session can produce a result — so
    /// a hit here indicates one of (a) the dispatch-tracking path
    /// skipped `record_dispatch_succeeded`, (b) cross-actor routing
    /// delivered a result for the wrong `context_id`, or (c)
    /// external code cleared the state map mid-session. Without the
    /// warn, those bugs are silently swallowed by `and_modify`'s
    /// no-op-on-missing-entry behaviour.
    pub(super) fn apply_result(&mut self, result: SyncSessionResult) {
        let _removed = self.last_dispatch_attempt.remove(&result.context_id);
        let _removed = self.initiator_dispatched_at.remove(&result.context_id);

        if !self.state.contains_key(&result.context_id) {
            warn!(
                context_id = %result.context_id,
                peer_id = %result.peer_id,
                took = ?result.took,
                "SyncSessionResult arrived for context with no tracked SyncState — \
                 dispatch-tracking inconsistency or external state mutation (logic bug, #2445)"
            );
        }

        let SyncSessionResult {
            context_id,
            peer_id,
            took,
            result,
        } = result;

        let _ignored = self.state.entry(context_id).and_modify(|s| match result {
            Ok(Ok(ref protocol)) => {
                s.on_success(peer_id, TrackingSyncProtocol::from(protocol));
                info!(
                    %context_id,
                    ?took,
                    ?protocol,
                    success_count = s.success_count,
                    "Sync finished successfully"
                );
            }
            Ok(Err(ref err)) => {
                // #2422 Option 4: PeerNotMaterialized is benign — the
                // responder told us they're a valid namespace peer
                // that simply hasn't joined this context. Do not
                // increment failure_count or apply backoff — doing so
                // starves legitimate sync against other peers behind
                // 256s exponential delays. The peer-selection filter
                // in peers.rs::namespace-fallback already excludes
                // non-followers up-front; this arm catches the
                // residual race (peer in flight of materialising,
                // mixed-version cluster, etc.).
                if err.downcast_ref::<PeerNotMaterialized>().is_some() {
                    debug!(
                        %context_id,
                        ?took,
                        %peer_id,
                        "peer has not materialised this context — \
                         dropping for this round, not a failure"
                    );
                    return;
                }
                s.on_failure(err.to_string());
                warn!(
                    %context_id,
                    ?took,
                    error = %err,
                    failure_count = s.failure_count(),
                    backoff_secs = s.backoff_delay().as_secs(),
                    "Sync failed, applying exponential backoff"
                );
            }
            Err(ref timeout_err) => {
                s.on_failure(timeout_err.to_string());
                warn!(
                    %context_id,
                    ?took,
                    failure_count = s.failure_count(),
                    backoff_secs = s.backoff_delay().as_secs(),
                    "Sync timed out, applying exponential backoff"
                );
            }
        });
    }

    /// Wedge-watchdog tick. Returns contexts whose initiator was
    /// dispatched more than `session_wedge_grace` ago and whose state
    /// still shows "in progress" (no result has cleared it). Each
    /// returned context's `SyncState` has had `on_failure` applied;
    /// caller emits the warn log per context with the grace value
    /// from [`Self::session_wedge_grace`].
    ///
    /// Returned `Vec` is sorted so call sites and tests see a
    /// deterministic order across runs (the underlying iteration is
    /// over a `HashMap`, which has randomised hash seeds per process).
    ///
    /// Also prunes any past-grace entries from
    /// `initiator_dispatched_at` — including ones that arrived to a
    /// result first and weren't wedged — so the map doesn't grow
    /// unboundedly. Pruning runs AFTER the `on_failure` step so a
    /// future extension to `on_failure` that touches the dispatch map
    /// is not silently undone by `retain`.
    pub(super) fn tick_wedge_watchdog(&mut self) -> Vec<ContextId> {
        let grace = self.session_wedge_grace;
        let mut wedged: Vec<ContextId> = self
            .initiator_dispatched_at
            .keys()
            .copied()
            .filter(|ctx| {
                session_dispatch_wedged(&self.initiator_dispatched_at, &self.state, ctx, grace)
            })
            .collect();
        wedged.sort();
        for ctx in &wedged {
            if let Some(s) = self.state.get_mut(ctx) {
                s.on_failure(
                    "sync session wedged — no SyncSessionResult within watchdog grace (#2319)"
                        .to_owned(),
                );
            }
        }
        self.initiator_dispatched_at
            .retain(|_, dispatched_at| dispatched_at.elapsed() < grace);
        wedged
    }

    /// Full-drops rollup tick. If the rate-limit window has elapsed
    /// since [`Self::full_window_started`] AND there were drops to
    /// summarise, returns `Some((drops, contexts))` and resets the
    /// window; otherwise returns `None` (and still resets the window
    /// if elapsed, to avoid unbounded `last_full_warn` growth).
    ///
    /// `contexts_affected` is the size of [`Self::drop_contexts_in_window`]
    /// at tick time — exactly the distinct contexts that had at least
    /// one drop in the just-closed window. Both the drop counter and
    /// the context set are reset alongside `full_window_started`.
    /// `last_full_warn` is pruned of entries past the rate-limit
    /// window so it doesn't grow unboundedly; that prune is bookkeeping
    /// only and does not feed into the rollup count.
    pub(super) fn tick_full_drops_summary(&mut self) -> Option<FullDropsRollup> {
        if self.full_window_started.elapsed() < MAILBOX_FULL_SUMMARY_WINDOW {
            return None;
        }
        let drops = self.full_drops_in_window;
        let contexts_affected = self.drop_contexts_in_window.len();
        self.full_drops_in_window = 0;
        self.drop_contexts_in_window.clear();
        self.full_window_started = Instant::now();
        self.last_full_warn
            .retain(|_, t| t.elapsed() < MAILBOX_FULL_SUMMARY_WINDOW);
        if drops > 0 {
            Some(FullDropsRollup {
                drops,
                contexts_affected,
            })
        } else {
            None
        }
    }

    /// Test-only: force the rollup window to be expired so the next
    /// `tick_full_drops_summary` call fires.
    #[cfg(test)]
    fn force_full_window_expired(&mut self) {
        self.full_window_started =
            Instant::now() - MAILBOX_FULL_SUMMARY_WINDOW - Duration::from_secs(1);
    }
}

// =========================================================================
// Internal helpers — module-private predicates the methods above route to.
// Kept as separate fns so the dispatch-backoff and wedge-detection
// invariants stay unit-testable against synthetic input maps without
// needing a full `SessionTracker`.
// =========================================================================

fn dispatch_recently_attempted(
    map: &HashMap<ContextId, Instant>,
    context_id: &ContextId,
    interval: Duration,
) -> bool {
    map.get(context_id)
        .is_some_and(|attempted| attempted.elapsed() < interval)
}

fn session_dispatch_wedged(
    dispatched_at: &HashMap<ContextId, Instant>,
    state: &HashMap<ContextId, SyncState>,
    context_id: &ContextId,
    grace: Duration,
) -> bool {
    dispatched_at
        .get(context_id)
        .is_some_and(|dispatched| dispatched.elapsed() >= grace)
        && state
            .get(context_id)
            .is_some_and(|s| s.last_sync().is_none())
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(byte: u8) -> ContextId {
        ContextId::from([byte; 32])
    }

    fn tracker() -> SessionTracker {
        // Use small but realistic durations so the `force` override
        // tests have a meaningful "minimum".
        SessionTracker::new(Duration::from_secs(30), Duration::from_secs(5))
    }

    // -----------------------------------------------------------------
    // dispatch_decision
    // -----------------------------------------------------------------

    #[test]
    fn dispatch_decision_first_sync_when_no_state() {
        let t = tracker();
        match t.dispatch_decision(&ctx(1), false) {
            DispatchDecision::Eligible {
                is_first_sync: true,
                forced_despite_recency: None,
            } => {}
            other => panic!("expected first-sync eligible, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_decision_already_in_progress() {
        let mut t = tracker();
        // Insert an in-progress state (last_sync = None after start()).
        let mut s = SyncState::new();
        s.start();
        let _ = t.state.insert(ctx(1), s);
        match t.dispatch_decision(&ctx(1), false) {
            DispatchDecision::Skip(SkipReason::AlreadyInProgress) => {}
            other => panic!("expected AlreadyInProgress, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_decision_dispatch_backoff_blocks() {
        let mut t = tracker();
        let _ = t.last_dispatch_attempt.insert(ctx(1), Instant::now());
        match t.dispatch_decision(&ctx(1), false) {
            DispatchDecision::Skip(SkipReason::DispatchRecentlyAttempted) => {}
            other => panic!("expected DispatchRecentlyAttempted, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_decision_force_overrides_dispatch_backoff() {
        let mut t = tracker();
        let _ = t.last_dispatch_attempt.insert(ctx(1), Instant::now());
        // force = true → no state → first sync, no recency override
        // metadata (because there's no last_sync to compare against).
        match t.dispatch_decision(&ctx(1), true) {
            DispatchDecision::Eligible {
                is_first_sync: true,
                forced_despite_recency: None,
            } => {}
            other => panic!("expected forced first-sync eligible, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_decision_last_sync_too_recent_blocks_unforced() {
        let mut t = tracker();
        // Simulate a successful sync within `dispatch_backoff`.
        let mut s = SyncState::new();
        s.on_success(
            libp2p::PeerId::random(),
            super::super::tracking::SyncProtocol::DagCatchup,
        );
        let _ = t.state.insert(ctx(1), s);
        match t.dispatch_decision(&ctx(1), false) {
            DispatchDecision::Skip(SkipReason::LastSyncTooRecent {
                time_since,
                minimum,
            }) => {
                assert_eq!(minimum, Duration::from_secs(5));
                assert!(time_since < minimum);
            }
            other => panic!("expected LastSyncTooRecent, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_decision_force_overrides_recency_with_metadata() {
        let mut t = tracker();
        let mut s = SyncState::new();
        s.on_success(
            libp2p::PeerId::random(),
            super::super::tracking::SyncProtocol::DagCatchup,
        );
        let _ = t.state.insert(ctx(1), s);
        match t.dispatch_decision(&ctx(1), true) {
            DispatchDecision::Eligible {
                is_first_sync: false,
                forced_despite_recency: Some(_),
            } => {}
            other => panic!("expected forced-despite-recency eligible, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // record_dispatch_full / record_dispatch_closed / record_dispatch_succeeded
    // -----------------------------------------------------------------

    #[test]
    fn record_dispatch_full_first_emits_warn_then_debug() {
        let mut t = tracker();
        assert_eq!(t.record_dispatch_full(ctx(1)), FullWarnHint::EmitWarn);
        // Second drop in the same window → debug.
        assert_eq!(t.record_dispatch_full(ctx(1)), FullWarnHint::EmitDebug);
        // Different context still gets the first-warn for itself.
        assert_eq!(t.record_dispatch_full(ctx(2)), FullWarnHint::EmitWarn);
    }

    #[test]
    fn record_dispatch_full_bumps_backoff_so_next_decision_skips() {
        let mut t = tracker();
        let _ = t.record_dispatch_full(ctx(1));
        match t.dispatch_decision(&ctx(1), false) {
            DispatchDecision::Skip(SkipReason::DispatchRecentlyAttempted) => {}
            other => panic!("expected DispatchRecentlyAttempted, got {other:?}"),
        }
    }

    #[test]
    fn record_dispatch_closed_bumps_backoff_too() {
        let mut t = tracker();
        t.record_dispatch_closed(ctx(1));
        match t.dispatch_decision(&ctx(1), false) {
            DispatchDecision::Skip(SkipReason::DispatchRecentlyAttempted) => {}
            other => panic!("expected DispatchRecentlyAttempted, got {other:?}"),
        }
    }

    #[test]
    fn record_dispatch_succeeded_first_sync_inserts_in_progress_state() {
        let mut t = tracker();
        t.record_dispatch_succeeded(ctx(1), true);
        // State exists, last_sync is None (in-progress).
        let s = t.state.get(&ctx(1)).expect("state inserted");
        assert!(s.last_sync().is_none());
        // Wedge timer set.
        assert!(t.initiator_dispatched_at.contains_key(&ctx(1)));
    }

    #[test]
    fn record_dispatch_succeeded_clears_stale_dispatch_attempt() {
        let mut t = tracker();
        // Simulate a prior Full outcome that bumped the backoff.
        let _ = t.record_dispatch_full(ctx(1));
        assert!(t.last_dispatch_attempt.contains_key(&ctx(1)));
        // Subsequent dispatch succeeds; backoff stamp must clear so
        // the next decision doesn't mis-report DispatchRecentlyAttempted.
        t.record_dispatch_succeeded(ctx(1), true);
        assert!(!t.last_dispatch_attempt.contains_key(&ctx(1)));
        // The context is now AlreadyInProgress, not RecentlyAttempted.
        match t.dispatch_decision(&ctx(1), false) {
            DispatchDecision::Skip(SkipReason::AlreadyInProgress) => {}
            other => panic!("expected AlreadyInProgress, got {other:?}"),
        }
    }

    #[test]
    fn record_dispatch_succeeded_not_first_sync_takes_last_sync() {
        let mut t = tracker();
        let mut s = SyncState::new();
        s.on_success(
            libp2p::PeerId::random(),
            super::super::tracking::SyncProtocol::DagCatchup,
        );
        assert!(s.last_sync().is_some());
        let _ = t.state.insert(ctx(1), s);
        t.record_dispatch_succeeded(ctx(1), false);
        let s = t.state.get(&ctx(1)).expect("state present");
        assert!(
            s.last_sync().is_none(),
            "take_last_sync should clear the marker"
        );
    }

    // -----------------------------------------------------------------
    // apply_result
    // -----------------------------------------------------------------

    fn ok_result(context_id: ContextId) -> SyncSessionResult {
        SyncSessionResult {
            context_id,
            peer_id: libp2p::PeerId::random(),
            took: Duration::from_millis(50),
            // The inner-protocol variant doesn't matter for the
            // tracker's apply-path — `on_success` increments
            // `success_count` regardless. `None` is the simplest
            // well-formed value.
            result: Ok(Ok(calimero_node_primitives::sync::SyncProtocol::None)),
        }
    }

    fn err_result(context_id: ContextId, msg: &str) -> SyncSessionResult {
        SyncSessionResult {
            context_id,
            peer_id: libp2p::PeerId::random(),
            took: Duration::from_millis(50),
            result: Ok(Err(eyre::eyre!("{msg}"))),
        }
    }

    fn peer_not_materialized_result(context_id: ContextId) -> SyncSessionResult {
        SyncSessionResult {
            context_id,
            peer_id: libp2p::PeerId::random(),
            took: Duration::from_millis(10),
            result: Ok(Err(eyre::Report::new(PeerNotMaterialized))),
        }
    }

    #[test]
    fn apply_result_success_clears_wedge_and_records_success() {
        let mut t = tracker();
        t.record_dispatch_succeeded(ctx(1), true);
        assert!(t.initiator_dispatched_at.contains_key(&ctx(1)));
        t.apply_result(ok_result(ctx(1)));
        assert!(
            !t.initiator_dispatched_at.contains_key(&ctx(1)),
            "wedge timer must be cleared on result"
        );
        assert!(
            !t.last_dispatch_attempt.contains_key(&ctx(1)),
            "dispatch-attempt timer must be cleared too"
        );
        let s = t.state.get(&ctx(1)).expect("state present");
        assert!(s.last_sync().is_some(), "on_success sets last_sync");
        assert_eq!(s.success_count, 1);
    }

    #[test]
    fn apply_result_error_records_failure() {
        let mut t = tracker();
        t.record_dispatch_succeeded(ctx(1), true);
        t.apply_result(err_result(ctx(1), "boom"));
        let s = t.state.get(&ctx(1)).expect("state present");
        assert_eq!(s.failure_count(), 1);
    }

    #[test]
    fn apply_result_peer_not_materialized_does_not_increment_failure_count() {
        let mut t = tracker();
        t.record_dispatch_succeeded(ctx(1), true);
        t.apply_result(peer_not_materialized_result(ctx(1)));
        let s = t.state.get(&ctx(1)).expect("state present");
        assert_eq!(
            s.failure_count(),
            0,
            "PeerNotMaterialized must not count as failure"
        );
    }

    #[test]
    fn apply_result_with_missing_state_does_not_panic_or_create_entry() {
        // #2445 defensive-warn invariant: a `SyncSessionResult`
        // arriving for a context with no tracked `SyncState` must
        // (a) not panic, (b) not silently create a state entry, and
        // (c) clear any backoff / wedge timers that may exist for
        // the context (they shouldn't either, but the clears are
        // idempotent on absent keys).
        //
        // The accompanying `warn!` log fires inside `apply_result`
        // and is the operator-visible signal that the dispatch-
        // tracking invariant was violated; this test exercises the
        // state side-effects.
        let mut t = tracker();
        assert!(
            !t.state.contains_key(&ctx(1)),
            "precondition: no state entry for ctx(1)"
        );

        t.apply_result(ok_result(ctx(1)));

        assert!(
            !t.state.contains_key(&ctx(1)),
            "apply_result must NOT create a state entry on missing-state path \
             (and_modify's behaviour must be preserved)"
        );
        // Timers are removed unconditionally; verify they're still absent.
        assert!(!t.last_dispatch_attempt.contains_key(&ctx(1)));
        assert!(!t.initiator_dispatched_at.contains_key(&ctx(1)));
    }

    // -----------------------------------------------------------------
    // tick_wedge_watchdog
    // -----------------------------------------------------------------

    #[test]
    fn tick_wedge_watchdog_returns_nothing_when_nothing_wedged() {
        let mut t = tracker();
        assert!(t.tick_wedge_watchdog().is_empty());
    }

    #[test]
    fn tick_wedge_watchdog_returns_only_past_grace_in_progress() {
        let mut t = tracker();
        // ctx(1): fresh dispatch — not wedged.
        t.record_dispatch_succeeded(ctx(1), true);
        // ctx(2): synthesise a past-grace dispatch + in-progress state.
        let grace = t.session_wedge_grace;
        let _ = t
            .initiator_dispatched_at
            .insert(ctx(2), Instant::now() - grace - Duration::from_secs(5));
        let mut s = SyncState::new();
        s.start();
        let _ = t.state.insert(ctx(2), s);
        // ctx(3): past-grace dispatch but state is settled — not wedged.
        let _ = t
            .initiator_dispatched_at
            .insert(ctx(3), Instant::now() - grace - Duration::from_secs(5));
        let mut s = SyncState::new();
        s.on_failure("prior failure".to_owned());
        let _ = t.state.insert(ctx(3), s);

        let wedged = t.tick_wedge_watchdog();
        assert_eq!(wedged, vec![ctx(2)]);

        // ctx(2)'s state has on_failure applied.
        let s = t.state.get(&ctx(2)).expect("state present");
        assert_eq!(s.failure_count(), 1);
        // ctx(3) was pruned from the dispatch map but its state was
        // not touched (already settled).
        assert!(!t.initiator_dispatched_at.contains_key(&ctx(3)));
    }

    // -----------------------------------------------------------------
    // tick_full_drops_summary
    // -----------------------------------------------------------------

    #[test]
    fn tick_full_drops_summary_within_window_is_none() {
        let mut t = tracker();
        let _ = t.record_dispatch_full(ctx(1));
        assert!(t.tick_full_drops_summary().is_none());
    }

    #[test]
    fn tick_full_drops_summary_after_window_reports_drops() {
        let mut t = tracker();
        let _ = t.record_dispatch_full(ctx(1));
        let _ = t.record_dispatch_full(ctx(1));
        let _ = t.record_dispatch_full(ctx(2));
        t.force_full_window_expired();
        let rollup = t.tick_full_drops_summary().expect("rollup fired");
        assert_eq!(rollup.drops, 3);
        assert_eq!(rollup.contexts_affected, 2);
        // Counter + context set reset.
        assert_eq!(t.full_drops_in_window, 0);
        assert!(t.drop_contexts_in_window.is_empty());
    }

    #[test]
    fn tick_full_drops_summary_count_is_per_window_not_cumulative() {
        // Reproduces the bug the L390/L393 review caught: a context
        // whose only drop happened in window N must NOT be counted
        // in window N+1's rollup, even though its `last_full_warn`
        // entry may linger in the rate-limit map for a while.
        let mut t = tracker();
        // Window 1: one context drops.
        let _ = t.record_dispatch_full(ctx(1));
        t.force_full_window_expired();
        let r1 = t.tick_full_drops_summary().expect("window 1 fires");
        assert_eq!(r1.drops, 1);
        assert_eq!(r1.contexts_affected, 1);

        // Window 2: a DIFFERENT context drops. ctx(1) had no drops
        // this window, so it must not show up in the count even if
        // its `last_full_warn` entry hasn't been pruned yet.
        let _ = t.record_dispatch_full(ctx(2));
        t.force_full_window_expired();
        let r2 = t.tick_full_drops_summary().expect("window 2 fires");
        assert_eq!(r2.drops, 1);
        assert_eq!(
            r2.contexts_affected, 1,
            "must count only contexts that dropped IN window 2, not the cumulative set"
        );
    }

    #[test]
    fn tick_full_drops_summary_after_empty_window_is_none() {
        let mut t = tracker();
        t.force_full_window_expired();
        // No drops during window → no rollup line, but window resets.
        assert!(t.tick_full_drops_summary().is_none());
    }

    // -----------------------------------------------------------------
    // Internal predicates (preserved from manager::tests)
    // -----------------------------------------------------------------

    fn in_progress_state() -> SyncState {
        let mut s = SyncState::new();
        s.start();
        s
    }

    fn settled_state() -> SyncState {
        let mut s = SyncState::new();
        s.on_failure("prior failure".to_owned());
        s
    }

    const PRED_GRACE: Duration = Duration::from_secs(60);

    #[test]
    fn dispatch_recently_attempted_no_entry_is_not_recent() {
        let map: HashMap<ContextId, Instant> = HashMap::new();
        assert!(!dispatch_recently_attempted(
            &map,
            &ctx(1),
            Duration::from_secs(5)
        ));
    }

    #[test]
    fn dispatch_recently_attempted_fresh_is_recent() {
        let mut map = HashMap::new();
        let _ = map.insert(ctx(2), Instant::now());
        assert!(dispatch_recently_attempted(
            &map,
            &ctx(2),
            Duration::from_secs(5)
        ));
    }

    #[test]
    fn dispatch_recently_attempted_old_is_not_recent() {
        let mut map = HashMap::new();
        let _ = map.insert(ctx(3), Instant::now() - Duration::from_secs(10));
        assert!(!dispatch_recently_attempted(
            &map,
            &ctx(3),
            Duration::from_secs(5)
        ));
    }

    #[test]
    fn session_dispatch_wedged_fresh_in_progress_is_not_wedged() {
        let mut dispatched = HashMap::new();
        let _ = dispatched.insert(ctx(1), Instant::now());
        let mut state = HashMap::new();
        let _ = state.insert(ctx(1), in_progress_state());
        assert!(!session_dispatch_wedged(
            &dispatched,
            &state,
            &ctx(1),
            PRED_GRACE
        ));
    }

    #[test]
    fn session_dispatch_wedged_stale_in_progress_is_wedged() {
        let mut dispatched = HashMap::new();
        let _ = dispatched.insert(ctx(2), Instant::now() - Duration::from_secs(120));
        let mut state = HashMap::new();
        let _ = state.insert(ctx(2), in_progress_state());
        assert!(session_dispatch_wedged(
            &dispatched,
            &state,
            &ctx(2),
            PRED_GRACE
        ));
    }

    #[test]
    fn session_dispatch_wedged_stale_but_settled_is_not_wedged() {
        let mut dispatched = HashMap::new();
        let _ = dispatched.insert(ctx(3), Instant::now() - Duration::from_secs(120));
        let mut state = HashMap::new();
        let _ = state.insert(ctx(3), settled_state());
        assert!(!session_dispatch_wedged(
            &dispatched,
            &state,
            &ctx(3),
            PRED_GRACE
        ));
    }
}
