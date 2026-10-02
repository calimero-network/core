use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_account::AccountId;
use calimero_context::error::ContextError;
use calimero_context_config::types::ContextGroupId;
use calimero_crypto::{seal_to_root, Purpose, SealedEnvelope};
use calimero_governance_store::{
    AccountBindingRepository, MembershipRepository, NamespaceRepository,
};
use calimero_server_primitives::admin::{
    SealToAccountApiRequest, SealToAccountApiResponse, SealedEnvelopeApiData,
};
use calimero_store::Store;
use eyre::Result as EyreResult;
use reqwest::StatusCode;
use tracing::{debug, error};

use super::{parse_account, parse_group_id};
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

/// The account this node acts as in `group_id`.
///
/// Same principal `list_member_devices` gates on: the caller of an admin route
/// is the node, so its namespace identity is what resolves to a governance
/// account. An identity bound to no account is refused exactly as a non-member
/// is — it names nobody the membership rows could match.
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
        group_id: group_id.to_string(),
    }
    .into()
}

/// Seal `plaintext` to `target`'s root key, as seen from `group_id`.
///
/// Two membership questions, and they are not the same one:
///
/// * **the caller** must be a member of the group, so the surface does not widen
///   past "someone who already holds this group's key";
/// * **the target** must be an account this group knows, so the route cannot be
///   used to fish for whether an arbitrary account id exists on this node.
///
/// The root key itself is not a secret — it is hashed into the `AccountId` and
/// travels in every genesis — so neither check is protecting the value. They
/// keep the route's reach equal to the caller's existing reach.
fn seal(
    store: &Store,
    group_id: &ContextGroupId,
    target: AccountId,
    plaintext: Vec<u8>,
) -> EyreResult<Option<(u32, SealedEnvelope)>> {
    let caller = caller_account(store, group_id)?;
    let membership = MembershipRepository::new(store);
    // A meta-row admin holds no member row, so it is not an effective member.
    let in_group = |account: &AccountId| -> EyreResult<bool> {
        Ok(membership
            .effective_capabilities(group_id, account)?
            .is_some()
            || membership.is_admin(group_id, account)?)
    };
    if !in_group(&caller)? {
        return Err(not_a_group_member(group_id));
    }

    // Binding rows are keyed by NAMESPACE — a subgroup owns none — which is why
    // the root lookup below resolves upward first, exactly as
    // `list_member_devices` does.
    let namespace = NamespaceRepository::new(store).resolve(group_id)?;

    if !in_group(&target)? {
        return Ok(None);
    }

    // The CURRENT root and its epoch, not epoch 0: an account that has rotated
    // its root is opened by the key it rotated to, and the epoch travels with
    // the envelope so a holder of several knows which root opens which.
    let Some((epoch, root_pk)) =
        AccountBindingRepository::new(store).account_key(&namespace, target)?
    else {
        return Ok(None);
    };

    let purpose = Purpose::Account { recipient: root_pk };
    let envelope = seal_to_root(&mut rand::rng(), &root_pk, plaintext, purpose)?;
    Ok(Some((epoch, envelope)))
}

