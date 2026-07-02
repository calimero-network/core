use std::pin::pin;
use std::sync::Arc;

use axum::extract::Query;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::GetGroupForContextRequest;
use calimero_server_primitives::admin::{ContextWithGroup, GetContextsResponse};
use futures_util::TryStreamExt;
use serde::Deserialize;
use tracing::{error, info, warn};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

/// Hard cap on the number of contexts returned in a single response, regardless
/// of the requested `limit`. Bounds the O(N) per-request work (each returned
/// context costs two awaited lookups below).
const MAX_PAGE: usize = 1000;

/// Page size used when the caller does not specify a `limit`.
const DEFAULT_LIMIT: usize = 100;

#[derive(Debug, Deserialize)]
pub struct GetContextsQuery {
    /// Number of existing contexts to skip before collecting the page.
    offset: Option<usize>,
    /// Maximum number of contexts to return (clamped to `MAX_PAGE`).
    limit: Option<usize>,
}

pub async fn handler(
    Query(query): Query<GetContextsQuery>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_PAGE);

    info!(offset, limit, "Listing contexts");

    let context_ids = state.ctx_client.get_context_ids(None);
    let mut context_ids = pin!(context_ids);
    let mut contexts = Vec::new();
    // Count of existing contexts scanned so far, used to apply `offset` over
    // real contexts (ids that don't resolve to a context row are skipped and
    // don't consume an offset slot).
    let mut seen = 0usize;

    while let Some(context_id) = context_ids.try_next().await.transpose() {
        // Stop as soon as the page is full — no need to drain the rest of the
        // stream or do any further per-context work.
        if contexts.len() >= limit {
            break;
        }

        let context_id = match context_id {
            Ok(id) => id,
            Err(err) => {
                error!(error=?err, "Failed to get context IDs");
                return parse_api_error(err).into_response();
            }
        };

        match state.ctx_client.get_context(&context_id) {
            Ok(None) => {}
            Ok(Some(mut context)) => {
                // Skip contexts before the requested window without paying for
                // the heavy per-context resolution below.
                if seen < offset {
                    seen += 1;
                    continue;
                }
                seen += 1;

                // Per-context executing version (activation marker) wins over
                // the application row's latest-installed version.
                if let Some(v) = state
                    .ctx_client
                    .executing_application_version(&context_id)
                    .await
                {
                    context.application_version = Some(v);
                }
                let group_id = match state
                    .ctx_client
                    .get_group_for_context(GetGroupForContextRequest { context_id })
                    .await
                {
                    Ok(gid) => gid.map(|g| hex::encode(g.to_bytes())),
                    Err(err) => {
                        warn!(context_id=%context_id, error=?err, "Failed to resolve group for context");
                        None
                    }
                };
                contexts.push(ContextWithGroup { context, group_id });
            }
            Err(err) => {
                error!(context_id=%context_id, error=?err, "Failed to get context");
                return parse_api_error(err).into_response();
            }
        }
    }

    info!(count=%contexts.len(), "Contexts listed successfully");

    ApiResponse {
        payload: GetContextsResponse::new(contexts),
    }
    .into_response()
}
