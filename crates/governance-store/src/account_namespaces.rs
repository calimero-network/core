//! The namespaces one account takes part in, as its own namespace records them.
//!
//! One row per namespace, written by the `AccountNamespaceGained` apply and
//! deleted by `AccountNamespaceLeft`. A device paired after the account gained
//! its namespaces, or widened later, walks this to find what it may now follow.

use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ApplicationId;
use calimero_store::key::{GroupAccountNamespace, GroupAccountNamespaceValue};
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::debug;

use crate::collect_keys_with_prefix;

/// Reads and writes one account namespace's set of namespaces.
pub struct AccountNamespaceSet<'a> {
    store: &'a Store,
    account_namespace: ContextGroupId,
}

impl<'a> AccountNamespaceSet<'a> {
    /// Bind to one account namespace's rows.
    #[must_use]
    pub const fn new(store: &'a Store, account_namespace: ContextGroupId) -> Self {
        Self {
            store,
            account_namespace,
        }
    }

    /// Record `namespace` under the application the gain read, replacing an
    /// application recorded before: that gain read the metadata more recently.
    ///
    /// A gain that read NO application keeps the one already recorded. It knows
    /// less than the set does, and a scoped device follows on that value alone.
    ///
    /// # Errors
    /// Propagates the store read or write failure.
    pub fn record(
        &self,
        namespace: ContextGroupId,
        application: Option<ApplicationId>,
    ) -> EyreResult<()> {
        let mut handle = self.store.handle();
        let key =
            GroupAccountNamespace::new(self.account_namespace.to_bytes(), namespace.to_bytes());
        let application = match application {
            Some(read) => Some(read),
            None => {
                let kept = handle
                    .get(&key)?
                    .and_then(|value: GroupAccountNamespaceValue| value.application);
                if let Some(kept) = kept {
                    debug!(
                        ?namespace,
                        ?kept,
                        "a gain read no target; kept the recorded one"
                    );
                }
                kept
            }
        };
        handle.put(&key, &GroupAccountNamespaceValue { application })?;
        Ok(())
    }

    /// Drop `namespace`. Absent is not an error: a leave may reach a device that
    /// never folded the gain.
    ///
    /// # Errors
    /// Propagates the store write failure.
    pub fn forget(&self, namespace: ContextGroupId) -> EyreResult<()> {
        let mut handle = self.store.handle();
        let key =
            GroupAccountNamespace::new(self.account_namespace.to_bytes(), namespace.to_bytes());
        handle.delete(&key)?;
        Ok(())
    }

    /// The application recorded for `namespace`, or `None` when the set does not
    /// name it at all.
    ///
    /// # Errors
    /// Propagates the store read failure.
    pub fn contains(&self, namespace: ContextGroupId) -> EyreResult<Option<Option<ApplicationId>>> {
        let handle = self.store.handle();
        let key =
            GroupAccountNamespace::new(self.account_namespace.to_bytes(), namespace.to_bytes());
        Ok(handle
            .get(&key)?
            .map(|value: GroupAccountNamespaceValue| value.application))
    }

    /// Every namespace in the set, with the application each was recorded under.
    ///
    /// # Errors
    /// Propagates the store scan failure.
    pub fn namespaces(&self) -> EyreResult<Vec<(ContextGroupId, Option<ApplicationId>)>> {
        let account = self.account_namespace.to_bytes();

        // Keys first, then one `get` each - deliberately, and NOT the cursor's
        // `entries()`: other families in this column carry the same 65 bytes, so
        // the typed value iterator would fail decoding a neighbour's row mid-scan.
        let keys = collect_keys_with_prefix(
            self.store,
            GroupAccountNamespace::new(account, [0u8; 32]),
            calimero_store::key::GROUP_ACCOUNT_NAMESPACE_PREFIX,
            |k| k.account_namespace() == account,
        )?;

        let handle = self.store.handle();
        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            let Some(value) = handle.get(&key)? else {
                continue;
            };
            out.push((ContextGroupId::from(key.namespace()), value.application));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use calimero_context_config::types::ContextGroupId;
    use calimero_primitives::application::ApplicationId;

    use crate::test_fixtures::test_store;
    use crate::AccountNamespaceSet;

    const ACCOUNT: [u8; 32] = [0x4F; 32];
    const OTHER_ACCOUNT: [u8; 32] = [0x50; 32];

    fn ns(seed: u8) -> ContextGroupId {
        ContextGroupId::from([seed; 32])
    }

    fn app(seed: u8) -> ApplicationId {
        ApplicationId::from([seed; 32])
    }

    /// The set one account's namespace records, and the three answers a reader
    /// needs from it: absent, present with a target, present with none.
    #[test]
    fn the_set_records_forgets_and_scans_one_account() {
        let store = test_store();
        let set = AccountNamespaceSet::new(&store, ContextGroupId::from(ACCOUNT));

        assert_eq!(set.contains(ns(0x61)).expect("read"), None);

        set.record(ns(0x61), Some(app(0x11))).expect("record");
        set.record(ns(0x62), None)
            .expect("record a target-less one");
        // A namespace of a DIFFERENT account, to prove the scan is bounded.
        AccountNamespaceSet::new(&store, ContextGroupId::from(OTHER_ACCOUNT))
            .record(ns(0x63), Some(app(0x22)))
            .expect("record elsewhere");

        assert_eq!(set.contains(ns(0x61)).expect("read"), Some(Some(app(0x11))));
        assert_eq!(set.contains(ns(0x62)).expect("read"), Some(None));

        let mut listed = set.namespaces().expect("scan");
        listed.sort_by_key(|(namespace, _)| namespace.to_bytes());
        assert_eq!(listed, vec![(ns(0x61), Some(app(0x11))), (ns(0x62), None)]);

        // A later gain knows the target better than an earlier one.
        set.record(ns(0x62), Some(app(0x33))).expect("re-record");
        assert_eq!(set.contains(ns(0x62)).expect("read"), Some(Some(app(0x33))));

        set.forget(ns(0x62)).expect("forget");
        set.forget(ns(0x62))
            .expect("forgetting twice is not an error");
        assert_eq!(set.contains(ns(0x62)).expect("read"), None);
        assert_eq!(
            set.namespaces().expect("scan"),
            vec![(ns(0x61), Some(app(0x11)))]
        );
    }

    /// A gain that had not folded the namespace's target must not un-know what
    /// the set already records: a scoped device follows on that value alone.
    #[test]
    fn a_gain_that_read_no_target_keeps_the_recorded_one() {
        let store = test_store();
        let set = AccountNamespaceSet::new(&store, ContextGroupId::from(ACCOUNT));

        set.record(ns(0x61), Some(app(0x11))).expect("record");
        set.record(ns(0x61), None)
            .expect("re-record, target unread");

        assert_eq!(set.contains(ns(0x61)).expect("read"), Some(Some(app(0x11))));
    }
}
