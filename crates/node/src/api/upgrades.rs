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

/// Whether upgrades are judged by the rule only an organization's owner and admins
/// approve ([`crate::system::cluster_version::Feature::OwnerApprovedUpgrades`]): once
/// every node reads the flag the entries then carry (`owner_approves`). Before that, as
/// an earlier release judges them, so no node refuses what another applies.
pub(crate) async fn owner_approves(node: &Node) -> bool {
    node.require_features(&[crate::system::cluster_version::Feature::OwnerApprovedUpgrades])
        .await
        .is_ok()
}

/// The class of `app_id`'s move to `release` alone (its members staying where they are),
/// as `UpgradeApp` carries it: once every node judges a class
/// ([`Feature::UpgradeClasses`]); `None` before that, for a raw bundle, a release of
/// another package than the one it runs, or one the classifier could not read, which
/// every rule takes as red.
pub(crate) async fn move_class(
    node: &Node,
    app_id: &str,
    release: Option<&PackageRef>,
) -> Option<crate::types::UpgradeClass> {
    class_of(node, app_id, release, false).await
}

/// [`move_class`] as the classifier's whole verdict, with what the log does not keep
/// (`removed_methods`), for the leader's updates on their own.
pub(crate) async fn move_verdict(
    node: &Node,
    app_id: &str,
    release: Option<&PackageRef>,
) -> Option<calimero_bundle::update::Classification> {
    verdict_of(node, app_id, release, false).await
}

/// The class an upgrade run's `UpgradeApp` of `app_id` carries: for the instance of a
/// unit's run, the whole unit's (its members moving to the run's targets), as apply
/// judges it ([`crate::system::SystemState::run_update_gate`]); else [`move_class`].
async fn run_class(
    node: &Node,
    app_id: &str,
    release: Option<&PackageRef>,
    run: Option<&str>,
) -> Option<crate::types::UpgradeClass> {
    let whole_unit = node.sys(|s| s.is_unit_instance(run, app_id));
    class_of(node, app_id, release, whole_unit).await
}

/// [`move_class`], or with `whole_unit` the class of the unit's move (`upgrade-unit`).
async fn class_of(
    node: &Node,
    app_id: &str,
    release: Option<&PackageRef>,
    whole_unit: bool,
) -> Option<crate::types::UpgradeClass> {
    verdict_of(node, app_id, release, whole_unit)
        .await
        .and_then(upgrade_class)
}

/// The verdict [`class_of`] logs as a class.
async fn verdict_of(
    node: &Node,
    app_id: &str,
    release: Option<&PackageRef>,
    whole_unit: bool,
) -> Option<calimero_bundle::update::Classification> {
    let to = release?;
    if node
        .require_features(&[Feature::UpgradeClasses])
        .await
        .is_err()
    {
        return None;
    }
    let app = node.sys(|s| s.apps.get(app_id).cloned())?;
    // The package it runs, named by its record even once its release is uninstalled.
    let runs = node
        .release_of_app(&app)
        .map(|r| r.at)
        .or_else(|| node.sys(|s| s.running(&app).cloned()))?;
    if runs.package != to.package {
        return None;
    }
    match crate::egress::judged_update(node, &app, &to.version, whole_unit).await {
        // Below `Feature::UnclassifiedUpdates` an rc.34 node cannot decode the
        // `unclassified` reason: the move is logged without a class, as rc.34 logs it
        // (taken as red by every rule; `unclassified_gate` refuses it where that needs an
        // approval no node may log yet).
        Ok(Some(c))
            if crate::egress::unclassified(&c)
                && node
                    .require_features(&[Feature::UnclassifiedUpdates])
                    .await
                    .is_err() =>
        {
            None
        }
        Ok(c) => c,
        Err(e) => {
            warn!(app = %app_id, status = e.status, code = %e.code, "the upgrade was not classified");
            None
        }
    }
}

/// A move the log cannot classify (a part's release no longer installed,
/// `SystemState::unclassified`) is approved first and logged with its `unclassified`
/// class only from [`Feature::UnclassifiedUpdates`] (rc.35), as an rc.34 node cannot
/// decode it. Below that version, where `app`'s policy asks for a second approver, such a
/// move could never be approved: `409 FEATURE_NOT_ACTIVE` (every node must run rc.35
/// first), asked before any passkey by the approval, the upgrade, the unit's upgrade and
/// the consent page. Without that policy it moves as rc.34 moved it, logged without a
/// class.
///
/// # Errors
/// `409 FEATURE_NOT_ACTIVE`, as [`Node::require_features`].
pub(crate) async fn unclassified_gate(
    node: &Node,
    app: &crate::types::AppRecord,
    to: Option<&PackageRef>,
) -> Result<(), ApiError> {
    let Some(to) = to else {
        return Ok(());
    };
    let needs = node.sys(|s| {
        s.upgrade_policy_of(app).0.second_approver
            && s.running(app).is_some_and(|r| r.package == to.package)
            && s.unclassified(app, (&to.package, &to.version), &BTreeMap::new())
                .is_some()
    });
    if !needs {
        return Ok(());
    }
    node.require_features(&[Feature::UnclassifiedUpdates]).await
}

/// The classifier's verdict as the log keeps it; `None` past `MAX_CLASS_REASONS`.
pub(crate) fn upgrade_class(
    c: calimero_bundle::update::Classification,
) -> Option<crate::types::UpgradeClass> {
    (c.reasons.len() <= crate::types::MAX_CLASS_REASONS).then_some(crate::types::UpgradeClass {
        class: c.class,
        reasons: c.reasons,
    })
}

/// Why `account` may not upgrade `app` under the rule in force (`owner_approves`), as
/// `UpgradeApp` would refuse it: the API's `403` (or, before the rule, `409` for a
/// package its publisher upgrades while its key is unbound), asked before any step-up.
pub(crate) fn upgrade_refusal(
    s: &SystemState,
    app: &crate::types::AppRecord,
    account: &str,
    owner_approves: bool,
) -> Option<ApiError> {
    if !owner_approves && s.upgrades_unbound(app) {
        return Some(ApiError::conflict(
            "this app's package is upgraded by its publisher, and no organization has bound its key yet",
        ));
    }
    if s.may_upgrade(app, account, owner_approves) {
        return None;
    }
    Some(ApiError::forbidden(if owner_approves {
        crate::system::NOT_AN_UPGRADE_APPROVER
    } else {
        "upgrading this app needs whoever its package names: its owner (or MANAGE_APPLICATION in its organization), or its publisher's MANAGE_APPLICATION holders"
    }))
}

