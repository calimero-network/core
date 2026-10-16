use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{AccountBindingRepository, NamespaceRepository};
use calimero_server_primitives::admin::NamespaceRevokedDevicesApiResponse;
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::{debug, error};

use crate::admin::handlers::groups::list_member_devices::caller_account;
use crate::admin::handlers::groups::parse_group_id;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

/// Tombstones and withdrawal floors are keyed by the namespace, so a subgroup id is
/// resolved to its root first. Only an account bound in that namespace may read them.
fn collect(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<NamespaceRevokedDevicesApiResponse> {
    let namespace = NamespaceRepository::new(store).resolve(group_id)?;
    caller_account(store, &namespace)?;
    let bindings = AccountBindingRepository::new(store);
    Ok(NamespaceRevokedDevicesApiResponse {
        revoked_devices: bindings.revoked_devices(&namespace)?,
        withdrawn_slots: bindings
            .withdrawn_slots(&namespace)?
            .into_iter()
            .map(hex::encode)
            .collect(),
    })
}

/// `GET /admin-api/namespaces/:namespace_id/revoked-devices`
pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let namespace_id = match parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    match collect(&state.store, &namespace_id) {
        Ok(payload) => {
            debug!(namespace_id=%namespace_id_str, revoked=%payload.revoked_devices.len(), withdrawn=%payload.withdrawn_slots.len(), "Revoked devices retrieved");
            ApiResponse { payload }.into_response()
        }
        Err(err) => {
            error!(namespace_id=%namespace_id_str, error=?err, "Failed to list revoked devices");
            parse_api_error(err).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_account::{AccountGenesis, DeviceCert, DeviceId, KemPublicKey};
    use calimero_context::error::ContextError;
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::{
        AccountBindingRepository, MembershipRepository, NamespaceRepository,
    };
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use sha2::{Digest, Sha256};

    use super::collect;

    fn namespace() -> ContextGroupId {
        ContextGroupId::from([0x11; 32])
    }

    /// This node's identity is bound to an account that is a namespace member,
    /// and one device of that account is revoked.
    fn seed_member() -> (Store, DeviceId, String) {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let sk = PrivateKey::from([0x44; 32]);
        let account = AccountGenesis::new(sk.public_key()).account_id();
        let device = DeviceId::mint(account, [0xA1; 16]);
        let repo = AccountBindingRepository::new(&store);
        repo.apply_revocation(&namespace(), device).expect("revoke");
        let thin_client = DeviceId::mint(account, [0xC3; 16]);
        repo.withdraw_for_account(&namespace(), account, thin_client)
            .expect("withdraw");
        let slot = hex::encode(
            Sha256::new()
                .chain_update(account.as_bytes())
                .chain_update(thin_client.as_bytes())
                .finalize(),
        );
        // Binds the node's signing key to the account so the caller resolves.
        let genesis = AccountGenesis::new(sk.public_key());
        let cert = DeviceCert::sign(
            &sk,
            account,
            DeviceId::mint(account, [0xB2; 16]),
            &sk.public_key(),
            &KemPublicKey::from([0x2B; 32]),
            0,
            0,
        )
        .expect("sign");
        repo.apply_link(&namespace(), &genesis, &[], &cert, 0)
            .expect("link")
            .expect("binds");
        NamespaceRepository::new(&store)
            .store_identity(&namespace(), &sk.public_key(), sk.as_bytes())
            .expect("store identity");
        MembershipRepository::new(&store)
            .add_member(&namespace(), &account, GroupMemberRole::Member)
            .expect("add member");
        (store, device, slot)
    }

    #[test]
    fn a_member_reads_the_revoked_devices() {
        let (store, device, slot) = seed_member();

        let got = collect(&store, &namespace()).expect("collect");

        assert_eq!(got.revoked_devices, vec![device]);
        assert_eq!(got.withdrawn_slots, vec![slot]);
    }

    #[test]
    fn a_caller_bound_to_no_account_is_refused() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let stranger = PrivateKey::from([0x99; 32]);
        NamespaceRepository::new(&store)
            .store_identity(&namespace(), &stranger.public_key(), stranger.as_bytes())
            .expect("store identity");

        let err = collect(&store, &namespace()).expect_err("a non-member reads nothing");

        assert!(matches!(
            err.downcast_ref::<ContextError>(),
            Some(ContextError::NotAGroupMember { .. })
        ));
    }
}
