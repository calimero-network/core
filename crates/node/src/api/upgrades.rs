//! The API's routes for app upgrades and upgrade runs.

#[allow(
    clippy::wildcard_imports,
    reason = "the handlers moved here from `api.rs` read its items"
)]
use super::*;

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpgradeBody {
    /// A bundle hash, or a `version` of the app's package (or another `package`).
    #[serde(default)]
    pub(crate) bundle: Option<String>,
    #[serde(default)]
    pub(crate) package: Option<String>,
    #[serde(default)]
    pub(crate) version: Option<String>,
    #[serde(default)]
    pub(crate) service: Option<String>,
    /// The owner's passkey approval of this exact upgrade (412 asks for it).
    #[serde(default)]
    pub(crate) confirm: Option<ConfirmBody>,
    /// The bundle's migration export, if the state shape changed.
    #[serde(default)]
    pub(crate) migrate: Option<String>,
}

async fn app_upgrade(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<UpgradeBody>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    // An upgrade key upgrades here and nowhere else; any other credential
    // is a passkey session's, as before.
    let p = credential(&node, &headers)
        .await?
        .ok_or_else(|| ApiError::unauthenticated("sign in first"))?;
    if p.upgrade_key() {
        let key = bearer(&headers).unwrap_or_default();
        return upgrade_app(&node, &p, app_id, body, Some(key)).await;
    }
    let p = holder(&node, &headers).await?;
    upgrade_app(&node, &p, app_id, body, None).await
}

