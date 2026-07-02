use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::{InstallApplicationResponse, InstallDevApplicationRequest};
use tracing::{debug, error, info, warn};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

/// Env var that, when set, confines `install-dev-application` to a directory.
///
/// This endpoint reads an arbitrary node-local filesystem path and exposes its
/// bytes as a blob. It is admin-auth gated and rejects `..` traversal, and
/// reading the node owner's own files is not a privilege boundary — so absolute
/// paths are allowed by default. Operators who want to lock the endpoint down
/// (e.g. a shared/managed node) can set `MEROD_DEV_INSTALL_ROOT` to a directory;
/// dev installs then only succeed for paths that resolve within it.
///
/// Note: when confinement is enabled the target must exist at request time —
/// confinement resolves the path with `canonicalize`, which fails (request
/// refused) for a missing file or a dangling symlink.
const DEV_INSTALL_ROOT_ENV: &str = "MEROD_DEV_INSTALL_ROOT";

/// When `MEROD_DEV_INSTALL_ROOT` is set, require `path` to resolve within it.
/// Returns `Ok(())` when unset (historical behavior) or when confined.
///
/// The `Err` string is deliberately generic — detailed reasons (including the
/// operator-configured root and OS errno) are logged server-side only, so the
/// endpoint never discloses server filesystem layout to the caller.
async fn check_dev_install_confinement(path: &str) -> Result<(), String> {
    let Ok(root) = std::env::var(DEV_INSTALL_ROOT_ENV) else {
        return Ok(());
    };
    // Use the async filesystem API so the stat/readlink syscalls don't block the
    // executor thread.
    let canon_root = tokio::fs::canonicalize(&root).await.map_err(|e| {
        error!(root = %root, error = %e, "invalid MEROD_DEV_INSTALL_ROOT");
        "server misconfiguration: dev-install root is unavailable".to_owned()
    })?;
    let canon_path = tokio::fs::canonicalize(path).await.map_err(|e| {
        warn!(path = %path, error = %e, "dev-install path could not be resolved");
        "install path could not be resolved".to_owned()
    })?;
    // `Path::starts_with` is component-aware, so a root of "/srv/app" does NOT
    // match "/srv/application/x" — no string-prefix false positives.
    if canon_path.starts_with(&canon_root) {
        Ok(())
    } else {
        warn!(path = %path, "dev-install path is outside the configured root");
        Err("install path is not permitted".to_owned())
    }
}

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<InstallDevApplicationRequest>,
) -> impl IntoResponse {
    info!(path=%req.path, "Installing dev application");

    // Detailed reason is logged inside the check; `msg` is a generic,
    // caller-safe string.
    if let Err(msg) = check_dev_install_confinement(req.path.as_str()).await {
        return ApiError {
            status_code: StatusCode::FORBIDDEN,
            message: msg,
        }
        .into_response();
    }
    let metadata_len = req.metadata.len();
    debug!(
        path=%req.path,
        metadata_len,
        package = req.package.as_deref().unwrap_or("unknown"),
        version = req.version.as_deref().unwrap_or("0.0.0"),
        "install_dev_application request received"
    );

    match state
        .node_client
        .install_application_from_path(
            req.path.clone(),
            req.metadata,
            req.package.clone(),
            req.version.clone(),
        )
        .await
    {
        Ok(application_id) => {
            info!(application_id=%application_id, "Dev application installed successfully");
            ApiResponse {
                payload: InstallApplicationResponse::new(application_id),
            }
            .into_response()
        }
        Err(err) => {
            error!(
                path=%req.path,
                package = req.package.as_deref().unwrap_or("unknown"),
                version = req.version.as_deref().unwrap_or("0.0.0"),
                error = ?err,
                "Failed to install dev application"
            );
            (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response()
        }
    }
}
