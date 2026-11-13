//! The API's routes for the app directory: instances, their metadata, access, transfers and moves.

#[allow(
    clippy::wildcard_imports,
    reason = "the handlers moved here from `api.rs` read its items"
)]
use super::*;

/// `GET /apps/{app}/attestation`'s query: only the nonce, since the app is the path's and
/// the purpose is always `receipts`. Other parameters are ignored, as they always were.
#[derive(Deserialize)]
pub(super) struct AppAttestQuery {
    nonce: String,
}

/// Every replica of an app attests its receipt key over `nonce`: what a user
/// checks to know the whole group holding the app is genuine, and which keys
/// sign its receipts.
async fn app_attestation(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Query(q): Query<AppAttestQuery>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "*", false).await?;
    let mut replicas = Vec::new();
    for id in &app.replicas {
        let Some(addr) = node.sys(|s| s.nodes.get(id).map(|n| n.addr.clone())) else {
            continue;
        };
        let doc: Result<Value, String> = async {
            let mut url: url::Url = addr.parse().map_err(|e| format!("{e}"))?;
            url = url.join("attestation").map_err(|e| format!("{e}"))?;
            let _q = url
                .query_pairs_mut()
                .append_pair("nonce", &q.nonce)
                .append_pair("purpose", "receipts");
            node.http
                .get(url)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|e| e.to_string())?
                .json::<Value>()
                .await
                .map_err(|e| e.to_string())
        }
        .await;
        replicas.push(match doc {
            Ok(d) => json!({ "node": id, "attestation": d }),
            Err(e) => json!({ "node": id, "error": e }),
        });
    }
    // The cluster, said over the caller's attested channel: what each
    // replica's quote must bind, since the documents came in the clear.
    Ok(Json(
        json!({ "appId": app_id, "cluster": node.config.cluster, "replicas": replicas }),
    ))
}

/// `422 INIT_FAILED`: app `app_id`'s `init` failed with the app log's `error`.
pub(super) fn init_failed(app_id: &str, error: &Value) -> ApiError {
    ApiError::new(422, "INIT_FAILED", "the app's init failed")
        .with_details(json!({ "appId": app_id, "error": error }))
}

/// A release's service name to bundle hash map.
type Services = BTreeMap<String, String>;

/// The bundle an instance of `package` from deployment `id` runs: the
/// release the deployment offers for it (`version`, when given, must be that
/// release), service `service`. `404` for an unknown deployment; `400
/// NOT_OFFERED` for a package or version it does not offer. `CreateApp`
/// checks the offer again where it applies.
async fn offered_bundle(
    node: &Node,
    id: &str,
    package: &str,
    version: Option<&str>,
    service: Option<&str>,
) -> Result<(String, Option<String>, PackageRef), ApiError> {
    let find = || {
        node.sys(|s| {
            let d = s.deployments.get(id)?;
            let mut releases: Vec<(String, Services, Services)> = d
                .apps
                .values()
                .filter(|o| o.package == package && version.is_none_or(|v| v == o.version))
                .filter_map(|o| {
                    let r = s.offered_release(o)?;
                    Some((o.version.clone(), r.app_parts().clone(), r.compute.clone()))
                })
                .collect();
            releases.sort_by(|a, b| a.0.cmp(&b.0));
            releases.dedup_by(|a, b| a.0 == b.0);
            Some(releases)
        })
    };
    let releases = match find() {
        Some(r) => r,
        None => {
            let _caught_up = node.sys_fresh().await;
            find().ok_or_else(|| ApiError::not_found("no such deployment"))?
        }
    };
    let (version, services, compute) = match releases.as_slice() {
        [] => {
            return Err(ApiError::bad_request(
                "NOT_OFFERED",
                &format!(
                    "the deployment does not offer {package}{}",
                    version.map(|v| format!("@{v}")).unwrap_or_default()
                ),
            ))
        }
        [one] => one,
        _ => {
            return Err(ApiError::bad_request(
                "VERSION_REQUIRED",
                &format!("the deployment offers several releases of {package}; name one"),
            ))
        }
    };
    let key = service.unwrap_or_default();
    let release = PackageRef {
        package: package.to_owned(),
        version: version.clone(),
        service: (!key.is_empty()).then(|| key.to_owned()),
        compute: false,
    };
    let found = services
        .get(key)
        .map(|b| (b.clone(), compute.get(key).cloned(), release));
    found.ok_or_else(|| {
        ApiError::bad_request(
            "NO_SUCH_SERVICE",
            &format!("{package}@{version} has no service {key:?}"),
        )
    })
}

