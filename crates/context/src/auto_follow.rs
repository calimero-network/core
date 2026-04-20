//! Auto-follow handler for group members.
//!
//! Subscribes to the op-apply event channel (see [`crate::op_events`])
//! and reacts to governance-DAG ops by emitting the corresponding
//! join ops on behalf of this node — subject to the member having the
//! relevant [`AutoFollowFlags`] set for the group in question.
//!
//! See `architecture/auto-follow.html` for the full
//! architecture. This module implements the context side of Phase 3:
//!
//! - `OpEvent::ContextRegistered { group, context }` — if this node is
//!   a member of `group` with `auto_follow.contexts = true`, emit a
//!   `JoinContext { context }`.
//! - `OpEvent::AutoFollowSet { group, member = self, contexts: true }` —
//!   enumerate existing contexts in the group and join any we haven't
//!   already joined. Covers the "flag flipped on after contexts already
//!   exist" case without a separate reconcile loop.
//!
//! Subgroup auto-follow (`subgroups: true`) is implemented per-role in
//! a follow-up: for `ReadOnlyTee` it will reuse the TDX attestation
//! flow from `fleet_join.rs`; for regular roles it requires a new
//! admission op since existing `MemberAdded` must be admin-signed.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use calimero_context_client::client::ContextClient;
use calimero_context_client::group::JoinContextRequest;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use tokio::sync::Semaphore;
use tokio::task::AbortHandle;
use tracing::{debug, info, warn};

use crate::group_store;
use crate::op_events::{self, OpEvent};

/// Token-bucket rate limit for auto-follow emissions.
///
/// The default is 20 `JoinContext` emissions per minute per node. This
/// bounds amplification when a chatty namespace (many members, rapid
/// context creation) intersects with many peers having `auto_follow.
/// contexts = true`.
pub const DEFAULT_BURST: usize = 20;
pub const DEFAULT_PER: Duration = Duration::from_secs(60);

/// Maximum number of contexts to backfill in one pass when
/// `auto_follow.contexts` flips to true on a group that already has
/// contexts. This is a single-pass cap — after flipping, future
/// contexts are picked up event-driven with no additional limit.
/// If a group has more than this many contexts at flip-time, the
/// remainder must be joined via a subsequent trigger (e.g. re-enabling
/// the flag).
pub const BACKFILL_LIMIT: usize = 1000;

/// Simple token-bucket rate limiter. Acquire blocks until a token is
/// available; tokens refill at `per / burst` intervals up to `burst`.
///
/// Cancellation: dropping the limiter aborts its refill task, so no
/// orphaned tasks leak. Safe to construct many times in tests.
pub struct RateLimiter {
    sem: Arc<Semaphore>,
    refill_task: AbortHandle,
}

impl RateLimiter {
    pub fn new(burst: usize, per: Duration) -> Self {
        assert!(burst > 0, "rate-limiter burst must be positive");
        assert!(
            burst <= u32::MAX as usize,
            "rate-limiter burst must fit in u32 for refill-interval math"
        );
        let sem = Arc::new(Semaphore::new(burst));
        let refill_sem = Arc::clone(&sem);
        let refill_interval = per
            .checked_div(u32::try_from(burst).expect("burst fits in u32"))
            .unwrap_or(per);
        let refill_task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(refill_interval);
            // Skip the first immediate tick — bucket starts full.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if refill_sem.available_permits() < burst {
                    refill_sem.add_permits(1);
                }
            }
        })
        .abort_handle();
        Self { sem, refill_task }
    }

    /// Acquire a token. Awaits until one is available.
    ///
    /// On semaphore close (only happens if the `Arc<Semaphore>` is
    /// dropped, which requires the refill task to also be dropped),
    /// returns immediately without consuming a token — in that case
    /// the limiter is shutting down and the caller proceeds unlimited,
    /// which is the least-bad behavior (keeping the handler alive
    /// rather than blocking forever).
    pub async fn acquire(&self) {
        match self.sem.acquire().await {
            Ok(permit) => permit.forget(),
            Err(_) => {
                warn!(
                    "auto-follow rate limiter semaphore closed — proceeding \
                     without rate limit (likely shutdown)"
                );
            }
        }
    }
}