/// Why `account` may not start an upgrade run in `org` (on `team`) under the rule in
/// force: from [`crate::system::cluster_version::Feature::OwnerApprovedUpgrades`] on, an
/// owner or admin of `org`; before it, whoever holds `MANAGE_APPLICATION` there.
fn run_refusal(
    node: &Node,
    org: &str,
    account: &str,
    team: Option<&str>,
    owner_approves: bool,
) -> Result<(), ApiError> {
    if owner_approves {
        if !node.sys(|s| s.approves_upgrades(org, account)) {
            return Err(ApiError::forbidden(
                "only an owner or admin of the organization approves its apps' upgrades",
            ));
        }
        return Ok(());
    }
    if !node.sys(|s| s.manages_apps(org, account, team)) {
        return Err(ApiError::forbidden(
            "upgrading the organization's apps needs MANAGE_APPLICATION there",
        ));
    }
    Ok(())
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
    // a refused or failed upgrade records nothing. An upgrade is the decision of an
    // owner or admin of the app's organization (`SystemState::approves_upgrades`),
    // each with their own passkey: new code reads the app's data. An upgrade key is
    // judged the same, as its account.
    let app = node.app(&app_id).await?;
    owner_holds(node, |g| g.app_upgrades, "app upgrades")?;
    let owner_approves = owner_approves(node).await;
    if let Some(e) = node.sys(|s| upgrade_refusal(s, &app, &account, owner_approves)) {
        return Err(e);
    }
    // A version alone means the app's own package and service: the release
    // it runs, not merely one shipping its WASM; named by the record even once
    // uninstalled while another release ships its WASM.
    let current = node
        .release_of_app(&app)
        .map(|r| r.at)
        .or_else(|| app.release.clone());
    let package = body.package.clone().or_else(|| {
        // An instance from a deployment names nothing to accept its offer.
        (body.version.is_some() || app.deployment.is_some())
            .then(|| current.as_ref().map(|r| r.package.clone()))
            .flatten()
    });
    let service = body
        .service
        .clone()
        .or_else(|| current.as_ref().and_then(|r| r.service.clone()));
    // An upgrade to a release with the app's own bundle (a release that changes
    // only its frontend) is an upgrade like any other: the same authority,
    // consent and record, and the instance runs the new release after it.
    let (bundle, compute_bundle, release) = match app.deployment.as_deref() {
        // From a deployment: only to the release it offers for the instance's
        // package, whether or not the caller sees it otherwise; refused before
        // any passkey is asked for, and again where it applies.
        Some(d) => {
            let package = package
                .as_deref()
                .filter(|_| body.bundle.is_none())
                .ok_or_else(crate::node::outside_deployment)?;
            let (bundle, compute, release) = super::apps::offered_bundle(
                node,
                d,
                package,
                body.version.as_deref(),
                service.as_deref(),
            )
            .await
            .map_err(|e| {
                if e.code == "NOT_OFFERED" {
                    crate::node::outside_deployment()
                } else {
                    e
                }
            })?;
            (bundle, compute, Some(release))
        }
        None => {
            bundle_named(
                node,
                body.bundle.clone(),
                package.as_deref(),
                body.version.as_deref(),
                service.as_deref(),
                (&account, Some(&app.org)),
                Via::Account,
            )
            .await?
        }
    };
    if let Err(why) = node.sys(|s| {
        s.within_deployment(&app, release.as_ref(), None)?;
        s.within_lineage(&app, release.as_ref())
    }) {
        return Err(crate::node::rejected(why));
    }
    let may = || {
        node.sys(|s| {
            s.upgrades_outside_offers(&app, &account, release.as_ref(), &bundle, now_secs())
        })
    };
    if !may() {
        let _caught_up = node.sys_fresh().await;
        if !may() {
            return Err(crate::node::not_a_deployer());
        }
    }
    if !node.sys(|s| s.bundles.contains_key(&bundle)) {
        return Err(crate::node::rejected("unknown bundle"));
    }
    refuse_off_unit_range(node, &app, release.as_ref()).await?;
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
        release
            .as_ref()
            .map(|r| (r.package.as_str(), r.version.as_str())),
        &bundle,
    )
    .await?;
    // A red update whose policy asks for a second approver waits for another owner's or
    // admin's first approval: refused before the passkey, as `UpgradeApp` refuses it.
    unclassified_gate(node, &app, release.as_ref()).await?;
    let class = move_class(node, &app_id, release.as_ref()).await;
    if let Err(why) = node.sys(|s| {
        s.update_gate(
            &app,
            release.as_ref(),
            class.as_ref(),
            &BTreeMap::new(),
            &account,
            now,
        )
    }) {
        return Err(crate::node::rejected(why));
    }
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
            owner_approves,
            auto: false,
            from_release: None,
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
    /// Judged by the rule only an organization's owner and admins approve
    /// ([`owner_approves`]): carried on its entries.
    owner_approves: bool,
    /// An update applying on its own under the app's policy (`UpgradeApp::auto`), the
    /// system leader's (`node::auto_updates`): judged by the policy, not by who may.
    auto: bool,
    /// For an update on its own, the release the app ran when the pass chose it
    /// (`UpgradeFailed::from_release`).
    from_release: Option<PackageRef>,
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
    let begun = crate::types::update_now();
    let has_compute = compute_bundle.is_some();
    let class = run_class(node, &app_id, release.as_ref(), who.run.as_deref()).await;
    let members_stay = who.auto && members_stay_rule(node).await;
    // Who may, and where to: judged here, for every path (one app, an
    // organization's run, a publisher's run), before anything runs on the
    // app's group; `UpgradeApp` judges both again.
    let judge_upgrade = |s: &crate::system::SystemState| -> Option<ApiError> {
        let a = s.apps.get(&app_id)?;
        if who.auto {
            if let Err(why) = s.auto_update_allows(
                a,
                release.as_ref(),
                class.as_ref(),
                begun,
                s.members_stay(members_stay),
            ) {
                return Some(crate::node::rejected(why));
            }
        } else if !who.by_operator && !s.may_upgrade(a, &who.actor, who.owner_approves) {
            return upgrade_refusal(s, a, &who.actor, who.owner_approves);
        }
        let to = s.governing(release.as_ref(), &bundle, begun);
        if !who.by_operator && !s.upgrade_keeps_authority(s.governing_release(a), to) {
            return Some(ApiError::forbidden(
                "an app whose owner upgrades it is not moved to a release its publisher upgrades",
            ));
        }
        if let Err(why) = s.within_deployment(a, release.as_ref(), who.run.as_deref()) {
            return Some(crate::node::rejected(why));
        }
        // The real gate: the app's group runs `__upgrade` before `UpgradeApp` is logged.
        if let Err(why) = s.within_lineage(a, release.as_ref()) {
            return Some(crate::node::rejected(why));
        }
        if !who.by_operator
            && !s.upgrades_outside_offers(a, &who.actor, release.as_ref(), &bundle, begun)
        {
            return Some(crate::node::not_a_deployer());
        }
        if s.components_of(release.as_ref()) != a.components {
            return Some(ApiError::new(
                409,
                "COMPONENTS_CHANGED",
                "an app keeps the components it was made with: its package cannot gain or lose its app component in an upgrade",
            ));
        }
        if !who.by_operator {
            if let Err(why) = s.run_update_gate(
                a,
                release.as_ref(),
                class.as_ref(),
                who.run.as_deref(),
                &who.actor,
                begun,
            ) {
                return Some(crate::node::rejected(why));
            }
        }
        s.stalled_for_release(to, &a.org, begun)
            .map(|stall| crate::node::stalled(&stall))
    };
    // The bundle judged is the one the group must still run when it switches (`BundleMoved`).
    let (refusal, judged_on) = node.sys(|s| {
        let judged_on = s.apps.get(&app_id).map(|a| a.bundle.clone());
        (judge_upgrade(s), judged_on)
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
        // An update on its own refused before anything ran is not the release's
        // failure: the leader tries it again after a backoff (`node::auto_updates`), and
        // a failed record would stop it applying until a person upgraded the app.
        if !who.auto {
            let error = json!({ "code": e.code, "message": e.message });
            note_failed_upgrade(node, &app_id, &bundle, release.as_ref(), &who, error).await;
        }
        return Err(e);
    }
    // A compute-only app has no group state to switch: the directory record is all there is.
    let compute_only = node.sys(|s| !s.components_of(release.as_ref()).is_app());
    if compute_only && migrate.is_some() {
        return Err(ApiError::no_app_component());
    }
    // The restore point an undo goes back to, taken just before anything runs.
    let restore_point = match take_restore_point(node, &app_id, &bundle, compute_only).await {
        Ok(taken) => taken,
        // An update on its own needs one; a replica that is down or has no snapshot yet
        // is a passing failure, retried by the leader, not noted on the app.
        Err(e) if who.auto => return Err(e),
        Err(e) => {
            warn!(app = %app_id, status = e.status, code = %e.code, "no restore point was taken; this upgrade cannot be undone");
            None
        }
    };
    let ran = if compute_only {
        Ok(crate::app::Applied {
            index: 0,
            status: Status::Ok,
            result: Value::Null,
            error: Value::Null,
            events: Vec::new(),
            conflict: false,
        })
    } else {
        node.call(&CallRequest::host(
            app_id.clone(),
            "__upgrade",
            json!({ "bundle": bundle, "computeBundle": compute_bundle, "migrate": migrate, "from": judged_on }),
            who.caller.clone(),
        ))
        .await
    };
    let applied = match ran {
        Ok(a) => a,
        Err(e) => {
            // The app's group did not answer (no leader, a bundle it could not fetch):
            // for an update on its own, passing, so retried rather than noted.
            if !who.auto {
                let error = json!({ "code": e.code, "message": e.message });
                note_failed_upgrade(node, &app_id, &bundle, release.as_ref(), &who, error).await;
            }
            return Err(e);
        }
    };
    // A refused or failed upgrade leaves the app on its bundle; say so as an
    // error, not a 200 a client can mistake for success. This one is the release's
    // own (the group ran it, or refused it for good), so it is noted for an update on
    // its own too, which then never applies that release again by itself.
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
    // The app now runs the new bundle: the directory says so too. Catching up ran nothing,
    // and the snapshot is of the code it already ran, so it leaves no undo.
    let restore_point = restore_point.filter(|_| applied.result["caughtUp"] != json!(true));
    let in_deployment = in_deployment_rule(node, &app_id).await;
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
            in_deployment,
            owner_approves: who.owner_approves,
            class,
            auto: who.auto,
            members_stay,
            restore_point,
        })
        .await?;
    Ok(Json(json!(UpgradeApplied {
        index: applied.index,
        status: applied.status,
        error: applied.error,
        compute: has_compute,
    })))
}

