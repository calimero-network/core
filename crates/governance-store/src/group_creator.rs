//! The account that created a subgroup, recorded once at its `GroupCreated`.
//!
//! The creator is bound into the subgroup's id
//! (`calimero_account::created_subgroup_id(creator, parent, restricted, salt)`),
//! so it never changes - unlike the group's admin and owner, which later ops
//! move, and which for a Restricted subgroup are moved by ops sealed under its
//! own key. That is what lets every namespace member decide an opening flip: only
//! the creator may open a Restricted subgroup, and the creator is read from here,
//! not from the subgroup's sealed history (core#4511, #4522).
//!
//! A local `Generic` row, like the namespace founding record: every node that
//! applies the subgroup's `GroupCreated` - which is readable by every namespace
//! member - writes the same value. It outlives the group: an id recreated later
//! derives from the same creator.

use calimero_account::AccountId;
use calimero_context_config::types::ContextGroupId;
use calimero_store::key::Generic as GenericKey;
use calimero_store::slice::Slice;
use calimero_store::types::GenericData;
use calimero_store::Store;
use eyre::Result as EyreResult;

/// 16-byte `Generic` scope of the rows: one per subgroup, keyed by its id.
const SCOPE: [u8; 16] = *b"calimero-gcreatr";

pub struct GroupCreatorRepository<'a> {
    store: &'a Store,
}

impl<'a> GroupCreatorRepository<'a> {
    #[must_use]
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Record `creator` for `group`. A second record for the same group is a
    /// no-op: the id commits to the creator, so a re-applied `GroupCreated`
    /// names the same account.
    pub(crate) fn record(&self, group: &ContextGroupId, creator: &AccountId) -> EyreResult<()> {
        if self.creator(group)?.is_some() {
            return Ok(());
        }
        let data = GenericData::from(Slice::from(creator.as_bytes().to_vec()));
        self.store.handle().put(&Self::key(group), &data)?;
        Ok(())
    }

    /// The account that created `group`, or `None` when this node has not
    /// applied its `GroupCreated` (or it is a namespace root, which no
    /// `GroupCreated` makes).
    pub fn creator(&self, group: &ContextGroupId) -> EyreResult<Option<AccountId>> {
        let handle = self.store.handle();
        let Some(data) = handle.get(&Self::key(group))? else {
            return Ok(None);
        };
        let bytes: &[u8] = data.as_ref();
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| eyre::eyre!("group creator row is not 32 bytes"))?;
        Ok(Some(AccountId::from(bytes)))
    }

    fn key(group: &ContextGroupId) -> GenericKey {
        GenericKey::new(SCOPE, group.to_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::test_store;

    #[test]
    fn the_first_creator_recorded_stays() {
        let store = test_store();
        let repo = GroupCreatorRepository::new(&store);
        let group = ContextGroupId::from([1; 32]);
        assert_eq!(repo.creator(&group).expect("read"), None);

        repo.record(&group, &AccountId::from([7; 32]))
            .expect("record");
        repo.record(&group, &AccountId::from([8; 32]))
            .expect("a second record is a no-op");
        assert_eq!(
            repo.creator(&group).expect("read"),
            Some(AccountId::from([7; 32]))
        );
    }
}