/// Upgrade `app_id` as `p`: the route's, and the consent page's for an
/// upgrade request.
///
/// `key` is the upgrade key `p` came with (`ApiKeyRecord::upgrades`): it
/// signs the upgrade in place of a passkey, only to a signed release of the
/// key's package; every other rule is the same.
pub(crate) async fn upgrade_app(
    node: &Node,
    p: &Principal,
    app_id: String,
    mut body: UpgradeBody,
    key: Option<&str>,
) -> ApiResult {
    let account = p.account()?;
    let scope = match key {
        Some(key) => {
            let k = p
                .api_key
                .as_ref()
                .filter(|k| k.upgrades.is_some())
                .ok_or_else(upgrade_key_only)?;
            Some((key, k))
        }
        None if p.upgrade_key() => return Err(upgrade_key_only()),
        None => None,
    };
    if let Some((_, k)) = scope {
        let package = k.upgrades.as_ref().map(|u| u.package.clone());
        if k.upgrades.as_ref().is_some_and(|u| u.app != app_id) {
            return Err(ApiError::forbidden("this API key upgrades another app"));
        }
        // A key moves its app to a signed release of its package only: a
        // raw bundle hash or another package is refused.
        if body.bundle.is_some()
            || body
                .package
                .as_ref()
                .is_some_and(|p| Some(p) != package.as_ref())
        {
            return Err(ApiError::forbidden(
                "an upgrade key upgrades only to a version of its own package",
            ));
        }
        if body.version.is_none() {
            return Err(ApiError::bad_request(
                "VERSION_REQUIRED",
                "an upgrade key names the version to upgrade to",
            ));
        }
        body.package = package;
    }
    // Who may upgrade is the system log's rule (`UpgradeApp`); asked first, so
    // a refused or failed upgrade records nothing. An upgrade is the app's
    // owner's decision, or, for an org app, of whoever holds
    // `MANAGE_APPLICATION` there (`SystemState::manages_apps`), each with
    // their own passkey: new code reads the app's data.
    let app = node.app(&app_id).await?;
    owner_holds(node, |g| g.app_upgrades, "app upgrades")?;
    let (may, unbound) = node.sys(|s| (s.may_upgrade(&app, &account), s.upgrades_unbound(&app)));
    if unbound {
        return Err(ApiError::conflict(
            "this app's package is upgraded by its publisher, and no organization has bound its key yet",
        ));
    }
    if !may {
        return Err(ApiError::forbidden(
            "upgrading this app needs whoever its package names: its owner (or MANAGE_APPLICATION in its organization), or its publisher's MANAGE_APPLICATION holders",
        ));
    }
    // A version alone means the app's own package and service: the release
    // it runs, not merely one shipping its WASM.
    let current = node.release_of_app(&app);
    let package = body.package.clone().or_else(|| {
        body.version
            .as_ref()
            .and(current.as_ref().map(|r| r.at.package.clone()))
    });
    let service = body
        .service
        .clone()
        .or_else(|| current.as_ref().and_then(|r| r.at.service.clone()));
    // An upgrade to a release with the app's own bundle (a release that changes
    // only its frontend) is an upgrade like any other: the same authority,
    // consent and record, and the instance runs the new release after it.
    let (bundle, compute_bundle, release) = bundle_named(
        node,
        body.bundle.clone(),
        package.as_deref(),
        body.version.as_deref(),
        service.as_deref(),
        &account,
        false,
    )
    .await?;
    if !node.sys(|s| s.bundles.contains_key(&bundle)) {
        return Err(crate::node::rejected("unknown bundle"));
    }
    let now = now_secs();
    if !node.sys(|s| {
        let to = s.governing(release.as_ref(), &bundle, now);
        s.upgrade_keeps_authority(s.governing_release(&app), to)
    }) {
        return Err(ApiError::forbidden(
            "an app whose owner upgrades it is not moved to a release its publisher upgrades",
        ));
    }
    refuse_stalled(node, &app, &bundle, release.as_ref(), now)?;
    // Before the passkey is asked for: an upgrade no replica could run is
    // refused the same way after it (`run_upgrade`).
    node.require_runtime(
        &app.replicas,
        release.as_ref(),
        &bundle,
        &release_label(release.as_ref(), &bundle),
    )
    .await?;
    // What the publisher signed for the target: the WASM hash alone would let
    // another manifest (other outbound prefixes, frontend origins) under the
    // same version and WASM pass for the one approved.
    let manifest = crate::consent_page::target_manifest_hash(
        node,
        package.as_deref().zip(body.version.as_deref()),
        &bundle,
    )
    .await?;
    let args = json!({ "bundle": bundle, "migrate": body.migrate, "manifest": manifest });
    let consent = match scope {
        // The upgrade key's own signature over this exact upgrade, judged
        // here as `authorize` judges it where it applies.
        Some((key, k)) => {
            let refused = node.sys(|s| s.key_may_upgrade(k, &app_id, &bundle, now_secs()).err());
            if let Some(e) = refused {
                return Err(ApiError::forbidden(e));
            }
            tracing::info!(
                app = %app_id,
                key_id = %k.key_id,
                %bundle,
                "an upgrade key upgrades its app"
            );
            let statement = crate::proof::Statement::new(&app_id, "__upgrade", &args);
            json!(crate::proof::sign_key_upgrade(key, &k.key_id, statement))
        }
        // A fresh passkey assertion over this exact upgrade, logged as consent.
        None => consent(node, p, &app_id, "__upgrade", &args, body.confirm.take()).await?,
    };
    run_upgrade(
        node,
        app_id,
        Target {
            bundle,
            compute_bundle,
            release,
        },
        body.migrate,
        Upgrader {
            caller: Caller::Account {
                account: account.clone(),
            },
            actor: account,
            by_operator: false,
            consent,
            run: None,
        },
    )
    .await
}

/// Who is upgrading, as the directory records it.
#[derive(Clone)]
struct Upgrader {
    /// Who the migration runs as.
    caller: Caller,
    /// The account, or empty for the operator.
    actor: String,
    by_operator: bool,
    consent: Value,
    /// The organization-wide run this belongs to.
    run: Option<String>,
}

impl Upgrader {
    fn by(&self) -> String {
        if self.by_operator {
            crate::system::OPERATOR.to_owned()
        } else {
            self.actor.clone()
        }
    }
}

