use std::collections::BTreeMap;
use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_account::DeviceId;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{
    AccountBindingRepository, AccountDeviceRegistry, NamespaceRepository, NodeDeviceRepository,
};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{AccountDeviceApiEntry, AccountDevicesApiResponse};
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::error;

use crate::admin::handlers::account::no_account_error;
use crate::admin::handlers::identity::get_node_identity::node_identity;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

/// One device before it is known whether it is bound anywhere: the scope the
/// account namespace records for it, and the namespaces a scan of the live
/// bindings has found so far.
struct Draft {
    signing_key: PublicKey,
    applications: Vec<ApplicationId>,
    namespaces: Vec<ContextGroupId>,
}

/// Every device of this node's own account, joined from three sources: the
/// account namespace's registry, the node-local certificate cache, and the live
/// bindings of every namespace this node takes part in.
///
/// `None` when this node holds no account, mirroring `GET /admin-api/identity`,
/// since there is nothing to report on. Reuses that route's account resolution
/// rather than re-deriving it, so the two never disagree about that state.
///
/// # Errors
/// Propagates the underlying store scan or read failure.
fn collect(store: &Store) -> EyreResult<Option<Vec<AccountDeviceApiEntry>>> {
    let Some((account, _root_pk, self_device, ..)) = node_identity(store)? else {
        return Ok(None);
    };

    let devices = NodeDeviceRepository::new(store);
    let bindings = AccountBindingRepository::new(store);

    let mut by_device: BTreeMap<DeviceId, Draft> = BTreeMap::new();
    for cert in devices.device_certs()? {
        by_device.insert(
            cert.device(),
            Draft {
                signing_key: cert.proof.statement.sign_pk,
                applications: cert.applications,
                namespaces: Vec::new(),
            },
        );
    }

    // Replicated, so a device that is not the holder reads every sibling's scope
    // here; the cache still answers for devices the registry drops, revoked ones
    // included.
    if let Some(namespace) = devices.account_namespace()? {
        for cert in AccountDeviceRegistry::new(store, namespace).devices()? {
            let entry = by_device.entry(cert.device()).or_insert_with(|| Draft {
                signing_key: cert.proof.statement.sign_pk,
                applications: Vec::new(),
                namespaces: Vec::new(),
            });
            entry.applications = cert.applications;
        }
    }

    for namespace in NamespaceRepository::new(store).participating_namespaces()? {
        for binding in bindings.devices_of(&namespace, account)? {
            by_device
                .entry(binding.device)
                .or_insert_with(|| Draft {
                    signing_key: binding.sign_pk,
                    // No cached certificate: empty is the same "every
                    // application" this scope already means when a cert says it.
                    applications: Vec::new(),
                    namespaces: Vec::new(),
                })
                .namespaces
                .push(namespace);
        }
    }

    let mut entries = Vec::with_capacity(by_device.len());
    for (device_id, draft) in by_device {
        entries.push(AccountDeviceApiEntry {
            device_id,
            signing_key: draft.signing_key,
            is_self: Some(device_id) == self_device,
            revoked: !devices.revoked_in(device_id)?.is_empty(),
            applications: draft.applications,
            namespaces: draft
                .namespaces
                .into_iter()
                .map(|namespace| hex::encode(namespace.to_bytes()))
                .collect(),
        });
    }
    Ok(Some(entries))
}

