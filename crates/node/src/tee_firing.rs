//! Firing a TEE trigger: which TEE authority fires it, and when.
//!
//! A TEE trigger — a `tee:` event handler, or an `#[app::tee(every = "..")]`
//! timer tick — should run on exactly one TEE authority. Every authority ranks
//! all of them for the trigger ([`tee_rank`]); the first fires at once, and each
//! later one waits its turn and fires only if no firing has reached it by then
//! ([`plan_tee_firing`]). A firing is recognised by the
//! [`TeeTriggerId`](tee_trigger::TeeTriggerId) its delta carries, which every
//! node records as it applies the delta.
//!
//! Without a consensus round this is at-least-once, not exactly-once: a TEE that
//! is up but partitioned from the others for longer than a turn is doubled by
//! the next one. The two firings' `TeeOnly` writes converge like any concurrent
//! writes do.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::Duration;

use calimero_context_client::client::ContextClient;
use calimero_context_client::tee_trigger;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use eyre::Result;
use tracing::{debug, info, warn};

/// Domain separator for ranking TEE authorities per firing.
const TEE_TRIGGER_RANK_DOMAIN: &[u8] = b"calimero.tee-trigger-rank.v1";

/// How long each TEE authority's turn lasts. The authority ranked `k` fires
/// `k` turns after the trigger, if no firing has reached it by then.
///
/// Long enough for a firing to gossip to every other authority, so a live
/// first-ranked TEE is not doubled by the second; short enough that a game
/// whose TEE is down stalls for seconds, not minutes.
pub(crate) const TEE_FAILOVER_GRACE: Duration = Duration::from_secs(15);

/// This node's place in the order the TEE authorities fire the trigger ranked
/// on `seed`, or `None` if it is not a TEE authority for the context.
///
/// Every authority ranks all of them by `H(seed ‖ account)`, lowest first. The
/// seed names the firing — the delta for an event trigger, the tick for a
/// timer — and never anything a TEE produces, so no TEE can grind an outcome by
/// choosing whether to fire, and the ranking needs no messages.
pub(crate) fn tee_rank(
    context_client: &ContextClient,
    context_id: &ContextId,
    our_identity: &PublicKey,
    seed: &[u8; 32],
) -> Result<Option<usize>> {
    let store = context_client.datastore();
    if !calimero_governance_store::is_tee_authority_for_context(store, context_id, our_identity)? {
        return Ok(None);
    }
    let Some(group_id) = calimero_governance_store::get_group_for_context(store, context_id)?
    else {
        return Ok(None);
    };
    let Some(our_account) =
        calimero_governance_store::member_account_in_namespace(store, &group_id, our_identity)?
    else {
        return Ok(None);
    };
    let mut ranked = calimero_governance_store::tee_authorities_for_context(store, context_id)?;
    ranked.sort_by_cached_key(|account| {
        calimero_primitives::identity::domain_hash(
            TEE_TRIGGER_RANK_DOMAIN,
            &[seed.as_slice(), account.as_bytes().as_slice()],
        )
    });
    Ok(ranked.iter().position(|account| *account == our_account))
}

/// What a TEE authority does with one trigger.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TeePlan {
    /// This node is no TEE authority for the context.
    NotOurs,
    /// Some authority fired it already.
    AlreadyFired,
    /// Fire it now.
    Now,
    /// Fire it after this long, unless a firing arrives first.
    After(Duration),
}

/// When the authority ranked `rank` fires a trigger that has not fired yet.
///
/// The first-ranked fires at once, but only a **fresh** trigger: one no older
/// than a turn by this node's clock. A stale one is a trigger this node is
/// catching up on, after being down or partitioned, and a fallback may well
/// have fired it meanwhile; that firing is still on its way, so even the
/// first-ranked waits a turn for it. Every later rank waits one more turn than
/// the rank before. `age` is `None` when the trigger's time is unknown, which
/// counts as stale.
pub(crate) fn plan_tee_firing(rank: usize, age: Option<Duration>, grace: Duration) -> TeePlan {
    let fresh = age.is_some_and(|age| age <= grace);
    let turns = rank.saturating_add(usize::from(!fresh));
    match u32::try_from(turns) {
        Ok(0) => TeePlan::Now,
        Ok(turns) => TeePlan::After(grace.saturating_mul(turns)),
        Err(_) => TeePlan::After(Duration::MAX),
    }
}

/// What came of offering a trigger to this node.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TeeRun {
    /// Not this node's to fire, or fired already. Nothing is owed.
    Settled,
    /// This node fired it.
    Fired,
    /// This node tried and failed, or could not tell what to do.
    Failed,
    /// This node waits its turn to fall back on it.
    Waiting,
}

/// A context and one of its triggers.
type WaitingKey = ([u8; 32], tee_trigger::TeeTriggerId);

/// Triggers a fallback is waiting on in this process, so a trigger offered
/// again while it waits does not start a second wait.
static WAITING: LazyLock<Mutex<HashSet<WaitingKey>>> = LazyLock::new(Default::default);

fn waiting() -> std::sync::MutexGuard<'static, HashSet<WaitingKey>> {
    WAITING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One TEE trigger this node may fire.
pub(crate) struct TeeFiring {
    pub(crate) context_id: ContextId,
    pub(crate) executor: PublicKey,
    pub(crate) method: String,
    pub(crate) payload: Vec<u8>,
    pub(crate) trigger: tee_trigger::TeeTriggerId,
}

