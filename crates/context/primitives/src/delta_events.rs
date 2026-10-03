//! A delta's `events_hash`, kept beside its row so catch-up can serve it after the
//! events are cleared (as [`crate::tee_trigger`] keeps a trigger); none if no events.

use calimero_primitives::context::ContextId;
use calimero_primitives::identity::domain_hash;
use calimero_store::key::Generic as GenericKey;
use calimero_store::slice::Slice;
use calimero_store::types::GenericData;
use calimero_store::Store;

const KEY_DOMAIN: &[u8] = b"calimero.delta.events-hash"; // store-key domain separator

/// Store scope of these rows in `Column::Generic`. Their keys are hashed, so
/// the scope is the only range they can be walked by (DAG compaction does, to
/// sweep rows whose delta row is gone).
pub const EVENTS_HASH_SCOPE: [u8; 16] = *b"calimero-deltevh";

/// The store key of `delta_id`'s events-hash row, for a caller that deletes
/// the row with its delta's.
#[must_use]
pub fn events_hash_key(context_id: &ContextId, delta_id: &[u8; 32]) -> GenericKey {
    GenericKey::new(
        EVENTS_HASH_SCOPE,
        domain_hash(
            KEY_DOMAIN,
            &[
                AsRef::<[u8; 32]>::as_ref(context_id).as_slice(),
                delta_id.as_slice(),
            ],
        ),
    )
}

/// Keep `events_hash` for `delta_id`; a `None` records nothing.
///
/// # Errors
/// A store write error.
pub fn record_events_hash(
    store: &Store,
    context_id: &ContextId,
    delta_id: &[u8; 32],
    events_hash: Option<&[u8; 32]>,
) -> eyre::Result<()> {
    let Some(events_hash) = events_hash else {
        return Ok(());
    };
    store
        .handle()
        .put(
            &events_hash_key(context_id, delta_id),
            &GenericData::from(Slice::from(events_hash.to_vec())),
        )
        .map_err(|err| eyre::eyre!("recording a delta's events hash: {err}"))
}

/// The events hash kept for `delta_id`, or `None` for a delta with no events.
///
/// # Errors
/// A store read error, or a row that is not 32 bytes.
pub fn events_hash(
    store: &Store,
    context_id: &ContextId,
    delta_id: &[u8; 32],
) -> eyre::Result<Option<[u8; 32]>> {
    let handle = store.handle();
    let Some(data) = handle
        .get(&events_hash_key(context_id, delta_id))
        .map_err(|err| eyre::eyre!("reading a delta's events hash: {err}"))?
    else {
        return Ok(None);
    };
    <[u8; 32]>::try_from(data.as_ref())
        .map(Some)
        .map_err(|_| eyre::eyre!("a delta's events hash row is not 32 bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_delta_keeps_its_events_hash_and_one_without_events_has_none() {
        let store = Store::new(std::sync::Arc::new(calimero_store::db::InMemoryDB::owned()));
        let ctx = ContextId::from([1; 32]);

        record_events_hash(&store, &ctx, &[9; 32], Some(&[7; 32])).unwrap();
        record_events_hash(&store, &ctx, &[8; 32], None).unwrap();

        assert_eq!(events_hash(&store, &ctx, &[9; 32]).unwrap(), Some([7; 32]));
        assert_eq!(events_hash(&store, &ctx, &[8; 32]).unwrap(), None);
        assert_eq!(
            events_hash(&store, &ContextId::from([2; 32]), &[9; 32]).unwrap(),
            None
        );
    }
}
