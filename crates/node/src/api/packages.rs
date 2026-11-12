//! The API's routes for bundles, package releases and publishers.

#[allow(
    clippy::wildcard_imports,
    reason = "the handlers moved here from `api.rs` read its items"
)]
use super::*;

async fn bundle_put(State(node): State<Arc<Node>>, headers: HeaderMap, body: Body) -> ApiResult {
    let account = installer(&node, &headers).await?.account()?;
    node.refuse_undeployable(&account).await?;
    if !node.config.allow_unsigned_bundles {
        return Err(ApiError::forbidden(
            "this node takes signed packages only (POST /packages); raw WASM is for development",
        ));
    }
    if body.len() > 16 * 1024 * 1024 {
        return Err(ApiError::new(
            413,
            "TOO_LARGE",
            "bundles are limited to 16 MiB",
        ));
    }
    let hash = hex::encode(sha256(&body));
    // Refuse what no app could run before replicating it: a core module (an SDK
    // 0.12 app) is `400 UNSUPPORTED_BUNDLE`; a component must pass the engine's
    // prepare, else `400 BAD_BUNDLE`.
    if crate::types::Executor::of(&body) != crate::types::Executor::Component {
        return Err(ApiError::unsupported_bundle());
    }
    let _prepared = node
        .prepare_bytes(&hash, body.to_vec())
        .await
        .map_err(|e| {
            if e.code == crate::node::BAD_BUNDLE {
                ApiError::bad_request(crate::node::BAD_BUNDLE, &e.message)
            } else {
                e
            }
        })?;
    node.replicate_bundle(&hash, &body).await?;
    let _ok = node
        .sys_write(&SystemCmd::PutBundle {
            hash: hash.clone(),
            size: body.len() as u64,
            actor: account,
            at: now_secs(),
        })
        .await?;
    Ok(Json(json!({ "hash": hash })))
}

