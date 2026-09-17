//! The account namespace's device registry: one row per device, holding the full
//! certificate proof and the scope the root signed. Replicated, unlike the
//! node-local cache, so any device can carry a sibling into a namespace it gains.

use core::cmp::Ordering;

use calimero_account::{AccountProof, DeviceCert, DeviceId, DeviceScope};
use calimero_context_config::types::ContextGroupId;
use calimero_store::key::{
    GroupAccountDevice, GroupAccountDeviceValue, GROUP_ACCOUNT_DEVICE_PREFIX,
};
use calimero_store::Store;
use eyre::Result as EyreResult;

use crate::{collect_keys_with_prefix, AccountBindingRepository, KnownDeviceCert};

/// Does `offered` replace `stored`? A higher epoch wins; at an equal one the
/// lower signature does, so two replicas folding a race in opposite orders keep
/// the same statement. A re-stated row never supersedes itself.
fn supersedes(stored: &DeviceScope, offered: &DeviceScope) -> bool {
    match offered.scope_epoch.cmp(&stored.scope_epoch) {
        Ordering::Greater => true,
        Ordering::Less => false,
        Ordering::Equal => offered.signature < stored.signature,
    }
}

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

    /// Record `proof` under the scope `scope` states. `false` means the stored
    /// row already stands, which makes a re-gossiped op a no-op.
    ///
    /// # Errors
    /// Propagates the store read or write failure.
    pub fn record(
        &self,
        proof: &AccountProof<DeviceCert>,
        scope: &AccountProof<DeviceScope>,
    ) -> EyreResult<bool> {
        let key = GroupAccountDevice::new(
            self.namespace.to_bytes(),
            *proof.statement.device.as_bytes(),
        );
        let mut handle = self.store.handle();
        if let Some(stored) = handle.get::<GroupAccountDevice>(&key)? {
            if !supersedes(&stored.scope.statement, &scope.statement) {
                return Ok(false);
            }
        }
        handle.put(
            &key,
            &GroupAccountDeviceValue {
                proof: proof.clone(),
                scope: scope.clone(),
            },
        )?;
        Ok(true)
    }

    /// The row for `device`: its certificate and the scope in force for it.
    ///
    /// # Errors
    /// Propagates the store read failure.
    pub fn device(&self, device: DeviceId) -> EyreResult<Option<KnownDeviceCert>> {
        let key = GroupAccountDevice::new(self.namespace.to_bytes(), *device.as_bytes());
        Ok(self
            .store
            .handle()
            .get(&key)?
            .map(|value: GroupAccountDeviceValue| KnownDeviceCert {
                proof: value.proof,
                scope: value.scope,
            }))
    }

    /// Every device row in this namespace's registry, revoked ones included.
    ///
    /// The device listing needs the revoked rows - `revoked: true` is what a
    /// settings UI renders - while a binder must never see one; see [`Self::devices`].
    ///
    /// # Errors
    /// Propagates the store scan or read failure.
    pub fn all_devices(&self) -> EyreResult<Vec<KnownDeviceCert>> {
        let namespace = self.namespace.to_bytes();
        let keys = collect_keys_with_prefix(
            self.store,
            GroupAccountDevice::new(namespace, [0u8; 32]),
            GROUP_ACCOUNT_DEVICE_PREFIX,
            |k| k.group_id() == namespace,
        )?;
        let handle = self.store.handle();
        let mut certs = Vec::with_capacity(keys.len());
        for key in keys {
            let Some(value) = handle.get::<GroupAccountDevice>(&key)? else {
                continue;
            };
            certs.push(KnownDeviceCert {
                proof: value.proof,
                scope: value.scope,
            });
        }
        Ok(certs)
    }

    /// Every device of the account that this namespace has not revoked.
    ///
    /// Filtered here, not at each caller: a binder checks the TARGET namespace's
    /// tombstones, so one revoked only here would be carried on regardless.
    ///
    /// # Errors
    /// Propagates the store scan or read failure.
    pub fn devices(&self) -> EyreResult<Vec<KnownDeviceCert>> {
        let bindings = AccountBindingRepository::new(self.store);
        let mut kept = Vec::new();
        for cert in self.all_devices()? {
            if !bindings.is_revoked(&self.namespace, cert.device())? {
                kept.push(cert);
            }
        }
        Ok(kept)
    }
}

#[cfg(test)]
mod tests {
    use calimero_account::{AccountProof, DeviceCert, DeviceId, DeviceScope, KemPublicKey};
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

    /// A certificate this node's own root signed. The root is reused, so two
    /// calls certify two devices of one account.
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

    /// The root-signed scope a row is recorded under.
    fn scope(
        store: &Store,
        device: u8,
        applications: &[ApplicationId],
        scope_epoch: u32,
    ) -> AccountProof<DeviceScope> {
        let root = NodeDeviceRepository::new(store)
            .provision_account_root()
            .expect("this node's root");
        AccountProof {
            genesis: root.genesis(),
            chain: vec![],
            statement: DeviceScope::sign(
                root.signing_key(),
                root.account(),
                DeviceId::from([device; 32]),
                applications.to_vec(),
                scope_epoch,
                0,
            )
            .expect("the account root signs its own device scope"),
        }
    }