/// Apply `to` to `app` on its own under the app's update policy, as the system leader's
/// pass does (`node::auto_updates`): the release's part for the service the app runs,
/// judged like any upgrade (runtime, authority, deployment, release deadlines) and by
/// `SystemState::auto_update_allows` in place of who may, with a restore point first;
/// `UpgradeApp { auto }` is judged again at apply. Only a failure of the release itself
/// is noted on the app (the group ran the upgrade and it failed, or refused it): a
/// refusal or failure before that leaves no record, and the leader tries again after a
/// backoff (`node::auto_updates`).
pub(crate) async fn auto_upgrade(
    node: &Node,
    app: &crate::types::AppRecord,
    to: &PackageRef,
) -> ApiResult {
    let service = to.service.clone().unwrap_or_default();
    let target = node
        .sys(|s| {
            let v = s.packages.get(&to.package)?.versions.get(&to.version)?;
            Some(Target {
                bundle: v.app_parts().get(&service).cloned()?,
                compute_bundle: v.compute.get(&service).cloned(),
                release: Some(to.clone()),
            })
        })
        .ok_or_else(|| crate::node::rejected(crate::system::AUTO_UPDATE_REFUSED))?;
    run_upgrade(
        node,
        app.app_id.clone(),
        target,
        None,
        Upgrader {
            caller: Caller::System,
            actor: crate::system::AUTO_UPDATE.to_owned(),
            by_operator: false,
            consent: Value::Null,
            run: None,
            owner_approves: true,
            auto: true,
            from_release: node.sys(|s| s.auto_update_from(app)),
        },
    )
    .await
}

/// Whether an update on its own carries `members_stay` (every node reads it,
/// [`Feature::AutoUpdateMembersStay`]).
async fn members_stay_rule(node: &Node) -> bool {
    node.require_features(&[Feature::AutoUpdateMembersStay])
        .await
        .is_ok()
}

/// Whether upgrades take restore points ([`Feature::RestorePoints`]): once every node
/// reads `UpgradeApp::restore_point`.
pub(crate) async fn restore_points_rule(node: &Node) -> bool {
    node.require_features(&[Feature::RestorePoints])
        .await
        .is_ok()
}

/// The restore point an upgrade of `app_id` to `bundle` takes before it runs, once every
/// node reads it ([`restore_points_rule`]): none needed (`file: None`) where nothing runs
/// on the app's group (a compute-only app, the bundle it runs already), else the group's
/// snapshot on a replica (`Node::take_restore_point`). `Ok(None)` below the feature.
async fn take_restore_point(
    node: &Node,
    app_id: &str,
    bundle: &str,
    compute_only: bool,
) -> Result<Option<crate::types::TakenRestorePoint>, ApiError> {
    if !restore_points_rule(node).await {
        return Ok(None);
    }
    let app = node.app(app_id).await?;
    if compute_only || app.bundle == bundle {
        return Ok(Some(crate::types::TakenRestorePoint::default()));
    }
    let file = node.take_restore_point(&app).await?;
    Ok(Some(crate::types::TakenRestorePoint { file: Some(file) }))
}