impl Drop for RateLimiter {
    fn drop(&mut self) {
        self.refill_task.abort();
    }
}

/// Process-wide handle to the currently-running auto-follow handler.
/// Tracked so that tests and shutdown code can cleanly abort the task
/// (including its [`RateLimiter`] refill loop via `Drop`), and so that
/// a second [`spawn`] call returns instead of double-subscribing.
static HANDLE: Mutex<Option<AbortHandle>> = Mutex::new(None);

/// Spawn the auto-follow handler. Returns immediately; the handler
/// runs as a detached tokio task for the process lifetime.
///
/// Idempotent: subsequent calls (e.g. after an Actix actor restart) are
/// no-ops unless [`shutdown`] is called first. Re-subscribing without
/// aborting would duplicate every auto-join and multiply the rate limit.
pub fn spawn(store: Store, context_client: ContextClient) {
    let mut slot = HANDLE.lock().expect("auto-follow HANDLE poisoned");
    if slot.as_ref().is_some_and(|h| !h.is_finished()) {
        debug!("auto-follow handler already running; skipping re-spawn");
        return;
    }
    let limiter = Arc::new(RateLimiter::new(DEFAULT_BURST, DEFAULT_PER));
    let abort = tokio::spawn(async move {
        run(store, context_client, limiter).await;
    })
    .abort_handle();
    *slot = Some(abort);
}

/// Abort the running auto-follow handler, if any. Intended for tests
/// and graceful-shutdown hooks. Safe to call even if no handler is
/// running. After calling this, [`spawn`] may be called again.
pub fn shutdown() {
    if let Some(handle) = HANDLE.lock().expect("auto-follow HANDLE poisoned").take() {
        handle.abort();
    }
}

async fn run(store: Store, context_client: ContextClient, limiter: Arc<RateLimiter>) {
    let mut rx = op_events::subscribe();
    info!("auto-follow handler started");

    loop {
        let event = match rx.recv().await {
            Ok(e) => e,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                warn!(
                    skipped,
                    "auto-follow subscriber lagged; some events were dropped. The DAG is \
                     authoritative — missed events can be recovered on restart via replay."
                );
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                warn!("auto-follow op-event channel closed; handler exiting");
                break;
            }
        };

        match event {
            OpEvent::ContextRegistered {
                group_id,
                context_id,
            } => {
                handle_context_registered(&store, &context_client, &limiter, group_id, context_id)
                    .await;
            }
            OpEvent::AutoFollowSet {
                group_id,
                member,
                contexts,
                subgroups: _,
            } => {
                if contexts {
                    handle_auto_follow_enabled(&store, &context_client, &limiter, group_id, member)
                        .await;
                }
            }
            // Subgroup auto-follow (SubgroupNested) and other variants
            // are handled in a separate pass — see module docs.
            _ => {}
        }
    }
}

async fn handle_context_registered(
    store: &Store,
    context_client: &ContextClient,
    limiter: &Arc<RateLimiter>,
    group_id: [u8; 32],
    context_id: calimero_primitives::context::ContextId,
) {
    let gid = ContextGroupId::from(group_id);
    let self_pk = match self_pk_for_group(store, &gid) {
        Some(pk) => pk,
        None => return,
    };
    if !should_auto_follow_contexts(store, &gid, &self_pk) {
        return;
    }
    limiter.acquire().await;
    debug!(
        group_id = %hex::encode(group_id),
        %context_id,
        "auto-follow: joining new context"
    );
    match context_client
        .join_context(JoinContextRequest { context_id })
        .await
    {
        Ok(_) => info!(
            group_id = %hex::encode(group_id),
            %context_id,
            "auto-follow: joined context"
        ),
        Err(err) => warn!(
            group_id = %hex::encode(group_id),
            %context_id,
            ?err,
            "auto-follow: join_context failed"
        ),
    }
}

