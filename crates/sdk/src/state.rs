use borsh::{BorshDeserialize, BorshSerialize};
use calimero_prelude::root_storage_key;

use crate::event::AppEvent;

pub trait AppState: BorshSerialize + BorshDeserialize + AppStateInit {
    type Event<'a>: AppEvent + 'a;

    /// The schema version this binary's identity-gated writes target.
    ///
    /// Owner-driven migration (PR-6c) stamps this value into a stale
    /// identity-gated entry's `Metadata.schema_version` when the owner's
    /// binary next rewrites it, so peers can tell a converted entry from a
    /// not-yet-converted one (Task 6c.2's `entry_needs_convert` predicate
    /// compares the stored version against this target).
    ///
    /// It defaults to `0` — the unversioned value legacy apps carry — so
    /// existing `#[app::state]` types compile unchanged and stamp nothing new.
    /// A v2 binary declares its target by overriding this const in its
    /// `AppState` impl. The runtime reads it type-erased via
    /// [`app::schema_version`](crate::app::schema_version), registered at
    /// install/migrate alongside the event emitter.
    const SCHEMA_VERSION: u32 = 0;
}

pub trait Identity<This = Self> {}

impl<T: AppState> Identity<T> for T {}

#[diagnostic::on_unimplemented(
    message = "(calimero)> no method named `#[app::init]` found for type `{Self}`",
    label = "add an `#[app::init]` method to this type"
)]
pub trait AppStateInit: Sized {
    type Return: Identity<Self>;
}

/// Result of a [`migrate_my_entries`] batch convert.
///
/// `converted` = the caller's identity-gated entries re-written to the target
/// schema this call; `remaining` = the caller's entries still below target
/// after it (a re-write that failed this pass, or a non-empty count that drives
/// the frontend to re-offer "finish"). The generated `migrate_my_entries()`
/// sums these across every declared identity-gated collection.
///
/// [`migrate_my_entries`]: the `#[app::state]`-generated method
#[derive(
    BorshSerialize,
    BorshDeserialize,
    Debug,
    Clone,
    PartialEq,
    Eq,
    crate::serde::Serialize,
    crate::serde::Deserialize,
)]
#[serde(crate = "crate::serde")]
pub struct MigrateMyEntriesSummary {
    /// Entries re-written to the target schema version this call.
    pub converted: u32,
    /// Entries the caller still owns that are below target after this call.
    pub remaining: u32,
}

impl MigrateMyEntriesSummary {
    /// The caller's data is fully migrated: nothing left below target.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.remaining == 0
    }
}

/// Reads the raw bytes of the application's root state from storage.
///
/// This function directly reads the serialized state bytes without deserializing them.
/// It is primarily used during state migrations to access the old state format
/// before transforming it to a new schema.
///
/// The storage layer wraps user data in an `Entry<T>` envelope that appends a
/// 32-byte `Element.id` suffix after the Borsh-serialized user struct. This
/// function strips that suffix so callers receive only the user data portion,
/// matching the layout of the user's `#[app::state]` struct.
///
/// # Returns
///
/// * `Some(Vec<u8>)` - The raw serialized state bytes (user data only) if state exists
/// * `None` - If no state has been stored yet
///
/// # Panics
///
/// Panics (aborting the guest) if the stored root entry is shorter than the
/// 32-byte `Element.id` suffix. That is structurally impossible for a valid
/// `Entry<T>`, so it indicates corrupt storage — an unrecoverable condition,
/// not a "no state yet" (`None`) one. Migration is the only caller and cannot
/// meaningfully proceed from a corrupt root, so this aborts rather than handing
/// back the truncated id bytes as if they were user state.
#[must_use]
pub fn read_raw() -> Option<Vec<u8>> {
    let root_key = root_storage_key();
    let bytes = crate::env::storage_read(&root_key)?;

    // The storage layer stores entities as Entry<T> = borsh(T) ++ borsh(Element.id).
    // Element only serializes its `id: Id` field ([u8; 32]), all other fields are
    // #[borsh(skip)]. Strip this 32-byte suffix so migration code sees only the
    // user's state struct bytes.
    const ELEMENT_SUFFIX_LEN: usize = 32;
    if bytes.len() < ELEMENT_SUFFIX_LEN {
        // A valid Entry<T> always carries the 32-byte Element.id suffix, so a
        // shorter blob is corrupt storage — not user data. Returning these
        // bytes would hand the truncated id back to migration code as if it
        // were state; abort instead (see the `# Panics` section).
        crate::env::panic_str(
            "root state entry is smaller than the 32-byte Element.id suffix; storage is corrupt",
        );
    }
    // When len == ELEMENT_SUFFIX_LEN the slice is empty, yielding `Some(vec![])`
    // — the legitimate "0-byte user state" case.
    Some(bytes[..bytes.len() - ELEMENT_SUFFIX_LEN].to_vec())
}

#[cfg(test)]
mod tests {
    use borsh::BorshDeserialize;

    use super::MigrateMyEntriesSummary;

    #[test]
    fn migrate_summary_roundtrips_and_reports_completion() {
        let done = MigrateMyEntriesSummary {
            converted: 3,
            remaining: 0,
        };
        let bytes = borsh::to_vec(&done).unwrap();
        assert_eq!(
            MigrateMyEntriesSummary::try_from_slice(&bytes).unwrap(),
            done
        );
        assert!(done.is_complete());

        let pending = MigrateMyEntriesSummary {
            converted: 1,
            remaining: 2,
        };
        assert!(!pending.is_complete());
    }

    #[test]
    fn read_raw_strips_element_id_suffix() {
        // User data "hi" (2 bytes) followed by the 32-byte Element.id suffix.
        let mut entry = b"hi".to_vec();
        entry.extend_from_slice(&[0u8; 32]);
        crate::env::__test_seed_root(entry);

        assert_eq!(super::read_raw(), Some(b"hi".to_vec()));
    }

    #[test]
    fn read_raw_returns_empty_for_zero_byte_user_state() {
        // Exactly the suffix length: 0 bytes of user state, stripped to empty.
        crate::env::__test_seed_root(vec![0u8; 32]);

        assert_eq!(super::read_raw(), Some(Vec::new()));
    }

    #[test]
    #[should_panic(expected = "storage is corrupt")]
    fn read_raw_panics_on_truncated_entry() {
        // Fewer than 32 bytes can't carry the Element.id suffix, so the entry
        // is corrupt — `read_raw` must fail loudly rather than hand the id
        // fragment back as if it were user state.
        crate::env::__test_seed_root(vec![0u8; 10]);

        let _ = super::read_raw();
    }
}
