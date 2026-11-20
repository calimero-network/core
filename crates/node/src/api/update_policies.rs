//! The API's routes for update policies, the first approval of a red update, and the
//! updates waiting across an organization's apps.

#[allow(
    clippy::wildcard_imports,
    reason = "the handlers read `api.rs`'s items, as every area's module does"
)]
use super::*;

use crate::types::UpgradePolicy;

/// The most apps `GET /orgs/{org}/updates` reads in one page: each is classified, which
/// reads its releases.
pub const MAX_UPDATES_PAGE: usize = 50;

/// A policy to set at one scope; `null` drops it (an app then follows its organization's,
/// an organization the defaults).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct UpgradePolicyBody {
    policy: Option<UpgradePolicy>,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// The first approval of a red update: the app to `version` of the package it runs, or
/// with `unit` the app and the members its upgrade moves (`upgrade-unit`'s plan).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct UpgradeApprovalBody {
    version: String,
    #[serde(default)]
    unit: bool,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// `?after=<app>`: the page after that app id.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct UpdatesQuery {
    #[serde(default)]
    after: Option<String>,
}

/// `400 BAD_UPGRADE_POLICY` for a hold outside a day to a week, before any passkey.
fn check_policy(policy: Option<&UpgradePolicy>) -> Result<(), ApiError> {
    match policy.map(UpgradePolicy::check) {
        Some(Err(why)) => Err(crate::node::rejected(why)),
        _ => Ok(()),
    }
}

/// `404` to anyone outside `org`, `403` to a member who is not its owner or an admin.
pub(crate) fn approver_of(node: &Node, org: &str, account: &str) -> Result<(), ApiError> {
    let (member, approves) = node.sys(|s| {
        (
            s.orgs.get(org).and_then(|o| o.role(account)).is_some(),
            s.approves_upgrades(org, account),
        )
    });
    if !member {
        return Err(ApiError::not_found("no such organization"));
    }
    if !approves {
        return Err(ApiError::forbidden(crate::system::NOT_AN_UPGRADE_APPROVER));
    }
    Ok(())
}

/// `GET /orgs/{org}/upgrade-policy`: the organization's update policy, for its members.
async fn org_policy(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
) -> ApiResult {
    let p = person(&node, &headers).await?;
    let account = p.account()?;
    node.sys_fresh_from(p.sys_index).await;
    if !node.sys(|s| s.orgs.get(&org).and_then(|o| o.role(&account)).is_some()) {
        return Err(ApiError::not_found("no such organization"));
    }
    Ok(Json(node.sys(|s| s.policy_json(&org, None))))
}

/// `POST /orgs/{org}/upgrade-policy`: set or drop the organization's update policy, for
/// every app of it that sets none of its own: its owner or an admin, with a passkey over
/// `org:<org> / __set_upgrade_policy / { app: null, policy }`.
async fn org_policy_set(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<UpgradePolicyBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    node.sys_fresh_from(p.sys_index).await;
    set_upgrade_policy(&node, &p, &org, None, body.policy, body.confirm).await
}

/// Set or drop the update policy of `org` (`app: None`) or of its app `app`, as
/// `POST /orgs/{org}/upgrade-policy` and `POST /apps/{app}/upgrade-policy` (and the consent
/// page's `updatePolicy`) do: `p`'s account an owner or admin of `org`, the policy within
/// bounds and the feature active, all before the passkey over
/// `org:<org> / __set_upgrade_policy / { app, policy }`.
pub(crate) async fn set_upgrade_policy(
    node: &Node,
    p: &Principal,
    org: &str,
    app: Option<&str>,
    policy: Option<UpgradePolicy>,
    confirm: Option<ConfirmBody>,
) -> ApiResult {
    let actor = p.account()?;
    approver_of(node, org, &actor)?;
    check_policy(policy.as_ref())?;
    node.require_features(&[Feature::UpgradePolicies]).await?;
    let args = crate::system::upgrade_policy_args(app, policy.as_ref());
    let consent = org_consent(node, p, org, "__set_upgrade_policy", &args, confirm).await?;
    Ok(Json(
        node.sys_write(&SystemCmd::SetUpgradePolicy {
            org: org.to_owned(),
            app: app.map(str::to_owned),
            policy,
            actor,
            consent,
        })
        .await?,
    ))
}

/// `GET /apps/{app}/upgrade-policy`: the policy an app follows, for whoever may read it.
async fn app_policy(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let (app, _p) = super::upgrades::readable_app(&node, &headers, &app_id).await?;
    Ok(Json(
        node.sys(|s| s.policy_json(&app.org, Some(&app.app_id))),
    ))
}

/// `POST /apps/{app}/upgrade-policy`: set or drop an app's own update policy: an owner or
/// admin of its organization, with a passkey over
/// `org:<org> / __set_upgrade_policy / { app, policy }`.
async fn app_policy_set(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<UpgradePolicyBody>,
) -> ApiResult {
    let (app, _p) = super::upgrades::readable_app(&node, &headers, &app_id).await?;
    let p = holder(&node, &headers).await?;
    set_upgrade_policy(
        &node,
        &p,
        &app.org,
        Some(&app.app_id),
        body.policy,
        body.confirm,
    )
    .await
}

