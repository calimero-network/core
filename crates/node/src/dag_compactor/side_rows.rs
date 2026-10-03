//! The rows kept beside a delta's row, which go when it goes.
//!
//! Two tables hold a row per delta, keyed by `(context, delta id)`, for the
//! catch-up responder to serve with the delta: its events hash
//! (`calimero_context_client::delta_events`, since the row's events are
//! cleared once the handlers ran) and, for a TEE delta, the trigger its
//! envelope signed (`calimero_context_client::tee_trigger`). Both live in
//! `Column::Generic` under a key hashed from the pair, so a context's rows are
//! not a range of their own: each is found from its delta id, and only the
//! table's whole scope can be walked.
//!
//! Every reader reaches one through its delta: the responder reads both only
//! once it has found the delta's row, and an absorbed delta's replay reads its
//! trigger while its absorb record stands. A side row whose delta has neither
//! is read by nothing.

use calimero_context_client::{delta_events, tee_trigger};
use calimero_primitives::context::ContextId;
use calimero_store::key;
use calimero_store::key::{FRAGMENT_SIZE, SCOPE_SIZE};

/// One table of rows kept beside a delta's row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SideTable {
    /// The events hash the delta's id covers.
    EventsHash,
    /// The trigger a TEE delta's envelope signed.
    TeeTrigger,
}

impl SideTable {
    /// Every side table, in a fixed order.
    pub(crate) const ALL: [Self; 2] = [Self::EventsHash, Self::TeeTrigger];

    /// The table's scope in `Column::Generic`.
    pub(crate) fn scope(self) -> [u8; SCOPE_SIZE] {
        match self {
            Self::EventsHash => delta_events::EVENTS_HASH_SCOPE,
            Self::TeeTrigger => tee_trigger::DELTA_TRIGGER_SCOPE,
        }
    }

    /// The key of `delta_id`'s row in this table.
    pub(crate) fn key(self, context_id: &ContextId, delta_id: &[u8; 32]) -> key::Generic {
        match self {
            Self::EventsHash => delta_events::events_hash_key(context_id, delta_id),
            Self::TeeTrigger => tee_trigger::delta_trigger_key(context_id, delta_id),
        }
    }

    /// The first and last key the table can hold.
    pub(crate) fn range(self) -> (key::Generic, key::Generic) {
        (
            key::Generic::new(self.scope(), [0; FRAGMENT_SIZE]),
            key::Generic::new(self.scope(), [u8::MAX; FRAGMENT_SIZE]),
        )
    }
}

/// Most rows one pruned delta takes with it: its own and one per side table.
pub(crate) const ROWS_PER_DELTA: usize = 1 + SideTable::ALL.len();

#[cfg(test)]
mod tests {
    use super::*;

    /// The keys are the ones the recording functions write under, inside the
    /// table's scope: a mismatch would prune nothing and sweep live rows.
    #[test]
    fn a_side_tables_key_is_the_one_its_rows_are_written_under() {
        let store = calimero_store::Store::new(std::sync::Arc::new(
            calimero_store::db::InMemoryDB::owned(),
        ));
        let (context_id, delta) = (ContextId::from([1; 32]), [2; 32]);
        delta_events::record_events_hash(&store, &context_id, &delta, Some(&[3; 32])).unwrap();
        tee_trigger::record_delta_trigger(
            &store,
            &context_id,
            &delta,
            &tee_trigger::TeeTriggerCause::Event {
                cause: [4; 32],
                method: "m".to_owned(),
            },
        )
        .unwrap();
        for table in SideTable::ALL {
            let row = table.key(&context_id, &delta);
            assert_eq!(row.scope(), table.scope());
            assert!(store.handle().has(&row).unwrap(), "{table:?}");
        }
    }
}
