use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_config::VisibilityMode;
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
    let caller = match calimero_context::group_store::resolve_namespace_identity(
        &state.store,
        &group_id,
    ) {
        Ok(Some((pk, _, _))) => Some(pk),
        Ok(None) => None,
        Err(err) => {
            warn!(?err, group_id=%group_id_str,
                  "resolve_namespace_identity failed; falling back to unfiltered listing");
            None
        }
    };

    let mut subgroups = Vec::with_capacity(children.len());
    for child in children {
        // Restricted subgroups stay hidden from non-members. Open
        // subgroups are always listed — their existence is public by
        // design (that's what CAN_JOIN_OPEN_SUBGROUPS at the namespace
        // root authorises against).
        if let Some(ref caller_pk) = caller {
            let visibility =
                match calimero_context::group_store::get_subgroup_visibility(&state.store, &child) {
                    Ok(v) => v,
                    Err(err) => {
                        warn!(?err, group_id=%hex::encode(child.to_bytes()),
                              "get_subgroup_visibility failed; skipping from list");
                        continue;
                    }
                };
            if matches!(visibility, VisibilityMode::Restricted) {
                let is_member = match calimero_context::group_store::check_group_membership(
                    &state.store,
                    &child,
                    caller_pk,
                ) {
                    Ok(b) => b,
                    Err(err) => {
                        warn!(?err, group_id=%hex::encode(child.to_bytes()),
                              "check_group_membership failed; skipping from list");
                        continue;
                    }
                };
                if !is_member {
                    continue;
                }
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
