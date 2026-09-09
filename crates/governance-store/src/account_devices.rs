//! The account namespace's device registry - one row per device of the account,
//! holding the full certificate proof and the applications the root scoped it to.
//!
//! Replicated, unlike the node-local certificate cache it stands beside, which is
//! what lets a device that is not the holder carry a sibling into a namespace it
//! gains: a binding row drops the root signature, so nothing else replicated can
//! rebuild a link.

use calimero_account::{AccountProof, DeviceCert, DeviceId};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ApplicationId;
use calimero_store::key::{
    GroupAccountDevice, GroupAccountDeviceValue, GROUP_ACCOUNT_DEVICE_PREFIX,
};
use calimero_store::Store;
use eyre::Result as EyreResult;

use crate::{collect_keys_with_prefix, AccountBindingRepository, KnownDeviceCert};

/// Reads and writes one account namespace's device registry.
pub struct AccountDeviceRegistry<'a> {
    store: &'a Store,
    namespace: ContextGroupId,
}

impl<'a> AccountDeviceRegistry<'a> {
    /// Bind to the registry of `namespace`.
    #[must_use]
    pub const fn new(store: &'a Store, namespace: ContextGroupId) -> Self {
        Self { store, namespace }
    }

    /// Record `proof` and `applications` at `scope_epoch`.
    ///
    /// `false` means the stored row was already at that epoch or above and was
    /// left alone, which is what makes a re-gossiped op a no-op rather than a
    /// rollback to an older scope.
    ///
    /// # Errors
    /// Propagates the store read or write failure.
    pub fn record(
        &self,
        proof: &AccountProof<DeviceCert>,
        applications: &[ApplicationId],
        scope_epoch: u32,
    ) -> EyreResult<bool> {
        let key = GroupAccountDevice::new(
            self.namespace.to_bytes(),
            *proof.statement.device.as_bytes(),
        );
        let mut handle = self.store.handle();
        if let Some(stored) = handle.get::<GroupAccountDevice>(&key)? {
            if stored.scope_epoch >= scope_epoch {
                return Ok(false);
            }
        }
        handle.put(
            &key,
            &GroupAccountDeviceValue {
                proof: proof.clone(),
                applications: applications.to_vec(),
                scope_epoch,
            },
        )?;
        Ok(true)
    }

    /// The row for `device`: its certificate and scope, and the epoch that wrote
    /// them.
    ///
    /// # Errors
    /// Propagates the store read failure.
    pub fn device(&self, device: DeviceId) -> EyreResult<Option<(KnownDeviceCert, u32)>> {
        let key = GroupAccountDevice::new(self.namespace.to_bytes(), *device.as_bytes());
        Ok(self
            .store
            .handle()
            .get(&key)?
            .map(|value: GroupAccountDeviceValue| {
                (
                    KnownDeviceCert {
                        proof: value.proof,
                        applications: value.applications,
                    },
                    value.scope_epoch,
                )
            }))
    }

    /// Every device of the account that this namespace has not revoked.
    ///
    /// The filter lives here rather than at each caller: a binder checks the
    /// TARGET namespace's tombstones, so a device revoked only here would
    /// otherwise be carried into every namespace gained afterwards.
    ///
    /// # Errors
    /// Propagates the store scan or read failure.
    pub fn devices(&self) -> EyreResult<Vec<KnownDeviceCert>> {
        let namespace = self.namespace.to_bytes();
        let keys = collect_keys_with_prefix(
            self.store,
            GroupAccountDevice::new(namespace, [0u8; 32]),
            GROUP_ACCOUNT_DEVICE_PREFIX,
            |k| k.group_id() == namespace,
        )?;
        let handle = self.store.handle();
        let bindings = AccountBindingRepository::new(self.store);
        let mut certs = Vec::with_capacity(keys.len());
        for key in keys {
            let Some(value) = handle.get::<GroupAccountDevice>(&key)? else {
                continue;
            };
            if bindings.is_revoked(&self.namespace, value.proof.statement.device)? {
                continue;
            }
            certs.push(KnownDeviceCert {
                proof: value.proof,
                applications: value.applications,
            });
        }
        Ok(certs)
    }
}