/// `POST /admin-api/groups/:group_id/accounts/:account/seal`
///
/// Produce a blob that only `:account`'s root key can open. The node resolves
/// the root itself — a caller naming a key would sooner or later name a device
/// key, and an envelope sealed to a device is unopenable in exactly the case an
/// envelope is written for.
///
/// Confidentiality only. The sender key is ephemeral and unauthenticated, so an
/// opened envelope proves nothing about who wrote it; the service that accepts
/// a sealed payload is what has to decide it is legitimate.
pub async fn handler(
    Path((group_id_str, account_str)): Path<(String, String)>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<SealToAccountApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let account = match parse_account(&account_str) {
        Ok(account) => account,
        Err(err) => return err.into_response(),
    };

    // Validated as hex by `SealToAccountApiRequest::validate` before reaching
    // here, so a decode failure is a bug rather than bad input.
    let plaintext = match hex::decode(&req.plaintext) {
        Ok(bytes) => bytes,
        Err(err) => {
            error!(error = ?err, "plaintext passed validation but did not decode");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "could not decode the validated plaintext".to_owned(),
            }
            .into_response();
        }
    };

    // The plaintext is the thing being protected, so nothing about it is logged
    // — not its content and not its length, which leaks the size of a namespace
    // set on its own.
    debug!(group_id=%group_id_str, account=%account_str, "Sealing a payload to an account root");

    match seal(&state.store, &group_id, account, plaintext) {
        Ok(Some((account_root_epoch, envelope))) => ApiResponse {
            payload: SealToAccountApiResponse {
                data: SealedEnvelopeApiData {
                    account_root_epoch,
                    ephemeral_public_key: hex::encode(AsRef::<[u8; 32]>::as_ref(
                        &envelope.ephemeral_public_key,
                    )),
                    nonce: hex::encode(envelope.nonce),
                    ciphertext: hex::encode(envelope.ciphertext),
                },
            },
        }
        .into_response(),
        // One answer for "no such account here" and "this group does not know
        // it": both mean the same thing to a caller, and separating them would
        // let the route report which account ids exist on this node.
        Ok(None) => ApiError {
            status_code: StatusCode::NOT_FOUND,
            message: format!(
                "group {group_id_str} knows no account {account_str} with a root key on record"
            ),
        }
        .into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};
    use calimero_governance_store::test_fixtures::{
        bootstrap_namespace_with_admin_account, enrol_member, test_store,
    };
    use calimero_governance_store::{CapabilitiesRepository, DenyListRepository};
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PrivateKey;
    use rand::rand_core::UnwrapErr;
    use rand::rngs::SysRng;

    use super::*;

    const NS: [u8; 32] = [0xAA; 32];
    const SUBGROUP: [u8; 32] = [0xAB; 32]; // an Open subgroup of `NS`

    fn namespace() -> ContextGroupId {
        ContextGroupId::from(NS)
    }

    /// An enrolled account that is a member of the namespace.
    fn member_of(store: &Store) -> AccountId {
        let sk = PrivateKey::random(&mut UnwrapErr(SysRng));
        let account = enrol_member(store, &namespace(), &sk.public_key());
        MembershipRepository::new(store)
            .add_member(&namespace(), &account, GroupMemberRole::Member)
            .expect("add the member");
        account
    }

    /// Enrolled — so it has binding rows and a resolvable root — but never added
    /// to this group. The distinction the second gate turns on.
    fn enrolled_outsider(store: &Store) -> AccountId {
        let sk = PrivateKey::random(&mut UnwrapErr(SysRng));
        enrol_member(store, &namespace(), &sk.public_key())
    }

    /// This node, as a plain member of the namespace.
    fn node_as_member(store: &Store) -> AccountId {
        let sk_bytes: [u8; 32] = rand::RngExt::random(&mut UnwrapErr(SysRng));
        let sk = PrivateKey::from(sk_bytes);
        NamespaceRepository::new(store)
            .store_identity(&namespace(), &sk.public_key(), &sk_bytes)
            .expect("store the node identity");
        let account = enrol_member(store, &namespace(), &sk.public_key());
        MembershipRepository::new(store)
            .add_member(&namespace(), &account, GroupMemberRole::Member)
            .expect("add this node");
        account
    }

    /// An Open subgroup of the namespace that `account` inherits into.
    fn open_subgroup_inherited_by(store: &Store, account: &AccountId) -> ContextGroupId {
        let subgroup = ContextGroupId::from(SUBGROUP);
        NamespaceRepository::new(store)
            .nest(&namespace(), &subgroup)
            .expect("nest the subgroup");
        let capabilities = CapabilitiesRepository::new(store);
        capabilities
            .set_subgroup_visibility(&subgroup, VisibilityMode::Open)
            .expect("open the subgroup");
        capabilities
            .set_member_capability(
                &namespace(),
                account,
                MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits(),
            )
            .expect("let the account inherit into it");
        subgroup
    }

    /// Kicking or leaving an Open subgroup a member inherits deny-lists it there
    /// rather than deleting a row, and that entry is what refuses it.
    #[test]
    fn a_caller_removed_from_an_inherited_subgroup_is_refused() {
        let store = test_store();
        let caller = node_as_member(&store);
        let subgroup = open_subgroup_inherited_by(&store, &caller);
        let target = member_of(&store);
        MembershipRepository::new(&store)
            .add_member(&subgroup, &target, GroupMemberRole::Member)
            .expect("add the target to the subgroup");
        assert!(
            seal(&store, &subgroup, target, b"x".to_vec())
                .expect("an inheritor seals")
                .is_some(),
            "precondition: the inheritor seals before it is removed"
        );

        DenyListRepository::new(&store)
            .mark(&subgroup, &caller)
            .expect("remove this node from the subgroup");

        let err = seal(&store, &subgroup, target, b"x".to_vec())
            .expect_err("a removed caller must be refused, not served");
        let rendered = format!("{err}");
        assert!(
            rendered.contains("not a member of group"),
            "expected the membership refusal, got: {rendered}"
        );
    }

    #[test]
    fn a_target_removed_from_an_inherited_subgroup_is_absent() {
        let store = test_store();
        let _ = bootstrap_namespace_with_admin_account(&store, NS);
        let target = member_of(&store);
        let subgroup = open_subgroup_inherited_by(&store, &target);
        assert!(
            seal(&store, &subgroup, target, b"x".to_vec())
                .expect("no error")
                .is_some(),
            "precondition: an inheritor is sealed to before it is removed"
        );

        DenyListRepository::new(&store)
            .mark(&subgroup, &target)
            .expect("remove the target from the subgroup");

        let out = seal(&store, &subgroup, target, b"x".to_vec()).expect("no error");
        assert!(out.is_none(), "a removed member must not be sealed to");
    }

    #[test]
    fn a_member_can_seal_to_another_member() {
        let store = test_store();
        // Makes this node's identity the namespace admin, so `caller_account`
        // resolves to an account the membership rows know.
        let _ = bootstrap_namespace_with_admin_account(&store, NS);
        let target = member_of(&store);

        let sealed = seal(&store, &namespace(), target, b"recovery".to_vec())
            .expect("the seal itself should not error");
        let (epoch, envelope) = sealed.expect("a member target resolves to a root");

        assert!(!envelope.ciphertext.is_empty());
        // The CURRENT epoch travels with the envelope. Asserting it is present
        // rather than asserting `0`: a fixture that rotated would make a
        // hard-coded 0 a false negative about a real regression.
        let _ = epoch;
    }

    #[test]
    fn a_caller_that_is_not_a_member_is_refused() {
        // The namespace has an identity and enrolled accounts, but this node's
        // identity names nobody the membership rows match — the same position a
        // node holding the group key but removed from the group is in.
        let store = test_store();
        let ns = namespace();
        // Raw bytes alongside the key, because `store_identity` takes the secret
        // as `&[u8; 32]` — the same shape `bootstrap_namespace_with_admin_account`
        // builds it in.
        let outsider_sk_bytes: [u8; 32] = rand::RngExt::random(&mut UnwrapErr(SysRng));
        let outsider_sk = PrivateKey::from(outsider_sk_bytes);
        calimero_governance_store::NamespaceRepository::new(&store)
            .store_identity(&ns, &outsider_sk.public_key(), &outsider_sk_bytes)
            .expect("store the node identity");
        let _ = enrol_member(&store, &ns, &outsider_sk.public_key());
        let target = member_of(&store);

        let err = seal(&store, &ns, target, b"recovery".to_vec())
            .expect_err("a non-member caller must be refused, not served");
        // Asserted on the refusal the caller actually sees, not on the variant
        // name: eyre renders `Display`, and the message is the part that has to
        // stay a refusal rather than drift into something a caller could mistake
        // for "no such account".
        let rendered = format!("{err}");
        assert!(
            rendered.contains("not a member of group"),
            "expected the membership refusal, got: {rendered}"
        );
    }

    #[test]
    fn an_account_this_group_does_not_know_is_absent_rather_than_refused() {
        // `None` becomes one 404 shared with "no such account", so the route
        // cannot be used to discover which accounts exist on this node. A
        // distinct error here would leak exactly that.
        let store = test_store();
        let _ = bootstrap_namespace_with_admin_account(&store, NS);
        let outsider = enrolled_outsider(&store);

        let out = seal(&store, &namespace(), outsider, b"recovery".to_vec())
            .expect("an unknown target is a state, not an error");
        assert!(out.is_none(), "a non-member target must not be sealed to");
    }

    #[test]
    fn an_unknown_account_is_indistinguishable_from_a_non_member() {
        // The property the shared 404 rests on, asserted as a property: both
        // reach the caller as the same `None`.
        let store = test_store();
        let _ = bootstrap_namespace_with_admin_account(&store, NS);

        let never_enrolled = AccountId::from([0x5E; 32]);
        let enrolled_but_outside = enrolled_outsider(&store);

        let a = seal(&store, &namespace(), never_enrolled, b"x".to_vec()).expect("no error");
        let b = seal(&store, &namespace(), enrolled_but_outside, b"x".to_vec()).expect("no error");
        assert!(
            a.is_none() && b.is_none(),
            "both must read as simply absent"
        );
    }
}
