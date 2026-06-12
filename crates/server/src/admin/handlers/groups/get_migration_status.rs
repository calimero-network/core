use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::{
    GetMigrationStatusRequest, MemberMigrationReport, MigrationFailureKind,
};
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    GetMigrationStatusApiResponse, MemberMigrationReportApiData, MemberMigrationStatusApiEntry,
    MigrationStatusRollupApiData,
};
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

/// `GET /admin/contexts/migration-status/{namespace_id}` — the operator-facing
/// "have all peers migrated?" rollup (Task 6c.10).
///
/// Mirrors [`super::get_cascade_status::handler`]: parse the namespace id,
/// dispatch the read to the context actor, map the typed result into the admin
/// JSON shape. The per-member heartbeat reports are snapshotted from the
/// node-side TTL cache (Task 6c.8) via `NodeClient::migration_status_reports`
/// and threaded into the rollup request, because the context actor cannot reach
/// that node-local cache itself. Observability only — never gates a write.
pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let namespace_id = match parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(namespace_id=%namespace_id_str, "Getting migration status");

    // Snapshot the node-side migration-heartbeat cache and map the transport DTO
    // into the rollup's report type. A member absent here resolves to `unknown`.
    let member_reports: BTreeMap<PublicKey, MemberMigrationReport> = match state
        .node_client
        .migration_status_reports(namespace_id.to_bytes())
        .await
    {
        Ok(reports) => reports
            .into_iter()
            .map(|(peer, r)| {
                (
                    peer,
                    MemberMigrationReport {
                        schema_version: r.schema_version,
                        residue_auto: r.residue_auto,
                        residue_identity: r.residue_identity,
                        synced_up_to_hlc: r.synced_up_to_hlc,
                        reported_at: r.reported_at,
                        authored_remaining: r.authored_remaining,
                        migration_failed: MigrationFailureKind::from_u8(r.migration_failed),
                    },
                )
            })
            .collect(),
        Err(err) => {
            error!(namespace_id=%namespace_id_str, error=?err, "Failed to read migration heartbeat cache");
            return parse_api_error(err).into_response();
        }
    };

    let result = state
        .ctx_client
        .get_migration_status(GetMigrationStatusRequest {
            namespace_id,
            member_reports,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(status) => {
            let members = status
                .members
                .into_iter()
                .map(|m| MemberMigrationStatusApiEntry {
                    peer: m.peer,
                    report: m.report.map(|r| MemberMigrationReportApiData {
                        schema_version: r.schema_version,
                        residue_auto: r.residue_auto,
                        residue_identity: r.residue_identity,
                        synced_up_to_hlc: r.synced_up_to_hlc,
                        reported_at: r.reported_at,
                        authored_remaining: r.authored_remaining,
                        migration_failed: r.migration_failed.map(|k| k.as_str().to_owned()),
                    }),
                    state: m.state.as_str().to_owned(),
                })
                .collect();

            ApiResponse {
                payload: GetMigrationStatusApiResponse {
                    target_version: status.target_version,
                    expected_members: status.expected_members,
                    cohort_pinned_at_hlc: status.cohort_pinned_at_hlc.map(|ts| ts.to_string()),
                    rollup: MigrationStatusRollupApiData {
                        migrated: status.rollup.migrated,
                        in_progress: status.rollup.in_progress,
                        unknown: status.rollup.unknown,
                        failed: status.rollup.failed,
                        total: status.rollup.total,
                        all_migrated: status.rollup.all_migrated,
                        members_pending_signature: status.rollup.members_pending_signature,
                    },
                    members,
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(namespace_id=%namespace_id_str, error=?err, "Failed to get migration status");
            err.into_response()
        }
    }
}