/// Where an upgrade moves an app.
struct Target {
    bundle: String,
    /// The compute component of the release `bundle` is taken from.
    compute_bundle: Option<String>,
    /// The release (`AppRecord::release`); `None` for a raw upload.
    release: Option<PackageRef>,
}

/// Run an upgrade on the app's group and, once it succeeded, record the new
/// bundle and release in the directory (where `UpgradeApp` judges who may). A
/// failure is recorded too: the app's last upgrade says why.
/// `422 UPGRADE_FAILED`: the app log's `__upgrade` entry at `index` ended `status`, with the
/// app log's `error`.
pub(super) fn upgrade_failed(why: &str, index: u64, status: &Status, error: &Value) -> ApiError {
    ApiError::new(422, "UPGRADE_FAILED", why)
        .with_details(json!({ "index": index, "status": status, "error": error }))
}

async fn run_upgrade(
    node: &Node,
    app_id: String,
    target: Target,
    migrate: Option<String>,
    who: Upgrader,
) -> ApiResult {
    let Target {
        bundle,
        compute_bundle,
        release,
    } = target;
    // Judged once, before the migration runs; `UpgradeApp` carries this time
    // so `apply` judges the deadline at the same moment and never refuses
    // the record of an upgrade the app's group already ran.
    let begun = now_secs();
    let has_compute = compute_bundle.is_some();
    // Who may, and where to: judged here, for every path (one app, an
    // organization's run, a publisher's run), before anything runs on the
    // app's group; `UpgradeApp` judges both again.
    let refusal = node.sys(|s| {
        let a = s.apps.get(&app_id)?;
        if !who.by_operator && !s.may_upgrade(a, &who.actor) {
            return Some(ApiError::forbidden(
                "upgrading this app needs whoever its package names: its owner (or MANAGE_APPLICATION in its organization), or its publisher's MANAGE_APPLICATION holders",
            ));
        }
        let to = s.governing(release.as_ref(), &bundle, begun);
        if !who.by_operator && !s.upgrade_keeps_authority(s.governing_release(a), to) {
            return Some(ApiError::forbidden(
                "an app whose owner upgrades it is not moved to a release its publisher upgrades",
            ));
        }
        s.stalled_for_release(to, a.org.as_deref(), &a.owner, begun)
            .map(|stall| crate::node::stalled(&stall))
    });
    // Every replica's node must run the runtime the target needs, or the
    // replicas below it could not run the app once it moved.
    let replicas = node
        .sys(|s| s.apps.get(&app_id).map(|a| a.replicas.clone()))
        .unwrap_or_default();
    let refusal = match refusal {
        Some(e) => Some(e),
        None => node
            .require_runtime(
                &replicas,
                release.as_ref(),
                &bundle,
                &release_label(release.as_ref(), &bundle),
            )
            .await
            .err(),
    };
    if let Some(e) = refusal {
        let error = json!({ "code": e.code, "message": e.message });
        note_failed_upgrade(node, &app_id, &bundle, release.as_ref(), &who, error).await;
        return Err(e);
    }
    let applied = match node
        .call(&CallRequest::host(
            app_id.clone(),
            "__upgrade",
            json!({ "bundle": bundle, "computeBundle": compute_bundle, "migrate": migrate }),
            who.caller.clone(),
        ))
        .await
    {
        Ok(a) => a,
        Err(e) => {
            let error = json!({ "code": e.code, "message": e.message });
            note_failed_upgrade(node, &app_id, &bundle, release.as_ref(), &who, error).await;
            return Err(e);
        }
    };
    // A refused or failed upgrade leaves the app on its bundle; say so as an
    // error, not a 200 a client can mistake for success.
    if applied.status != Status::Ok {
        note_failed_upgrade(
            node,
            &app_id,
            &bundle,
            release.as_ref(),
            &who,
            applied.error.clone(),
        )
        .await;
        let why = applied.error.as_str().map_or_else(
            || "the upgrade did not run".to_owned(),
            |e| format!("the upgrade did not run: {e}"),
        );
        return Err(upgrade_failed(
            &why,
            applied.index,
            &applied.status,
            &applied.error,
        ));
    }
    // The app now runs the new bundle: the directory says so too.
    let _ok = node
        .sys_write(&SystemCmd::UpgradeApp {
            app_id,
            bundle,
            compute_bundle,
            by_operator: who.by_operator,
            actor: who.actor,
            at: begun,
            run: who.run,
            consent: who.consent,
            release,
            owner_in_org: true,
            owner_in_team: true,
        })
        .await?;
    Ok(Json(json!({
        "index": applied.index,
        "status": applied.status,
        "error": applied.error,
        "compute": has_compute,
    })))
}