/// Whether an `UpgradeApp` of `app_id` carries `in_deployment`: for an instance
/// from a deployment, once every node reads the flag
/// ([`Feature::UpgradeInDeployment`]); an earlier node would ignore it and move
/// the instance where this one refuses. The API judges the rule either way.
async fn in_deployment_rule(node: &Node, app_id: &str) -> bool {
    node.sys(|s| s.apps.get(app_id).is_some_and(|a| a.deployment.is_some()))
        && node
            .require_features(&[Feature::UpgradeInDeployment])
            .await
            .is_ok()
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
        s.stalled_for_release(to, &app.org, now)
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
    // An update on its own names the release it began from, so apply notes nothing if
    // another move of the app won meanwhile.
    let from_release = if who.auto {
        who.from_release.clone()
    } else {
        None
    };
    let _r = node
        .sys_write(&SystemCmd::UpgradeFailed {
            app_id: app_id.to_owned(),
            bundle: bundle.to_owned(),
            by: who.by(),
            at: now_secs(),
            error,
            run: who.run.clone(),
            release: release.cloned(),
            from_release,
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

/// The apps an organization-wide upgrade of `package` covers, as `apply`
/// derives them (`SystemState::org_run_apps`).
fn run_targets(
    node: &Node,
    org: &str,
    package: &str,
    team: Option<&str>,
    cascade: bool,
) -> Vec<crate::types::AppRecord> {
    node.sys(|s| {
        s.org_run_apps(org, package, team, cascade)
            .iter()
            .filter_map(|a| s.apps.get(a).cloned())
            .collect()
    })
}

/// Where `version` of the package moves an app (same service): its bundle, that
/// release's compute component and the release, if installed and the upgrader
/// may use it ([`upgrade_release`]).
fn target_bundle(
    node: &Node,
    app: &crate::types::AppRecord,
    version: &str,
    who: &Upgrader,
) -> Option<Target> {
    let r = node.release_of_app(app)?;
    let service = r.at.service.clone().unwrap_or_default();
    node.sys(|s| {
        let release = PackageRef {
            package: r.at.package.clone(),
            version: version.to_owned(),
            service: r.at.service.clone(),
            compute: false,
        };
        // An instance from a deployment runs what its developer chose, not what
        // its owner sees: a move that stays on the deployment (its offer that still
        // serves, or a unit member's own package) needs no visibility of the release, as
        // creating from the offer needs none; any other move does.
        let v = s
            .release_offered_to(app, &release, who.run.as_deref())
            .or_else(|| {
                upgrade_release(
                    s,
                    &release,
                    app,
                    &who.actor,
                    who.by_operator,
                    who.run.is_some(),
                )
            })?;
        let bundle = v.app_parts().get(&service).cloned()?;
        Some(Target {
            bundle,
            compute_bundle: v.compute.get(&service).cloned(),
            release: Some(release),
        })
    })
}

/// The release `to` an upgrade may move `app` to: one the upgrading account may use, or,
/// for the operator, one it installed or one the app's owner may use. A run moves an app
/// only to one its owner may use, whoever starts it. A release of another organization
/// never reaches an app.
fn upgrade_release<'s>(
    s: &'s SystemState,
    to: &PackageRef,
    app: &crate::types::AppRecord,
    actor: &str,
    by_operator: bool,
    in_run: bool,
) -> Option<&'s crate::system::PackageVersion> {
    let used = |by: &str| s.release_used(&to.package, &to.version, by);
    if by_operator {
        used(crate::system::OPERATOR).or_else(|| used(&app.owner))
    } else if in_run {
        used(&app.owner)
    } else {
        used(actor)
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
    by_release: bool,
) -> ApiResult {
    start_run_with(node, org, body, apps, who, by_release, BTreeMap::new()).await
}

/// [`start_run`], with a unit's member targets (`UpgradeRun::targets`).
async fn start_run_with(
    node: Arc<Node>,
    org: String,
    body: OrgUpgradeBody,
    apps: Vec<crate::types::AppRecord>,
    who: Upgrader,
    by_release: bool,
    targets: BTreeMap<String, crate::types::RunTarget>,
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
        targets,
    };
    let stored = node
        .sys_write(&SystemCmd::StartUpgradeRun {
            org: org.clone(),
            run: Box::new(run.clone()),
            consent: who.consent.clone(),
            by_release,
            owner_approves: who.owner_approves,
        })
        .await?;
    // The run as it applied: its apps are the state's (`SystemState::upgrade_run_apps`).
    let run: crate::types::UpgradeRun = serde_json::from_value(stored).unwrap_or(run);
    let apps: Vec<crate::types::AppRecord> = apps
        .into_iter()
        .filter(|a| run.apps.contains(&a.app_id))
        .collect();
    let who = Upgrader {
        run: Some(run_id.clone()),
        ..who
    };
    let migrate = body.migrate.clone();
    let targets = run.targets.clone();
    let _task = tokio::spawn(async move {
        for app in apps {
            // Judged as `UpgradeApp { run }` judges it, before anything runs
            // on the app's group: an app on another package or a newer
            // release is noted as failed.
            let refused = node.sys(|s| {
                let r = s.orgs.get(&org)?.upgrade_runs.get(&run_id)?;
                s.run_may_move(r, s.apps.get(&app.app_id)?).err()
            });
            if let Some(why) = refused {
                let error = json!({ "code": "NOT_IN_RUN", "message": why });
                note_failed_upgrade(&node, &app.app_id, "", None, &who, error).await;
                continue;
            }
            let version = targets
                .get(&app.app_id)
                .map_or(body.version.as_str(), |t| t.version.as_str());
            match target_bundle(&node, &app, version, &who) {
                // Only the deployment's managers move its instances: an app from a
                // deployment the run would take off its offer is noted as failed.
                Some(t)
                    if node.sys(|s| {
                        s.within_deployment(&app, t.release.as_ref(), who.run.as_deref())
                            .is_err()
                    }) =>
                {
                    let e = crate::node::outside_deployment();
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
                Some(t) if t.bundle == app.bundle => {
                    // Same app component: nothing to run on the app's group,
                    // though its compute component and its release (a
                    // frontend-only one) may differ, which the directory
                    // records; `UpgradeApp` judges who may as for any upgrade,
                    // and a refusal is noted on the app and the run.
                    let class =
                        run_class(&node, &app.app_id, t.release.as_ref(), who.run.as_deref()).await;
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
                            in_deployment: in_deployment_rule(&node, &app.app_id).await,
                            owner_approves: who.owner_approves,
                            class,
                            auto: false,
                            members_stay: false,
                            restore_point: restore_points_rule(&node)
                                .await
                                .then(crate::types::TakenRestorePoint::default),
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
/// subtree) that runs `package` to `version`: for the organization's owner or an
/// admin (before [`crate::system::cluster_version::Feature::OwnerApprovedUpgrades`],
/// whoever holds `MANAGE_APPLICATION` there), with one passkey assertion over the
/// whole run, where owners hold app upgrades. Each app is still checked on its own
/// (`UpgradeApp`).
async fn org_upgrade(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(mut body): Json<OrgUpgradeBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    owner_holds(&node, |g| g.app_upgrades, "app upgrades")?;
    let owner_approves = owner_approves(&node).await;
    run_refusal(&node, &org, &account, body.team.as_deref(), owner_approves)?;
    // Installed from the registry first if the cluster lacks the release.
    let _bundle = node
        .resolve_release(
            &body.package,
            Some(&body.version),
            None,
            (&account, Some(&org)),
            Via::Account,
        )
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
        owner_approves,
        auto: false,
        from_release: None,
    };
    start_run(node, org, body, apps, who, false).await
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

/// Upgrade the instances of `package` the starter may upgrade, with one passkey
/// assertion over the run, kept on the publisher organization (`org`, which proved it
/// holds the package's key); each app is still checked on its own. From
/// [`crate::system::cluster_version::Feature::OwnerApprovedUpgrades`] on, for an owner
/// or admin of `org`, over the instances of the organizations they are an owner or
/// admin of (their own included), never another organization's; before it, for whoever
/// holds `MANAGE_APPLICATION` in `org`, over every instance its publisher upgrades (its
/// release declares `upgrades: publisher`), whoever owns it.
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
    let owner_approves = owner_approves(&node).await;
    if owner_approves {
        run_refusal(&node, &org, &account, None, true)?;
    } else if !node.sys(|s| s.manages_apps(&org, &account, None)) {
        return Err(ApiError::forbidden(
            "upgrading a published package's instances needs MANAGE_APPLICATION in the publisher",
        ));
    }
    // Before the passkey step-up: a node that could not apply the run's
    // `by_release` would cover other instances than this node does.
    node.require_features(&[crate::system::cluster_version::Feature::PublishedRunByRelease])
        .await?;
    // Installed from the registry first if the cluster lacks the release.
    let _bundle = node
        .resolve_release(
            &package,
            Some(&body.version),
            None,
            (&account, Some(&org)),
            Via::Account,
        )
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
        s.published_run_apps(&package, &account, true, owner_approves)
            .iter()
            .filter_map(|a| s.apps.get(a).cloned())
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
        owner_approves,
        auto: false,
        from_release: None,
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
    start_run(node, org, run, apps, who, true).await
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
        owner_approves: false,
        auto: false,
        from_release: None,
    };
    start_run(node, org, body, apps, who, false).await
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
            owner_approves: false,
            auto: false,
            from_release: None,
        };
        return start_run(node, org, retry, failed, who, false).await;
    }
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    owner_holds(&node, |g| g.app_upgrades, "app upgrades")?;
    let owner_approves = owner_approves(&node).await;
    run_refusal(&node, &org, &account, retry.team.as_deref(), owner_approves)?;
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
        owner_approves,
        auto: false,
        from_release: None,
    };
    start_run(node, org, retry, failed, who, false).await
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
                let release = PackageRef {
                    package: package.clone(),
                    version: version.clone(),
                    service: (!service.is_empty()).then(|| service.clone()),
                    compute: false,
                };
                let v = upgrade_release(s, &release, &app, crate::system::OPERATOR, true, false)?;
                Some(Target {
                    bundle: v.app_parts().get(&service).cloned()?,
                    compute_bundle: v.compute.get(&service).cloned(),
                    release: Some(release),
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
            owner_approves: false,
            auto: false,
            from_release: None,
        },
    )
    .await
}

/// This area's routes, merged into [`super::router`].
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UnitUpgradeBody {
    version: String,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// Upgrade an instance and the apps its release created with it (`AppRecord::links`) in
/// one consent: the instance to `version` of its package, and each member the target
/// release's `uses` gives a release other than the one it runs to that release (the one
/// its lock pinned, else the newest installed one inside its range). One passkey
/// assertion over the instance, its version and every member's target; then each app is
/// upgraded on its own, as an organization's run does (`UpgradeApp { run }`).
async fn app_upgrade_unit(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(mut body): Json<UnitUpgradeBody>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = holder(&node, &headers).await?;
    let version = body.version.clone();
    upgrade_unit(&node, &p, &app_id, &version, body.confirm.take()).await
}

/// What upgrading a unit to `version` moves: the instance's target release, each member's
/// target (`UpgradeRun::targets`) and the member records. The route's and the consent
/// page's one plan, judged on the state as it is now.
pub(crate) struct UnitPlan {
    pub app: crate::types::AppRecord,
    pub target: PackageRef,
    pub targets: BTreeMap<String, crate::types::RunTarget>,
    pub members: Vec<crate::types::AppRecord>,
    /// Judged by the rule only an organization's owner and admins approve ([`owner_approves`]).
    pub owner_approves: bool,
}

/// [`UnitPlan`] for `app_id` to `version`, as `account` (who must be one who may upgrade
/// it); the release is installed from the registry first if the cluster lacks it.
pub(crate) async fn unit_plan(
    node: &Node,
    account: &str,
    app_id: &str,
    version: &str,
) -> Result<UnitPlan, ApiError> {
    owner_holds(node, |g| g.app_upgrades, "app upgrades")?;
    let app = node.app(app_id).await?;
    if app.links.is_empty() {
        return Err(ApiError::bad_request(
            "NOT_A_UNIT",
            "this app was created with no apps of its own: upgrade it with /apps/{app}/upgrade",
        ));
    }
    let owner_approves = owner_approves(node).await;
    if let Some(e) = node.sys(|s| upgrade_refusal(s, &app, account, owner_approves)) {
        return Err(e);
    }
    let current = node
        .release_of_app(&app)
        .ok_or_else(|| ApiError::bad_request("NOT_A_UNIT", "this app runs no package release"))?;
    let target = PackageRef {
        package: current.at.package.clone(),
        version: version.to_owned(),
        service: current.at.service.clone(),
        compute: false,
    };
    // Before anything of the target is read, or a passkey asked for: a unit from a
    // deployment moves only to its offer that still serves, as one app's upgrade does; any
    // other only to a release the account may use, else answered as a missing one.
    if app.deployment.is_some() {
        if node.sys(|s| s.release_offered_to(&app, &target, None).is_none()) {
            return Err(crate::node::outside_deployment());
        }
    } else {
        let _bundle = node
            .resolve_release(
                &target.package,
                Some(version),
                target.service.as_deref(),
                (account, Some(&app.org)),
                Via::Account,
            )
            .await?;
    }
    let unit =
        super::apps::unit_members(node, &target, &app.owner, app.deployment.as_deref()).await?;
    if let Some(added) = unit.members.keys().find(|n| !app.links.contains_key(*n)) {
        return Err(ApiError::conflict(&format!(
            "{}@{} creates {added}, which this instance does not have: a unit gains members only when it is created",
            target.package, target.version
        )));
    }
    let mut targets = BTreeMap::new();
    let mut members = Vec::new();
    for (name, member_id) in &app.links {
        let Some((package, version)) = unit.members.get(name) else {
            continue;
        };
        let Some(member) = node.sys(|s| s.apps.get(member_id).cloned()) else {
            continue;
        };
        let runs = node.release_of_app(&member).map(|r| r.at);
        if runs
            .as_ref()
            .is_some_and(|r| &r.version == version && &r.package == package)
        {
            continue;
        }
        if runs.as_ref().is_some_and(|r| &r.package != package) {
            return Err(ApiError::conflict(&format!(
                "{name} runs another package than {package}: upgrade it on its own"
            )));
        }
        let _prev = targets.insert(
            member_id.clone(),
            crate::types::RunTarget {
                package: package.clone(),
                version: version.clone(),
            },
        );
        members.push(member);
    }
    Ok(UnitPlan {
        app,
        target,
        targets,
        members,
        owner_approves,
    })
}

/// Upgrade the unit of `app_id` to `version` as `p`, with `confirm` the step-up's answer:
/// the route's, and the consent page's for an `upgradeUnit` request.
pub(crate) async fn upgrade_unit(
    node: &Node,
    p: &Principal,
    app_id: &str,
    version: &str,
    confirm: Option<ConfirmBody>,
) -> ApiResult {
    let account = p.account()?;
    let plan = unit_plan(node, &account, app_id, version).await?;
    // Before the passkey step-up: a node that could not apply the run's targets would
    // refuse each member's upgrade.
    if !plan.targets.is_empty() {
        node.require_features(&[crate::system::cluster_version::Feature::UnitUpgrade])
            .await?;
    }
    unit_gate(node, &plan, &account).await?;
    let consent = consent(
        node,
        p,
        &format!("org:{}", plan.app.org),
        "__upgrade_unit",
        &json!({
            "app": plan.app.app_id, "package": plan.target.package,
            "version": plan.target.version, "targets": plan.targets,
        }),
        confirm,
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
        owner_approves: plan.owner_approves,
        auto: false,
        from_release: None,
    };
    let run = OrgUpgradeBody {
        package: plan.target.package,
        version: plan.target.version,
        team: None,
        cascade: false,
        migrate: None,
        confirm: None,
        operator_proof: None,
    };
    let node = node
        .shared()
        .ok_or_else(|| ApiError::unavailable("the node is shutting down"))?;
    let org = plan.app.org.clone();
    let mut apps = vec![plan.app];
    apps.extend(plan.members);
    start_run_with(node, org, run, apps, who, false, plan.targets).await
}

/// A unit's run, judged once as a whole under its instance's policy, as its `UpgradeApp`s
/// apply ([`crate::system::SystemState::run_update_gate`]): the unit's class (its members
/// moving to `plan`'s targets), and where the instance's policy asks for a second
/// approver and the unit is not green, another owner's or admin's first approval of this
/// very unit (`upgrade-approvals` with `unit`). Asked before the passkey, by the route
/// and the consent page alike.
pub(crate) async fn unit_gate(node: &Node, plan: &UnitPlan, account: &str) -> Result<(), ApiError> {
    // Every part at once: a member refused after its instance moved would split the unit.
    node.sys(|s| {
        s.within_lineage(&plan.app, Some(&plan.target))?;
        plan.members.iter().try_for_each(|m| {
            let Some(t) = plan.targets.get(&m.app_id) else {
                return Ok(());
            };
            let to = PackageRef {
                package: t.package.clone(),
                version: t.version.clone(),
                service: None,
                compute: false,
            };
            s.within_lineage(m, Some(&to))
        })
    })
    .map_err(crate::node::rejected)?;
    unclassified_gate(node, &plan.app, Some(&plan.target)).await?;
    let class = class_of(node, &plan.app.app_id, Some(&plan.target), true).await;
    let now = now_secs();
    node.sys(|s| {
        s.update_gate(
            &plan.app,
            Some(&plan.target),
            class.as_ref(),
            &plan.targets,
            account,
            now,
        )
    })
    .map(|_| ())
    .map_err(crate::node::rejected)
}

/// A unit member is not moved on its own off the range the instance that created it
/// references it with (`uses` in the release the instance runs): `409
/// OUTSIDE_UNIT_RANGE`. Moving the whole unit is [`app_upgrade_unit`]'s.
async fn refuse_off_unit_range(
    node: &Node,
    app: &crate::types::AppRecord,
    to: Option<&crate::system::PackageRef>,
) -> Result<(), ApiError> {
    let Some(to) = to else {
        return Ok(());
    };
    let owner = node.sys(|s| {
        s.apps.values().find_map(|root| {
            root.links
                .iter()
                .find(|(_, m)| **m == app.app_id)
                .map(|(name, _)| (root.clone(), name.clone()))
        })
    });
    let Some((root, name)) = owner else {
        return Ok(());
    };
    let Some(release) = node.release_of_app(&root) else {
        return Ok(());
    };
    let Some(version) = node.sys(|s| {
        s.packages
            .get(&release.at.package)
            .and_then(|p| p.versions.get(&release.at.version))
            .cloned()
    }) else {
        return Ok(());
    };
    let card = crate::consent_page::release_capabilities(
        node,
        &release.at.package,
        &version,
        release.at.service.as_deref(),
    )
    .await?;
    let Ok(u) = serde_json::from_value::<calimero_bundle::BundleUse>(card["uses"][&name].clone())
    else {
        return Ok(());
    };
    let inside = semver::VersionReq::parse(&u.range)
        .is_ok_and(|range| semver::Version::parse(&to.version).is_ok_and(|v| range.matches(&v)));
    if u.package != to.package || !inside {
        return Err(ApiError::new(
            409,
            "OUTSIDE_UNIT_RANGE",
            &format!(
                "{} is {}'s {name}, which takes {} {}: upgrade the unit with /apps/{}/upgrade-unit",
                app.app_id, root.app_id, u.package, u.range, root.app_id
            ),
        ));
    }
    Ok(())
}

/// `GET /apps/{app}/update`: whether a newer release is there for the instance to
/// move to, and whether the caller may approve the move. For whoever may read the app's
/// own record (`GET /apps/{app}`'s rule, a session bound to its package included), so
/// its frontend can say "an update is available" and open the consent page.
///
/// `available`: for an instance from a deployment, the release its deployment offers
/// for the package it runs (the newest, when it offers several) where that is not the
/// one it runs (`source: deployment`); for any other, the newest release of its package
/// the caller sees (a session bound to the package sees them all) that is newer than
/// the one it runs and that the upgrade would take (`report::newest_reachable`:
/// authority and runtime; `source: release`). `mayApprove`: whether the caller may
/// approve it under the rule in force ([`upgrade_refusal`]). `approvers`: `operator`
/// where the cluster gives it app upgrades, `owner-and-admins` (the owner and admins of
/// the app's organization) once `Feature::OwnerApprovedUpgrades` is in force, else
/// `managers` (the rule before it: its owner or `MANAGE_APPLICATION` holders, or its
/// publisher's).
async fn app_update(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let (app, p) = readable_app(&node, &headers, &app_id).await?;
    let account = p.as_ref().and_then(|p| p.caller.account());
    let bound = p.as_ref().and_then(Principal::bound_app);
    Ok(Json(update_view(&node, &app, account, bound).await))
}

/// `GET /apps/{app}/update`'s answer for `app`, read by `account` (through package
/// `bound`'s frontend, `SystemState::sees_release_from`); `GET /orgs/{org}/updates` lists the same.
pub(crate) async fn update_view(
    node: &Node,
    app: &crate::types::AppRecord,
    account: Option<&str>,
    bound: Option<&str>,
) -> Value {
    let app = app.clone();
    // The release it runs, named by its record even once uninstalled while another ships
    // its WASM: its update is then shown, unclassified. A record naming none is told only a
    // release its reader reads.
    let running = match &app.release {
        Some(r) => node
            .release_of_app(&app)
            .map(|r| r.at)
            .or_else(|| Some(r.clone())),
        None => node.sys(|s| {
            let reader = crate::report::Reader::Account(account.unwrap_or_default());
            crate::report::release_of_bundle(s, &app.bundle, reader).map(|r| {
                crate::system::PackageRef {
                    package: r.package,
                    version: r.version,
                    service: r.service,
                    compute: false,
                }
            })
        }),
    };
    let runtimes = node.runtimes(&app.replicas).await;
    let owner_approves = owner_approves(node).await;
    let by_operator =
        node.sys(|s| s.governance().app_upgrades == crate::system::Authority::Operator);
    let available = node.sys(|s| {
        let r = running.as_ref()?;
        if let Some(d) = &app.deployment {
            let d = s.deployments.get(d)?;
            let offered = d
                .apps
                .values()
                .filter(|o| {
                    o.package == r.package
                        && s.offered_release(o).is_some_and(|v| s.offer_serves(d, v))
                })
                .map(|o| o.version.as_str())
                .max_by(|a, b| crate::system::compare_versions(a, b))?;
            return (offered != r.version).then(
                || json!({ "package": r.package, "version": offered, "source": "deployment" }),
            );
        }
        let current = crate::report::ReleaseName {
            package: r.package.clone(),
            version: r.version.clone(),
            service: r.service.clone(),
        };
        let frontend = bound == Some(r.package.as_str());
        let sees = |v: &crate::system::PackageVersion| {
            account.is_some_and(|a| s.sees_release_from(v, a, frontend))
                || s.sees_release(v, crate::system::OPERATOR)
        };
        let newer = crate::report::newer_versions(s, &current, sees);
        let v = crate::report::newest_reachable(s, &app, &current, &newer, &runtimes)?;
        Some(json!({ "package": r.package, "version": v.version, "source": "release" }))
    });
    let may_approve = !by_operator
        && account
            .is_some_and(|a| node.sys(|s| upgrade_refusal(s, &app, a, owner_approves).is_none()));
    let approvers = if by_operator {
        "operator"
    } else if owner_approves {
        "owner-and-admins"
    } else {
        "managers"
    };
    // The classifier's verdict on the available update: over the whole unit when the
    // instance has members (they move with it, `upgrade-unit`), else the app alone.
    let mut available = available;
    if let Some(a) = available.as_mut() {
        let version = a["version"].as_str().unwrap_or_default().to_owned();
        let whole_unit = !app.links.is_empty()
            && member_targets_readable(node, &app, running.as_ref(), &version, account).await;
        match crate::egress::judged_update(node, &app, &version, whole_unit).await {
            Ok(Some(c)) => {
                let summary = crate::consent_page::update_summary(&c);
                // Unclassified: what compares the two sides is unknown (`null`), not unchanged.
                if let (Some(o), Value::Object(c)) =
                    (a.as_object_mut(), crate::egress::verdict_json(&c))
                {
                    o.extend(c);
                    let _prev = o.insert("summary".into(), json!(summary));
                }
            }
            Ok(None) => {}
            Err(e) => {
                warn!(app = %app.app_id, status = e.status, code = %e.code, "the available update was not classified")
            }
        }
    }
    // The policy in force, the live first approvals of this app's updates, and how many
    // approvals the available one needs: two for a red (or unclassified) one where the
    // policy asks for a second approver.
    let now = now_secs();
    let (policy, source) = node.sys(|s| s.upgrade_policy_of(&app));
    let pending = node.sys(|s| pending_approvals(s, &app, now));
    if let Some(a) = available.as_mut() {
        let green = a["class"] == json!("green");
        a["approvalsNeeded"] = json!(if policy.second_approver && !green {
            2
        } else {
            1
        });
        let version = a["version"].as_str().unwrap_or_default().to_owned();
        let package = a["package"].as_str().unwrap_or_default().to_owned();
        let first = node.sys(|s| {
            s.orgs.get(&app.org).and_then(|o| {
                o.pending_upgrades
                    .iter()
                    .find(|(approved, p)| {
                        p.covers(approved, &app.app_id, &package, &version)
                            && s.approval_live(&app.org, approved, p, now)
                    })
                    .map(|(_, p)| p.by.clone())
            })
        });
        if let Some(by) = first {
            a["approvedBy"] = json!(by);
        }
    }
    json!(UpdateView {
        app: app.app_id.clone(),
        running: running.map(|r| ReleaseRef {
            package: r.package,
            version: r.version,
        }),
        available,
        unit: !app.links.is_empty(),
        may_approve,
        approvers: approvers.to_owned(),
        policy: PolicyInForce {
            auto_apply: policy.auto_apply,
            hold_hours: policy.hold_hours,
            second_approver: policy.second_approver,
            source: source.to_owned(),
        },
        pending_approvals: pending,
        deployment: app
            .deployment
            .as_ref()
            .and_then(|d| node.sys(|s| s.deployment_shown(d))),
    })
}

/// What `POST /apps/{app}/upgrade` answers: the app log's entry, how it ended, the app's error
/// (`null` when it ended `ok`) and whether the release ships a compute component.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct UpgradeApplied {
    index: u64,
    status: crate::app::Status,
    error: Value,
    compute: bool,
}

/// `GET /apps/{app}/update`: the release the instance runs, the update available to it (with
/// the classifier's verdict), who may approve it, the policy in force and the live first
/// approvals.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpdateView {
    app: String,
    running: Option<ReleaseRef>,
    available: Option<Value>,
    unit: bool,
    may_approve: bool,
    approvers: String,
    policy: PolicyInForce,
    pending_approvals: Vec<PendingApprovalView>,
    deployment: Option<Value>,
}

/// A release: its package and version.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ReleaseRef {
    package: String,
    version: String,
}

/// The update policy an instance follows, and where it comes from.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PolicyInForce {
    auto_apply: crate::types::AutoApply,
    hold_hours: u32,
    second_approver: bool,
    source: String,
}

