use std::pin::pin;
use std::sync::Arc;

use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::GetGroupForContextRequest;
use calimero_server_primitives::admin::{ContextWithGroup, GetContextsResponse};
use futures_util::TryStreamExt;
use serde::Deserialize;
use tracing::{error, info, warn};

use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

/// Hard cap on the number of contexts returned in a single response, regardless
/// of the requested `limit`. Bounds the O(N) per-request work (each returned
/// context costs two awaited lookups below).
const MAX_PAGE: usize = 1000;

/// Page size used when the caller does not specify a `limit`.
const DEFAULT_LIMIT: usize = 100;

/// Hard cap on `offset`. Even though skipped ids no longer pay for `get_context`,
/// a huge offset still scans that many ids from the stream, so bound it to keep
/// a single request's work finite.
const MAX_OFFSET: usize = 100_000;

#[derive(Debug, Deserialize)]
pub struct GetContextsQuery {
    /// Number of context ids to skip from the stream before collecting the page.
    offset: Option<usize>,
    /// Maximum number of contexts to return (clamped to `MAX_PAGE`).
    limit: Option<usize>,
}

pub async fn handler(
    Query(query): Query<GetContextsQuery>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let offset = query.offset.unwrap_or(0);
    if offset > MAX_OFFSET {
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: format!("offset exceeds maximum of {MAX_OFFSET}"),
        }
        .into_response();
    }
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_PAGE);

    info!(offset, limit, "Listing contexts");

    let context_ids = state.ctx_client.get_context_ids(None);
    let mut context_ids = pin!(context_ids);
    let mut contexts = Vec::with_capacity(limit);
    // Number of stream ids skipped so far to satisfy `offset`.
    let mut skipped = 0usize;

    while let Some(context_id) = context_ids.try_next().await.transpose() {
        // Surface stream errors regardless of page/offset state — never let a
        // full page or an offset skip swallow a fatal stream error.
        let context_id = match context_id {
            Ok(id) => id,
            Err(err) => {
                error!(error=?err, "Failed to get context IDs");
                return parse_api_error(err).into_response();
            }
        };

        // Apply `offset` up front, before any per-context lookups, so skipped
        // ids don't pay for `get_context` or the awaited resolutions.
        if skipped < offset {
            skipped += 1;
            continue;
        }

        match state.ctx_client.get_context(&context_id) {
            Ok(None) => {}
            Ok(Some(mut context)) => {
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

                // Stop once the page holds `limit` *collected* contexts (counts
                // pushes, not attempts, so ids that resolve to None never
                // under-fill the page).
                if contexts.len() >= limit {
                    break;
                }
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
