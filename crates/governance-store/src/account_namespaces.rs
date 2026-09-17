//! The namespaces one account takes part in, as its own namespace records them.
//!
//! One row per namespace, written by the `AccountNamespaceGained` apply and
//! deleted by `AccountNamespaceLeft`. A device paired or widened later walks it.
//!
//! A second row beside it carries the registry coordinates
//! `AccountNamespaceTargetNamed` records, which is what a device outside the
//! application's scope needs in order to offer to install it.

use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ApplicationId;
use calimero_store::key::{
    GroupAccountNamespace, GroupAccountNamespaceTarget, GroupAccountNamespaceTargetValue,
};
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

    /// Record `namespace` under the application the gain read, replacing one
    /// recorded before: that gain read the metadata more recently. A gain that
    /// read NONE keeps what is recorded, since a scoped device follows on it.
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
                let kept = handle.get(&key)?.flatten();
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
        handle.put(&key, &application)?;
        Ok(())
    }

    /// Record the registry coordinates of a namespace the set already names, so
    /// a device outside its scope can offer to install the application.
    ///
    /// `false` for a namespace the set does not name: the account left it, or
    /// never gained it, and a later gain announces its own coordinates.
    ///
    /// # Errors
    /// Propagates the store read or write failure.
    pub fn name_target(
        &self,
        namespace: ContextGroupId,
        application: ApplicationId,
        package: &str,
        version: &str,
    ) -> EyreResult<bool> {
        if self.contains(namespace)?.is_none() {
            return Ok(false);
        }
        let mut handle = self.store.handle();
        handle.put(
            &self.target_key(namespace),
            &GroupAccountNamespaceTargetValue {
                application,
                package: package.to_owned(),
                version: version.to_owned(),
            },
        )?;
        Ok(true)
    }

    /// The coordinates recorded for `namespace`, if any have been.
    ///
    /// # Errors
    /// Propagates the store read failure.
    pub fn target(
        &self,
        namespace: ContextGroupId,
    ) -> EyreResult<Option<GroupAccountNamespaceTargetValue>> {
        let handle = self.store.handle();
        Ok(handle.get(&self.target_key(namespace))?)
    }

    /// Drop `namespace`, coordinates and all. Absent is not an error: a leave
    /// may reach a device that never folded the gain.
    ///
    /// # Errors
    /// Propagates the store write failure.
    pub fn forget(&self, namespace: ContextGroupId) -> EyreResult<()> {
        let mut handle = self.store.handle();
        let key =
            GroupAccountNamespace::new(self.account_namespace.to_bytes(), namespace.to_bytes());
        handle.delete(&key)?;
        handle.delete(&self.target_key(namespace))?;
        Ok(())
    }

    fn target_key(&self, namespace: ContextGroupId) -> GroupAccountNamespaceTarget {
        GroupAccountNamespaceTarget::new(self.account_namespace.to_bytes(), namespace.to_bytes())
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
        Ok(handle.get(&key)?)
    }

    /// Every namespace in the set, with the application each was recorded under.
    ///
    /// # Errors
    /// Propagates the store scan failure.
    pub fn namespaces(&self) -> EyreResult<Vec<(ContextGroupId, Option<ApplicationId>)>> {
        let account = self.account_namespace.to_bytes();

        // Keys first, then one `get` each, NOT the cursor's `entries()`: other
        // families share these 65 bytes and would fail decoding mid-scan.
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
            out.push((ContextGroupId::from(key.namespace()), value));
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

    /// Coordinates attach only to a namespace the set already names, and leave
    /// with it: a stale pair would offer an install for a namespace the account
    /// is no longer in.
    #[test]
    fn coordinates_follow_the_row_they_belong_to() {
        let store = test_store();
        let set = AccountNamespaceSet::new(&store, ContextGroupId::from(ACCOUNT));

        assert!(
            !set.name_target(ns(0x61), app(0x11), "com.acme.app", "1.0.0")
                .expect("name a target the set does not hold"),
            "a namespace the set never gained must take no coordinates"
        );
        assert_eq!(set.target(ns(0x61)).expect("read"), None);

        set.record(ns(0x61), Some(app(0x11))).expect("record");
        assert!(set
            .name_target(ns(0x61), app(0x11), "com.acme.app", "1.0.0")
            .expect("name the target"));
        let named = set.target(ns(0x61)).expect("read").expect("named");
        assert_eq!(named.application, app(0x11));
        assert_eq!(named.package, "com.acme.app");
        assert_eq!(named.version, "1.0.0");

        // A later release replaces the pair rather than accumulating one.
        assert!(set
            .name_target(ns(0x61), app(0x22), "com.acme.app", "2.0.0")
            .expect("rename the target"));
        assert_eq!(
            set.target(ns(0x61)).expect("read").expect("named").version,
            "2.0.0"
        );

        set.forget(ns(0x61)).expect("forget");
        assert_eq!(
            set.target(ns(0x61)).expect("read"),
            None,
            "leaving a namespace has to drop its coordinates too"
        );
    }

    /// Dropping the one-field wrapper must not have moved a byte: borsh writes a
    /// struct's single field inline, so these rows are on disk already.
    #[test]
    fn dropping_the_wrapper_left_the_bytes_where_they_were() {
        #[derive(borsh::BorshSerialize)]
        struct Wrapped {
            application: Option<ApplicationId>,
        }

        for application in [Some(app(0x11)), None] {
            let wrapped = borsh::to_vec(&Wrapped { application }).expect("encode");
            assert_eq!(borsh::to_vec(&application).expect("encode"), wrapped);
            assert_eq!(
                borsh::from_slice::<Option<ApplicationId>>(&wrapped)
                    .expect("a row written wrapped"),
                application
            );
        }
    }
}