/// A first approval of a red update waiting for its second (`#PendingApproval`).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PendingApprovalView {
    app: String,
    package: String,
    version: String,
    targets: BTreeMap<String, crate::types::RunTarget>,
    by: String,
    at: u64,
    expires_at: u64,
    class: calimero_bundle::update::Class,
    reasons: Vec<calimero_bundle::update::Reason>,
}

/// `GET /apps/{app}/egress`: where the instance's unit may send data now (`destinations`,
/// `members`), and whether its egress switch is on.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EgressView {
    #[serde(flatten)]
    unit: crate::egress::UnitEgress,
    app: String,
    egress: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    off_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
}

/// Whether `account` may be shown the releases `app`'s members move to with its update to
/// `version`: a member of its organization, or one who may use each of them. They are
/// chosen for the owner (`unit_members`), so a guest or visitor reaching the app is not.
async fn member_targets_readable(
    node: &Node,
    app: &crate::types::AppRecord,
    running: Option<&crate::system::PackageRef>,
    version: &str,
    account: Option<&str>,
) -> bool {
    let (Some(account), Some(running)) = (account, running) else {
        return false;
    };
    if node.sys(|s| s.member_of(&app.org, account)) {
        return true;
    }
    let target = crate::system::PackageRef {
        version: version.to_owned(),
        ..running.clone()
    };
    let Ok(unit) =
        super::apps::unit_members(node, &target, &app.owner, app.deployment.as_deref()).await
    else {
        return false;
    };
    node.sys(|s| {
        unit.members
            .values()
            .all(|(package, version)| s.release_used(package, version, account).is_some())
    })
}

