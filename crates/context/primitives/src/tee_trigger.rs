//! Identity and bookkeeping of a TEE trigger firing.
//!
//! A TEE trigger is one run of an `#[app::tee]` method, fired by the node's TEE
//! scheduler. Several TEE authorities may be able to fire it, and only one
//! should. The elected one fires first; the others wait their turn and fire
//! only if no firing has reached them by then (see the failover notes in
//! `calimero-node`'s `state_delta/events.rs`).
//!
//! For that, every firing has a [`TeeTriggerId`], derived from what caused it
//! ([`TeeTriggerCause`]). The delta a firing produces is signed under
//! `calimero/tee/1`, which commits to that cause. A node that accepts such a
//! delta from a TEE, or fires the trigger itself, records the firing with
//! [`record_tee_fired`], and every TEE checks [`tee_fired`] before it fires.
//!
//! The cause is kept beside each such delta ([`record_delta_trigger`]) because
//! the signature covers it: a node serving the delta to a peer that catches up
//! must hand it over too, or the peer cannot verify what it was sent.

pub use calimero_node_primitives::sync::delta_auth::TeeTriggerCause;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::domain_hash;
use calimero_store::key::Generic as GenericKey;
use calimero_store::slice::Slice;
use calimero_store::types::GenericData;
use calimero_store::Store;

/// Names one firing of one TEE trigger.
pub type TeeTriggerId = [u8; 32];

/// Domain separator for the fired-marker's store key.
const FIRED_KEY_DOMAIN: &[u8] = b"calimero.tee-trigger.fired.v1";

/// Store scope of the fired markers.
const FIRED_SCOPE: [u8; 16] = *b"calimero-teefire";

/// Domain separator for the store key of a delta's trigger.
const DELTA_TRIGGER_KEY_DOMAIN: &[u8] = b"calimero.tee-trigger.delta.v1";

/// Store scope of the triggers kept beside TEE deltas.
const DELTA_TRIGGER_SCOPE: [u8; 16] = *b"calimero-teedelt";

/// How far past this node's clock a timer's tick may begin and its firing
/// still be recorded: the drift the HLC allows a peer.
const TICK_CLOCK_SLACK_SECS: u64 = 5;

fn fired_key(context_id: &ContextId, trigger: &TeeTriggerId) -> GenericKey {
    GenericKey::new(
        FIRED_SCOPE,
        domain_hash(
            FIRED_KEY_DOMAIN,
            &[
                AsRef::<[u8; 32]>::as_ref(context_id).as_slice(),
                trigger.as_slice(),
            ],
        ),
    )
}

/// Whether this node has seen `trigger` fired in `context_id`, by itself or by
/// another TEE authority.
///
/// # Errors
/// A store read error. A caller must not fire on one: it cannot tell whether
/// the trigger already ran.
pub fn tee_fired(
    store: &Store,
    context_id: &ContextId,
    trigger: &TeeTriggerId,
) -> eyre::Result<bool> {
    store
        .handle()
        .has(&fired_key(context_id, trigger))
        .map_err(|err| eyre::eyre!("reading a TEE fired marker: {err}"))
}

/// Record that `trigger` has fired in `context_id`.
///
/// Node-local and never gossiped: every node derives it from the deltas it
/// applies, so there is nothing to agree on.
///
/// # Errors
/// A store write error.
pub fn record_tee_fired(
    store: &Store,
    context_id: &ContextId,
    trigger: &TeeTriggerId,
) -> eyre::Result<()> {
    store
        .handle()
        .put(
            &fired_key(context_id, trigger),
            &GenericData::from(Slice::from(Vec::new())),
        )
        .map_err(|err| eyre::eyre!("recording a TEE fired marker: {err}"))
}

fn delta_trigger_key(context_id: &ContextId, delta_id: &[u8; 32]) -> GenericKey {
    GenericKey::new(
        DELTA_TRIGGER_SCOPE,
        domain_hash(
            DELTA_TRIGGER_KEY_DOMAIN,
            &[
                AsRef::<[u8; 32]>::as_ref(context_id).as_slice(),
                delta_id.as_slice(),
            ],
        ),
    )
}

/// Keep the trigger a TEE delta's `calimero/tee/1` envelope committed to, so the
/// delta can be served with it.
///
/// A row of its own rather than a field on the persisted delta: that row is
/// plain borsh, so a new field would leave every delta already on disk
/// unreadable. A delta with no row here was not TEE-triggered.
///
/// # Errors
/// A store write error, or the cause failing to encode.
pub fn record_delta_trigger(
    store: &Store,
    context_id: &ContextId,
    delta_id: &[u8; 32],
    trigger: &TeeTriggerCause,
) -> eyre::Result<()> {
    let bytes = borsh::to_vec(trigger)
        .map_err(|err| eyre::eyre!("encoding a TEE delta's trigger: {err}"))?;
    store
        .handle()
        .put(
            &delta_trigger_key(context_id, delta_id),
            &GenericData::from(Slice::from(bytes)),
        )
        .map_err(|err| eyre::eyre!("recording a TEE delta's trigger: {err}"))
}

