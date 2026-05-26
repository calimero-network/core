#![allow(deprecated)] // #2303: per-file Repository migration deferred to follow-up

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

    let children = match calimero_context::group_store::list_child_groups(&state.store, &group_id) {
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
    let caller =
        match calimero_context::group_store::resolve_namespace_identity(&state.store, &group_id) {
            Ok(Some((pk, _, _))) => Some(pk),
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

    let mut subgroups = Vec::with_capacity(children.len());
    for child in children {
        // `Open` subgroups are always listed; `Restricted` subgroups are
        // listed only for the parent-group admin or a member of the
        // child (see `subgroup_visible_to`). On any visibility/membership
        // lookup error we skip the child — the conservative choice never
        // leaks a private subgroup. A `caller` of `None` (this node has
        // no namespace identity for the parent group) likewise hides all
        // `Restricted` children.
        match calimero_context::group_store::subgroup_visible_to(
            &state.store,
            &group_id,
            &child,
            caller.as_ref(),
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

        let name = match calimero_context::group_store::get_group_metadata(&state.store, &child) {
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
