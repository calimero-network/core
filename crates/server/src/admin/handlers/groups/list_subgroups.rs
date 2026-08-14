use calimero_governance_store::{MembershipRepository, MetadataRepository, NamespaceRepository};
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::{ListSubgroupsApiResponse, SubgroupEntryApiResponse};
use tracing::{info, warn};

use crate::admin::handlers::groups::parse_group_id;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, "Listing subgroups");

    let children = match NamespaceRepository::new(&state.store).list_children(&group_id) {
        Ok(children) => children,
        Err(err) => return parse_api_error(err).into_response(),
    };

    // Caller identity comes from the node's *own* namespace identity for
    // the parent group — NOT from the JWT subject. The JWT's `sub` is a
    // node-level key fingerprint that doesn't parse as a `PublicKey`
    // (see calimero_server::auth — emits a WARN and skips the
    // AuthenticatedKey extension). Using `resolve_namespace_identity`
    // matches what `list_group_members` already does to populate
    // `selfIdentity`.
    // Visibility is decided per account, so this node's namespace identity is
    // resolved to one. An identity bound to no account here sees the same as no
    // identity at all: every Restricted child stays hidden.
    let caller = match NamespaceRepository::new(&state.store).resolve_identity(&group_id) {
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
    let caller_account = caller.and_then(|pk| {
        calimero_governance_store::member_account_in_namespace(&state.store, &group_id, &pk)
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
            &group_id,
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

        let name = match MetadataRepository::new(&state.store).group_metadata(&child) {
            Ok(rec) => rec.and_then(|r| r.name),
            Err(err) => return parse_api_error(err).into_response(),
        };
        subgroups.push(SubgroupEntryApiResponse {
            group_id: hex::encode(child.to_bytes()),
            name,
        });
    }

    ApiResponse {
        payload: ListSubgroupsApiResponse { subgroups },
    }
    .into_response()
}
