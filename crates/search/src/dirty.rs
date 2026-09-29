//! The search dirty log ([`Column::SearchDirty`]).
//!
//! One row per committed execution that touched a search-enabled context:
//! key `context(32) ‖ seq u64 BE`, value the borsh `Vec<[u8; 32]>` of the
//! entity ids it changed. The row is staged into the execution's own
//! transaction ([`stage`]), so it lands in the same RocksDB write batch as the
//! state change — a crash can lose both or neither, never just the row.
//!
//! The indexer reads rows past its committed seq ([`read_after`]), and trims
//! them ([`trim_through`]) only after the index commit that covers them.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use calimero_primitives::utils::prefix_upper_bound;
use calimero_store::db::Column;
use calimero_store::slice::Slice;
use calimero_store::tx::Transaction;
use calimero_store::Store;
use eyre::Result as EyreResult;

static LAST_SEQ: AtomicU64 = AtomicU64::new(0);

/// A process-wide, strictly increasing seq, never below wall-clock nanos — so
/// it keeps increasing across a restart without being stored anywhere.
#[must_use]
pub fn next_seq() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
    let mut prev = LAST_SEQ.load(Ordering::Relaxed);
    loop {
        let next = now.max(prev + 1);
        match LAST_SEQ.compare_exchange_weak(prev, next, Ordering::SeqCst, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(actual) => prev = actual,
        }
    }
}

fn key(context: &[u8; 32], seq: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(40);
    key.extend_from_slice(context);
    key.extend_from_slice(&seq.to_be_bytes());
    key
}

/// Stage one dirty row for `ids` into `tx`, returning its seq and its size in
/// bytes (key + value). Nothing is staged for an empty `ids`.
///
/// # Errors
/// `ids` too long to encode.
pub fn stage(
    tx: &mut Transaction<'_>,
    context: &[u8; 32],
    ids: &[[u8; 32]],
) -> EyreResult<Option<(u64, usize)>> {
    if ids.is_empty() {
        return Ok(None);
    }
    let seq = next_seq();
    let key = key(context, seq);
    let value = borsh::to_vec(ids)?;
    let size = key.len() + value.len();
    tx.raw_put(Column::SearchDirty, Slice::from(key), Slice::from(value));
    Ok(Some((seq, size)))
}

/// One dirty row.
#[derive(Debug)]
pub struct DirtyRow {
    /// Its seq.
    pub seq: u64,
    /// The entity ids it names.
    pub ids: Vec<[u8; 32]>,
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
    let lo = key(context, after.saturating_add(1));
    let hi = prefix_upper_bound(context);
    store
        .raw_scan(Column::SearchDirty, &lo, &hi, Some(max))?
        .into_iter()
        .map(|(k, v)| {
            let seq = u64::from_be_bytes(k[32..40].try_into()?);
            Ok(DirtyRow {
                seq,
                ids: borsh::from_slice(&v)?,
            })
        })
        .collect()
}

/// The seq of `context`'s newest row, if it has any.
///
/// # Errors
/// A store failure.
pub fn last_seq(store: &Store, context: &[u8; 32]) -> EyreResult<Option<u64>> {
    let lo = context.to_vec();
    let hi = prefix_upper_bound(context);
    Ok(store
        .raw_last(Column::SearchDirty, &lo, &hi)?
        .and_then(|(k, _)| k.get(32..40).and_then(|s| s.try_into().ok()))
        .map(u64::from_be_bytes))
}

/// Drop every row of `context` up to and including `seq`.
///
/// # Errors
/// A store failure.
pub fn trim_through(store: &Store, context: &[u8; 32], seq: u64) -> EyreResult<()> {
    let lo = key(context, 0);
    let hi = key(context, seq.saturating_add(1));
    store.raw_delete_range(Column::SearchDirty, &lo, &hi)
}

/// Every context with at least one row: the indexer's backlog after a restart.
///
/// # Errors
/// A store failure.
pub fn contexts_with_rows(store: &Store) -> EyreResult<Vec<[u8; 32]>> {
    let mut out = Vec::new();
    let mut lo = vec![0_u8; 32];
    let hi = vec![0xFF_u8; 41];
    while let Some((k, _)) = store
        .raw_scan(Column::SearchDirty, &lo, &hi, Some(1))?
        .into_iter()
        .next()
    {
        let context: [u8; 32] = k[..32].try_into()?;
        out.push(context);
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

    #[test]
    fn rows_read_back_in_seq_order_and_trim() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let (a, b) = ([1_u8; 32], [2_u8; 32]);
        let mut seqs = Vec::new();
        for i in 0..5_u8 {
            let mut tx = Transaction::default();
            seqs.push(stage(&mut tx, &a, &[[i; 32]]).unwrap().unwrap().0);
            let _ = stage(&mut tx, &b, &[[i; 32], [i + 1; 32]])
                .unwrap()
                .unwrap();
            store.apply(&tx).unwrap();
        }
        let rows = read_after(&store, &a, 0, 100).unwrap();
        assert_eq!(rows.iter().map(|r| r.seq).collect::<Vec<_>>(), seqs);
        assert_eq!(rows[3].ids, vec![[3; 32]]);
        assert_eq!(read_after(&store, &a, seqs[2], 100).unwrap().len(), 2);
        assert_eq!(last_seq(&store, &a).unwrap(), Some(seqs[4]));
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
        assert!(stage(&mut Transaction::default(), &a, &[])
            .unwrap()
            .is_none());
    }
}
