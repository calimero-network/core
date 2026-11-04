//! `/package-origins/{package}`: the web origins a package's manager
//! registers for its frontend ([`crate::origins`]), listed, added and removed
//! with the manager's passkey session; adding and removing also with a
//! passkey assertion over the change (`412 CONFIRM_REQUIRED` first).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::{debug, info};

use super::{consent, holder, ApiResult, ConfirmBody};
use crate::error::ApiError;
use crate::node::Node;
use crate::origins::{
    covers_cluster, origin_host, origin_json, resolve_rp_id, MAX_ORIGINS_PER_PACKAGE,
    MAX_PACKAGE_ORIGINS,
};
use crate::system::SystemCmd;
use crate::types::now_secs;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AddOriginBody {
    origin: String,
    /// The relying-party id; the origin's host when absent.
    #[serde(default)]
    rp_id: Option<String>,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RemoveOriginBody {
    origin: String,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

fn bad_origin(why: &str) -> ApiError {
    ApiError::bad_request("BAD_ORIGIN", why)
}

fn taken(why: &str) -> ApiError {
    ApiError::new(409, "ORIGIN_TAKEN", why)
}

/// Refuse unless `account` manages `package`, on fresh state: `404` for a
/// package it does not see (as if not installed), `403` for one it sees.
async fn managed(node: &Node, account: &str, package: &str) -> Result<(), ApiError> {
    let _caught_up = node.sys_fresh().await;
    node.sys(|s| {
        let p = s
            .packages
            .get(package)
            .ok_or_else(|| ApiError::not_found("no such package"))?;
        if s.manages_package(package, account) {
            return Ok(());
        }
        if p.versions.values().any(|v| s.sees_release(v, account)) {
            return Err(ApiError::forbidden(
                "only the package's manager handles its origins: its publisher's MANAGE_APPLICATION holders, or whoever installed it first",
            ));
        }
        Err(ApiError::not_found("no such package"))
    })
}

/// The origins registered for `package`, for its manager.
pub(super) async fn list(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(package): Path<String>,
) -> ApiResult {
    let account = holder(&node, &headers).await?.account()?;
    managed(&node, &account, &package).await?;
    let origins: Vec<Value> = node.sys(|s| {
        s.origins_of(&package)
            .map(|(origin, o)| origin_json(origin, o))
            .collect()
    });
    Ok(Json(json!({ "package": package, "origins": origins })))
}

/// Register an origin for `package`, with the manager's passkey assertion
/// over `package:{package} / __add_origin / { origin, rpId }` (the rp-id as
/// the node resolves it).
pub(super) async fn add(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(package): Path<String>,
    Json(body): Json<AddOriginBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    let origin = body.origin;
    let rp_id = match resolve_rp_id(&origin, body.rp_id.as_deref()) {
        Ok(rp_id) => rp_id,
        Err(crate::origins::Refusal::Origin) => {
            return Err(bad_origin(crate::origins::Refusal::Origin.message()))
        }
        Err(r) => return Err(ApiError::bad_request("BAD_RP_ID", r.message())),
    };
    // `origin_host` accepted `http` only for a localhost name; this node
    // takes one only in development.
    if origin.starts_with("http://") && !node.config.allow_package_uploads {
        return Err(bad_origin(
            "a localhost origin is registered on a development node only (--dev); use https",
        ));
    }
    let _host = origin_host(&origin).map_err(|r| bad_origin(r.message()))?;
    if covers_cluster(&rp_id, &node.config.rp_id) {
        return Err(ApiError::bad_request(
            "BAD_RP_ID",
            "that rpId is the cluster's (or a parent of it): its passkeys stay on the cluster's own origins",
        ));
    }
    if node
        .config
        .origins
        .iter()
        .any(|o| o.trim_end_matches('/') == origin)
        || node.app_origin_package(&origin).is_some()
    {
        return Err(taken(
            "that is one of the cluster's own origins, or a frontend it serves",
        ));
    }
    managed(&node, &account, &package).await?;
    node.sys(|s| {
        match s.package_origins.get(&origin) {
            Some(o) if o.package != package => {
                return Err(taken("another package registered that origin"))
            }
            Some(_) => return Ok(()),
            None => {}
        }
        if s.origins_of(&package).count() >= MAX_ORIGINS_PER_PACKAGE {
            return Err(ApiError::new(
                429,
                "QUOTA_EXCEEDED",
                &format!("a package registers at most {MAX_ORIGINS_PER_PACKAGE} origins"),
            ));
        }
        if s.package_origins.len() >= MAX_PACKAGE_ORIGINS {
            return Err(ApiError::new(
                429,
                "QUOTA_EXCEEDED",
                "this cluster holds as many origins as it takes",
            ));
        }
        Ok(())
    })?;
    let consent = consent(
        &node,
        &p,
        &format!("package:{package}"),
        "__add_origin",
        &json!({ "origin": origin, "rpId": rp_id }),
        body.confirm,
    )
    .await?;
    let out = node
        .sys_write(&SystemCmd::AddPackageOrigin {
            package: package.clone(),
            origin: origin.clone(),
            rp_id: rp_id.clone(),
            actor: account.clone(),
            at: now_secs(),
            consent,
        })
        .await?;
    // Who did it is the log's to keep (the command's actor), not the journal's.
    info!(%package, %origin, %rp_id, "registered an origin for a package");
    debug!(%package, %origin, %account, "the origin's registrant");
    Ok(Json(out))
}

/// Remove an origin of `package`, with the manager's passkey assertion over
/// `package:{package} / __remove_origin / { origin }`. The sessions opened on
/// it are refused from then on.
pub(super) async fn remove(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(package): Path<String>,
    Json(body): Json<RemoveOriginBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    managed(&node, &account, &package).await?;
    let origin = body.origin;
    let held = node.sys(|s| {
        s.package_origins
            .get(&origin)
            .is_some_and(|o| o.package == package)
    });
    if !held {
        return Err(ApiError::not_found(
            "that origin is not registered for this package",
        ));
    }
    let consent = consent(
        &node,
        &p,
        &format!("package:{package}"),
        "__remove_origin",
        &json!({ "origin": origin }),
        body.confirm,
    )
    .await?;
    let out = node
        .sys_write(&SystemCmd::RemovePackageOrigin {
            package: package.clone(),
            origin: origin.clone(),
            actor: account.clone(),
            consent,
        })
        .await?;
    info!(%package, %origin, "removed an origin of a package");
    debug!(%package, %origin, %account, "the origin's remover");
    Ok(Json(out))
}
