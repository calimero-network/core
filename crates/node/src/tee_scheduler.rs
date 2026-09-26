//! The TEE scheduler: fires `#[app::tee(every = "..")]` methods on a timer.
//!
//! An event-driven TEE trigger needs a member to emit the event, so a game
//! cannot time out a turn by itself. A timer method can: its period is in the
//! module's ABI (`Method.tee_every_secs`), and this task fires it once per
//! period on one TEE authority, through the same election and failover as an
//! event trigger (see [`crate::tee_firing`]).
//!
//! Periods are counted from the Unix epoch, so every authority agrees which
//! tick is current, and the tick names the firing: the trigger id and the rank
//! seed both come from `(context, method, tick)`. Only the current tick is ever
//! offered. A node that was down for several periods does not replay the ones
//! it missed — a timer is "check now", not a queue — and after a restart it
//! offers the current tick again, which the fired markers deduplicate.
//!
//! A tick whose run writes nothing produces no delta, so no fired marker, and
//! the next-ranked authority runs it too when its turn comes. That is harmless
//! for a method that only acts when there is something to do, which is what a
//! timer method should be.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use calimero_context_client::client::ContextClient;
use calimero_context_client::tee_trigger;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use futures_util::{pin_mut, StreamExt};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::tee_firing::{self, TeeFiring};

/// How often the scheduler looks for a new tick.
const POLL: Duration = Duration::from_secs(1);

/// How often it re-reads which contexts it is a TEE authority for, and which
/// timer methods their modules declare.
const REFRESH: Duration = Duration::from_secs(30);

/// One timer method this node may fire.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Timer {
    context_id: ContextId,
    executor: PublicKey,
    method: String,
    every_secs: u64,
}

/// Spawn the scheduler for the life of the node.
pub(crate) fn spawn(context_client: ContextClient, node_client: NodeClient) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut timers = Vec::new();
        let mut refreshed: Option<Instant> = None;
        // The last tick offered per timer, so each is offered once.
        let mut offered: HashMap<(ContextId, String), u64> = HashMap::new();
        let mut poll = tokio::time::interval(POLL);
        loop {
            let _ = poll.tick().await;
            if refreshed.is_none_or(|at| at.elapsed() >= REFRESH) {
                timers = discover(&context_client, &node_client).await;
                offered.retain(|(context_id, method), _| {
                    timers
                        .iter()
                        .any(|t| t.context_id == *context_id && t.method == *method)
                });
                refreshed = Some(Instant::now());
            }
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default();
            for timer in &timers {
                let (tick, age) = current_tick(now, timer.every_secs);
                let last = offered.insert((timer.context_id, timer.method.clone()), tick);
                if last == Some(tick) {
                    continue;
                }
                offer(&context_client, timer, tick, age).await;
            }
        }
    })
}

/// The current tick of a timer with period `every_secs`, and how far into it
/// `now` is.
fn current_tick(now: Duration, every_secs: u64) -> (u64, Duration) {
    let secs = now.as_secs();
    let tick = secs / every_secs;
    let started = Duration::from_secs(tick.saturating_mul(every_secs));
    (tick, now.saturating_sub(started))
}

/// Offer one tick of `timer` to this node.
async fn offer(context_client: &ContextClient, timer: &Timer, tick: u64, age: Duration) {
    let trigger = tee_trigger::timer_trigger_id(&timer.context_id, &timer.method, tick);
    let rank = match tee_firing::tee_rank(
        context_client,
        &timer.context_id,
        &timer.executor,
        &trigger,
    ) {
        Ok(rank) => rank,
        Err(err) => {
            warn!(context_id = %timer.context_id, tee_method = timer.method, error = %err, "TEE election lookup failed for a timer");
            return;
        }
    };
    let firing = TeeFiring {
        context_id: timer.context_id,
        executor: timer.executor,
        method: timer.method.clone(),
        // A timer method takes no arguments; the macro refuses one that does.
        payload: b"{}".to_vec(),
        trigger,
    };
    let _ = firing.run(context_client, rank, Some(age)).await;
}

