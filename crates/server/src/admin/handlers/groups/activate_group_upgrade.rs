use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::ActivateNamespaceUpgradeRequest;
use calimero_server_primitives::admin::{
    ActivateGroupUpgradeApiRequest, ActivateGroupUpgradeApiResponse,
    ActivateGroupUpgradeApiResponseData, ActivateGroupUpgradeFailedContext,
};
use tracing::{debug, error, info};

use super::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

/// `POST /admin-api/groups/{namespace_id}/upgrade/activate` - switch this node's
/// contexts of the namespace to its stored target release and replay the deltas
/// held back for them. Never installs the release: that is `install-application`.
pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<ActivateGroupUpgradeApiRequest>,
) -> impl IntoResponse {
    let namespace_id = match parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(namespace_id=%namespace_id_str, "Activating namespace upgrade");

    let result = state
        .ctx_client
        .activate_namespace_upgrade(ActivateNamespaceUpgradeRequest {
            namespace_id,
            expected_application_id: req.expected_application_id,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            info!(
                namespace_id=%namespace_id_str,
                switched = resp.switched_contexts,
                replayed = resp.replayed_deltas,
                failed = resp.failed_contexts.len(),
                "Namespace upgrade activated"
            );
            let failed_contexts = resp
                .failed_contexts
                .into_iter()
                .map(|failed| ActivateGroupUpgradeFailedContext {
                    context_id: failed.context_id,
                    error: failed.error,
                    retryable: failed.retryable,
                })
                .collect();
            ApiResponse {
                payload: ActivateGroupUpgradeApiResponse {
                    data: ActivateGroupUpgradeApiResponseData {
                        target_application_id: resp.target_application_id,
                        switched_contexts: resp.switched_contexts,
                        replayed_deltas: resp.replayed_deltas,
                        failed_contexts,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            // A 4xx is the caller naming something absent or a target that moved,
            // not a fault of this node.
            if err.is_client_fault() {
                debug!(namespace_id=%namespace_id_str, error=?err, "Failed to activate namespace upgrade");
            } else {
                error!(namespace_id=%namespace_id_str, error=?err, "Failed to activate namespace upgrade");
            }
            err.into_response()
        }
    }
}
