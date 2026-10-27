use calimero_governance_store::private_space::Tree;
use calimero_governance_store::{MembershipRepository, MetadataRepository, NamespaceRepository};
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_account::AccountId;
use calimero_context_config::types::ContextGroupId;
use calimero_server_primitives::admin::{ListSubgroupsApiResponse, SubgroupEntryApiResponse};
use tracing::{info, warn};

use crate::admin::handlers::groups::parse_group_id;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    client_key: Option<Extension<crate::auth::ClientKeyScope>>,
    node_owner: Option<Extension<crate::auth::AuthenticatedNodeOwner>>,
    account: Option<Extension<crate::auth::AuthenticatedAccount>>,
    device: Option<Extension<crate::auth::AuthenticatedDevice>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    // Before any read: a delegated session reaches this route on the narrow
    // `group:list-own`, so it must be confined to its own groups here.
    if let Some(refusal) = crate::admin::caller_scope::refuse_group_outside_caller_scope(
        &state.ctx_client,
        client_key,
        node_owner,
        account.clone(),
        device,
        &group_id,
    ) {
        return refusal;
    }

    info!(group_id=%group_id_str, "Listing subgroups");

    let subgroups = match visible_children(&state, &group_id, account.map(|Extension(a)| a)) {
        Ok(children) => children
            .into_iter()
            .map(|(child, name)| SubgroupEntryApiResponse {
                group_id: hex::encode(child.to_bytes()),
                name,
            })
            .collect(),
        Err(err) => return parse_api_error(err).into_response(),
    };

    ApiResponse {
        payload: ListSubgroupsApiResponse { subgroups },
    }
    .into_response()
}

/// The children of `group_id` a caller may learn exist, with their names: every
/// Open child, and a Restricted one only to an admin of `group_id` or a member of it.
pub(crate) fn visible_children(
    state: &AdminState,
    group_id: &ContextGroupId,
    account: Option<crate::auth::AuthenticatedAccount>,
) -> eyre::Result<Vec<(ContextGroupId, Option<String>)>> {
    let group_id_str = hex::encode(group_id.to_bytes());

    let children = NamespaceRepository::new(&state.store).list_children(group_id)?;

    if hidden_by_space(state, group_id, account.clone())? {
        return Ok(Vec::new());
    }

    let caller_account = caller_account(state, group_id, account);
    let mut subgroups = Vec::with_capacity(children.len());
    for child in children {
        // `Open` subgroups are always listed; `Restricted` subgroups are
        // listed only for the parent-group admin or a member of the
        // child (see `subgroup_visible_to`). On any visibility/membership
        // lookup error we skip the child — the conservative choice never
        // leaks a private subgroup. A `caller` of `None` (this node has
        // no namespace identity for the parent group) likewise hides all
        // `Restricted` children.
        match MembershipRepository::new(&state.store).subgroup_visible_to(
            group_id,
            &child,
            caller_account.as_ref(),
        ) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(err) => {
                warn!(
                    ?err,
                    parent_group_id = %group_id_str,
                    child_group_id = %hex::encode(child.to_bytes()),
                    "subgroup visibility check failed; hiding subgroup from list"
                );
                continue;
            }
        }

        let name = MetadataRepository::new(&state.store)
            .group_metadata(&child)?
            .and_then(|r| r.name);
        subgroups.push((child, name));
    }

    Ok(subgroups)
}

/// The account a request asks as: its own session account, else this node's account in
/// `group_id`'s namespace. `None` is no account here, which sees nothing restricted.
pub(crate) fn caller_account(
    state: &AdminState,
    group_id: &ContextGroupId,
    account: Option<crate::auth::AuthenticatedAccount>,
) -> Option<AccountId> {
    let group_id_str = hex::encode(group_id.to_bytes());
    // Caller identity comes from the node's *own* namespace identity for
    // the parent group, NOT from the JWT subject. The JWT's `sub` is an
    // opaque user id that doesn't parse as a `PublicKey`
    // (see calimero_server::auth — emits a WARN and skips the
    // AuthenticatedKey extension), so the node's namespace identity is the
    // only thing here that resolves to a governance principal.
    // Visibility is decided per account, so this node's namespace identity is
    // resolved to one. An identity bound to no account here sees the same as no
    // identity at all: every Restricted child stays hidden.
    let caller = match NamespaceRepository::new(&state.store).resolve_identity(group_id) {
        Ok(Some((pk, _))) => Some(pk),
        Ok(None) => None,
        Err(err) => {
            warn!(
                ?err,
                group_id = %group_id_str,
                "resolve_namespace_identity failed; falling back to conservative listing \
                 (all Restricted subgroups hidden)"
            );
            None
        }
    };

    // Hiding every Restricted child is the conservative answer to both "no
    // account here" and "the store would not say", but only one of them is
    // normal. Unlogged they are the same event, so a persistent fault hides
    // every restricted subgroup from this node's own admin API and looks like a
    // visibility setting — the sibling `resolve_identity` above warns for the
    // same reason.
    // A delegated session asks as its own account: it sees Open children and
    // the Restricted ones it belongs to, never this node's view.
    let node_account = caller.and_then(|pk| {
        calimero_governance_store::member_account_in_namespace(&state.store, group_id, &pk)
            .unwrap_or_else(|err| {
                warn!(
                    ?err,
                    group_id = %group_id_str,
                    "resolving this node's account failed; falling back to conservative \
                     listing (all Restricted subgroups hidden)"
                );
                None
            })
    });
    account.map(|account| account.0).or(node_account)
}

