//! The search dirty log ([`Column::SearchDirty`]).
//!
//! One row per committed execution that changed a search-enabled context's
//! state: key `context(32) ‖ seq u64 BE`, value a borsh [`Record`] holding the
//! context's state root before and after the change and the entity ids it
//! touched. The row is staged into the execution's own transaction
//! ([`stage`]), so it lands in the same RocksDB write batch as the state
//! change: a crash loses both or neither, never just the row.
//!
//! ## Sequence numbers
//!
//! `seq` comes from a per-context counter persisted beside the rows, under the
//! bare `context(32)` key, and bumped in the same batch as the row it numbers.
//! It never reads a clock, so a node whose clock goes backwards across a
//! restart keeps numbering above every row it ever wrote, and the indexer
//! (which skips rows at or below the seq its last commit covered) skips
//! nothing. The counter key is a strict prefix of every row key of its
//! context, so it sorts before them and no row scan ever meets it.
//!
//! ## The root chain
//!
//! A row's `before` is the state root the change was applied on, `after` the
//! root it produced. Rows of one context therefore form a chain, and the
//! index records the root it reflects in every commit. A link that does not
//! match (`before` of a row differs from the root the index reached) means
//! state changed without a row: a snapshot install, a repair sync, a
//! migration, or a run on a node that had search turned off. The indexer then
//! rebuilds from state instead of trusting the log (see
//! [`crate::service::SearchService::index_context`]).
//!
//! The indexer reads rows past its committed seq ([`read_after`]), and trims
//! them ([`trim_through`]) only after the index commit that covers them.

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::utils::prefix_upper_bound;
use calimero_store::db::Column;
use calimero_store::slice::Slice;
use calimero_store::tx::Transaction;
use calimero_store::Store;
use eyre::{eyre, Result as EyreResult};

/// Bytes of a row key: the context id, then the big-endian seq.
const ROW_KEY_LEN: usize = 40;

/// One committed state change, as the execute path describes it.
#[derive(Clone, Copy, Debug)]
pub struct Change<'a> {
    /// The context's state root before the change.
    pub before: [u8; 32],
    /// The state root the change produced.
    pub after: [u8; 32],
    /// The entity ids it touched.
    pub ids: &'a [[u8; 32]],
}

/// A row's stored form. Versioned so a later layout can be told apart from
/// this one; an undecodable row fails the indexer pass loudly rather than
/// being skipped.
#[derive(BorshDeserialize, BorshSerialize)]
enum Record {
    V1 {
        before: [u8; 32],
        after: [u8; 32],
        ids: Vec<[u8; 32]>,
    },
}

/// One dirty row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirtyRow {
    /// Its seq.
    pub seq: u64,
    /// The state root the change was applied on.
    pub before: [u8; 32],
    /// The state root it produced.
    pub after: [u8; 32],
    /// The entity ids it names.
    pub ids: Vec<[u8; 32]>,
}

/// What [`stage`] put into the transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Staged {
    /// The row's seq.
    pub seq: u64,
    /// Key and value bytes of the row (the counter update is 40 more).
    pub bytes: usize,
}

fn counter_key(context: &[u8; 32]) -> Vec<u8> {
    context.to_vec()
}

fn key(context: &[u8; 32], seq: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(ROW_KEY_LEN);
    key.extend_from_slice(context);
    key.extend_from_slice(&seq.to_be_bytes());
    key
}

fn seq_of(key: &[u8]) -> Option<u64> {
    if key.len() != ROW_KEY_LEN {
        return None;
    }
    key[32..].try_into().ok().map(u64::from_be_bytes)
}

/// The highest seq ever staged for `context` (`0` before its first row). It
/// survives the trimming of the rows it numbered.
///
/// # Errors
/// A store failure, or a counter row that is not 8 bytes.
pub fn head(store: &Store, context: &[u8; 32]) -> EyreResult<u64> {
    store
        .raw_get(Column::SearchDirty, &counter_key(context))?
        .map_or(Ok(0), |bytes| {
            let bytes: [u8; 8] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| eyre!("corrupt search dirty-log counter"))?;
            Ok(u64::from_be_bytes(bytes))
        })
}

/// Stage one dirty row for `change` into `tx`, numbered one past the
/// persisted counter, and the counter's bump with it.
///
/// `store` must be the committed store `tx` will be applied to, and nothing
/// else may stage a row for `context` until `tx` lands: the execute path
/// guarantees that by staging under the context's exclusive lock.
///
/// # Errors
/// A store failure reading the counter, or `ids` too long to encode.
pub fn stage(
    tx: &mut Transaction<'_>,
    store: &Store,
    context: &[u8; 32],
    change: Change<'_>,
) -> EyreResult<Staged> {
    let seq = head(store, context)?
        .checked_add(1)
        .ok_or_else(|| eyre!("search dirty-log seq overflow"))?;
    let key = key(context, seq);
    let value = borsh::to_vec(&Record::V1 {
        before: change.before,
        after: change.after,
        ids: change.ids.to_vec(),
    })?;
    let bytes = key.len() + value.len();
    tx.raw_put(
        Column::SearchDirty,
        Slice::from(counter_key(context)),
        Slice::from(seq.to_be_bytes().to_vec()),
    );
    tx.raw_put(Column::SearchDirty, Slice::from(key), Slice::from(value));
    Ok(Staged { seq, bytes })
}