#[cfg(test)]
mod tests {
    use calimero_account::{AccountProof, DeviceCert, DeviceId, KemPublicKey};
    use calimero_context_config::types::ContextGroupId;
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::identity::PrivateKey;

    use calimero_store::Store;

    use crate::test_fixtures::test_store;
    use crate::{AccountBindingRepository, AccountDeviceRegistry, NodeDeviceRepository};

    const NS: [u8; 32] = [0x4D; 32];

    fn app(seed: u8) -> ApplicationId {
        ApplicationId::from([seed; 32])
    }

    /// A certificate this node's own root signed for `device`.
    ///
    /// `provision_account_root` returns the existing root, so calling this twice
    /// certifies two devices of one account rather than minting two accounts.
    fn proof(store: &Store, device: u8) -> AccountProof<DeviceCert> {
        let root = NodeDeviceRepository::new(store)
            .provision_account_root()
            .expect("this node's root");
        AccountProof {
            genesis: root.genesis(),
            chain: vec![],
            statement: DeviceCert::sign(
                root.signing_key(),
                root.account(),
                DeviceId::from([device; 32]),
                &PrivateKey::from([device; 32]).public_key(),
                &KemPublicKey::from([device ^ 0xFF; 32]),
                0,
                0,
            )
            .expect("the account root signs its own device cert"),
        }
    }

    /// The row a scope statement writes, and the epoch rule that keeps a
    /// re-gossiped op from re-narrowing a device.
    #[test]
    fn a_higher_scope_epoch_supersedes_and_nothing_else_does() {
        let store = test_store();
        let registry = AccountDeviceRegistry::new(&store, ContextGroupId::from(NS));
        let device = DeviceId::from([0x61; 32]);
        let proof = proof(&store, 0x61);

        assert!(registry.record(&proof, &[app(1)], 0).expect("first write"));
        assert!(
            !registry.record(&proof, &[], 0).expect("re-stated"),
            "the same epoch changes nothing, the way a re-gossiped link does"
        );
        let (cert, epoch) = registry
            .device(device)
            .expect("read")
            .expect("the row is there");
        assert_eq!(cert.applications, vec![app(1)]);
        assert_eq!(epoch, 0);

        assert!(registry
            .record(&proof, &[app(1), app(2)], 1)
            .expect("widen"));
        let (cert, epoch) = registry.device(device).expect("read").expect("row");
        assert_eq!(cert.applications, vec![app(1), app(2)]);
        assert_eq!(epoch, 1);
    }

    /// A device revoked here must never be served to a binder: the tombstone a
    /// binder checks is the target namespace's, so one revoked only in the
    /// account namespace would be carried into every namespace gained later.
    #[test]
    fn a_device_revoked_in_this_namespace_is_never_served() {
        let store = test_store();
        let namespace = ContextGroupId::from(NS);
        let registry = AccountDeviceRegistry::new(&store, namespace);
        let kept = proof(&store, 0x61);
        let spent = proof(&store, 0x62);
        assert!(registry.record(&kept, &[], 0).expect("record"));
        assert!(registry.record(&spent, &[], 0).expect("record"));

        AccountBindingRepository::new(&store)
            .apply_revocation(&namespace, spent.statement.device)
            .expect("tombstone");

        let served: Vec<_> = registry
            .devices()
            .expect("read")
            .into_iter()
            .map(|cert| cert.device())
            .collect();
        assert_eq!(served, vec![kept.statement.device]);
        assert!(
            registry
                .device(spent.statement.device)
                .expect("read")
                .is_some(),
            "the row itself survives; only the served set drops it"
        );
    }

    /// One namespace's registry is not another's. The rows are byte-identical in
    /// shape, so only the id bytes in the key keep them apart.
    #[test]
    fn a_registry_serves_only_its_own_namespace() {
        let store = test_store();
        let mine = AccountDeviceRegistry::new(&store, ContextGroupId::from(NS));
        let theirs = AccountDeviceRegistry::new(&store, ContextGroupId::from([0x4E; 32]));
        let proof = proof(&store, 0x61);
        assert!(mine.record(&proof, &[app(1)], 0).expect("record"));

        assert_eq!(mine.devices().expect("read").len(), 1);
        assert!(theirs.devices().expect("read").is_empty());
        assert!(theirs
            .device(proof.statement.device)
            .expect("read")
            .is_none());
    }
}
