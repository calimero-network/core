use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_account::{AccountId, DeviceId, KemPublicKey};
use calimero_governance_store::NodeDeviceRepository;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{NodeIdentityApiResponse, NodeIdentityApiResponseData};
use calimero_store::Store;
use eyre::Result as EyreResult;
use reqwest::StatusCode;
use tracing::error;

use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

/// Who this node speaks as: the account, the root that addresses it, and the
/// device it presents - `None` for a node that holds neither.
///
/// **The device row first, and the account root only as a fallback.** The row
/// names the account this node speaks for, and it is the only place that account
/// is written down: a PAIRED node adopted an account rooted at another node's
/// key, so it holds no root of its own - pairing mints a device
/// (`ensure_enrolled_into`) without ever minting a root. Starting from the root
/// therefore answered 404 for exactly the node whose identity a caller most
/// needs, and would have answered with the WRONG account had the node happened to
/// hold a root as well: a locally derived id no row in the group is keyed by.
///
/// **A revoked device is not a device.** One row serves every namespace while a
/// tombstone is per-namespace, so a device revoked anywhere is one whose id is
/// spent: enrolling the machine again mints a fresh one, which is why the
/// enrolment slot is already released on that basis. Reporting the old id would
/// name a device no peer will admit and - for a paired node - an adopted account
/// it no longer speaks for. So this falls back to the node's own root, which is
/// the only account it can still honestly claim.
pub(crate) type NodeIdentityParts = (
    AccountId,
    PublicKey,
    Option<DeviceId>,
    Option<KemPublicKey>,
    bool,
);

pub(crate) fn node_identity(store: &Store) -> EyreResult<Option<NodeIdentityParts>> {
    let devices = NodeDeviceRepository::new(store);
    if let Some(held) = devices.unrevoked_device()? {
        // Through the crate's own accessor, not by reaching into the secret:
        // `kem_public_key` is what certificates already publish, so the value
        // reported here cannot drift from the one that gets certified.
        //
        // A paired node adopted an account rooted on another machine and may hold a
        // root of its own besides, so the account has to match rather than a root
        // merely existing.
        let holds_root = devices
            .account_root()?
            .is_some_and(|root| root.account() == held.account);
        return Ok(Some((
            held.account,
            held.genesis.root_sign_pk,
            Some(held.device()),
            Some(held.kem_public_key()),
            holds_root,
        )));
    }
    // No usable device: this node speaks only for itself, and a node with neither
    // a usable device nor a root has taken part in nothing at all. No device also
    // means no agreement key, which is the device's, and the account reported is its
    // own root's - so it holds that root by construction.
    Ok(devices.account_root()?.map(|root| {
        (
            root.account(),
            root.genesis().root_sign_pk,
            None,
            None,
            true,
        )
    }))
}