/// The organization an instance that names none goes to: `owner`'s personal
/// organization ([`crate::types::Org::personal`]), made now if they have none.
async fn personal_org(node: &Node, owner: &str) -> Result<String, ApiError> {
    let find = || node.sys(|s| s.personal_org_of(owner).cloned());
    if let Some(org) = find() {
        return Ok(org);
    }
    let _caught_up = node.sys_fresh().await;
    if let Some(org) = find() {
        return Ok(org);
    }
    let org_id = typed_id::<16>();
    let made = node
        .sys_write(&SystemCmd::CreateOrg {
            org_id: org_id.clone(),
            name: crate::system::PERSONAL_ORG_NAME.to_owned(),
            actor: owner.to_owned(),
            release: Some(node.own_release()),
            recovery_key: None,
            at: recovery_now(node),
            name_fits: true,
            personal: true,
            app_team: None,
        })
        .await;
    match made {
        Ok(_) => Ok(org_id),
        // Another request of the same account made it first: use that one.
        Err(e) if e.code == crate::node::PERSONAL_ORG_EXISTS => {
            let _caught_up = node.sys_fresh().await;
            find().ok_or_else(|| {
                ApiError::unavailable("the personal organization is not readable yet")
            })
        }
        Err(e) => Err(e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreateAppBody {
    #[serde(default)]
    name: String,
    /// A bundle hash (a raw upload), or `package` (+ `version`, `service`).
    #[serde(default)]
    bundle: Option<String>,
    #[serde(default)]
    package: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    service: Option<String>,
    #[serde(default)]
    org: Option<String>,
    #[serde(default)]
    access: Option<Access>,
    /// Arguments for the app's `init`, as JSON.
    #[serde(default)]
    init_args: Value,
    /// The instance's metadata.
    #[serde(default)]
    metadata: Value,
    /// The deployment to create it from: then `package` names one of the
    /// packages it offers (and `version`, if given, the offered release),
    /// and the instance runs exactly the release it offers.
    #[serde(default)]
    deployment: Option<String>,
}

async fn app_create(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<CreateAppBody>,
) -> ApiResult {
    let p = signed_in(&node, &headers).await?;
    match p.bound_app() {
        // A frontend makes instances of its own package, already installed.
        Some(pkg) => {
            let installed = node.sys(|s| s.packages.contains_key(pkg));
            if body.bundle.is_some() || body.package.as_deref() != Some(pkg) || !installed {
                return Err(ApiError::forbidden(
                    "a session opened on an app's frontend creates instances of that app only",
                ));
            }
        }
        None => {
            let _p = installer(&node, &headers).await?;
        }
    }
    if p.api_key.as_ref().is_some_and(|k| k.read_only) {
        return Err(ApiError::forbidden("read-only API key"));
    }
    let owner = p.account()?;
    let now = now_secs();
    let (bundle, compute_bundle, release) = match &body.deployment {
        // From a deployment's offer: installed already, and the caller's
        // to run whether or not they see the release otherwise.
        Some(d) => {
            if body.bundle.is_some() {
                return Err(ApiError::bad_request(
                    "BAD_REQUEST",
                    "an instance from a deployment names its package, not a bundle",
                ));
            }
            let package = body.package.as_deref().ok_or_else(|| {
                ApiError::bad_request("BAD_REQUEST", "name the package the deployment offers")
            })?;
            let (bundle, compute, release) = offered_bundle(
                &node,
                d,
                package,
                body.version.as_deref(),
                body.service.as_deref(),
            )
            .await?;
            (bundle, compute, Some(release))
        }
        None => {
            bundle_named(
                &node,
                body.bundle,
                body.package.as_deref(),
                body.version.as_deref(),
                body.service.as_deref(),
                &owner,
                // Checked above: a bound session names its own package only.
                p.bound_app().is_some(),
            )
            .await?
        }
    };
    // The executor is decided once, here, from the WASM the bundle names (a
    // package's service, or the raw upload), and recorded with the app. A core
    // module (an SDK 0.12 app) makes no app.
    let executor = node.executor_of(&bundle).await?;
    if executor != crate::types::Executor::Component {
        return Err(ApiError::unsupported_bundle());
    }
    // A component's confirm methods are its ABI's (S5), never the request's.
    if serde_json::to_vec(&body.init_args).map_or(0, |a| a.len()) > crate::node::MAX_ARGS_BYTES {
        return Err(crate::node::args_too_large());
    }
    let confirm_methods: Vec<String> = Vec::new();
    // Every instance is in an organization: one that names none goes to its
    // owner's personal organization, made the first time.
    let org = match body.org {
        Some(org) => org,
        None => personal_org(&node, &owner).await?,
    };
    // Past a release deadline the app's approver has not met, no new app;
    // `CreateApp` asks again at the same time.
    if let Some(stall) = node.sys(|s| {
        let governing = s.governing(release.as_ref(), &bundle, now);
        s.stalled_for_release(governing, &org, now)
    }) {
        return Err(crate::node::stalled(&stall));
    }
    // Only nodes on an approved release (the publisher's, the owner's for an
    // unbound package, or the operator's where the deployment gave it
    // release approvals) may hold the app.
    let approved = node.sys(|s| {
        let governing = s.governing(release.as_ref(), &bundle, now);
        s.approved_for_release(governing, &org).cloned()
    });
    // Nor a node whose runtime is older than the release needs.
    let min_runtime = node.min_runtime_of(release.as_ref(), &bundle).await;
    let replicas = node
        .place(approved.as_ref(), now, min_runtime.as_ref())
        .await;
    if replicas.is_empty() {
        if let Some(min) = &min_runtime {
            let nodes = node.sys(|s| s.replica_nodes().map(|n| n.id).collect::<Vec<_>>());
            let short = crate::runtime::short_of(&nodes, &node.runtimes(&nodes).await, min);
            if !short.is_empty() {
                return Err(crate::runtime::refusal(
                    &release_label(release.as_ref(), &bundle),
                    min,
                    short,
                ));
            }
        }
        return Err(ApiError::unavailable(
            "no node runs a release this app's owner approved",
        ));
    }
    let app_id = typed_id::<16>();
    let has_compute = compute_bundle.is_some();
    let components = node.sys(|s| s.components_of(release.as_ref()));
    let access = body.access.unwrap_or(Access::Org);
    let out = node
        .sys_write(&SystemCmd::CreateApp {
            app: Box::new(crate::types::AppRecord {
                app_id: app_id.clone(),
                name: body.name,
                group: 0,
                bundle,
                owner,
                org,
                access,
                confirm_methods,
                executor,
                replicas,
                created_at: now,
                init_args: body.init_args,
                metadata: body.metadata,
                last_upgrade: None,
                deployment: body.deployment,
                compute_bundle,
                components,
                release,
                restore: None,
            }),
        })
        .await?;
    // Wait for the app's first entry (init) to commit somewhere; a compute-only app's is empty,
    // but its group cannot take a pipeline start before it.
    let deadline = tokio::time::Instant::now() + app_start_wait();
    loop {
        match app_ready(&node, &app_id).await {
            Some(Ok(())) => break,
            Some(Err(error)) => {
                return Err(init_failed(&app_id, &error));
            }
            None => {}
        }
        if tokio::time::Instant::now() > deadline {
            return Err(ApiError::unavailable(
                "the app was created but has not started yet",
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(Json(json!({
        "appId": app_id,
        "contextId": hex::encode(sha256(app_id.as_bytes())),
        "group": out["group"],
        "replicas": out["replicas"],
        "compute": has_compute,
    })))
}

/// How long creating an app waits for its `init` to commit before answering `503 UNAVAILABLE`
/// (the app stays created, and `init` keeps being retried).
pub const APP_START_WAIT: Duration = Duration::from_secs(30);

/// [`APP_START_WAIT`], or the wait a test set (`testing::app_start_wait`).
fn app_start_wait() -> Duration {
    #[cfg(feature = "testing")]
    if let Some(wait) = crate::testing::app_start_wait_override() {
        return wait;
    }
    APP_START_WAIT
}

/// Whether the app has run `init`, asking a replica if this node hosts none.
/// Whether the app's init has committed: `None` not yet, `Some(Err(why))` failed.
pub async fn app_ready(node: &Node, app_id: &str) -> Option<Result<(), Value>> {
    if let Some(g) = node.hosted(app_id) {
        return g.read_local(|m, _| init_state(&m.state));
    }
    let app = node.app(app_id).await.ok()?;
    let addr = node.sys(|s| {
        app.replicas
            .iter()
            .find_map(|r| s.nodes.get(r).map(|n| n.addr.clone()))
    })?;
    let v = node
        .peer_forward(&addr, "internal/app-ready", &json!({ "appId": app_id }))
        .await
        .ok()?;
    match (v.get("ready").and_then(Value::as_bool), v.get("initError")) {
        (_, Some(e)) if !e.is_null() => Some(Err(e.clone())),
        (Some(true), _) => Some(Ok(())),
        _ => None,
    }
}

#[derive(Deserialize)]
pub(super) struct AppListQuery {
    /// Only instances of this package (any version).
    #[serde(default)]
    package: Option<String>,
    /// Only instances running this bundle.
    #[serde(default)]
    bundle: Option<String>,
}

async fn app_list(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Query(q): Query<AppListQuery>,
) -> ApiResult {
    let p = principal(&node, &headers).await?;
    let account = p
        .as_ref()
        .and_then(|p| p.caller.account().map(str::to_owned));
    let bound = p.as_ref().and_then(|p| p.bound_app().map(str::to_owned));
    let apps: Vec<Value> = node.sys(|s| {
        s.apps
            .values()
            .filter(|a| {
                bound.as_ref().is_none_or(|pkg| {
                    s.bundles
                        .get(&a.bundle)
                        .and_then(|b| b.package.as_ref())
                        .is_some_and(|r| r.package == *pkg)
                })
            })
            .filter(|a| a.access != Access::Public || account.as_deref() == Some(a.owner.as_str()))
            .filter(|a| s.may_reach(a, account.as_deref()))
            .filter(|a| q.bundle.as_ref().is_none_or(|b| *b == a.bundle))
            .filter(|a| {
                q.package.as_ref().is_none_or(|p| {
                    s.bundles
                        .get(&a.bundle)
                        .and_then(|b| b.package.as_ref())
                        .is_some_and(|r| r.package == *p)
                })
            })
            .map(|a| {
                let release = s.bundles.get(&a.bundle).and_then(|b| b.package.clone());
                json!({
                    "appId": a.app_id,
                    "name": a.name,
                    "owner": a.owner,
                    "org": a.org,
                    "bundle": a.bundle,
                    "package": release.as_ref().map(|r| &r.package),
                    "version": release.as_ref().map(|r| &r.version),
                    "deployment": a.deployment,
                })
            })
            .collect()
    });
    Ok(Json(json!({ "apps": apps })))
}

/// Delete an app instance: its owner, or an admin of its organization.
async fn app_delete(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    let app = node.app(&app_id).await?;
    p.within(&node, &app)?;
    let _ok = node
        .sys_write(&SystemCmd::DeleteApp {
            app_id: app_id.clone(),
            actor: account,
            owner_in_org: true,
            owner_in_team: true,
        })
        .await?;
    Ok(Json(json!({ "appId": app_id, "deleted": true })))
}

async fn app_get(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = principal(&node, &headers).await?;
    // As in `gate`: an anonymous caller is judged on the cluster's latest
    // system state too, so an app made private through another node is hidden
    // here at once (#4723). `principal` already read fresh for a credential.
    // Read before the record, so the record answered is the fresh one.
    if p.is_none() {
        node.sys_fresh().await;
    }
    let app = node.app(&app_id).await?;
    if let Some(p) = &p {
        p.within(&node, &app)?;
    }
    let account = p.as_ref().and_then(|p| p.caller.account());
    if !reaches(&node, &app, account) {
        return Err(ApiError::not_found("no such app"));
    }
    Ok(Json(serde_json::to_value(app).unwrap_or_default()))
}

#[derive(Deserialize)]
pub(super) struct AccessBody {
    access: Access,
}

async fn app_access(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<AccessBody>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = person(&node, &headers).await?;
    let account = p.account()?;
    let app = node.app(&app_id).await?;
    p.within(&node, &app)?;
    // A package-bound session moves its package's app between its package's
    // teams only (a channel made private, or back to the app team).
    match (&app.org, &app.access, &body.access) {
        (org, Access::Team { team: from }, Access::Team { team: to }) => {
            p.within_team(&node, org, from, OwnTeam::AppTeamOrBelow)?;
            p.within_team(&node, org, to, OwnTeam::AppTeamOrBelow)?;
        }
        _ => p.unbound()?,
    }
    let _ok = node
        .sys_write(&SystemCmd::SetAccess {
            app_id,
            access: body.access,
            actor: account,
            owner_in_org: true,
            owner_in_team: true,
        })
        .await?;
    Ok(Json(json!({})))
}

/// Delete an organization, its teams, invites and apps: the owner's, with a
/// passkey assertion over this exact deletion (a 412 asks for it).
#[derive(Deserialize)]
pub(super) struct TransferBody {
    to: String,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// Hand an app to another account: the owner's (of an organization's app,
/// while a member of it) or an organization admin's, with a passkey assertion
/// over this transfer (412 first). Who then upgrades the app, and so which
/// code reads its data, is the new owner's.
async fn app_transfer(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<TransferBody>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    let app = node.app(&app_id).await?;
    if !node.sys(|s| s.manages_app(&app, &actor)) {
        return Err(ApiError::forbidden(crate::system::ONLY_MANAGERS_TRANSFER));
    }
    let consent = consent(
        &node,
        &p,
        &app_id,
        "__transfer",
        &json!({ "to": body.to }),
        body.confirm,
    )
    .await?;
    // An app in a personal organization handed to someone outside it goes to
    // theirs: make it first if they have none.
    let leaves_personal = node.sys(|s| {
        s.orgs.get(&app.org).is_some_and(|o| {
            o.personal && o.role(&body.to).is_none() && s.accounts.contains_key(&body.to)
        })
    });
    if leaves_personal {
        let _theirs = personal_org(&node, &body.to).await?;
    }
    Ok(Json(
        node.sys_write(&SystemCmd::TransferApp {
            app_id,
            to: body.to,
            actor,
            consent,
            owner_in_org: true,
            owner_in_team: true,
        })
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct MoveBody {
    to_org: String,
    #[serde(default)]
    bring_members: bool,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// Move an app to another organization (`SystemCmd::MoveApp`), its data
/// where it is: whoever manages it where it is now, as an admin or the owner
/// of the target, with a passkey assertion over this move (412 first).
/// Answers where it went, its access there, whether an app team was made,
/// the accounts brought in and the invites revoked.
async fn app_move(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<MoveBody>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    let app = node.app(&app_id).await?;
    if !node.sys(|s| s.manages_app(&app, &actor)) {
        return Err(ApiError::forbidden(crate::system::ONLY_MANAGERS_MOVE));
    }
    if !node.sys(|s| s.is_org_admin(&body.to_org, &actor)) {
        return Err(ApiError::forbidden(crate::system::ONLY_ADMINS_MOVE_IN));
    }
    // Before the passkey step-up: a node that could not apply it refuses it anyway.
    node.require_features(&[
        Feature::MoveApp,
        Feature::MoveAccountsAppTeam,
        Feature::MoveOrgQuotas,
    ])
    .await?;
    let args = crate::system::move_app_args(&body.to_org, body.bring_members);
    let consent = consent(&node, &p, &app_id, "__move", &args, body.confirm).await?;
    Ok(Json(
        node.sys_write(&SystemCmd::MoveApp {
            app_id,
            to_org: body.to_org,
            actor,
            consent,
            bring_members: body.bring_members,
            nonce: b64url(&random::<16>()),
            all_apps_rows: true,
            accounts_in_app_team: true,
            org_quotas: true,
        })
        .await?,
    ))
}

async fn app_metadata(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<MetadataBody>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = signed_in(&node, &headers).await?;
    let actor = p.account()?;
    p.within(&node, &node.app(&app_id).await?)?;
    let _ok = node
        .sys_write(&SystemCmd::SetAppMetadata {
            app_id,
            metadata: body.metadata,
            actor,
            owner_in_org: true,
            owner_in_team: true,
        })
        .await?;
    Ok(Json(json!({})))
}

/// A component app's xcalls (D25, D29): messages waiting in its outbox and its dead
/// letters, newest last. The app's owner's or an org admin's, from a replica.
async fn app_xcalls(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let p = gate(&node, &headers, &app, "*", false).await?;
    if !manages_app(&node, &app, p.caller.account()) {
        return Err(ApiError::forbidden(
            "only the app's owner or an org admin reads its xcalls",
        ));
    }
    if app.executor != crate::types::Executor::Component {
        return Err(ApiError::unsupported_engine());
    }
    Ok(Json(node.app_xcalls(&app).await?))
}

/// What the app's storage holds: keys, bytes, blobs, and when it last
/// committed a write (Unix seconds; `null` before its first).
async fn app_storage(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "*", false).await?;
    let stats = node.storage_stats(&app).await?;
    let last_write = node.last_write_ms(&app).await;
    Ok(Json(json!({
        "keys": stats.keys,
        "bytes": stats.bytes,
        "blobs": stats.blobs.len(),
        "blobBytes": stats.blob_bytes,
        "lastWriteAt": last_write.map(|ms| ms / 1000),
    })))
}

/// The ABI the app's bundle embeds; `null` for none.
async fn app_abi(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "*", false).await?;
    if app.executor != crate::types::Executor::Component {
        return Err(ApiError::unsupported_engine());
    }
    // The ABI the component embeds (its `calimero:abi` section), of the
    // bundle the app group runs.
    let bundle = node.running_bundle(&app);
    let wasm = node.bundle_wasm(&bundle).await?;
    let abi = crate::component::embedded_abi(&wasm);
    Ok(Json(json!({ "bundle": bundle, "abi": abi })))
}

/// The app's OpenAPI document, generated from the ABI of the bundle the app group runs
/// (`calimero_app_abi::openapi`), under the access `app_abi` has.
async fn app_openapi(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "*", false).await?;
    if app.executor != crate::types::Executor::Component {
        return Err(ApiError::unsupported_engine());
    }
    let bundle = node.running_bundle(&app);
    let wasm = node.bundle_wasm(&bundle).await?;
    let Some(abi) = crate::component::embedded_abi(&wasm) else {
        return Err(ApiError::not_found("this app's bundle embeds no ABI"));
    };
    let abi = calimero_app_abi::AppAbi::from_json(&abi)
        .map_err(|e| ApiError::new(422, "BAD_ABI", &format!("the app's ABI: {e}")))?;
    let info = calimero_app_abi::openapi::Info {
        title: app.name.clone(),
        version: bundle,
        app: Some(app_id),
    };
    Ok(Json(calimero_app_abi::openapi::document(&abi, &info)))
}

/// One day of an app's usage book on the wire: every figure a decimal string (D20).
/// `wallMs` is for information only; the quota counts fuel (D76).
fn usage_day(day: u64, u: &crate::component::DayUsage) -> Value {
    json!({
        "date": crate::component::utc_date(day),
        "runs": u.runs.to_string(),
        "fuel": u.fuel.to_string(),
        "wallMs": u.wall_ms.to_string(),
        "writes": u.writes.to_string(),
        "bytesWritten": u.bytes_written.to_string(),
        "events": u.events.to_string(),
    })
}

/// What an app's runs used (Q20), for its owner and the admins of its organization
/// only: `today` and `days`, the last 30 days before it (oldest first), each UTC on
/// the node's app time; a day nothing ran is zeros, and so is every day of an app
/// recorded before components (SDK 0.12, no longer run).
async fn app_usage(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let p = gate(&node, &headers, &app, "*", false).await?;
    // After the gate: one who can't reach the app learns nothing of it (404).
    if !manages_app(&node, &app, p.caller.account()) {
        return Err(ApiError::forbidden(
            "only the app's owner or an org admin reads its usage",
        ));
    }
    let book = node.app_usage(&app).await?;
    let today = crate::component::utc_day(node.app_clock().now_ms());
    let day = |d: u64| usage_day(d, &book.on(d));
    let first = today.saturating_sub(crate::component::USAGE_DAYS - 1);
    Ok(Json(json!({
        "today": day(today),
        "days": (first..today).map(day).collect::<Vec<_>>(),
    })))
}

/// This area's routes, merged into [`super::router`].
pub(super) fn routes() -> Router<Arc<Node>> {
    Router::new()
        .route("/apps", post(app_create).get(app_list))
        .route("/apps/{app}", get(app_get).delete(app_delete))
        .route("/apps/{app}/metadata", post(app_metadata))
        .route("/apps/{app}/storage", get(app_storage))
        .route("/apps/{app}/usage", get(app_usage))
        .route("/apps/{app}/abi", get(app_abi))
        .route("/apps/{app}/openapi.json", get(app_openapi))
        .route("/apps/{app}/attestation", get(app_attestation))
        .route("/apps/{app}/access", post(app_access))
        .route("/apps/{app}/transfer", post(app_transfer))
        .route("/apps/{app}/move", post(app_move))
        .route("/apps/{app}/xcalls", get(app_xcalls))
}