    /// The epoch rule: only a higher one supersedes.
    #[test]
    fn a_higher_scope_epoch_supersedes_and_nothing_else_does() {
        let store = test_store();
        let registry = AccountDeviceRegistry::new(&store, ContextGroupId::from(NS));
        let device = DeviceId::from([0x61; 32]);
        let proof = proof(&store, 0x61);

        assert!(registry
            .record(&proof, &scope(&store, 0x61, &[app(1)], 0))
            .expect("first write"));
        assert!(
            !registry
                .record(&proof, &scope(&store, 0x61, &[app(1)], 0))
                .expect("re-stated"),
            "re-stating the stored statement changes nothing, as a re-gossiped op does"
        );
        let cert = registry
            .device(device)
            .expect("read")
            .expect("the row is there");
        assert_eq!(cert.applications(), [app(1)]);
        assert_eq!(cert.scope.statement.scope_epoch, 0);

        assert!(registry
            .record(&proof, &scope(&store, 0x61, &[app(1), app(2)], 1))
            .expect("widen"));
        let cert = registry.device(device).expect("read").expect("row");
        assert_eq!(cert.applications(), [app(1), app(2)]);
        assert_eq!(cert.scope.statement.scope_epoch, 1);

        assert!(
            !registry
                .record(&proof, &scope(&store, 0x61, &[], 0))
                .expect("stale epoch"),
            "an epoch below the stored one never supersedes it"
        );
        let cert = registry.device(device).expect("read").expect("row");
        assert_eq!(cert.scope.statement.scope_epoch, 1);
    }

    /// Two statements the same root signed at one epoch - what two racing scope
    /// replacements produce. Replicas fold them in either order, so the survivor
    /// may not depend on which arrived first.
    #[test]
    fn two_scopes_at_one_epoch_converge_whichever_order_they_arrive_in() {
        let store = test_store();
        let device = DeviceId::from([0x61; 32]);
        let proof = proof(&store, 0x61);
        let rivals = [
            scope(&store, 0x61, &[app(1)], 4),
            scope(&store, 0x61, &[app(2)], 4),
        ];

        // One registry per arrival order, so the two folds cannot see each other.
        let mut survivors = Vec::new();
        for namespace in [ContextGroupId::from(NS), ContextGroupId::from([0x4E; 32])] {
            let registry = AccountDeviceRegistry::new(&store, namespace);
            let mut order = rivals.clone();
            if namespace != ContextGroupId::from(NS) {
                order.reverse();
            }
            for rival in &order {
                let _recorded = registry.record(&proof, rival).expect("record");
            }
            survivors.push(
                registry
                    .device(device)
                    .expect("read")
                    .expect("row")
                    .scope
                    .statement,
            );
        }

        assert_eq!(
            survivors[0], survivors[1],
            "the same pair of statements has to leave the same row whichever order it lands in"
        );
    }

    /// A device revoked here is never served, since a binder checks the target
    /// namespace's tombstones rather than this one's.
    #[test]
    fn a_device_revoked_in_this_namespace_is_never_served() {
        let store = test_store();
        let namespace = ContextGroupId::from(NS);
        let registry = AccountDeviceRegistry::new(&store, namespace);
        let kept = proof(&store, 0x61);
        let spent = proof(&store, 0x62);
        assert!(registry
            .record(&kept, &scope(&store, 0x61, &[], 0))
            .expect("record"));
        assert!(registry
            .record(&spent, &scope(&store, 0x62, &[], 0))
            .expect("record"));

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

    /// One namespace's registry is not another's; only the key keeps them apart.
    #[test]
    fn a_registry_serves_only_its_own_namespace() {
        let store = test_store();
        let mine = AccountDeviceRegistry::new(&store, ContextGroupId::from(NS));
        let theirs = AccountDeviceRegistry::new(&store, ContextGroupId::from([0x4E; 32]));
        let mine_proof = proof(&store, 0x61);
        let their_proof = proof(&store, 0x62);
        assert!(mine
            .record(&mine_proof, &scope(&store, 0x61, &[app(1)], 0))
            .expect("record"));
        // So the scan walks into a foreign row and the key filter has to reject it.
        assert!(theirs
            .record(&their_proof, &scope(&store, 0x62, &[app(2)], 0))
            .expect("record"));

        let served: Vec<_> = mine
            .devices()
            .expect("read")
            .into_iter()
            .map(|cert| cert.device())
            .collect();
        assert_eq!(served, vec![mine_proof.statement.device]);
        assert!(mine
            .device(their_proof.statement.device)
            .expect("read")
            .is_none());
        assert!(theirs
            .device(mine_proof.statement.device)
            .expect("read")
            .is_none());
    }
}
