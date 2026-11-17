//! The store's format version: one sealed node row, `format`, which
//! [`crate::Engine::open`] reads and settles before anything else touches the store.
//!
//! | Format | Written by | Rows |
//! | --- | --- | --- |
//! | 0 | every release up to 1.0.0-rc.15 (no `format` row) | every value sealed under the storage key |
//! | 1 | up to 1.0.0-rc.30 | format 0's, plus the `format` row |
//! | 2 | every release since | as they are, on the TD's encrypted data disk; node rows and snapshot rows sealed |
//!
//! This release reads format 2 only. An empty store (new, or created and never written)
//! takes [`STORE_FORMAT`] at once; any other format is refused ([`OtherStore`]), and the
//! node does not start: format 2 was a hard cutover, which reads no earlier store, and a
//! release never reads what a later one wrote.
//!
//! Changing the shape of any row (a column's key layout, a value's encoding, a machine's
//! rows) raises [`STORE_FORMAT`]; with no step to bring old rows forward that is a hard
//! cutover, declared by a breaking PR (`!` in its title), `.github/upgrade-from` and mdma's
//! release compatibility table before it ships (docs: protocol/storage, "Format versions").
//! Snapshots carry rows too: a change to a snapshot row raises [`crate::SNAPSHOT_FORMAT`].

use crate::store::Db;

/// The store format this release writes, and the only one it reads.
pub const STORE_FORMAT: u32 = 2;

/// The node row the format is kept in.
pub(crate) const ROW: &str = "format";

/// What opening found, for the log line [`crate::Engine::open`] writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settled {
    /// The store was empty and is now marked [`STORE_FORMAT`].
    Created,
    /// The store was already at [`STORE_FORMAT`].
    Current,
}

/// The store is in another format than this release reads (`None`: an unmarked one, from
/// before the marker).
#[derive(Debug, thiserror::Error)]
#[error(
    "the store is in format {}, and this release reads format {current} only. {}",
    found.map_or_else(|| "0".to_owned(), |f| f.to_string()),
    if found.is_some_and(|f| f > STORE_FORMAT) {
        "A later release wrote it: a node rolls forward only, so run that release or a later one"
    } else {
        "Format 2 was a hard cutover, which reads no earlier store: start the node on a new data \
         directory (the cluster's data comes back from a peer, or from recovery files)"
    },
    current = STORE_FORMAT
)]
pub struct OtherStore {
    pub found: Option<u32>,
}

/// The store's format: its marker, `None` when it has none (format 0, or empty).
pub(crate) fn marked(db: &Db) -> eyre::Result<Option<u32>> {
    db.get_sealed_meta(ROW).map_err(|e| {
        eyre::eyre!(
            "the store's format row does not open ({e}): the store is sealed under another \
             key, or damaged"
        )
    })
}

/// Read the store's format: mark an empty store [`STORE_FORMAT`], refuse any other.
pub(crate) fn settle(db: &Db) -> eyre::Result<Settled> {
    match marked(db)? {
        Some(STORE_FORMAT) => Ok(Settled::Current),
        None if db.is_empty()? => {
            db.put_sealed_meta(ROW, &STORE_FORMAT)?;
            Ok(Settled::Created)
        }
        found => Err(OtherStore { found }.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::NODE_ROWS;

    const KEY: [u8; 32] = [1; 32];

    #[test]
    fn a_new_store_takes_the_current_format() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path(), &KEY).unwrap();
        assert_eq!(settle(&db).unwrap(), Settled::Created);
        assert_eq!(marked(&db).unwrap(), Some(STORE_FORMAT));
        assert_eq!(settle(&db).unwrap(), Settled::Current, "marked once");
    }

    /// Format 2 is a hard cutover: a store from before (format 1, or unmarked with rows) is
    /// refused, named as such and left as it was; so is one a later release wrote.
    #[test]
    fn any_other_store_format_is_refused_and_left_as_it_was() {
        for (found, says) in [
            (Some(1), "hard cutover"),
            (None, "hard cutover"),
            (Some(STORE_FORMAT + 1), "rolls forward only"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let db = Db::open(dir.path(), &KEY).unwrap();
            db.put_meta(1, "vote", &7_u64).unwrap();
            if let Some(f) = found {
                db.put_sealed_meta(ROW, &f).unwrap();
            }
            let e = settle(&db).unwrap_err();
            assert_eq!(
                e.downcast_ref::<OtherStore>().map(|o| o.found),
                Some(found),
                "{e}"
            );
            assert!(e.to_string().contains(says), "{found:?}: {e}");
            assert_eq!(marked(&db).unwrap(), found, "untouched");
            assert_eq!(db.get_meta::<u64>(1, "vote").unwrap(), Some(7));
        }
    }

    #[test]
    fn a_store_sealed_under_another_key_says_so() {
        let dir = tempfile::tempdir().unwrap();
        settle(&Db::open(dir.path(), &KEY).unwrap()).unwrap();
        let other = Db::open(dir.path(), &[2; 32]).unwrap();
        let e = settle(&other).unwrap_err().to_string();
        assert!(e.contains("sealed under another key"), "{e}");
    }

    /// The marker is a node row: no group, segment listing or blob count sees it.
    #[test]
    fn the_marker_is_no_groups_row() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path(), &KEY).unwrap();
        settle(&db).unwrap();
        let summary = db.summary().unwrap();
        assert_eq!(summary.blobs, 0);
        assert_eq!(
            summary.groups.keys().copied().collect::<Vec<_>>(),
            vec![NODE_ROWS]
        );
        assert!(db.list_segments().unwrap().is_empty());
    }
}