/// The live first approvals ([`crate::system::SystemState::approval_live`]) that cover
/// `app` at `now`: of the app itself, or of the unit it is a member of, each with the app
/// approved, its targets, approver, time, expiry and the class its approver was shown.
pub(super) fn pending_approvals(
    s: &SystemState,
    app: &crate::types::AppRecord,
    now: u64,
) -> Vec<PendingApprovalView> {
    let Some(o) = s.orgs.get(&app.org) else {
        return Vec::new();
    };
    o.pending_upgrades
        .iter()
        .filter(|(approved, p)| {
            (**approved == app.app_id || p.targets.contains_key(&app.app_id))
                && s.approval_live(&app.org, approved, p, now)
        })
        .map(|(approved, p)| approval_json(approved, p))
        .collect()
}

/// A first approval `p` of `approved` as the API answers it (the `PendingApproval`
/// schema): `pendingApprovals` lists these, and `POST /apps/{app}/upgrade-approvals`
/// answers the one it logged.
pub(super) fn approval_json(
    approved: &str,
    p: &crate::types::PendingApproval,
) -> PendingApprovalView {
    PendingApprovalView {
        app: approved.to_owned(),
        package: p.package.clone(),
        version: p.version.clone(),
        targets: p.targets.clone(),
        by: p.by.clone(),
        at: p.at,
        expires_at: p.at.saturating_add(crate::types::MAX_APPROVAL_AGE_SECS),
        class: p.class.class,
        reasons: p.class.reasons.clone(),
    }
}