/// `POST /apps/{app}/upgrade-approvals`: the first of the two approvals a red update
/// needs where the app's policy asks for a second approver, logged as its own consent
/// (`<app> / __approve_upgrade / { package, version, targets, class }`); another owner or
/// admin then applies it (`/upgrade` or `/upgrade-unit`).
async fn app_approve_upgrade(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<UpgradeApprovalBody>,
) -> ApiResult {
    let (app, _p) = super::upgrades::readable_app(&node, &headers, &app_id).await?;
    let p = holder(&node, &headers).await?;
    approve_upgrade(&node, &p, &app, &body.version, body.unit, body.confirm).await
}

/// The first approval of `app`'s red update to `version` of the package it runs (with
/// `unit`, of the unit's move), as `POST /apps/{app}/upgrade-approvals` (and the consent
/// page's "Approve (1 of 2)") logs it, by `p`'s account.
pub(crate) async fn approve_upgrade(
    node: &Node,
    p: &Principal,
    app: &crate::types::AppRecord,
    version: &str,
    unit: bool,
    confirm: Option<ConfirmBody>,
) -> ApiResult {
    let actor = p.account()?;
    approver_of(node, &app.org, &actor)?;
    node.require_features(&[Feature::UpgradeClasses, Feature::UpgradePolicies])
        .await?;
    if !node.sys(|s| s.upgrade_policy_of(app).0.second_approver) {
        return Err(crate::node::rejected(crate::system::ONE_APPROVAL));
    }
    let running = node
        .release_of_app(app)
        .ok_or_else(|| ApiError::conflict("this app runs no package release: nothing to approve"))?
        .at;
    let target = PackageRef {
        package: running.package.clone(),
        version: version.to_owned(),
        service: running.service.clone(),
        compute: false,
    };
    if !node.sys(|s| {
        s.packages
            .get(&target.package)
            .is_some_and(|p| p.versions.contains_key(&target.version))
    }) {
        return Err(ApiError::new(
            409,
            "NOT_INSTALLED",
            &format!("{} {} is not installed", target.package, target.version),
        ));
    }
    if node.sys(|s| s.within_deployment(app, Some(&target), None).is_err()) {
        return Err(crate::node::outside_deployment());
    }
    let targets = if unit {
        super::upgrades::unit_plan(node, &actor, &app.app_id, &target.version)
            .await?
            .targets
    } else {
        BTreeMap::new()
    };
    let class = crate::egress::update(node, app, &target.version, unit)
        .await?
        .and_then(super::upgrades::upgrade_class)
        .ok_or_else(|| ApiError::conflict("this update could not be classified"))?;
    if class.is_green() {
        return Err(crate::node::rejected(crate::system::ONE_APPROVAL));
    }
    let now = now_secs();
    let pending = node.sys(|s| {
        s.approval_for(app, &target.package, &target.version, &actor, now)
            .is_some_and(|p| p.targets == targets)
    });
    if pending {
        return Err(crate::node::rejected(crate::system::APPROVAL_PENDING));
    }
    let args =
        crate::system::approve_upgrade_args(&target.package, &target.version, &targets, &class);
    let consent = consent(node, p, &app.app_id, "__approve_upgrade", &args, confirm).await?;
    let approval = node
        .sys_write(&SystemCmd::ApproveUpgrade {
            app_id: app.app_id.clone(),
            package: target.package,
            version: target.version,
            targets,
            class,
            actor,
            at: now,
            consent,
        })
        .await?;
    Ok(Json(json!({ "app": app.app_id, "approval": approval })))
}

/// `GET /orgs/{org}/updates?after=`: for the organization's owner and admins, its apps
/// (by id, at most [`MAX_UPDATES_PAGE`] read per page) that have an update available or
/// a first approval waiting, each as `GET /apps/{app}/update` answers it; `next` is the
/// cursor for the page after, `null` at the end.
async fn org_updates(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Query(q): Query<UpdatesQuery>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    node.sys_fresh_from(p.sys_index).await;
    approver_of(&node, &org, &account)?;
    let after = q.after.unwrap_or_default();
    let mut apps: Vec<crate::types::AppRecord> = node.sys(|s| {
        s.apps
            .values()
            .filter(|a| a.org == org && a.app_id > after)
            .cloned()
            .collect()
    });
    apps.sort_by(|a, b| a.app_id.cmp(&b.app_id));
    let next = (apps.len() > MAX_UPDATES_PAGE).then(|| apps[MAX_UPDATES_PAGE - 1].app_id.clone());
    apps.truncate(MAX_UPDATES_PAGE);
    let mut updates = Vec::new();
    for app in &apps {
        let view = super::upgrades::update_view(&node, app, Some(&account), None).await;
        let pending = view["pendingApprovals"]
            .as_array()
            .is_some_and(|p| !p.is_empty());
        if !view["available"].is_null() || pending {
            let mut view = view;
            view["name"] = json!(app.name);
            updates.push(view);
        }
    }
    Ok(Json(json!({
        "org": org,
        "policy": node.sys(|s| s.policy_json(&org, None))["policy"],
        "updates": updates,
        "next": next,
    })))
}

pub(super) fn routes() -> Router<Arc<Node>> {
    Router::new()
        .route(
            "/orgs/{org}/upgrade-policy",
            get(org_policy).post(org_policy_set),
        )
        .route(
            "/apps/{app}/upgrade-policy",
            get(app_policy).post(app_policy_set),
        )
        .route("/apps/{app}/upgrade-approvals", post(app_approve_upgrade))
        .route("/orgs/{org}/updates", get(org_updates))
}
