use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::{GetGroupMetadataRequest, SetGroupMetadataRequest};
use calimero_server_primitives::admin::{
    GetMetadataApiResponse, SetGroupMetadataApiRequest, SetMetadataApiResponse,
};
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<SetGroupMetadataApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, "Setting group metadata");

    let result = state
        .ctx_client
        .set_group_metadata(SetGroupMetadataRequest {
            group_id,
            name: req.name,
            data: req.data,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => {
            info!(group_id=%group_id_str, "Group metadata set");
            ApiResponse {
                payload: SetMetadataApiResponse {},
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to set group metadata");
            err.into_response()
        }
    }
}

pub async fn get_handler(
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
    match super::list_subgroups::hidden_by_space(&state, &group_id, account.map(|Extension(a)| a)) {
        Ok(false) => {}
        Ok(true) => {
            return ApiResponse {
                payload: GetMetadataApiResponse { data: None },
            }
            .into_response()
        }
        Err(err) => return parse_api_error(err).into_response(),
    }

    match state
        .ctx_client
        .get_group_metadata(GetGroupMetadataRequest { group_id })
        .await
        .map_err(parse_api_error)
    {
        Ok(record) => ApiResponse {
            payload: GetMetadataApiResponse { data: record },
        }
        .into_response(),
        Err(err) => err.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::routing::get;
    use axum::{Extension, Router};
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;

    use super::get_handler;
    #[actix::test]
    async fn a_private_spaces_metadata_is_not_read_by_an_outsider() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let (board, drafts, _account) = crate::test_support::seed_private_space(&store, false);
        let (state, _blob_dir) = crate::test_support::admin_state(&store).await;
        let router = Router::new()
            .route("/groups/{group_id}/metadata", get(get_handler))
            .layer(Extension(state));

        for group in [board, drafts] {
            let uri = format!("/groups/{}/metadata", hex::encode(group.to_bytes()));
            let (status, body) = crate::test_support::get(router.clone(), &uri).await;
            assert_eq!(status, axum::http::StatusCode::OK, "{body}");
            let body: serde_json::Value = serde_json::from_str(&body).expect("json");
            assert!(body["data"].is_null(), "an outsider read {body}");
        }
    }
}