/// The app `app_id` for whoever `GET /apps/{app}` admits (`principal`, `within`,
/// `reaches`; an anonymous caller judged on fresh state), with the caller; `404` for
/// anyone else.
pub(super) async fn readable_app(
    node: &Node,
    headers: &HeaderMap,
    app_id: &str,
) -> Result<(crate::types::AppRecord, Option<Principal>), ApiError> {
    let app_id = node.canonical_app_id(app_id);
    let p = principal(node, headers).await?;
    // As `GET /apps/{app}`: an anonymous caller is judged on fresh state too.
    if p.is_none() {
        node.sys_fresh().await;
    }
    let app = node.app(&app_id).await?;
    if let Some(p) = &p {
        p.within(node, &app)?;
    }
    let account = p.as_ref().and_then(|p| p.caller.account());
    if !reaches(node, &app, account) {
        return Err(ApiError::not_found("no such app"));
    }
    Ok((app, p))
}

/// `GET /apps/{app}/egress`: where the instance's unit may send data now, for whoever
/// `GET /apps/{app}` admits. `destinations` is every destination of the app and its
/// members (`links`, transitively; never an attached app), empty when no data leaves
/// Calimero (an external endpoint, a TEE included, is listed like any other), and
/// `members` each member by path with its app, release and own destinations. A
/// development bundle declares none: `destinations: []`.
async fn app_egress(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult<Json<EgressView>> {
    let (app, _p) = readable_app(&node, &headers, &app_id).await?;
    let placed = crate::egress::instance_unit(&node, &app).await?;
    let described = placed
        .as_ref()
        .map_or_else(crate::egress::UnitEgress::default, crate::egress::describe);
    // The egress switch, over the destinations: off, none of them is reached.
    let off = node.sys(|s| s.egress_off(&app));
    Ok(Json(EgressView {
        unit: described,
        app: app.app_id.clone(),
        egress: if off.is_some() { "off" } else { "on" }.to_owned(),
        off_by: off.map(|by| by.wire().to_owned()),
        summary: off.map(|_| crate::egress::NO_EGRESS.to_owned()),
    }))
}

pub(super) fn routes() -> Router<Arc<Node>> {
    Router::new()
        .route("/apps/{app}/upgrade", post(app_upgrade))
        .route("/apps/{app}/update", get(app_update))
        .route("/apps/{app}/egress", get(app_egress).post(app_set_egress))
        .route("/apps/{app}/upgrade-unit", post(app_upgrade_unit))
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
