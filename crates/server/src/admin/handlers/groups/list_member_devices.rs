use std::collections::BTreeSet;
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_account::AccountId;
use calimero_context::error::ContextError;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{
    AccountBindingRepository, DeviceBinding, MembershipRepository, NamespaceRepository,
};
use calimero_server_primitives::admin::{
    ListMemberDevicesApiResponse, MemberDeviceApiEntry, MemberDevicesApiEntry,
};
use calimero_store::Store;
use eyre::Result as EyreResult;
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

/// The account this node acts as in `group_id`.
///
/// The caller of an admin route is the node itself - the same principal
/// `list_group_members` gates on - so its namespace identity is what resolves to
/// a governance account. An identity bound to no account is refused exactly as a
/// non-member is: it names nobody the membership rows could match.
fn caller_account(store: &Store, group_id: &ContextGroupId) -> EyreResult<AccountId> {
    let account = match NamespaceRepository::new(store).resolve_identity(group_id)? {
        Some((node_key, _)) => {
            calimero_governance_store::member_account_in_namespace(store, group_id, &node_key)?
        }
        None => None,
    };
    account.ok_or_else(|| not_a_group_member(group_id))
}

fn not_a_group_member(group_id: &ContextGroupId) -> eyre::Report {
    // Typed so the admin API surfaces this precondition as a 403 rather than a
    // generic 500 (see `parse_api_error`).
    ContextError::NotAGroupMember {
        group_id: format!("{group_id:?}"),
    }
    .into()
}

fn entry(account: AccountId, bindings: Vec<DeviceBinding>) -> MemberDevicesApiEntry {
    MemberDevicesApiEntry {
        account,
        devices: bindings
            .into_iter()
            .map(|binding| MemberDeviceApiEntry {
                device_id: binding.device,
                signing_key: binding.sign_pk,
            })
            .collect(),
    }
}

/// Account -> devices, scoped to what the caller may see: an admin gets every
/// account in the group, a plain member only its own entry.
///
/// Read from the namespace because that is where binding rows are keyed - a
/// subgroup owns none - then filtered back to the group that was asked about.
fn collect(store: &Store, group_id: &ContextGroupId) -> EyreResult<Vec<MemberDevicesApiEntry>> {
    let namespace = NamespaceRepository::new(store).resolve(group_id)?;
    let caller = caller_account(store, group_id)?;
    let membership = MembershipRepository::new(store);
    let bindings = AccountBindingRepository::new(store);

    if membership.is_admin(group_id, &caller)? {
        let mut visible: BTreeSet<AccountId> = membership
            .list(group_id, 0, usize::MAX)?
            .into_iter()
            .chain(membership.enumerate_inherited(group_id)?)
            .map(|(account, _)| account)
            .collect();
        // A meta-row admin holds no member row, so it is not in `visible` yet.
        visible.insert(caller);

        return Ok(bindings
            .live_devices_by_account(&namespace)?
            .into_iter()
            .filter(|(account, _)| visible.contains(account))
            .map(|(account, devices)| entry(account, devices))
            .collect());
    }

    if !membership.is_member(group_id, &caller)? {
        return Err(not_a_group_member(group_id));
    }

    // `devices_of` filters the same scan `live_devices_by_account` groups, so the
    // self-scoped branch never materializes the peers it is not allowed to report.
    Ok(vec![entry(
        caller,
        bindings.devices_of(&namespace, caller)?,
    )])
}