/// The trigger kept for `delta_id`, or `None` for a delta that was not
/// TEE-triggered.
///
/// # Errors
/// A store read error, or a row that does not decode.
pub fn delta_trigger(
    store: &Store,
    context_id: &ContextId,
    delta_id: &[u8; 32],
) -> eyre::Result<Option<TeeTriggerCause>> {
    let handle = store.handle();
    let Some(data) = handle
        .get(&delta_trigger_key(context_id, delta_id))
        .map_err(|err| eyre::eyre!("reading a TEE delta's trigger: {err}"))?
    else {
        return Ok(None);
    };
    borsh::from_slice(data.as_ref())
        .map(Some)
        .map_err(|err| eyre::eyre!("decoding a TEE delta's trigger: {err}"))
}

/// Record that the TEE delta `delta_id`, whose envelope committed to
/// `trigger`, has been accepted: the delta is kept with its trigger so it can
/// be served, and the trigger has fired, as [`record_tee_firing`] decides.
///
/// # Errors
/// A store write error.
pub fn record_tee_delta(
    store: &Store,
    context_id: &ContextId,
    delta_id: &[u8; 32],
    trigger: &TeeTriggerCause,
    now_secs: u64,
) -> eyre::Result<()> {
    record_delta_trigger(store, context_id, delta_id, trigger)?;
    record_tee_firing(store, context_id, trigger, now_secs)
}

/// Record that an attested TEE ran `trigger`, from its signed delta or its
/// signed fired statement.
///
/// A timer whose tick has not begun by `now_secs` (seconds since the Unix
/// epoch, give or take the clock slack a peer is allowed) is not marked. An
/// honest TEE fires only the current tick, so such a firing comes from a TEE
/// that is not honest or from a clock this node disagrees with; marking it
/// would stand every TEE down when that tick comes.
///
/// # Errors
/// A store write error.
pub fn record_tee_firing(
    store: &Store,
    context_id: &ContextId,
    trigger: &TeeTriggerCause,
    now_secs: u64,
) -> eyre::Result<()> {
    let begun = match trigger {
        TeeTriggerCause::Event { .. } => true,
        TeeTriggerCause::Timer { .. } => trigger
            .tick_start_secs()
            .is_some_and(|start| start <= now_secs.saturating_add(TICK_CLOCK_SLACK_SECS)),
    };
    if begun {
        record_tee_fired(store, context_id, &trigger.id(context_id))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::new(std::sync::Arc::new(calimero_store::db::InMemoryDB::owned()))
    }

    fn timer(tick: u64) -> TeeTriggerCause {
        TeeTriggerCause::Timer {
            method: "sweep".to_owned(),
            tick,
            every_secs: 60,
        }
    }

    #[test]
    fn a_tee_delta_keeps_its_trigger_and_marks_it_fired() {
        let store = store();
        let ctx = ContextId::from([1; 32]);
        let trigger = TeeTriggerCause::Event {
            cause: [3; 32],
            method: "resolve".to_owned(),
        };

        assert_eq!(delta_trigger(&store, &ctx, &[9; 32]).unwrap(), None);
        record_tee_delta(&store, &ctx, &[9; 32], &trigger, 0).unwrap();
        assert_eq!(
            delta_trigger(&store, &ctx, &[9; 32]).unwrap(),
            Some(trigger.clone())
        );
        assert!(tee_fired(&store, &ctx, &trigger.id(&ctx)).unwrap());
        assert_eq!(delta_trigger(&store, &ctx, &[8; 32]).unwrap(), None);
    }

    #[test]
    fn a_timer_is_marked_fired_once_its_tick_has_begun() {
        let store = store();
        let ctx = ContextId::from([1; 32]);
        // Tick 10 of a 60s timer begins at 600s.
        record_tee_delta(&store, &ctx, &[1; 32], &timer(10), 600).unwrap();
        assert!(tee_fired(&store, &ctx, &timer(10).id(&ctx)).unwrap());
        // Within the slack a peer's clock is allowed.
        record_tee_delta(&store, &ctx, &[2; 32], &timer(11), 656).unwrap();
        assert!(tee_fired(&store, &ctx, &timer(11).id(&ctx)).unwrap());
    }

    #[test]
    fn a_timer_whose_tick_has_not_begun_is_kept_but_not_marked() {
        // Marking it would stand every TEE down when the tick comes.
        let store = store();
        let ctx = ContextId::from([1; 32]);
        record_tee_delta(&store, &ctx, &[1; 32], &timer(12), 600).unwrap();
        assert!(!tee_fired(&store, &ctx, &timer(12).id(&ctx)).unwrap());
        assert_eq!(
            delta_trigger(&store, &ctx, &[1; 32]).unwrap(),
            Some(timer(12)),
            "the delta must still be servable with the trigger it was signed over"
        );
        // Nor a tick past the end of time.
        record_tee_delta(&store, &ctx, &[2; 32], &timer(u64::MAX), u64::MAX).unwrap();
        assert!(!tee_fired(&store, &ctx, &timer(u64::MAX).id(&ctx)).unwrap());
    }

    #[test]
    fn a_marker_is_scoped_to_its_context() {
        let store = store();
        let (ctx_a, ctx_b) = (ContextId::from([1; 32]), ContextId::from([2; 32]));
        let trigger = timer(3).id(&ctx_a);

        assert!(!tee_fired(&store, &ctx_a, &trigger).unwrap());
        record_tee_fired(&store, &ctx_a, &trigger).unwrap();
        assert!(tee_fired(&store, &ctx_a, &trigger).unwrap());
        assert!(!tee_fired(&store, &ctx_b, &trigger).unwrap());
    }
}
