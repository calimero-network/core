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

/// The trigger a `tee:<method>` handler on the delta `cause` fires.
///
/// One delta may carry several TEE handlers, and each is its own firing, so the
/// method is part of the id.
#[must_use]
pub fn event_trigger_id(cause: &[u8; 32], method: &str) -> TeeTriggerId {
    TeeTriggerCause::Event {
        cause: *cause,
        method: method.to_owned(),
    }
    // An event trigger's id does not depend on the context.
    .id(&ContextId::from([0; 32]))
}

/// The trigger an `#[app::tee(every = "..")]` method `method` fires for its
/// `tick`th period in `context_id`.
#[must_use]
pub fn timer_trigger_id(context_id: &ContextId, method: &str, tick: u64) -> TeeTriggerId {
    TeeTriggerCause::Timer {
        method: method.to_owned(),
        tick,
    }
    .id(context_id)
}

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
/// `trigger`, has been accepted: its trigger has fired, and the delta is kept
/// with its trigger so it can be served.
///
/// # Errors
/// A store write error.
pub fn record_tee_delta(
    store: &Store,
    context_id: &ContextId,
    delta_id: &[u8; 32],
    trigger: &TeeTriggerCause,
) -> eyre::Result<()> {
    record_delta_trigger(store, context_id, delta_id, trigger)?;
    record_tee_fired(store, context_id, &trigger.id(context_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trigger_id_names_both_the_cause_and_the_method() {
        let a = event_trigger_id(&[1; 32], "resolve");
        assert_eq!(a, event_trigger_id(&[1; 32], "resolve"));
        assert_ne!(a, event_trigger_id(&[2; 32], "resolve"));
        assert_ne!(a, event_trigger_id(&[1; 32], "deal"));
    }

    #[test]
    fn a_timer_id_names_the_context_the_method_and_the_tick() {
        let ctx = ContextId::from([1; 32]);
        let a = timer_trigger_id(&ctx, "tick", 7);
        assert_eq!(a, timer_trigger_id(&ctx, "tick", 7));
        assert_ne!(a, timer_trigger_id(&ctx, "tick", 8));
        assert_ne!(a, timer_trigger_id(&ctx, "sweep", 7));
        assert_ne!(a, timer_trigger_id(&ContextId::from([2; 32]), "tick", 7));
    }

    #[test]
    fn a_tee_delta_keeps_its_trigger_and_marks_it_fired() {
        let store = Store::new(std::sync::Arc::new(calimero_store::db::InMemoryDB::owned()));
        let ctx = ContextId::from([1; 32]);
        let trigger = TeeTriggerCause::Event {
            cause: [3; 32],
            method: "resolve".to_owned(),
        };

        assert_eq!(delta_trigger(&store, &ctx, &[9; 32]).unwrap(), None);
        record_tee_delta(&store, &ctx, &[9; 32], &trigger).unwrap();
        assert_eq!(
            delta_trigger(&store, &ctx, &[9; 32]).unwrap(),
            Some(trigger.clone())
        );
        assert!(tee_fired(&store, &ctx, &trigger.id(&ctx)).unwrap());
        assert_eq!(delta_trigger(&store, &ctx, &[8; 32]).unwrap(), None);
    }

    #[test]
    fn a_marker_is_scoped_to_its_context() {
        let store = Store::new(std::sync::Arc::new(calimero_store::db::InMemoryDB::owned()));
        let (ctx_a, ctx_b) = (ContextId::from([1; 32]), ContextId::from([2; 32]));
        let trigger = event_trigger_id(&[3; 32], "resolve");

        assert!(!tee_fired(&store, &ctx_a, &trigger).unwrap());
        record_tee_fired(&store, &ctx_a, &trigger).unwrap();
        assert!(tee_fired(&store, &ctx_a, &trigger).unwrap());
        assert!(!tee_fired(&store, &ctx_b, &trigger).unwrap());
    }
}