/// Install an uploaded, signed `.mpk`.
async fn package_upload(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult {
    let account = installer(&node, &headers).await?.account()?;
    uploads_allowed(&node)?;
    Ok(Json(
        node.install_upload(Arc::from(body.to_vec()), &account)
            .await?,
    ))
}

/// Uploads are for development: a production cluster installs every release
/// from its registry, which any node can fetch it from again.
fn uploads_allowed(node: &Node) -> Result<(), ApiError> {
    if node.config.allow_package_uploads {
        return Ok(());
    }
    Err(ApiError::new(
        403,
        "UPLOADS_DISABLED",
        "this node installs packages from its registry only (POST /packages/install); uploads are for development (--dev)",
    ))
}

#[derive(Deserialize)]
pub(super) struct InstallBody {
    package: String,
    version: String,
}

/// Install `package@version` from the registry: verified here, then fetched
/// and verified again by every node that runs it.
async fn package_install(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<InstallBody>,
) -> ApiResult {
    let account = installer(&node, &headers).await?.account()?;
    Ok(Json(
        node.install_from_registry(&body.package, &body.version, &account)
            .await?,
    ))
}

/// `p` as `who` (an account, or `system::OPERATOR`) may see it: only the
/// releases [`SystemState::sees_release`] lets them see, `None` when that is
/// none, so a package they may not use is not listed and answers `404`.
/// `frontend`: `who` came through `p`'s own frontend, public by name, and
/// sees every release of it.
fn package_json(
    s: &SystemState,
    p: &crate::system::PackageRecord,
    who: &str,
    frontend: bool,
) -> Option<Value> {
    let mut versions: Vec<&crate::system::PackageVersion> = p
        .versions
        .values()
        .filter(|v| frontend || s.sees_release(v, who))
        .collect();
    if versions.is_empty() {
        return None;
    }
    versions.sort_by(|a, b| crate::system::compare_versions(&a.version, &b.version));
    Some(json!({
        "package": p.name,
        "signerId": p.signer_id,
        "owner": p.owner,
        "latest": versions.last().map(|v| v.version.clone()),
        "versions": versions.iter().map(|v| json!({
            "version": v.version,
            "mpk": v.mpk,
            "size": v.mpk_size,
            "source": v.source,
            "services": v.services,
            "components": v.components,
            "metadata": v.metadata,
            "outbound": v.outbound,
            "installedBy": v.installed_by,
            "installedAt": v.installed_at,
        })).collect::<Vec<_>>(),
    }))
}

/// The packages the caller may see (`package_json`): on a cluster several
/// organizations share, one organization's releases are not another's to list.
async fn package_list(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    let principal = signed_in(&node, &headers).await?;
    let account = principal.account()?;
    node.sys_fresh_from(principal.sys_index).await;
    let packages: Vec<Value> = node.sys(|s| {
        s.packages
            .values()
            .filter_map(|p| {
                package_json(
                    s,
                    p,
                    &account,
                    principal.bound_app() == Some(p.name.as_str()),
                )
            })
            .collect()
    });
    Ok(Json(json!({ "packages": packages })))
}

async fn package_get(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(package): Path<String>,
) -> ApiResult {
    let principal = signed_in(&node, &headers).await?;
    let account = principal.account()?;
    let frontend = principal.bound_app() == Some(package.as_str());
    node.sys_fresh_from(principal.sys_index).await;
    node.sys(|s| package_json(s, s.packages.get(&package)?, &account, frontend))
        .map(Json)
        .ok_or_else(|| ApiError::not_found("no such package"))
}

/// A release's registered pipelines: name to hash, worst-case fuel and canonical JSON.
async fn package_pipelines(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((package, version)): Path<(String, String)>,
) -> ApiResult {
    let principal = signed_in(&node, &headers).await?;
    let account = principal.account()?;
    let frontend = principal.bound_app() == Some(package.as_str());
    node.sys_fresh_from(principal.sys_index).await;
    let refs = node
        .sys(|s| {
            let v = s.packages.get(&package)?.versions.get(&version)?;
            (frontend || s.sees_release(v, &account)).then(|| v.pipelines.clone())
        })
        .ok_or_else(|| ApiError::not_found("no such package version"))?;
    Ok(Json(node.release_pipelines(&refs).await?))
}

async fn package_uninstall(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(package): Path<String>,
) -> ApiResult {
    let account = holder(&node, &headers).await?.account()?;
    Ok(Json(
        node.sys_write(&SystemCmd::UninstallPackage {
            package,
            version: None,
            actor: account,
        })
        .await?,
    ))
}

async fn package_version_uninstall(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((package, version)): Path<(String, String)>,
) -> ApiResult {
    let account = holder(&node, &headers).await?.account()?;
    Ok(Json(
        node.sys_write(&SystemCmd::UninstallPackage {
            package,
            version: Some(version),
            actor: account,
        })
        .await?,
    ))
}

/// The operator installs an uploaded, signed `.mpk` (where it upgrades apps,
/// it needs the releases it upgrades to). Checked as any install is.
async fn ops_package_upload(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult {
    operator(&node, &headers)?;
    uploads_allowed(&node)?;
    Ok(Json(
        node.install_upload(Arc::from(body.to_vec()), crate::system::OPERATOR)
            .await?,
    ))
}

/// The operator installs `package@version` from the registry.
async fn ops_package_install(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<InstallBody>,
) -> ApiResult {
    operator(&node, &headers)?;
    Ok(Json(
        node.install_from_registry(&body.package, &body.version, crate::system::OPERATOR)
            .await?,
    ))
}

/// The releases the operator installed: publisher metadata, no app data, and
/// nothing an account installed.
async fn ops_package_list(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    operator(&node, &headers)?;
    // An install committed through another node is listed (read index).
    node.sys_fresh().await;
    let packages: Vec<Value> = node.sys(|s| {
        s.packages
            .values()
            .filter_map(|p| package_json(s, p, crate::system::OPERATOR, false))
            .collect()
    });
    Ok(Json(json!({ "packages": packages })))
}

/// The operator uninstalls a package it installed.
async fn ops_package_uninstall(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(package): Path<String>,
) -> ApiResult {
    operator(&node, &headers)?;
    Ok(Json(
        node.sys_write(&SystemCmd::UninstallPackage {
            package,
            version: None,
            actor: crate::system::OPERATOR.to_owned(),
        })
        .await?,
    ))
}

async fn ops_package_version_uninstall(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((package, version)): Path<(String, String)>,
) -> ApiResult {
    operator(&node, &headers)?;
    Ok(Json(
        node.sys_write(&SystemCmd::UninstallPackage {
            package,
            version: Some(version),
            actor: crate::system::OPERATOR.to_owned(),
        })
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PublisherBody {
    signer_id: String,
    public_key: String,
    signature: String,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// Bind a package signing key to the organization: the key's Ed25519
/// signature over `system::publisher_binding_message` (this cluster's name,
/// the organization, the key, the key's binding generation from
/// `GET /publishers/{signer}`), by a member holding `APPROVE_RELEASES`, with a
/// passkey assertion. The organization then approves the node images every
/// instance of the key's packages runs on, and upgrades those whose package
/// says `upgrades: publisher`. Binding again moves the key, with a new
/// signature from it.
async fn org_publisher(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<PublisherBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    node.sys_fresh_from(p.sys_index).await;
    let (member, holds, generation) = node.sys(|s| {
        (
            s.orgs.get(&org).is_some_and(|o| o.role(&account).is_some()),
            s.holds_approve_releases(&org, &account),
            s.binding_generation(&body.signer_id),
        )
    });
    if !member {
        return Err(ApiError::not_found("no such organization"));
    }
    if !holds {
        return Err(ApiError::forbidden(
            "binding a publisher key needs APPROVE_RELEASES in the organization",
        ));
    }
    let cluster = node.config.cluster.clone();
    let proof = crate::system::PublisherProof {
        public_key: body.public_key,
        signature: body.signature,
    };
    let message =
        crate::system::publisher_binding_message(&cluster, &org, &body.signer_id, generation);
    if !proof.proves(&body.signer_id, &message) {
        return Err(ApiError::forbidden(
            "the proof is not that key's signature over this binding",
        ));
    }
    let consent = consent(
        &node,
        &p,
        &format!("org:{org}"),
        "__bind_publisher",
        &json!({ "signerId": body.signer_id, "generation": generation }),
        body.confirm,
    )
    .await?;
    Ok(Json(
        node.sys_write(&SystemCmd::BindPublisher {
            signer_id: body.signer_id,
            org,
            cluster,
            proof,
            actor: account,
            consent,
        })
        .await?,
    ))
}

/// What a key signs to bind it here next: this cluster's name and the key's
/// binding generation, and whether it is bound now. For anyone signed in.
async fn publisher_binding(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(signer): Path<String>,
) -> ApiResult {
    let _p = signed_in(&node, &headers).await?;
    node.sys_fresh().await;
    Ok(Json(node.sys(|s| {
        json!({
            "signerId": signer,
            "cluster": node.config.cluster,
            "generation": s.binding_generation(&signer),
            "bound": s.publisher_of_signer(&signer).is_some(),
        })
    })))
}

/// This area's routes, merged into [`super::router`].
pub(super) fn routes() -> Router<Arc<Node>> {
    Router::new()
        .route("/bundles", post(bundle_put))
        .route("/packages", post(package_upload).get(package_list))
        .route("/packages/install", post(package_install))
        .route(
            "/packages/{package}",
            get(package_get).delete(package_uninstall),
        )
        .route(
            "/packages/{package}/{version}",
            delete(package_version_uninstall),
        )
        .route(
            "/packages/{package}/{version}/pipelines",
            get(package_pipelines),
        )
        .route("/orgs/{org}/publishers", post(org_publisher))
        .route("/publishers/{signer}", get(publisher_binding))
        .route(
            "/ops/packages",
            post(ops_package_upload).get(ops_package_list),
        )
        .route("/ops/packages/install", post(ops_package_install))
        .route("/ops/packages/{package}", delete(ops_package_uninstall))
        .route(
            "/ops/packages/{package}/{version}",
            delete(ops_package_version_uninstall),
        )
}
