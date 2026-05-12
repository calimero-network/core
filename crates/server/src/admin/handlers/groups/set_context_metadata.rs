use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::{GetContextMetadataRequest, SetContextMetadataRequest};
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::{
    GetMetadataApiResponse, SetContextMetadataApiRequest, SetMetadataApiResponse,
};
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::auth::AuthenticatedKey;
use crate::AdminState;

fn parse_context_id(s: &str) -> Result<ContextId, Box<axum::response::Response>> {
    s.parse().map_err(|err| {
        Box::new(parse_api_error(eyre::eyre!("invalid context_id '{s}': {err}")).into_response())
    })
}

pub async fn handler(
    Path((group_id_str, context_id_str)): Path<(String, String)>,
    Extension(state): Extension<Arc<AdminState>>,
    auth_key: Option<Extension<AuthenticatedKey>>,
    ValidatedJson(req): ValidatedJson<SetContextMetadataApiRequest>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };
    let context_id = match parse_context_id(&context_id_str) {
        Ok(id) => id,
        Err(err) => return (*err).into_response(),
    };

    info!(group_id=%group_id_str, context_id=%context_id_str, "Setting context metadata");

    let result = state
        .ctx_client
        .set_context_metadata(SetContextMetadataRequest {
            group_id,
            context_id,
            name: req.name,
            data: req.data,
            requester: auth_key.map(|Extension(k)| k.0).or(req.requester),
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(()) => {
            info!(group_id=%group_id_str, context_id=%context_id_str, "Context metadata set");
            ApiResponse {
                payload: SetMetadataApiResponse {},
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, context_id=%context_id_str, error=?err, "Failed to set context metadata");
            err.into_response()
        }
    }
}

pub async fn get_handler(
    Path((group_id_str, context_id_str)): Path<(String, String)>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };
    let context_id = match parse_context_id(&context_id_str) {
        Ok(id) => id,
        Err(err) => return (*err).into_response(),
    };

    match state
        .ctx_client
        .get_context_metadata(GetContextMetadataRequest {
            group_id,
            context_id,
        })
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
