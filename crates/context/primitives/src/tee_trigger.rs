//! Identity and bookkeeping of a TEE trigger firing.
//!
//! A TEE trigger is one run of an `#[app::tee]` method, fired by the node's TEE
//! scheduler. Several TEE authorities may be able to fire it, and only one
//! should. The elected one fires first; the others wait their turn and fire
//! only if no firing has reached them by then (see the failover notes in
//! `calimero-node`'s `state_delta/events.rs`).
//!
//! For that, every firing has a [`TeeTriggerId`], and the delta a firing
//! produces carries a [`TEE_FIRED_EVENT_KIND`] event naming it. A node that
//! applies such a delta from a TEE authority, or fires the trigger itself,
//! records it with [`record_tee_fired`], and every TEE checks [`tee_fired`]
//! before it fires.

use calimero_primitives::context::ContextId;
use calimero_primitives::identity::domain_hash;
use calimero_store::key::Generic as GenericKey;
use calimero_store::slice::Slice;
use calimero_store::types::GenericData;
use calimero_store::Store;

/// Names one firing of one TEE trigger.
pub type TeeTriggerId = [u8; 32];

/// Kind of the event a TEE-triggered delta carries to say which trigger it
/// fired. Its `data` is the 32-byte [`TeeTriggerId`] and it has no handler.
///
/// Only honoured on a delta signed by a TEE authority: anyone may emit an event
/// of this kind from an ordinary method, and a marker from a member would let
/// them suppress a fallback.
pub const TEE_FIRED_EVENT_KIND: &str = "calimero:tee-fired";

/// Domain separator for [`event_trigger_id`].
const EVENT_TRIGGER_DOMAIN: &[u8] = b"calimero.tee-trigger.event.v1";

/// Domain separator for the fired-marker's store key.
const FIRED_KEY_DOMAIN: &[u8] = b"calimero.tee-trigger.fired.v1";

/// Store scope of the fired markers.
const FIRED_SCOPE: [u8; 16] = *b"calimero-teefire";

/// The trigger a `tee:<method>` handler on the delta `cause` fires.
///
/// One delta may carry several TEE handlers, and each is its own firing, so the
/// method is part of the id.
#[must_use]
pub fn event_trigger_id(cause: &[u8; 32], method: &str) -> TeeTriggerId {
    domain_hash(EVENT_TRIGGER_DOMAIN, &[cause.as_slice(), method.as_bytes()])
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

/// The fired markers a delta's events carry, well-formed or not.
///
/// A marker whose data is not 32 bytes names no trigger and is skipped.
pub fn fired_markers<'a, I>(events: I) -> impl Iterator<Item = TeeTriggerId> + 'a
where
    I: IntoIterator<Item = (&'a str, &'a [u8])>,
    I::IntoIter: 'a,
{
    events
        .into_iter()
        .filter(|(kind, _)| *kind == TEE_FIRED_EVENT_KIND)
        .filter_map(|(_, data)| TeeTriggerId::try_from(data).ok())
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
    fn only_well_formed_markers_are_read() {
        let good = [7u8; 32];
        let events = [
            (TEE_FIRED_EVENT_KIND, good.as_slice()),
            (TEE_FIRED_EVENT_KIND, [1u8; 3].as_slice()),
            ("RollResolved", [9u8; 32].as_slice()),
        ];
        assert_eq!(fired_markers(events).collect::<Vec<_>>(), vec![good]);
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