/// `GET /admin-api/account/devices`
///
/// The device list a settings UI renders: every device this node's account has
/// certified or bound, with the per-device application scope that decides which
/// apps it may speak for.
pub async fn handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    match collect(&state.store) {
        Ok(Some(devices)) => ApiResponse {
            payload: AccountDevicesApiResponse { devices },
        }
        .into_response(),
        Ok(None) => no_account_error().into_response(),
        Err(err) => {
            error!(error = ?err, "Failed to read this account's devices");
            parse_api_error(err).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_account::{AccountGenesis, AccountProof, DeviceCert, KemPublicKey};
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::{AccountDeviceRegistry, AccountRoot};
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;

    use super::*;

    const NS_A: [u8; 32] = [0xA1; 32];
    const NS_B: [u8; 32] = [0xB2; 32];

    fn ns(bytes: [u8; 32]) -> ContextGroupId {
        ContextGroupId::from(bytes)
    }

    /// A store where this node holds an account root and has taken part in
    /// `NS_A` and `NS_B`. Returns the root so tests can sign further device
    /// certificates under the same account.
    fn seeded_account() -> (Store, AccountRoot) {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let namespaces = NamespaceRepository::new(&store);
        namespaces.note_participation(&ns(NS_A)).expect("join A");
        namespaces.note_participation(&ns(NS_B)).expect("join B");
        let root = NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("mint a root");
        (store, root)
    }

    /// Certify `device` under `root`'s account and remember the cert with
    /// `applications` as its stored scope.
    fn remember_cert(
        store: &Store,
        root: &AccountRoot,
        device: [u8; 32],
        sign_pk: &PublicKey,
        applications: &[ApplicationId],
    ) -> DeviceId {
        let genesis = AccountGenesis::new(root.public_key());
        let device_id = DeviceId::from(device);
        let cert = DeviceCert::sign(
            root.signing_key(),
            genesis.account_id(),
            device_id,
            sign_pk,
            &KemPublicKey::from([0x2B; 32]),
            0,
            0,
        )
        .expect("the account root signs its own device cert");
        NodeDeviceRepository::new(store)
            .remember_device_cert(
                &AccountProof {
                    genesis,
                    chain: vec![],
                    statement: cert,
                },
                applications,
            )
            .expect("remember the cert");
        device_id
    }

    /// Bind `device` live in `namespace`, under `root`'s account.
    fn bind(
        store: &Store,
        root: &AccountRoot,
        namespace: &ContextGroupId,
        device: [u8; 32],
        sign_pk: &PublicKey,
    ) {
        let genesis = AccountGenesis::new(root.public_key());
        let cert = DeviceCert::sign(
            root.signing_key(),
            genesis.account_id(),
            DeviceId::from(device),
            sign_pk,
            &KemPublicKey::from([0x2B; 32]),
            0,
            0,
        )
        .expect("the account root signs its own device cert");
        AccountBindingRepository::new(store)
            .apply_link(namespace, &genesis, &[], &cert)
            .expect("the store write succeeds")
            .expect("the credential binds");
    }

    #[test]
    fn a_narrowed_scope_reports_exactly_those_applications() {
        let (store, root) = seeded_account();
        let app = ApplicationId::from([0x77; 32]);
        let device = remember_cert(
            &store,
            &root,
            [0x11; 32],
            &PrivateKey::from([0x22; 32]).public_key(),
            &[app],
        );

        let entries = collect(&store).expect("collect").expect("has account");

        let entry = entries
            .iter()
            .find(|entry| entry.device_id == device)
            .expect("the certified device is reported");
        assert_eq!(entry.applications, vec![app]);
    }

    #[test]
    fn an_empty_scope_reports_empty_meaning_all() {
        let (store, root) = seeded_account();
        let device = remember_cert(
            &store,
            &root,
            [0x11; 32],
            &PrivateKey::from([0x22; 32]).public_key(),
            &[],
        );

        let entries = collect(&store).expect("collect").expect("has account");

        let entry = entries
            .iter()
            .find(|entry| entry.device_id == device)
            .expect("the certified device is reported");
        assert!(entry.applications.is_empty());
    }

    #[test]
    fn is_self_is_set_only_on_this_nodes_own_device() {
        let (store, root) = seeded_account();
        // This node's own device: minted, then bound the way a namespace join
        // or creation binds the founder's device in production - minting alone
        // writes only the node-local identity row, not a binding.
        let own = NodeDeviceRepository::new(&store)
            .ensure_enrolled(&ns(NS_A))
            .expect("mint this node's device");
        let own_sign_pk = PrivateKey::from([0x77; 32]).public_key();
        bind(
            &store,
            &root,
            &ns(NS_A),
            *own.device().as_bytes(),
            &own_sign_pk,
        );
        // A second device of the same account, known only via a remembered cert.
        let other = remember_cert(
            &store,
            &root,
            [0x33; 32],
            &PrivateKey::from([0x44; 32]).public_key(),
            &[],
        );

        let entries = collect(&store).expect("collect").expect("has account");

        let own_entry = entries
            .iter()
            .find(|entry| entry.device_id == own.device())
            .expect("this node's device is reported");
        assert!(own_entry.is_self);
        let other_entry = entries
            .iter()
            .find(|entry| entry.device_id == other)
            .expect("the other device is reported");
        assert!(!other_entry.is_self);
    }

    #[test]
    fn a_revoked_device_reports_revoked() {
        let (store, root) = seeded_account();
        let device = remember_cert(
            &store,
            &root,
            [0x55; 32],
            &PrivateKey::from([0x66; 32]).public_key(),
            &[],
        );
        AccountBindingRepository::new(&store)
            .apply_revocation(&ns(NS_A), device)
            .expect("tombstone the device");

        let entries = collect(&store).expect("collect").expect("has account");

        let entry = entries
            .iter()
            .find(|entry| entry.device_id == device)
            .expect("the revoked device is still reported");
        assert!(entry.revoked);
    }

    #[test]
    fn a_bound_but_uncached_device_appears_with_its_namespace() {
        let (store, root) = seeded_account();
        let sign_pk = PrivateKey::from([0x88; 32]).public_key();
        bind(&store, &root, &ns(NS_A), [0x99; 32], &sign_pk);

        let entries = collect(&store).expect("collect").expect("has account");

        let entry = entries
            .iter()
            .find(|entry| entry.device_id == DeviceId::from([0x99; 32]))
            .expect("a bound device with no cached cert still appears");
        assert!(entry.applications.is_empty());
        assert_eq!(entry.namespaces, vec![hex::encode(NS_A)]);
    }

    #[test]
    fn a_cached_but_unbound_device_appears_with_no_namespaces() {
        let (store, root) = seeded_account();
        let device = remember_cert(
            &store,
            &root,
            [0xAA; 32],
            &PrivateKey::from([0xBB; 32]).public_key(),
            &[],
        );

        let entries = collect(&store).expect("collect").expect("has account");

        let entry = entries
            .iter()
            .find(|entry| entry.device_id == device)
            .expect("the certified device is reported");
        assert!(entry.namespaces.is_empty());
    }

    #[test]
    fn a_node_holding_no_account_reports_none() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));

        assert!(collect(&store).expect("read").is_none());
    }

    /// The registry is what a device that is not the holder has instead of a
    /// cache, so the scope in the listing comes from there - and wins over a
    /// cached row, which is the older statement by construction.
    #[test]
    fn the_listing_reads_the_scope_from_the_account_namespace_registry() {
        let (store, root) = seeded_account();
        let namespace = NodeDeviceRepository::new(&store)
            .account_namespace()
            .expect("read")
            .expect("a holder names an account namespace");
        let narrow = ApplicationId::from([0x77; 32]);
        let wide = ApplicationId::from([0x88; 32]);
        let sign_pk = PrivateKey::from([0x22; 32]).public_key();
        let device = remember_cert(&store, &root, [0x11; 32], &sign_pk, &[narrow]);

        let genesis = AccountGenesis::new(root.public_key());
        let cert = DeviceCert::sign(
            root.signing_key(),
            genesis.account_id(),
            device,
            &sign_pk,
            &KemPublicKey::from([0x2B; 32]),
            0,
            0,
        )
        .expect("sign");
        assert!(AccountDeviceRegistry::new(&store, namespace)
            .record(
                &AccountProof {
                    genesis,
                    chain: vec![],
                    statement: cert,
                },
                &[narrow, wide],
                1,
            )
            .expect("record"));

        let entries = collect(&store).expect("collect").expect("has account");

        let entry = entries
            .iter()
            .find(|entry| entry.device_id == device)
            .expect("the device is reported");
        assert_eq!(entry.applications, vec![narrow, wide]);
    }

    /// A device the registry knows and this node holds no certificate for is
    /// still listed: that is every sibling, seen from a paired device.
    #[test]
    fn a_device_known_only_to_the_registry_is_listed_with_its_scope() {
        let (store, root) = seeded_account();
        let namespace = NodeDeviceRepository::new(&store)
            .account_namespace()
            .expect("read")
            .expect("a holder names an account namespace");
        let app = ApplicationId::from([0x77; 32]);
        let sign_pk = PrivateKey::from([0x99; 32]).public_key();
        let device = DeviceId::from([0xAB; 32]);
        let genesis = AccountGenesis::new(root.public_key());
        let cert = DeviceCert::sign(
            root.signing_key(),
            genesis.account_id(),
            device,
            &sign_pk,
            &KemPublicKey::from([0x2B; 32]),
            0,
            0,
        )
        .expect("sign");
        assert!(AccountDeviceRegistry::new(&store, namespace)
            .record(
                &AccountProof {
                    genesis,
                    chain: vec![],
                    statement: cert,
                },
                &[app],
                0,
            )
            .expect("record"));

        let entries = collect(&store).expect("collect").expect("has account");

        let entry = entries
            .iter()
            .find(|entry| entry.device_id == device)
            .expect("a device known only to the registry is still a device of the account");
        assert_eq!(entry.applications, vec![app]);
        assert_eq!(entry.signing_key, sign_pk);
    }
}