/// Up to `max` rows of `context` with a seq above `after`, oldest first.
///
/// # Errors
/// A store failure, or a row that does not decode.
pub fn read_after(
    store: &Store,
    context: &[u8; 32],
    after: u64,
    max: usize,
) -> EyreResult<Vec<DirtyRow>> {
    let Some(from) = after.checked_add(1) else {
        return Ok(Vec::new());
    };
    let lo = key(context, from);
    let hi = prefix_upper_bound(context);
    store
        .raw_scan(Column::SearchDirty, &lo, &hi, Some(max))?
        .into_iter()
        .map(|(k, v)| {
            let seq = seq_of(&k).ok_or_else(|| eyre!("a search dirty row with a bad key"))?;
            let Record::V1 { before, after, ids } = borsh::from_slice(&v)?;
            Ok(DirtyRow {
                seq,
                before,
                after,
                ids,
            })
        })
        .collect()
}

/// Drop every row of `context` up to and including `seq`. The counter stays.
///
/// # Errors
/// A store failure.
pub fn trim_through(store: &Store, context: &[u8; 32], seq: u64) -> EyreResult<()> {
    let lo = key(context, 0);
    let hi = match seq.checked_add(1) {
        Some(next) => key(context, next),
        None => prefix_upper_bound(context),
    };
    store.raw_delete_range(Column::SearchDirty, &lo, &hi)
}

/// Every context with at least one row: the indexer's backlog after a restart.
///
/// # Errors
/// A store failure.
pub fn contexts_with_rows(store: &Store) -> EyreResult<Vec<[u8; 32]>> {
    let mut out = Vec::new();
    let mut lo = vec![0_u8; 32];
    let hi = vec![0xFF_u8; ROW_KEY_LEN + 1];
    while let Some((k, _)) = store
        .raw_scan(Column::SearchDirty, &lo, &hi, Some(1))?
        .into_iter()
        .next()
    {
        let Some(context) = k.get(..32).and_then(|c| <[u8; 32]>::try_from(c).ok()) else {
            break;
        };
        // A context whose rows were all trimmed keeps only its counter.
        if !read_after(store, &context, 0, 1)?.is_empty() {
            out.push(context);
        }
        lo = prefix_upper_bound(&context);
        if lo.len() != 32 {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_store::db::InMemoryDB;

    use super::*;

    fn stage_one(store: &Store, context: &[u8; 32], before: u8, ids: &[[u8; 32]]) -> u64 {
        let mut tx = Transaction::default();
        let staged = stage(
            &mut tx,
            store,
            context,
            Change {
                before: [before; 32],
                after: [before + 1; 32],
                ids,
            },
        )
        .unwrap();
        store.apply(&tx).unwrap();
        staged.seq
    }

    #[test]
    fn rows_read_back_in_seq_order_and_trim() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let (a, b) = ([1_u8; 32], [2_u8; 32]);
        let mut seqs = Vec::new();
        for i in 0..5_u8 {
            seqs.push(stage_one(&store, &a, i, &[[i; 32]]));
            let _ = stage_one(&store, &b, i, &[[i; 32], [i + 1; 32]]);
        }
        assert_eq!(seqs, [1, 2, 3, 4, 5], "a counter, not a clock");
        let rows = read_after(&store, &a, 0, 100).unwrap();
        assert_eq!(rows.iter().map(|r| r.seq).collect::<Vec<_>>(), seqs);
        assert_eq!(rows[3].ids, vec![[3; 32]]);
        assert_eq!((rows[3].before, rows[3].after), ([3; 32], [4; 32]));
        assert_eq!(read_after(&store, &a, seqs[2], 100).unwrap().len(), 2);
        assert_eq!(head(&store, &a).unwrap(), 5);
        assert_eq!(contexts_with_rows(&store).unwrap(), vec![a, b]);

        trim_through(&store, &a, seqs[3]).unwrap();
        let rows = read_after(&store, &a, 0, 100).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].seq, seqs[4]);
        assert_eq!(
            read_after(&store, &b, 0, 100).unwrap().len(),
            5,
            "b untouched"
        );
    }

    #[test]
    fn the_counter_outlives_the_rows_it_numbered() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let a = [7_u8; 32];
        for i in 0..3 {
            let _ = stage_one(&store, &a, i, &[[i; 32]]);
        }
        trim_through(&store, &a, 3).unwrap();
        assert!(read_after(&store, &a, 0, 100).unwrap().is_empty());
        assert!(
            contexts_with_rows(&store).unwrap().is_empty(),
            "a counter alone is no backlog"
        );
        assert_eq!(head(&store, &a).unwrap(), 3);
        assert_eq!(stage_one(&store, &a, 3, &[]), 4, "numbering resumes above");
        assert!(read_after(&store, &a, 3, 100).unwrap()[0].ids.is_empty());
    }
}