/// Refused `409 RELEASE_DEADLINE` when `app`, upgraded to `bundle` of
/// `release`, may start no new work at `now`
/// ([`crate::system::SystemState::stalled_for_release`]).
pub(crate) fn refuse_stalled(
    node: &Node,
    app: &crate::types::AppRecord,
    bundle: &str,
    release: Option<&PackageRef>,
    now: u64,
) -> Result<(), ApiError> {
    match node.sys(|s| {
        let to = s.governing(release, bundle, now);
        s.stalled_for_release(to, app.org.as_deref(), &app.owner, now)
    }) {
        Some(stall) => Err(crate::node::stalled(&stall)),
        None => Ok(()),
    }
}

async fn note_failed_upgrade(
    node: &Node,
    app_id: &str,
    bundle: &str,
    release: Option<&PackageRef>,
    who: &Upgrader,
    error: Value,
) {
    let _r = node
        .sys_write(&SystemCmd::UpgradeFailed {
            app_id: app_id.to_owned(),
            bundle: bundle.to_owned(),
            by: who.by(),
            at: now_secs(),
            error,
            run: who.run.clone(),
            release: release.cloned(),
        })
        .await;
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OrgUpgradeBody {
    package: String,
    version: String,
    /// Only the apps of this team (and, with `cascade`, of the teams below it).
    #[serde(default)]
    team: Option<String>,
    #[serde(default)]
    cascade: bool,
    #[serde(default)]
    migrate: Option<String>,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
    /// The operator's signature, for an operator's run.
    #[serde(default, rename = "operatorProof")]
    operator_proof: Option<OperatorProofBody>,
}

/// The apps an organization-wide upgrade of `package` covers.
fn run_targets(
    node: &Node,
    org: &str,
    package: &str,
    team: Option<&str>,
    cascade: bool,
) -> Vec<crate::types::AppRecord> {
    let apps: Vec<crate::types::AppRecord> = node.sys(|s| {
        let Some(o) = s.orgs.get(org) else {
            return Vec::new();
        };
        let teams: Option<Vec<String>> = team.map(|t| {
            if cascade {
                o.subtree(t)
            } else {
                vec![t.to_owned()]
            }
        });
        s.apps
            .values()
            .filter(|a| a.org.as_deref() == Some(org))
            .filter(|a| match (&teams, &a.access) {
                (None, _) => true,
                (Some(ts), Access::Team { team }) => ts.contains(team),
                (Some(_), _) => false,
            })
            .cloned()
            .collect()
    });
    apps.into_iter()
        .filter(|a| {
            node.release_of_app(a)
                .is_some_and(|r| r.at.package == package)
        })
        .collect()
}

/// Where `version` of the package moves an app (same service): its bundle, that
/// release's compute component and the release, if installed and the upgrader
/// may see it ([`upgrade_sees`]).
fn target_bundle(
    node: &Node,
    app: &crate::types::AppRecord,
    version: &str,
    who: &Upgrader,
) -> Option<Target> {
    let r = node.release_of_app(app)?;
    let service = r.at.service.clone().unwrap_or_default();
    node.sys(|s| {
        let v = s.packages.get(&r.at.package)?.versions.get(version)?;
        if !upgrade_sees(s, v, app, &who.actor, who.by_operator) {
            return None;
        }
        let bundle = v.services.get(&service).cloned()?;
        Some(Target {
            bundle,
            compute_bundle: v.compute.get(&service).cloned(),
            release: Some(PackageRef {
                package: r.at.package.clone(),
                version: version.to_owned(),
                service: r.at.service.clone(),
                compute: false,
            }),
        })
    })
}

/// Whether an upgrade may move `app` to release `v`: one the upgrading
/// account may see, or, for the operator, one it installed or one the app's
/// owner may see. A release of another organization never reaches an app.
fn upgrade_sees(
    s: &SystemState,
    v: &crate::system::PackageVersion,
    app: &crate::types::AppRecord,
    actor: &str,
    by_operator: bool,
) -> bool {
    if by_operator {
        s.sees_release(v, crate::system::OPERATOR) || s.sees_release(v, &app.owner)
    } else {
        s.sees_release(v, actor)
    }
}

/// Start an organization-wide upgrade: recorded, then run app by app in the
/// background; `GET /orgs/{org}/upgrades/{run}` follows it.
async fn start_run(
    node: Arc<Node>,
    org: String,
    body: OrgUpgradeBody,
    apps: Vec<crate::types::AppRecord>,
    who: Upgrader,
) -> ApiResult {
    let run_id = typed_id::<12>();
    let run = crate::types::UpgradeRun {
        run_id: run_id.clone(),
        package: body.package.clone(),
        version: body.version.clone(),
        team: body.team.clone(),
        cascade: body.cascade,
        by: who.by(),
        started_at: now_secs(),
        apps: apps.iter().map(|a| a.app_id.clone()).collect(),
        done: Vec::new(),
        failed: std::collections::BTreeMap::new(),
        finished: false,
    };
    let _ok = node
        .sys_write(&SystemCmd::StartUpgradeRun {
            org: org.clone(),
            run: Box::new(run.clone()),
            consent: who.consent.clone(),
        })
        .await?;
    let who = Upgrader {
        run: Some(run_id.clone()),
        ..who
    };
    let version = body.version.clone();
    let migrate = body.migrate.clone();
    let _task = tokio::spawn(async move {
        for app in apps {
            match target_bundle(&node, &app, &version, &who) {
                Some(t) if t.bundle == app.bundle => {
                    // Same app component: nothing to run on the app's group,
                    // though its compute component and its release (a
                    // frontend-only one) may differ, which the directory
                    // records; `UpgradeApp` judges who may as for any upgrade,
                    // and a refusal is noted on the app and the run.
                    let written = node
                        .sys_write(&SystemCmd::UpgradeApp {
                            app_id: app.app_id.clone(),
                            bundle: t.bundle.clone(),
                            compute_bundle: t.compute_bundle,
                            by_operator: who.by_operator,
                            actor: who.actor.clone(),
                            at: now_secs(),
                            run: who.run.clone(),
                            consent: who.consent.clone(),
                            release: t.release.clone(),
                            owner_in_org: true,
                            owner_in_team: true,
                        })
                        .await;
                    if let Err(e) = written {
                        let error = json!({ "code": e.code, "message": e.message });
                        note_failed_upgrade(
                            &node,
                            &app.app_id,
                            &t.bundle,
                            t.release.as_ref(),
                            &who,
                            error,
                        )
                        .await;
                    }
                }
                Some(t) => {
                    let _r =
                        run_upgrade(&node, app.app_id.clone(), t, migrate.clone(), who.clone())
                            .await;
                }
                None => {
                    note_failed_upgrade(
                        &node,
                        &app.app_id,
                        "",
                        None,
                        &who,
                        json!({ "code": "NOT_INSTALLED", "message": "the release ships no bundle for this app's service" }),
                    )
                    .await;
                }
            }
        }
        let _r = node
            .sys_write(&SystemCmd::FinishUpgradeRun { org, run_id })
            .await;
    });
    Ok(Json(serde_json::to_value(run).unwrap_or_default()))
}

/// Upgrade every app of an organization (or a team, with `cascade` its
/// subtree) that runs `package` to `version`: for whoever holds
/// `MANAGE_APPLICATION` there (the owner, an admin, a team's lead, or a member
/// granted it), with one passkey assertion over the whole run, where owners
/// hold app upgrades. Each app is still checked on its own (`UpgradeApp`).
async fn org_upgrade(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(mut body): Json<OrgUpgradeBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    owner_holds(&node, |g| g.app_upgrades, "app upgrades")?;
    if !node.sys(|s| s.manages_apps(&org, &account, body.team.as_deref())) {
        return Err(ApiError::forbidden(
            "upgrading the organization's apps needs MANAGE_APPLICATION there",
        ));
    }
    // Installed from the registry first if the cluster lacks the release.
    let _bundle = node
        .resolve_release(&body.package, Some(&body.version), None, &account, false)
        .await;
    let consent = consent(
        &node,
        &p,
        &format!("org:{org}"),
        "__upgrade_org",
        &json!({
            "package": body.package, "version": body.version, "team": body.team,
            "cascade": body.cascade, "migrate": body.migrate,
        }),
        body.confirm.take(),
    )
    .await?;
    let apps = run_targets(
        &node,
        &org,
        &body.package,
        body.team.as_deref(),
        body.cascade,
    );
    let who = Upgrader {
        caller: Caller::Account {
            account: account.clone(),
        },
        actor: account,
        by_operator: false,
        consent,
        run: None,
    };
    start_run(node, org, body, apps, who).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PublisherUpgradeBody {
    version: String,
    #[serde(default)]
    migrate: Option<String>,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// Upgrade every instance of `package`, whoever owns it, that its publisher
/// upgrades (its release declares `upgrades: publisher`): for whoever holds
/// `MANAGE_APPLICATION` in the publisher organization (`org`, which proved
/// it holds the package's key), with one passkey assertion over the run.
/// The run is kept on the publisher; each app is still checked on its own.
async fn publisher_upgrade(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((org, package)): Path<(String, String)>,
    Json(mut body): Json<PublisherUpgradeBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    owner_holds(&node, |g| g.app_upgrades, "app upgrades")?;
    node.sys_fresh_from(p.sys_index).await;
    let publishes = node.sys(|s| {
        s.orgs.get(&org).is_some_and(|o| o.role(&account).is_some())
            && s.packages
                .get(&package)
                .and_then(|pk| s.publisher_of_signer(&pk.signer_id))
                .is_some_and(|o| *o == org)
    });
    if !publishes {
        return Err(ApiError::not_found(
            "the organization publishes no such package",
        ));
    }
    if !node.sys(|s| s.manages_apps(&org, &account, None)) {
        return Err(ApiError::forbidden(
            "upgrading a published package's instances needs MANAGE_APPLICATION in the publisher",
        ));
    }
    // Installed from the registry first if the cluster lacks the release.
    let _bundle = node
        .resolve_release(&package, Some(&body.version), None, &account, false)
        .await;
    let consent = consent(
        &node,
        &p,
        &format!("org:{org}"),
        "__upgrade_published",
        &json!({ "package": package, "version": body.version, "migrate": body.migrate }),
        body.confirm.take(),
    )
    .await?;
    let apps: Vec<crate::types::AppRecord> = node.sys(|s| {
        s.apps
            .values()
            .filter(|a| {
                s.bundles
                    .get(&a.bundle)
                    .and_then(|b| b.package.as_ref())
                    .is_some_and(|at| at.package == package)
                    && s.may_upgrade(a, &account)
            })
            .cloned()
            .collect()
    });
    let who = Upgrader {
        caller: Caller::Account {
            account: account.clone(),
        },
        actor: account,
        by_operator: false,
        consent,
        run: None,
    };
    let run = OrgUpgradeBody {
        package,
        version: body.version,
        team: None,
        cascade: false,
        migrate: body.migrate,
        confirm: None,
        operator_proof: None,
    };
    start_run(node, org, run, apps, who).await
}

/// [`org_upgrade`] by the operator, where the deployment gave it app upgrades.
async fn ops_org_upgrade(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(mut body): Json<OrgUpgradeBody>,
) -> ApiResult {
    operator(&node, &headers)?;
    operator_holds(&node, |g| g.app_upgrades, "app upgrades")?;
    node.sys_fresh().await;
    if !node.sys(|s| s.orgs.contains_key(&org)) {
        return Err(ApiError::not_found("no such organization"));
    }
    let consent = operator_proof(
        &node,
        &format!("org:{org}"),
        "__upgrade_org",
        &json!({
            "package": body.package, "version": body.version, "team": body.team,
            "cascade": body.cascade, "migrate": body.migrate,
        }),
        body.operator_proof.take(),
    )?;
    let apps = run_targets(
        &node,
        &org,
        &body.package,
        body.team.as_deref(),
        body.cascade,
    );
    let who = Upgrader {
        caller: Caller::Public,
        actor: String::new(),
        by_operator: true,
        consent,
        run: None,
    };
    start_run(node, org, body, apps, who).await
}

/// An organization-wide upgrade's progress, cascade included.
async fn org_upgrade_status(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((org, run)): Path<(String, String)>,
) -> ApiResult {
    let account = signed_in(&node, &headers).await?.account()?;
    let r = node
        .sys(|s| {
            let o = s.orgs.get(&org)?;
            o.role(&account)?;
            o.upgrade_runs.get(&run).cloned()
        })
        .ok_or_else(|| ApiError::not_found("no such upgrade"))?;
    Ok(Json(serde_json::to_value(r).unwrap_or_default()))
}

/// Run an organization-wide upgrade again for the apps that failed, under the
/// same terms, as a new run.
async fn org_upgrade_retry(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((org, run)): Path<(String, String)>,
    body: Option<Json<Value>>,
) -> ApiResult {
    let prev = node
        .sys(|s| s.orgs.get(&org)?.upgrade_runs.get(&run).cloned())
        .ok_or_else(|| ApiError::not_found("no such upgrade"))?;
    let failed: Vec<crate::types::AppRecord> = node.sys(|s| {
        prev.failed
            .keys()
            .filter_map(|a| s.apps.get(a).cloned())
            .collect()
    });
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let mut retry = OrgUpgradeBody {
        package: prev.package.clone(),
        version: prev.version.clone(),
        team: prev.team.clone(),
        cascade: prev.cascade,
        migrate: None,
        confirm: serde_json::from_value(body["confirm"].clone()).ok(),
        operator_proof: None,
    };
    if prev.by == crate::system::OPERATOR {
        operator(&node, &headers)?;
        operator_holds(&node, |g| g.app_upgrades, "app upgrades")?;
        let consent = operator_proof(
            &node,
            &format!("org:{org}"),
            "__upgrade_org_retry",
            &json!({ "run": run }),
            serde_json::from_value(body["operatorProof"].clone()).ok(),
        )?;
        let who = Upgrader {
            caller: Caller::Public,
            actor: String::new(),
            by_operator: true,
            consent,
            run: None,
        };
        return start_run(node, org, retry, failed, who).await;
    }
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    owner_holds(&node, |g| g.app_upgrades, "app upgrades")?;
    if !node.sys(|s| s.manages_apps(&org, &account, retry.team.as_deref())) {
        return Err(ApiError::forbidden(
            "upgrading the organization's apps needs MANAGE_APPLICATION there",
        ));
    }
    let consent = consent(
        &node,
        &p,
        &format!("org:{org}"),
        "__upgrade_org_retry",
        &json!({ "run": run }),
        retry.confirm.take(),
    )
    .await?;
    let who = Upgrader {
        caller: Caller::Account {
            account: account.clone(),
        },
        actor: account,
        by_operator: false,
        consent,
        run: None,
    };
    start_run(node, org, retry, failed, who).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OpsUpgradeBody {
    #[serde(default)]
    bundle: Option<String>,
    #[serde(default)]
    package: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    service: Option<String>,
    #[serde(default)]
    migrate: Option<String>,
    #[serde(default)]
    operator_proof: Option<OperatorProofBody>,
}

/// Upgrade any app, on a cluster whose operator manages upgrades. The release
/// must be installed already (`/ops/packages`).
async fn ops_upgrade(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<OpsUpgradeBody>,
) -> ApiResult {
    operator(&node, &headers)?;
    operator_holds(&node, |g| g.app_upgrades, "app upgrades")?;
    // The release the operator installed a moment ago may have committed on
    // the system leader but not applied here yet: read fresh, or it is
    // answered `NOT_INSTALLED`.
    node.sys_fresh().await;
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let current = node.release_of_app(&app);
    let target = match (body.bundle, body.version) {
        (Some(b), _) => {
            let compute = node
                .sys(|s| s.compute_of(&b, crate::system::OPERATOR))
                .map_err(|why| ApiError::bad_request(AMBIGUOUS_COMPUTE, why))?;
            Target {
                bundle: b,
                compute_bundle: compute,
                release: None,
            }
        }
        (None, Some(version)) => {
            let package = body
                .package
                .or_else(|| current.as_ref().map(|r| r.at.package.clone()))
                .ok_or_else(|| {
                    ApiError::bad_request("BAD_REQUEST", "name the package to upgrade to")
                })?;
            let service = body
                .service
                .or_else(|| current.as_ref().and_then(|r| r.at.service.clone()))
                .unwrap_or_default();
            node.sys(|s| {
                s.packages
                    .get(&package)
                    .and_then(|p| p.versions.get(&version))
                    .filter(|v| upgrade_sees(s, v, &app, crate::system::OPERATOR, true))
                    .and_then(|v| {
                        let bundle = v.services.get(&service).cloned()?;
                        Some(Target {
                            bundle,
                            compute_bundle: v.compute.get(&service).cloned(),
                            release: Some(PackageRef {
                                package: package.clone(),
                                version: version.clone(),
                                service: (!service.is_empty()).then(|| service.clone()),
                                compute: false,
                            }),
                        })
                    })
            })
            .ok_or_else(|| {
                ApiError::new(
                    409,
                    "NOT_INSTALLED",
                    &format!("{package} {version} is not installed"),
                )
            })?
        }
        (None, None) => {
            return Err(ApiError::bad_request(
                "BAD_REQUEST",
                "name a version or a bundle",
            ))
        }
    };
    // A raw hash is the operator's own, or one the app's owner may name.
    if !node.sys(|s| {
        s.sees_bundle(&target.bundle, crate::system::OPERATOR)
            || s.sees_bundle(&target.bundle, &app.owner)
    }) {
        return Err(crate::node::rejected("unknown bundle"));
    }
    // Signed before the upgrade runs on the app's group.
    let consent = operator_proof(
        &node,
        &app_id,
        "__upgrade",
        &json!({ "bundle": target.bundle, "migrate": body.migrate }),
        body.operator_proof,
    )?;
    run_upgrade(
        &node,
        app_id,
        target,
        body.migrate,
        Upgrader {
            caller: Caller::Public,
            actor: String::new(),
            by_operator: true,
            consent,
            run: None,
        },
    )
    .await
}

/// This area's routes, merged into [`super::router`].
pub(super) fn routes() -> Router<Arc<Node>> {
    Router::new()
        .route("/apps/{app}/upgrade", post(app_upgrade))
        .route("/orgs/{org}/upgrade", post(org_upgrade))
        .route("/orgs/{org}/upgrades/{run}", get(org_upgrade_status))
        .route("/orgs/{org}/upgrades/{run}/retry", post(org_upgrade_retry))
        .route(
            "/orgs/{org}/packages/{package}/upgrade",
            post(publisher_upgrade),
        )
        .route("/ops/apps/{app}/upgrade", post(ops_upgrade))
        .route("/ops/orgs/{org}/upgrade", post(ops_org_upgrade))
}