impl TeeFiring {
    /// Offer the trigger to this node, which is ranked `rank` for it (`None`
    /// if it is no TEE authority), `age` after the trigger happened.
    pub(crate) async fn run(
        self,
        context_client: &ContextClient,
        rank: Option<usize>,
        age: Option<Duration>,
    ) -> TeeRun {
        let context_id = self.context_id;
        let tee_method = self.method.clone();
        match self.plan(context_client, rank, age) {
            Ok(TeePlan::NotOurs) => {
                debug!(%context_id, tee_method, "Skipping TEE trigger: this node is not a TEE authority");
                TeeRun::Settled
            }
            Ok(TeePlan::AlreadyFired) => {
                debug!(%context_id, tee_method, "Skipping TEE trigger: already fired");
                TeeRun::Settled
            }
            Ok(TeePlan::Now) => {
                if self.fire(context_client).await {
                    TeeRun::Fired
                } else {
                    TeeRun::Failed
                }
            }
            Ok(TeePlan::After(delay)) => {
                self.fire_after(context_client.clone(), delay);
                TeeRun::Waiting
            }
            Err(err) => {
                warn!(%context_id, tee_method, error = %err, "TEE firing lookup failed");
                TeeRun::Failed
            }
        }
    }

    fn plan(
        &self,
        context_client: &ContextClient,
        rank: Option<usize>,
        age: Option<Duration>,
    ) -> Result<TeePlan> {
        let Some(rank) = rank else {
            return Ok(TeePlan::NotOurs);
        };
        if tee_trigger::tee_fired(context_client.datastore(), &self.context_id, &self.trigger)? {
            return Ok(TeePlan::AlreadyFired);
        }
        Ok(plan_tee_firing(rank, age, TEE_FAILOVER_GRACE))
    }

    /// Fire now. `true` if the run went through.
    async fn fire(&self, context_client: &ContextClient) -> bool {
        let context_id = &self.context_id;
        let tee_method = &self.method;
        info!(%context_id, tee_method, "Firing TEE trigger");
        match context_client
            .execute_tee_trigger(
                context_id,
                &self.executor,
                self.method.clone(),
                self.payload.clone(),
                self.trigger,
            )
            .await
        {
            Ok(_) => {
                // Our own firing never comes back to us as a received delta.
                if let Err(err) = tee_trigger::record_tee_fired(
                    context_client.datastore(),
                    context_id,
                    &self.trigger,
                ) {
                    warn!(%context_id, tee_method, error = %err, "Failed to record our own TEE firing");
                }
                true
            }
            Err(err) => {
                warn!(tee_method, error = %err, "TEE trigger failed");
                false
            }
        }
    }

    /// Wait `delay`, then fire unless a firing has arrived meanwhile.
    ///
    /// Not persisted. An event trigger's caller keeps the delta's events in the
    /// DB, so a restart before the turn comes replays them and waits again; a
    /// timer's tick is offered again by the scheduler after a restart.
    fn fire_after(self, context_client: ContextClient, delay: Duration) {
        let key = (*self.context_id, self.trigger);
        if !waiting().insert(key) {
            return;
        }
        let context_id = self.context_id;
        let tee_method = self.method.clone();
        info!(%context_id, tee_method, ?delay, "Waiting to fall back on a TEE trigger");
        drop(tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            match tee_trigger::tee_fired(context_client.datastore(), &context_id, &self.trigger) {
                Ok(true) => {
                    debug!(%context_id, tee_method, "TEE trigger fired elsewhere; standing down");
                }
                Ok(false) => {
                    info!(%context_id, tee_method, "Falling back on a TEE trigger");
                    let _ = self.fire(&context_client).await;
                }
                Err(err) => {
                    warn!(%context_id, tee_method, error = %err, "TEE fired lookup failed; not falling back");
                }
            }
            let _ = waiting().remove(&key);
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRACE: Duration = Duration::from_secs(10);

    #[test]
    fn the_first_ranked_fires_a_fresh_trigger_at_once() {
        assert_eq!(
            plan_tee_firing(0, Some(Duration::ZERO), GRACE),
            TeePlan::Now
        );
        assert_eq!(plan_tee_firing(0, Some(GRACE), GRACE), TeePlan::Now);
    }

    #[test]
    fn each_later_rank_waits_one_more_turn() {
        let fresh = Some(Duration::from_secs(1));
        assert_eq!(plan_tee_firing(1, fresh, GRACE), TeePlan::After(GRACE));
        assert_eq!(plan_tee_firing(3, fresh, GRACE), TeePlan::After(GRACE * 3));
    }

    /// A node catching up must not fire before the fallback's firing it has
    /// not received yet, so a stale trigger costs every rank one extra turn.
    #[test]
    fn a_stale_or_undated_trigger_waits_a_turn_even_for_the_first_ranked() {
        let stale = Some(GRACE + Duration::from_secs(1));
        assert_eq!(plan_tee_firing(0, stale, GRACE), TeePlan::After(GRACE));
        assert_eq!(plan_tee_firing(2, stale, GRACE), TeePlan::After(GRACE * 3));
        assert_eq!(plan_tee_firing(0, None, GRACE), TeePlan::After(GRACE));
    }
}