/// Who this node is: the account it writes as, the device it is, and the key it
/// signs with.
///
/// Takes no namespace, and that is the whole point of it. Each of the three is
/// node-level, so a namespace in the path could only ever be decoration — the
/// endpoints it replaces took one and returned the same answer regardless, which
/// read as though the answer varied.
pub async fn handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    let store = state.ctx_client.datastore();

    // A failed READ is not a missing row, and reporting them alike would tell an
    // operator "not enrolled" when the truth is "could not look".
    let held = match node_identity(store) {
        Ok(held) => held,
        Err(err) => {
            error!(error = ?err, "Failed to read this node's identity");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to read this node's identity".to_owned(),
            }
            .into_response();
        }
    };

    let Some((account, account_root_pk, device, agreement_key, holds_account_root)) = held else {
        return ApiError {
            status_code: StatusCode::NOT_FOUND,
            message: "this node holds neither a usable device nor an account root yet; \
                      both are minted the first time it takes part in a namespace"
                .to_owned(),
        }
        .into_response();
    };
    let device = device.map(|device| hex::encode(device.as_bytes()));
    let agreement_key = agreement_key.map(|key| hex::encode(key.as_bytes()));

    // The key this node SIGNS with, which is the device's — not the account
    // root's. The root signs certificates and handoffs and never an op, so
    // reporting it here would name a key no signature on the wire verifies
    // against.
    let signing_key =
        match calimero_governance_store::NamespaceRepository::new(store).node_identity() {
            Ok(Some(record)) => record.public_key.to_string(),
            Ok(None) => {
                return ApiError {
                    status_code: StatusCode::NOT_FOUND,
                    message: "this node holds an account root but no signing identity. \
                          `merod init` provisions one; a node initialised before that \
                          did so mints it on its first namespace join"
                        .to_owned(),
                }
                .into_response();
            }
            Err(err) => {
                error!(error = ?err, "Failed to read this node's signing identity");
                return ApiError {
                    status_code: StatusCode::INTERNAL_SERVER_ERROR,
                    message: "Failed to read this node's signing identity".to_owned(),
                }
                .into_response();
            }
        };

    ApiResponse {
        payload: NodeIdentityApiResponse {
            data: NodeIdentityApiResponseData {
                account_id: hex::encode(account.as_bytes()),
                device_id: device,
                public_key: signing_key,
                // The root of the account this node SPEAKS FOR, which for a paired
                // node belongs to another machine. Public by construction — it is
                // hashed into the account id and travels in every genesis — and it
                // is what a further device needs in order to pair into the same
                // account.
                account_root_public_key: hex::encode(AsRef::<[u8; 32]>::as_ref(&account_root_pk)),
                // The third input `sign-cert` needs. An operator holding the
                // offline root can now read all three from one call and certify
                // this node's device without the node ever touching the root.
                device_agreement_key: agreement_key,
                holds_account_root,
            },
        },
    }
    .into_response()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_account::AccountGenesis;
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::{AccountBindingRepository, NamespaceRepository};
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;

    use super::*;

    const NS: [u8; 32] = [0xA1; 32];

    fn a_node_taking_part_somewhere() -> Store {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        NamespaceRepository::new(&store)
            .note_participation(&NS.into())
            .expect("take part in the namespace");
        store
    }

    /// A node that has taken part in nothing has no device and no root, and there
    /// is nothing to report - a 404 rather than a made-up id.
    #[test]
    fn a_node_that_has_taken_part_in_nothing_reports_no_identity() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));

        assert!(node_identity(&store).expect("read").is_none());
    }

    /// The ordinary case: the device row names the account and the device.
    #[test]
    fn an_enrolled_node_reports_the_device_it_speaks_as() {
        let store = a_node_taking_part_somewhere();
        let devices = NodeDeviceRepository::new(&store);
        let _root = devices
            .provision_account_root()
            .expect("a node that ran `merod init` holds a root");
        let held = devices
            .ensure_enrolled(&NS.into())
            .expect("mint this node's device");

        let (account, root_pk, device, _agreement, _holds) = node_identity(&store)
            .expect("read")
            .expect("an enrolled node has an identity");

        assert_eq!(account, held.account);
        assert_eq!(root_pk, held.genesis.root_sign_pk);
        assert_eq!(device, Some(held.device()));
    }

    /// The fallback. A tombstone spends the `DeviceId` everywhere, so reporting it
    /// would name a device no peer will admit - and for a PAIRED node, an adopted
    /// account it no longer speaks for. What is left is this node's own root, and
    /// no device.
    #[test]
    fn a_node_whose_own_device_was_revoked_falls_back_to_its_own_account_root() {
        let store = a_node_taking_part_somewhere();
        let devices = NodeDeviceRepository::new(&store);
        let own_root = devices.provision_account_root().expect("root").account();

        // Paired INTO somebody else's account, which is what makes the fallback
        // observable: the adopted account and this node's own root differ.
        let adopted = devices
            .ensure_enrolled_into(
                &[ContextGroupId::from(NS)],
                AccountGenesis::new(PrivateKey::from([0x51; 32]).public_key()),
            )
            .expect("adopt");
        assert_ne!(
            adopted.account, own_root,
            "the fixture has to make them differ"
        );
        assert_eq!(
            node_identity(&store).expect("read").expect("present").0,
            adopted.account,
            "while the device is live it speaks for the account it adopted",
        );

        AccountBindingRepository::new(&store)
            .apply_revocation(&NS.into(), adopted.device())
            .expect("tombstone this node's device");

        let (account, _root_pk, device, _agreement, _holds) = node_identity(&store)
            .expect("read")
            .expect("the node still has its own root to fall back on");
        assert_eq!(account, own_root);
        assert_eq!(
            device, None,
            "a spent id must not be presented as this node's"
        );
    }

    /// A node paired into another account holds no root of its own, so once its
    /// device is revoked it has no identity to report at all - which is a 404, not
    /// a locally derived account no row in any group is keyed by.
    #[test]
    fn a_paired_node_with_no_root_of_its_own_reports_nothing_once_revoked() {
        let store = a_node_taking_part_somewhere();
        let devices = NodeDeviceRepository::new(&store);
        let adopted = devices
            .ensure_enrolled_into(
                &[ContextGroupId::from(NS)],
                AccountGenesis::new(PrivateKey::from([0x52; 32]).public_key()),
            )
            .expect("adopt");
        AccountBindingRepository::new(&store)
            .apply_revocation(&NS.into(), adopted.device())
            .expect("tombstone this node's device");

        assert!(node_identity(&store).expect("read").is_none());
    }

    /// The holder: it minted the root the account is derived from, so it is the
    /// one machine that can certify another device into it.
    #[test]
    fn a_node_speaking_for_its_own_account_holds_that_account_s_root() {
        let store = a_node_taking_part_somewhere();
        let devices = NodeDeviceRepository::new(&store);
        let _root = devices.provision_account_root().expect("root");
        let held = devices.ensure_enrolled(&NS.into()).expect("mint");

        let (account, .., holds_root) = node_identity(&store).expect("read").expect("present");
        assert_eq!(account, held.account);
        assert!(holds_root);
    }

    /// The case a bare "does a root exist" check gets wrong. This node ran
    /// `merod init` and so holds a root, but it speaks for an account rooted on
    /// another machine, and certifying into that one is not its to do.
    #[test]
    fn a_paired_node_does_not_hold_the_root_of_the_account_it_adopted() {
        let store = a_node_taking_part_somewhere();
        let devices = NodeDeviceRepository::new(&store);
        let own = devices.provision_account_root().expect("root").account();
        let adopted = devices
            .ensure_enrolled_into(
                &[ContextGroupId::from(NS)],
                AccountGenesis::new(PrivateKey::from([0x53; 32]).public_key()),
            )
            .expect("adopt");
        assert_ne!(adopted.account, own, "the fixture has to make them differ");

        let (account, .., holds_root) = node_identity(&store).expect("read").expect("present");
        assert_eq!(
            account, adopted.account,
            "it speaks for the account it adopted"
        );
        assert!(
            !holds_root,
            "it holds a root, but not the one the account it speaks for is derived from"
        );
    }

    /// A node that holds no root at all can certify nothing.
    #[test]
    fn a_node_with_no_root_of_its_own_holds_none() {
        let store = a_node_taking_part_somewhere();
        let adopted = NodeDeviceRepository::new(&store)
            .ensure_enrolled_into(
                &[ContextGroupId::from(NS)],
                AccountGenesis::new(PrivateKey::from([0x54; 32]).public_key()),
            )
            .expect("adopt");

        let (account, .., holds_root) = node_identity(&store).expect("read").expect("present");
        assert_eq!(account, adopted.account);
        assert!(!holds_root);
    }
}
