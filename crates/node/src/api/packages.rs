//! The API's routes for bundles, package releases and publishers.

#[allow(
    clippy::wildcard_imports,
    reason = "the handlers moved here from `api.rs` read its items"
)]
use super::*;

const MAX_SERVICE_QUERY_BYTES: usize = 128; // a service name in `?service=`

async fn bundle_put(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<Json<BundleStored>> {
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
        .compile(crate::compile::Engine::App, &hash, body.to_vec())
        .await?;
    node.replicate_bundle(&hash, &body).await?;
    let _ok = node
        .sys_write(&SystemCmd::PutBundle {
            hash: hash.clone(),
            size: body.len() as u64,
            actor: account,
            at: now_secs(),
        })
        .await?;
    Ok(Json(BundleStored { hash }))
}

/// Install an uploaded, signed `.mpk`.
async fn package_upload(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<Json<crate::packages::PackageInstalled>> {
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
) -> ApiResult<Json<crate::packages::PackageInstalled>> {
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
pub(super) fn package_json(
    s: &SystemState,
    p: &crate::system::PackageRecord,
    who: &str,
    frontend: bool,
) -> Option<PackageAnswer> {
    let mut versions: Vec<&crate::system::PackageVersion> = p
        .versions
        .values()
        .filter(|v| frontend || s.sees_release(v, who))
        .collect();
    if versions.is_empty() {
        return None;
    }
    versions.sort_by(|a, b| crate::system::compare_versions(&a.version, &b.version));
    Some(PackageAnswer {
        package: p.name.clone(),
        signer_id: p.signer_id.clone(),
        owner: p.owner.clone(),
        latest: versions.last()?.version.clone(),
        versions: versions
            .iter()
            .map(|v| PackageVersionView {
                version: v.version.clone(),
                mpk: v.mpk.clone(),
                size: v.mpk_size,
                source: v.source,
                services: v.services.clone(),
                components: v.components,
                metadata: v.metadata.clone(),
                outbound: v.outbound.clone(),
                installed_by: v.installed_by.clone(),
                installed_at: v.installed_at,
            })
            .collect(),
    })
}

/// A package as the API answers it (`#Package`): the releases the caller sees, oldest first.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PackageAnswer {
    package: String,
    signer_id: String,
    owner: String,
    latest: String,
    versions: Vec<PackageVersionView>,
}

/// One release of a [`PackageAnswer`].
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PackageVersionView {
    version: String,
    mpk: String,
    size: u64,
    source: crate::system::BundleSource,
    services: BTreeMap<String, String>,
    components: crate::types::Components,
    metadata: Value,
    outbound: Vec<String>,
    installed_by: String,
    installed_at: u64,
}

/// The packages the caller may see (`package_json`): on a cluster several
/// organizations share, one organization's releases are not another's to list.
async fn package_list(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
) -> ApiResult<Json<PackageList>> {
    let principal = signed_in(&node, &headers).await?;
    let account = principal.account()?;
    node.sys_fresh_from(principal.sys_index).await;
    let packages: Vec<PackageAnswer> = node.sys(|s| {
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
    Ok(Json(PackageList { packages }))
}

async fn package_get(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(package): Path<String>,
) -> ApiResult<Json<PackageAnswer>> {
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
    Ok(Json(json!(node.release_pipelines(&refs).await?)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CapabilitiesQuery {
    #[serde(default)]
    service: Option<String>,
}

/// A release's capability card, before any app of it exists.
async fn package_capabilities(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((package, version)): Path<(String, String)>,
    Query(q): Query<CapabilitiesQuery>,
) -> ApiResult<Json<CapabilityCard>> {
    let principal = signed_in(&node, &headers).await?;
    if q.service
        .as_ref()
        .is_some_and(|s| s.len() > MAX_SERVICE_QUERY_BYTES)
    {
        return Err(ApiError::bad_request(
            "BAD_REQUEST",
            &format!("service is limited to {MAX_SERVICE_QUERY_BYTES} bytes"),
        ));
    }
    let account = principal.account()?;
    let frontend = principal.bound_app() == Some(package.as_str());
    node.sys_fresh_from(principal.sys_index).await;
    let release = node
        .sys(|s| {
            let v = s.packages.get(&package)?.versions.get(&version)?;
            (frontend || s.sees_release(v, &account)).then(|| v.clone())
        })
        .ok_or_else(|| ApiError::not_found("no such package version"))?;
    let card =
        crate::consent_page::release_capabilities(&node, &package, &release, q.service.as_deref())
            .await?;
    // Where an instance of it could send data: the release and every app its `uses`
    // would create with it (empty: no data leaves Calimero).
    let at = crate::system::PackageRef {
        package: package.clone(),
        version: version.clone(),
        service: card["service"].as_str().map(str::to_owned),
        compute: false,
    };
    // A member release that is not installed here (`409 USES_NOT_INSTALLED` at create)
    // leaves the unit unknown: `null`, never a card the release alone would mislead with.
    let described = crate::egress::release_unit(&node, &at)
        .await
        .ok()
        .map(|unit| {
            crate::egress::describe(&crate::egress::Placed {
                unit,
                apps: std::collections::BTreeMap::new(),
            })
        });
    let mut card: CapabilityCard =
        serde_json::from_value(card).map_err(|e| ApiError::internal(&e.to_string()))?;
    card.destinations = described.as_ref().map(|d| d.destinations.clone());
    card.members = described.map(|d| d.members);
    Ok(Json(card))
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
) -> ApiResult<Json<crate::packages::PackageInstalled>> {
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
) -> ApiResult<Json<crate::packages::PackageInstalled>> {
    operator(&node, &headers)?;
    Ok(Json(
        node.install_from_registry(&body.package, &body.version, crate::system::OPERATOR)
            .await?,
    ))
}

/// The releases the operator installed: publisher metadata, no app data, and
/// nothing an account installed.
async fn ops_package_list(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
) -> ApiResult<Json<PackageList>> {
    operator(&node, &headers)?;
    // An install committed through another node is listed (read index).
    node.sys_fresh().await;
    let packages: Vec<PackageAnswer> = node.sys(|s| {
        s.packages
            .values()
            .filter_map(|p| package_json(s, p, crate::system::OPERATOR, false))
            .collect()
    });
    Ok(Json(PackageList { packages }))
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
) -> ApiResult<Json<PublisherBinding>> {
    let _p = signed_in(&node, &headers).await?;
    node.sys_fresh().await;
    Ok(Json(node.sys(|s| PublisherBinding {
        generation: s.binding_generation(&signer),
        bound: s.publisher_of_signer(&signer).is_some(),
        signer_id: signer.clone(),
        cluster: node.config.cluster.clone(),
    })))
}

/// `GET /publishers/{signer}`: whether a package signing key is bound to an organization here
/// (never which), and its binding generation (`0`: never bound).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PublisherBinding {
    signer_id: String,
    cluster: String,
    generation: u64,
    bound: bool,
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
        .route(
            "/packages/{package}/{version}/capabilities",
            get(package_capabilities),
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

/// `POST /bundles`: the stored WASM's hash (development only).
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct BundleStored {
    hash: String,
}

/// The packages the caller may see.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct PackageList {
    packages: Vec<PackageAnswer>,
}

/// `GET /packages/{package}/{version}/capabilities`: a release's capability card (the release,
/// its signer, outbound prefixes, secrets, `uses` and ABI, as the consent page shows each side
/// of an upgrade), and where an instance of it could send data (`null`: a member release is
/// not installed here).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CapabilityCard {
    package: String,
    version: String,
    service: Option<String>,
    bundle: String,
    mpk: String,
    manifest_hash: String,
    signer: String,
    name: Option<String>,
    author: Option<String>,
    outbound: Vec<String>,
    secret_bindings: Vec<String>,
    secrets: Value,
    uses: Value,
    abi: Value,
    #[serde(default)]
    destinations: Option<Vec<String>>,
    #[serde(default)]
    members: Option<std::collections::BTreeMap<String, crate::egress::UnitMember>>,
}
