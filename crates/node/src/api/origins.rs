//! `/package-origins/{package}`: the web origins a package's manager
//! registers for its frontend ([`crate::origins`]), listed, added and removed
//! with the manager's passkey session; adding and removing also with a
//! passkey assertion over the change (`412 CONFIRM_REQUIRED` first). An
//! origin added with `served: true` is a custom domain the cluster serves the
//! frontend on (`app_host`); `GET /ops/served-origins` lists every one for
//! the operator (mdma), which orders their certificates and routes them.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, info};

use super::{consent, holder, operator, ApiResult, ConfirmBody};
use crate::app_host::label;
use crate::error::ApiError;
use crate::node::Node;
use crate::origins::{
    add_args, covers_cluster, new_registration, origin_host, origin_json, resolve_rp_id,
    served_host, within, MAX_ORIGINS_PER_PACKAGE, MAX_SERVED_ORIGINS,
    MAX_SERVED_ORIGINS_PER_PACKAGE,
};
use crate::system::cluster_version::Feature;
use crate::system::SystemCmd;
use crate::types::now_secs;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AddOriginBody {
    origin: String,
    /// The relying-party id; the origin's host when absent.
    #[serde(default)]
    rp_id: Option<String>,
    /// The cluster serves the package's frontend on this origin too (a
    /// custom domain).
    #[serde(default)]
    served: bool,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RemoveOriginBody {
    pub(super) origin: String,
    #[serde(default)]
    pub(super) confirm: Option<ConfirmBody>,
}

fn bad_origin(why: &str) -> ApiError {
    ApiError::bad_request("BAD_ORIGIN", why)
}

pub(super) fn taken(why: &str) -> ApiError {
    ApiError::new(409, "ORIGIN_TAKEN", why)
}

/// Refuse unless `account` manages `package`, on fresh state: `404` for a
/// package it does not see (as if not installed), `403` for one it sees.
async fn managed(node: &Node, account: &str, package: &str) -> Result<(), ApiError> {
    let _caught_up = node.sys_fresh().await;
    node.sys(|s| {
        if s.manages_package(package, account) {
            return Ok(());
        }
        if s.package_used(package, account, false).is_some() {
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
    let origins: Vec<crate::origins::OriginView> = node.sys(|s| {
        s.origins_of(&package)
            .map(|(origin, o)| origin_json(origin, o))
            .collect()
    });
    Ok(Json(json!(OriginList { package, origins })))
}

/// `GET /package-origins/{package}`: the origins registered for a package.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct OriginList {
    package: String,
    origins: Vec<crate::origins::OriginView>,
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
    let rp_id = registrable(&node, &origin, body.rp_id.as_deref())?;
    if body.served {
        served_fits(&node, &origin)?;
    }
    managed(&node, &account, &package).await?;
    let served = body.served;
    node.sys(|s| {
        match s.package_origins.get(&origin) {
            Some(o) if o.package != package => {
                return Err(taken("another package or app registered that origin"))
            }
            // An instance of the package holds it: its manager takes it back.
            Some(o) if o.app.is_some() => {}
            Some(o) if o.served != served => {
                return Err(taken(
                    "that origin is registered with the other `served`; remove it first",
                ))
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
        if served
            && s.origins_of(&package).filter(|(_, o)| o.served).count()
                >= MAX_SERVED_ORIGINS_PER_PACKAGE
        {
            return Err(ApiError::new(
                429,
                "QUOTA_EXCEEDED",
                &format!(
                    "a package has the cluster serve it on at most {MAX_SERVED_ORIGINS_PER_PACKAGE} origins"
                ),
            ));
        }
        if served && s.package_origins.values().filter(|o| o.served).count() >= MAX_SERVED_ORIGINS {
            return Err(ApiError::new(
                429,
                "QUOTA_EXCEEDED",
                "this cluster serves as many custom domains as it takes",
            ));
        }
        if s.origins_full(false) {
            return Err(ApiError::new(
                429,
                "QUOTA_EXCEEDED",
                "this cluster holds as many origins as it takes",
            ));
        }
        Ok(())
    })?;
    if served {
        // No node that would read the record without `served`, and so hold
        // other state than the rest, may be in the cluster.
        node.require_upgraded_nodes().await?;
    }
    let consent = consent(
        &node,
        &p,
        &format!("package:{package}"),
        "__add_origin",
        &add_args(&origin, &rp_id, served),
        body.confirm,
    )
    .await?;
    // An earlier release would drop the registration and hold other state.
    let registration = node
        .require_features(&[Feature::AppOrigins])
        .await
        .map_or_else(|_| String::new(), |()| new_registration());
    let out = node
        .sys_write(&SystemCmd::AddPackageOrigin {
            package: package.clone(),
            origin: origin.clone(),
            rp_id: rp_id.clone(),
            served,
            registration,
            actor: account.clone(),
            at: now_secs(),
            consent,
        })
        .await?;
    // Who did it is the log's to keep (the command's actor), not the journal's.
    info!(%package, %origin, %rp_id, served, "registered an origin for a package");
    debug!(%package, %origin, %account, "the origin's registrant");
    Ok(Json(out))
}

/// The rp-id `origin` is registered with: `rp_id`, or the origin's host. Refused: an
/// origin or rp-id that does not fit, the cluster's own, or a frontend it serves.
pub(super) fn registrable(
    node: &Node,
    origin: &str,
    rp_id: Option<&str>,
) -> Result<String, ApiError> {
    let rp_id = match resolve_rp_id(origin, rp_id) {
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
    let _host = origin_host(origin).map_err(|r| bad_origin(r.message()))?;
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
        || node.app_origin_package(origin).is_some()
    {
        return Err(taken(
            "that is one of the cluster's own origins, or a frontend it serves",
        ));
    }
    Ok(rp_id)
}

/// Refuse a served origin (a custom domain) the cluster cannot or must not
/// serve: one with a port, on a node that serves no frontends, under the apps
/// domain (served there already), on a node's API host, or under the
/// cluster's rp-id (whose pages must stay the cluster's own).
fn served_fits(node: &Node, origin: &str) -> Result<(), ApiError> {
    let host = served_host(origin).map_err(|r| bad_origin(r.message()))?;
    let Some(domain) = node.config.apps_domain.as_deref() else {
        return Err(ApiError::bad_request(
            "NOT_SERVING",
            "this cluster serves no frontends (merod --apps-domain); register the origin unserved",
        ));
    };
    if within(&host, domain) {
        return Err(bad_origin(
            "that name is under the cluster's apps domain, where the frontend is served already",
        ));
    }
    let rp = node.config.rp_id.as_str();
    if rp != "localhost" && within(&host, rp) {
        return Err(bad_origin(
            "that name is under the cluster's rpId: its pages stay the cluster's own",
        ));
    }
    if node.is_api_host(&host) {
        return Err(taken("that is a node's own API host"));
    }
    Ok(())
}

/// Every served origin of the cluster (custom domains), for the operator:
/// mdma orders each one's certificate for the cluster's attested nodes and
/// routes its name here, but only for what the log holds. As the rest of the
/// Ops API, no package name and no account: the package's [`label`], which
/// mdma already routes by, ties a domain to its package.
pub(super) async fn ops_served(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
) -> ApiResult<Json<ServedOrigins>> {
    operator(&node, &headers)?;
    let fresh = node.sys_fresh().await;
    let served: Vec<ServedOrigin> = node.sys(|s| {
        s.package_origins
            .iter()
            .filter(|(_, o)| o.served)
            .filter_map(|(origin, o)| {
                Some(ServedOrigin {
                    origin: origin.clone(),
                    host: served_host(origin).ok()?,
                    label: label(&o.package),
                    added_at: crate::system::rfc3339(o.added_at),
                })
            })
            .collect()
    });
    Ok(Json(ServedOrigins {
        fresh,
        origins: served,
    }))
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
            .is_some_and(|o| o.package == package && o.app.is_none())
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

/// `GET /ops/served-origins`: the origins the operator serves frontends on, each with its host,
/// its package's label and when it was added (RFC 3339).
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ServedOrigins {
    fresh: bool,
    origins: Vec<ServedOrigin>,
}

/// An origin the operator serves a package's frontend on.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ServedOrigin {
    origin: String,
    host: String,
    label: String,
    added_at: String,
}
