//! The store's format version: one sealed node row, `format`, which
//! [`crate::Engine::open`] reads and settles before anything else touches the store.
//!
//! | Format | Written by | Rows |
//! | --- | --- | --- |
//! | 0 | every release up to 1.0.0-rc.15 (no `format` row) | exactly format 1's |
//! | 1 | every release since | format 0's, plus the `format` row |
//!
//! A store marked newer than [`STORE_FORMAT`] is refused ([`NewerStore`]): a release never
//! reads what a later one wrote, so a node rolls forward only. An older store is brought
//! forward one step at a time through [`MIGRATIONS`], the marker written after each step, so a
//! crash part way resumes at the step it stopped in (every step must be safe to run again on
//! the store it left). An empty store (new, or created and never written) takes
//! [`STORE_FORMAT`] at once.
//!
//! Changing the shape of any row (a column's key layout, a sealed value's encoding, a
//! machine's rows) raises [`STORE_FORMAT`] and appends the step that rewrites the old rows;
//! a step that cannot exist is a hard cutover, declared in `CHANGELOG.md` and mdma's release
//! compatibility table before it ships (docs: protocol/storage, "Format versions"). Snapshots
//! carry rows too: the same change raises the snapshot format ([`crate::SNAPSHOT_FORMAT`]).

use crate::store::{Db, NODE_ROWS};

/// The store format this release writes, and the newest it reads.
pub const STORE_FORMAT: u32 = 1;

/// The node row the format is kept in.
pub(crate) const ROW: &str = "format";

/// One step: brings a store from format `n` to `n + 1`.
type Migration = fn(&Db) -> eyre::Result<()>;

/// `MIGRATIONS[n]` brings a store from format `n` to `n + 1`; one per format below the current.
const MIGRATIONS: [Migration; STORE_FORMAT as usize] = [from_unmarked];

/// Format 0 (no marker, up to 1.0.0-rc.15) holds the rows format 1 does: nothing to rewrite.
fn from_unmarked(_db: &Db) -> eyre::Result<()> {
    Ok(())
}

/// What opening found, for the log line [`crate::Engine::open`] writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settled {
    /// The store was empty and is now marked [`STORE_FORMAT`].
    Created,
    /// The store was already at [`STORE_FORMAT`].
    Current,
    /// The store was at `from` and has been brought to [`STORE_FORMAT`].
    Migrated { from: u32 },
}

/// The store was written by a later release than this one.
#[derive(Debug, thiserror::Error)]
#[error(
    "the store is in format {found}, which a later release wrote; this release reads store \
     formats up to {current}. A node rolls forward only: run the release that wrote it, or a \
     later one",
    current = STORE_FORMAT
)]
pub struct NewerStore {
    pub found: u32,
}

/// The store's format: its marker, `None` when it has none (format 0, or empty).
pub(crate) fn marked(db: &Db) -> eyre::Result<Option<u32>> {
    db.get_meta(NODE_ROWS, ROW).map_err(|e| {
        eyre::eyre!(
            "the store's format row does not open ({e}): the store is sealed under another \
             key, or damaged"
        )
    })
}

/// Read the store's format and bring it to [`STORE_FORMAT`], or refuse it ([`NewerStore`]).
pub(crate) fn settle(db: &Db) -> eyre::Result<Settled> {
    let from = match marked(db)? {
        Some(found) if found > STORE_FORMAT => return Err(NewerStore { found }.into()),
        Some(STORE_FORMAT) => return Ok(Settled::Current),
        Some(older) => older,
        None if db.is_empty()? => {
            db.put_meta(NODE_ROWS, ROW, &STORE_FORMAT)?;
            return Ok(Settled::Created);
        }
        None => 0,
    };
    for (n, step) in (from..).zip(&MIGRATIONS[from as usize..]) {
        step(db).map_err(|e| eyre::eyre!("migrating the store from format {n}: {e}"))?;
        db.put_meta(NODE_ROWS, ROW, &(n + 1))?;
    }
    Ok(Settled::Migrated { from })
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 32] = [1; 32];

    #[test]
    fn a_new_store_takes_the_current_format() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path(), &KEY).unwrap();
        assert_eq!(settle(&db).unwrap(), Settled::Created);
        assert_eq!(marked(&db).unwrap(), Some(STORE_FORMAT));
        assert_eq!(settle(&db).unwrap(), Settled::Current, "marked once");
    }

    /// A store as 1.0.0-rc.15 and every release before the marker left it: rows, no `format`.
    #[test]
    fn a_store_from_before_the_marker_opens_with_its_rows_and_is_marked() {
        let dir = tempfile::tempdir().unwrap();
        {
            let db = Db::open(dir.path(), &KEY).unwrap();
            db.put_meta(1, "vote", &7_u64).unwrap();
            db.put_meta(NODE_ROWS, "rollback", &3_u64).unwrap();
            db.put_blob("bundle", b"wasm").unwrap();
            db.put_segment("aa", b"segment").unwrap();
        }
        let db = Db::open(dir.path(), &KEY).unwrap();
        assert_eq!(marked(&db).unwrap(), None);
        assert_eq!(settle(&db).unwrap(), Settled::Migrated { from: 0 });
        assert_eq!(marked(&db).unwrap(), Some(STORE_FORMAT));
        assert_eq!(db.get_meta::<u64>(1, "vote").unwrap(), Some(7));
        assert_eq!(db.get_meta::<u64>(NODE_ROWS, "rollback").unwrap(), Some(3));
        assert_eq!(
            db.get_blob("bundle").unwrap().as_deref(),
            Some(&b"wasm"[..])
        );
        assert_eq!(db.list_segments().unwrap(), vec!["aa"]);
    }

    #[test]
    fn a_store_from_a_later_release_is_refused_and_left_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path(), &KEY).unwrap();
        db.put_meta(NODE_ROWS, ROW, &(STORE_FORMAT + 1)).unwrap();
        let e = settle(&db).unwrap_err();
        assert_eq!(
            e.downcast_ref::<NewerStore>().map(|n| n.found),
            Some(STORE_FORMAT + 1),
            "{e}"
        );
        let text = e.to_string();
        assert!(
            text.contains(&format!("format {}", STORE_FORMAT + 1))
                && text.contains(&format!("up to {STORE_FORMAT}")),
            "names both formats: {text}"
        );
        assert_eq!(marked(&db).unwrap(), Some(STORE_FORMAT + 1), "untouched");
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
