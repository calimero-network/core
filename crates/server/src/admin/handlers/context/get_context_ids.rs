use std::collections::BTreeSet;
use std::pin::pin;
use std::sync::Arc;

use axum::extract::Query;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::GetGroupForContextRequest;
use calimero_governance_store::ContextTreeService;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::{ContextWithGroup, GetContextsResponse};
use futures_util::future::Either;
use futures_util::TryStreamExt;
use serde::Deserialize;
use tracing::{debug, error, info, warn};

use crate::admin::caller_scope::{list_scope_for, ListScope};
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::auth::{AuthenticatedAccount, AuthenticatedNodeOwner};
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
    /// Number of context ids to skip from the stream (clamped to `MAX_OFFSET`).
    offset: Option<usize>,
    /// Maximum number of contexts to return (clamped to `MAX_PAGE`).
    limit: Option<usize>,
}

pub async fn handler(
    Query(query): Query<GetContextsQuery>,
    Extension(state): Extension<Arc<AdminState>>,
    node_owner: Option<Extension<AuthenticatedNodeOwner>>,
    account: Option<Extension<AuthenticatedAccount>>,
) -> impl IntoResponse {
    // Both bounds are silently clamped (rather than one clamped and one
    // rejected) so the endpoint treats over-large paging params consistently.
    let offset = query.offset.unwrap_or(0).min(MAX_OFFSET);
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_PAGE);

    let scope = match list_scope_for(&state.ctx_client, node_owner, account) {
        Ok(scope) => scope,
        Err(err) => {
            // Fail closed: a caller whose groups could not be resolved is not a
            // caller whose scope is "everything".
            error!(error=?err, "Failed to resolve the caller's list scope");
            return parse_api_error(err).into_response();
        }
    };

    debug!(offset, limit, account = ?scope.account(), "Listing contexts");

    // An account scope enumerates the caller's OWN contexts, from their groups,
    // rather than scanning the node's and discarding what does not match. Two
    // reasons, and the second is the one that matters:
    //
    // * it is O(the caller's contexts) rather than O(the node's), which is what
    //   #3941 asks for;
    // * `offset` then pages the caller's list. Applied to the node's stream — as
    //   it is below, deliberately, before any per-context lookup — an offset of
    //   10 would skip ten of the NODE's contexts, so a scoped caller with five
    //   would page past their own and see nothing, with no error to explain it.
    let context_ids = match scoped_context_ids(state.ctx_client.datastore(), &scope) {
        Ok(Some(ids)) => Either::Left(futures_util::stream::iter(ids.into_iter().map(Ok))),
        Ok(None) => Either::Right(state.ctx_client.get_context_ids(None)),
        Err(err) => {
            error!(error=?err, "Failed to enumerate the caller's contexts");
            return parse_api_error(err).into_response();
        }
    };
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

/// The caller's own context ids, or `None` when the scope is node-wide and the
/// existing stream should be used unchanged.
///
/// Returns them already de-duplicated and ordered: a context belongs to exactly
/// one group, so the per-group enumerations cannot overlap, but ordering is what
/// makes `offset`/`limit` paging stable across requests — store iteration order
/// per group says nothing about order across groups.
fn scoped_context_ids(
    store: &calimero_store::Store,
    scope: &ListScope,
) -> eyre::Result<Option<Vec<ContextId>>> {
    let ListScope::Account { groups, .. } = scope else {
        return Ok(None);
    };

    let mut ids = BTreeSet::new();
    for group in groups {
        ids.extend(ContextTreeService::new(store, *group).enumerate_contexts(0, usize::MAX)?);
    }
    Ok(Some(ids.into_iter().collect()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use calimero_account::AccountId;
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::ContextTreeService;
    use calimero_primitives::context::ContextId;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;

    use super::scoped_context_ids;
    use crate::admin::caller_scope::ListScope;

    /// The wiring, not the predicate: `caller_scope`'s tests prove a scope
    /// admits the right groups, and this proves the endpoint enumerates from it
    /// rather than computing a scope and then listing the node anyway.
    #[test]
    fn an_account_scope_enumerates_only_its_own_groups_contexts() {
        let store = Store::new(std::sync::Arc::new(InMemoryDB::owned()));

        let mine = ContextGroupId::from([0xa1; 32]);
        let theirs = ContextGroupId::from([0xb1; 32]);
        let my_context = ContextId::from([0x01; 32]);
        let their_context = ContextId::from([0x02; 32]);

        ContextTreeService::new(&store, mine)
            .register_context(&my_context)
            .unwrap();
        ContextTreeService::new(&store, theirs)
            .register_context(&their_context)
            .unwrap();

        let scope = ListScope::Account {
            account: AccountId::from([0x01; 32]),
            groups: BTreeSet::from([mine]),
        };

        let ids = scoped_context_ids(&store, &scope)
            .unwrap()
            .expect("an account scope enumerates rather than streaming the node");
        assert_eq!(
            ids,
            vec![my_context],
            "the other tenant's context is the disclosure this endpoint had"
        );
    }

    /// `None` means "use the node-wide stream unchanged", which is what keeps a
    /// node owner's view exactly as it was.
    #[test]
    fn a_node_wide_scope_enumerates_nothing_of_its_own() {
        let store = Store::new(std::sync::Arc::new(InMemoryDB::owned()));
        assert!(scoped_context_ids(&store, &ListScope::NodeWide)
            .unwrap()
            .is_none());
    }
}