/// The answer to a read that `hidden_by_space` rules out.
pub(crate) fn space_outsider_refusal() -> axum::response::Response {
    use axum::response::IntoResponse;
    crate::admin::service::ApiError {
        status_code: axum::http::StatusCode::FORBIDDEN,
        message: "not a member of this private space".to_owned(),
    }
    .into_response()
}

/// Whether `group_id` is a private space root or inside one, and the caller no member of it:
/// outsiders learn that a space exists, never what is inside it.
pub(crate) fn hidden_by_space(
    state: &AdminState,
    group_id: &ContextGroupId,
    account: Option<crate::auth::AuthenticatedAccount>,
) -> eyre::Result<bool> {
    let Some(space) = Tree::live(&state.store, group_id)?.space_of(group_id)? else {
        return Ok(false);
    };
    let Some(caller) = caller_account(state, group_id, account) else {
        return Ok(true);
    };
    Ok(!MembershipRepository::new(&state.store).is_live_member(&space, &caller)?)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;

    use super::*;
    use crate::auth::AuthenticatedAccount;

    /// Whether a workspace member is shown `drafts` among the children of the private space
    /// `board`; `in_board` seats the member in the space.
    async fn lists_the_folder_in_the_space(in_board: bool) -> bool {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let (board, drafts, account) = crate::test_support::seed_private_space(&store, in_board);
        let (state, _blob_dir) = crate::test_support::admin_state(&store).await;

        visible_children(&state, &board, Some(AuthenticatedAccount(account)))
            .expect("the listing answers")
            .iter()
            .any(|(child, _)| *child == drafts)
    }

    #[actix::test]
    async fn a_folder_in_a_private_space_is_listed_only_to_its_members() {
        assert!(
            lists_the_folder_in_the_space(true).await,
            "precondition: a member of the space sees it"
        );
        assert!(!lists_the_folder_in_the_space(false).await);
    }

    #[actix::test]
    async fn a_private_space_is_hidden_from_an_account_removed_from_the_workspace() {
        use calimero_governance_store::test_fixtures::{dummy_member_removed_op, enrol_member};
        use calimero_primitives::context::GroupMemberRole;
        use calimero_primitives::identity::PrivateKey;

        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let (board, _drafts, account) = crate::test_support::seed_private_space(&store, true);
        let (state, _blob_dir) = crate::test_support::admin_state(&store).await;
        let namespace = NamespaceRepository::new(&store).resolve(&board).unwrap();
        let hidden =
            || hidden_by_space(&state, &board, Some(AuthenticatedAccount(account))).unwrap();
        assert!(!hidden(), "precondition: a member of the space reads it");

        let admin_sk = PrivateKey::from([0xAD; 32]);
        let admin = enrol_member(&store, &namespace, &admin_sk.public_key());
        MembershipRepository::new(&store)
            .add_member(&namespace, &admin, GroupMemberRole::Admin)
            .unwrap();
        crate::test_support::apply_through_governance(
            &store,
            &namespace,
            &admin_sk,
            dummy_member_removed_op(account),
        )
        .expect("the admin removes the account from the workspace");

        assert!(hidden(), "its space row ended with its workspace row");
    }

    #[actix::test]
    async fn a_space_creator_removed_from_it_reads_nothing_below_it() {
        use calimero_governance_store::MetaRepository;

        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let (board, drafts, account) = crate::test_support::seed_private_space(&store, false);
        let mut meta = calimero_governance_store::test_fixtures::test_meta();
        meta.admin_identity = account;
        MetaRepository::new(&store).save(&board, &meta).unwrap();
        let (state, _blob_dir) = crate::test_support::admin_state(&store).await;

        for group in [board, drafts] {
            assert!(hidden_by_space(&state, &group, Some(AuthenticatedAccount(account))).unwrap());
        }
    }
}