/// Every timer method of every context this node is a TEE authority for.
///
/// Best-effort: a context whose module cannot be read is skipped until the
/// next refresh.
async fn discover(context_client: &ContextClient, node_client: &NodeClient) -> Vec<Timer> {
    let mut timers = Vec::new();
    // Modules are shared between contexts; read each one once per refresh.
    let mut periods_by_app: BTreeMap<(_, Option<String>), Vec<(String, u64)>> = BTreeMap::new();
    let context_ids = context_client.get_context_ids(None);
    pin_mut!(context_ids);
    while let Some(context_id) = context_ids.next().await {
        let context_id = match context_id {
            Ok(context_id) => context_id,
            Err(err) => {
                warn!(error = %err, "TEE scheduler could not list contexts");
                break;
            }
        };
        let Some(executor) = tee_identity(context_client, &context_id).await else {
            continue;
        };
        let context = match context_client.get_context(&context_id) {
            Ok(Some(context)) => context,
            _ => continue,
        };
        let key = (context.application_id, context.service_name.clone());
        if !periods_by_app.contains_key(&key) {
            let periods = timer_periods(node_client, &key.0, key.1.as_deref()).await;
            let _ = periods_by_app.insert(key.clone(), periods);
        }
        for (method, every_secs) in &periods_by_app[&key] {
            timers.push(Timer {
                context_id,
                executor,
                method: method.clone(),
                every_secs: *every_secs,
            });
        }
    }
    if !timers.is_empty() {
        debug!(count = timers.len(), "TEE scheduler refreshed its timers");
    }
    timers
}

/// This node's identity in `context_id` if it is a TEE authority there.
async fn tee_identity(context_client: &ContextClient, context_id: &ContextId) -> Option<PublicKey> {
    let owned = context_client.get_context_members(context_id, Some(true));
    pin_mut!(owned);
    while let Some(Ok((identity, _))) = owned.next().await {
        if calimero_governance_store::is_tee_authority_for_context(
            context_client.datastore(),
            context_id,
            &identity,
        )
        .unwrap_or(false)
        {
            return Some(identity);
        }
    }
    None
}

/// The timer methods a module declares, with their periods.
async fn timer_periods(
    node_client: &NodeClient,
    application_id: &calimero_primitives::application::ApplicationId,
    service_name: Option<&str>,
) -> Vec<(String, u64)> {
    let bytecode = match node_client
        .get_application_bytes(application_id, service_name)
        .await
    {
        Ok(Some(bytecode)) => bytecode,
        _ => return Vec::new(),
    };
    calimero_wasm_abi::embed::read_embedded_state_schema(&bytecode)
        .map(|manifest| timer_methods(&manifest))
        .unwrap_or_default()
}

/// The methods of `manifest` a timer may fire: a period of at least a second
/// and no parameters, as `#[app::tee(every = "..")]` guarantees.
fn timer_methods(manifest: &calimero_wasm_abi::schema::Manifest) -> Vec<(String, u64)> {
    manifest
        .methods
        .iter()
        .filter(|method| method.params.is_empty())
        .filter_map(|method| {
            method
                .tee_every_secs
                .filter(|secs| *secs > 0)
                .map(|secs| (method.name.clone(), secs))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use calimero_wasm_abi::schema::{Manifest, Method, MethodIntent};

    use super::*;

    #[test]
    fn ticks_count_whole_periods_from_the_epoch() {
        assert_eq!(
            current_tick(Duration::from_secs(125), 60),
            (2, Duration::from_secs(5))
        );
        assert_eq!(
            current_tick(Duration::from_secs(120), 60),
            (2, Duration::ZERO)
        );
        assert_eq!(
            current_tick(Duration::from_millis(59_500), 60),
            (0, Duration::from_millis(59_500))
        );
    }

    fn method(name: &str, every: Option<u64>) -> Method {
        Method {
            name: name.to_owned(),
            params: vec![],
            returns: None,
            returns_nullable: None,
            errors: vec![],
            intent: MethodIntent::Mutating,
            xcall_callable: false,
            xcall_callers: Default::default(),
            tee_every_secs: every,
        }
    }

    #[test]
    fn only_declared_periods_are_timers() {
        let manifest = Manifest {
            methods: vec![
                method("sweep", Some(30)),
                method("roll", None),
                method("never", Some(0)),
            ],
            ..Manifest::default()
        };
        assert_eq!(timer_methods(&manifest), vec![("sweep".to_owned(), 30)]);
    }
}