async fn handle_auto_follow_enabled(
    store: &Store,
    context_client: &ContextClient,
    limiter: &Arc<RateLimiter>,
    group_id: [u8; 32],
    member: PublicKey,
) {
    let gid = ContextGroupId::from(group_id);
    let self_pk = match self_pk_for_group(store, &gid) {
        Some(pk) => pk,
        None => return,
    };
    // Only backfill if the event is for self.
    if self_pk != member {
        return;
    }
    // Cap backfill at `BACKFILL_LIMIT` contexts per flip. The common
    // case is << 100. Over-cap records are joined via subsequent
    // triggers (re-flipping the flag or future ContextRegistered
    // events, which are event-driven and not subject to this cap).
    let contexts = match group_store::enumerate_group_contexts(store, &gid, 0, BACKFILL_LIMIT) {
        Ok(ids) => ids,
        Err(err) => {
            warn!(
                group_id = %hex::encode(group_id),
                ?err,
                "auto-follow: failed to enumerate contexts for backfill"
            );
            return;
        }
    };
    if contexts.is_empty() {
        return;
    }
    if contexts.len() == BACKFILL_LIMIT {
        warn!(
            group_id = %hex::encode(group_id),
            limit = BACKFILL_LIMIT,
            "auto-follow: backfill truncated — group has at least BACKFILL_LIMIT contexts; \
             remaining contexts will be picked up event-driven as new ContextRegistered \
             ops apply, or by re-flipping the flag"
        );
    }
    info!(
        group_id = %hex::encode(group_id),
        count = contexts.len(),
        "auto-follow: backfilling existing contexts after flag enabled"
    );
    for context_id in contexts {
        limiter.acquire().await;
        match context_client
            .join_context(JoinContextRequest { context_id })
            .await
        {
            Ok(_) => debug!(
                group_id = %hex::encode(group_id),
                %context_id,
                "auto-follow: backfill joined context"
            ),
            Err(err) => warn!(
                group_id = %hex::encode(group_id),
                %context_id,
                ?err,
                "auto-follow: backfill join_context failed"
            ),
        }
    }
}

/// Return this node's public key for the namespace containing `group_id`,
/// or `None` if this node has no identity for that namespace (meaning
/// we're not a member, so auto-follow doesn't apply).
fn self_pk_for_group(store: &Store, group_id: &ContextGroupId) -> Option<PublicKey> {
    match group_store::resolve_namespace_identity(store, group_id) {
        Ok(Some((pk, _, _))) => Some(pk),
        Ok(None) => None,
        Err(err) => {
            warn!(
                group_id = %hex::encode(group_id.to_bytes()),
                ?err,
                "auto-follow: failed to resolve namespace identity"
            );
            None
        }
    }
}

/// Check if `member` is in `group_id` with `auto_follow.contexts = true`.
fn should_auto_follow_contexts(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> bool {
    match group_store::get_group_member_value(store, group_id, member) {
        Ok(Some(v)) => v.auto_follow.contexts,
        Ok(None) => false,
        Err(err) => {
            warn!(
                group_id = %hex::encode(group_id.to_bytes()),
                ?err,
                "auto-follow: failed to read member value"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[tokio::test]
    async fn rate_limiter_bursts_then_throttles() {
        // 3 tokens, refill every 60 ms (180ms/3 rounded).
        let limiter = RateLimiter::new(3, Duration::from_millis(180));
        let start = Instant::now();

        // First 3 should be near-instant (full bucket).
        for _ in 0..3 {
            limiter.acquire().await;
        }
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "burst took too long: {:?}",
            start.elapsed()
        );

        // 4th must wait for a refill tick (~60 ms).
        let before_wait = Instant::now();
        limiter.acquire().await;
        let waited = before_wait.elapsed();
        assert!(
            waited >= Duration::from_millis(30),
            "4th acquire did not wait long enough: {waited:?}"
        );
    }

    #[tokio::test]
    async fn rate_limiter_large_burst_runs_serially() {
        // 2 tokens, refill every 25 ms (50ms/2). 6 acquires should
        // take at least 4*25 = 100ms worth of waits (2 free, 4 waited).
        let limiter = RateLimiter::new(2, Duration::from_millis(50));
        let start = Instant::now();
        for _ in 0..6 {
            limiter.acquire().await;
        }
        assert!(
            start.elapsed() >= Duration::from_millis(80),
            "serial acquires finished too fast: {:?}",
            start.elapsed()
        );
    }
}