/// `GET /admin-api/groups/:group_id/member-devices`
///
/// The join between `/groups/:group_id/members`, which names accounts, and
/// `/contexts/:context_id/identities`, which names bare signing keys. Batch
/// rather than per-account: the key -> account direction is asked about
/// arbitrary authors, which a per-account route would answer in N calls.
pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    match collect(&state.store, &group_id) {
        Ok(members) => {
            info!(group_id=%group_id_str, count=%members.len(), "Member devices retrieved");
            ApiResponse {
                payload: ListMemberDevicesApiResponse { members },
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to list member devices");
            parse_api_error(err).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_account::{AccountGenesis, AccountId, DeviceCert, DeviceId, KemPublicKey};
    use calimero_context::error::ContextError;
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::{
        AccountBindingRepository, MembershipRepository, NamespaceRepository,
    };
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::{PrivateKey, PublicKey};
    use calimero_server_primitives::admin::MemberDevicesApiEntry;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;

    use super::collect;

    const NAMESPACE: [u8; 32] = [0x11; 32];
    // Device ids must differ in their first 16 bytes: that prefix is the HLC
    // seed, and colliding seeds leave only the lower device live.
    const LAPTOP: DeviceId = DeviceId::from_raw([0xA1; 32]);
    const PHONE: DeviceId = DeviceId::from_raw([0xB2; 32]);
    const PEER_DEVICE: DeviceId = DeviceId::from_raw([0xC3; 32]);
    const OUTSIDER_DEVICE: DeviceId = DeviceId::from_raw([0xD4; 32]);
    const NODE_ROOT: [u8; 32] = [0x33; 32];
    const LAPTOP_KEY: [u8; 32] = [0x44; 32];

    fn namespace() -> ContextGroupId {
        ContextGroupId::from(NAMESPACE)
    }

    /// Bind `sign_pk` as `device` of the account rooted at `root_sk`.
    fn link(store: &Store, root_sk: &PrivateKey, device: DeviceId, sign_pk: &PublicKey) {
        let genesis = AccountGenesis::new(root_sk.public_key());
        let cert = DeviceCert::sign(
            root_sk,
            genesis.account_id(),
            device,
            sign_pk,
            &KemPublicKey::from([0x2B; 32]),
            0,
            0,
        )
        .expect("the account root signs its own device cert");
        let _binding = AccountBindingRepository::new(store)
            .apply_link(&namespace(), &genesis, &[], &cert)
            .expect("the store write succeeds")
            .expect("the credential binds");
    }

    /// A namespace holding two accounts: this node's, with a laptop and a phone,
    /// and a peer's, with one device. The node joins in `role`; the peer is
    /// always a plain member.
    fn seed(role: GroupMemberRole) -> (Store, AccountId, AccountId) {
        let store = Store::new(Arc::new(InMemoryDB::owned()));

        let node_sk = PrivateKey::from(NODE_ROOT);
        let node_account = AccountGenesis::new(node_sk.public_key()).account_id();
        let laptop_sk = PrivateKey::from(LAPTOP_KEY);
        link(&store, &node_sk, LAPTOP, &laptop_sk.public_key());
        link(
            &store,
            &node_sk,
            PHONE,
            &PrivateKey::from([0x55; 32]).public_key(),
        );

        let peer_sk = PrivateKey::from([0x77; 32]);
        let peer_account = AccountGenesis::new(peer_sk.public_key()).account_id();
        link(
            &store,
            &peer_sk,
            PEER_DEVICE,
            &PrivateKey::from([0x88; 32]).public_key(),
        );

        // The laptop key is what this node signs with, so it is the identity the
        // caller is resolved from.
        NamespaceRepository::new(&store)
            .store_identity(&namespace(), &laptop_sk.public_key(), laptop_sk.as_bytes())
            .expect("store the node identity");
        let membership = MembershipRepository::new(&store);
        membership
            .add_member(&namespace(), &node_account, role)
            .expect("add this node");
        membership
            .add_member(&namespace(), &peer_account, GroupMemberRole::Member)
            .expect("add the peer");

        (store, node_account, peer_account)
    }

    fn accounts(entries: &[MemberDevicesApiEntry]) -> Vec<AccountId> {
        let mut out: Vec<AccountId> = entries.iter().map(|entry| entry.account).collect();
        out.sort_by_key(|account| *account.as_bytes());
        out
    }

    #[test]
    fn admin_caller_sees_every_account() {
        let (store, node_account, peer_account) = seed(GroupMemberRole::Admin);

        let members = collect(&store, &namespace()).expect("collect");

        let mut want = vec![node_account, peer_account];
        want.sort_by_key(|account| *account.as_bytes());
        assert_eq!(accounts(&members), want);
    }

    /// A plain member gets its own devices and nobody else's.
    #[test]
    fn plain_member_caller_sees_only_its_own_devices() {
        let (store, node_account, peer_account) = seed(GroupMemberRole::Member);

        let members = collect(&store, &namespace()).expect("collect");

        assert_eq!(accounts(&members), vec![node_account]);
        assert!(
            !accounts(&members).contains(&peer_account),
            "a plain member must not see a peer's devices"
        );
        // One account with two devices is one entry, not one entry per device.
        let mut devices: Vec<DeviceId> = members[0]
            .devices
            .iter()
            .map(|device| device.device_id)
            .collect();
        devices.sort_by_key(|device| *device.as_bytes());
        assert_eq!(devices, vec![LAPTOP, PHONE]);
    }

    #[test]
    fn a_caller_bound_to_no_account_is_refused() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let stranger = PrivateKey::from([0x99; 32]);
        NamespaceRepository::new(&store)
            .store_identity(&namespace(), &stranger.public_key(), stranger.as_bytes())
            .expect("store the node identity");

        let err = collect(&store, &namespace()).expect_err("an unbound identity names nobody");

        assert!(matches!(
            err.downcast_ref::<ContextError>(),
            Some(ContextError::NotAGroupMember { .. })
        ));
    }

    /// A subgroup in the path resolves to the namespace that holds the binding
    /// rows - the subgroup itself owns none.
    #[test]
    fn a_subgroup_resolves_to_its_namespace() {
        let (store, node_account, _peer) = seed(GroupMemberRole::Member);
        let subgroup = ContextGroupId::from([0x22; 32]);
        NamespaceRepository::new(&store)
            .nest(&namespace(), &subgroup)
            .expect("nest the subgroup");
        MembershipRepository::new(&store)
            .add_member(&subgroup, &node_account, GroupMemberRole::Member)
            .expect("add this node to the subgroup");

        let members = collect(&store, &subgroup).expect("collect");

        assert_eq!(accounts(&members), vec![node_account]);
        assert_eq!(members[0].devices.len(), 2);
    }

    /// Symmetric with `/groups/:group_id/members`: an admin of a subgroup sees
    /// that subgroup's members, not an account whose only row is a sibling's.
    #[test]
    fn subgroup_admin_caller_does_not_see_a_sibling_subgroup() {
        let (store, node_account, _peer) = seed(GroupMemberRole::Member);
        let namespaces = NamespaceRepository::new(&store);
        let membership = MembershipRepository::new(&store);

        let mine = ContextGroupId::from([0x22; 32]);
        let sibling = ContextGroupId::from([0x23; 32]);
        namespaces.nest(&namespace(), &mine).expect("nest mine");
        namespaces
            .nest(&namespace(), &sibling)
            .expect("nest the sibling");
        membership
            .add_member(&mine, &node_account, GroupMemberRole::Admin)
            .expect("admin of its own subgroup only");

        let outsider_sk = PrivateKey::from([0xAA; 32]);
        let outsider = AccountGenesis::new(outsider_sk.public_key()).account_id();
        link(
            &store,
            &outsider_sk,
            OUTSIDER_DEVICE,
            &PrivateKey::from([0xBB; 32]).public_key(),
        );
        membership
            .add_member(&sibling, &outsider, GroupMemberRole::Member)
            .expect("add the outsider to the sibling subgroup");

        let members = collect(&store, &mine).expect("collect");

        assert!(
            !accounts(&members).contains(&outsider),
            "a subgroup admin must not see a sibling subgroup's member"
        );
        assert_eq!(accounts(&members), vec![node_account]);
    }
}
