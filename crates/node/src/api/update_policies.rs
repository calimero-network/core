//! The API's routes for update policies, the first approval of a red update, and the
//! updates waiting across an organization's apps.

#[allow(
    clippy::wildcard_imports,
    reason = "the handlers read `api.rs`'s items, as every area's module does"
)]
use super::*;

use crate::system::PolicyView;
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
    let (member, approves) = node.sys_with_org(org, |s, o| {
        (
            o.and_then(|o| o.role(account)).is_some(),
            s.is_org_admin(o, account),
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
) -> ApiResult<Json<PolicyView>> {
    let p = person(&node, &headers).await?;
    p.beyond_instance(&node)?;
    let account = p.account()?;
    node.sys_fresh_from(p.sys_index).await;
    if !node.sys_with_org(&org, |_, o| o.and_then(|o| o.role(&account)).is_some()) {
        return Err(ApiError::not_found("no such organization"));
    }
    Ok(Json(node.sys_org(&org, |s| s.policy_view(&org, None))))
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
) -> ApiResult<Json<PolicyView>> {
    let (app, _p) = super::upgrades::readable_app(&node, &headers, &app_id).await?;
    Ok(Json(
        node.sys(|s| s.policy_view(&app.org, Some(&app.app_id))),
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
    // The release the log says the app runs, installed or not: one uninstalled while
    // another ships its WASM still names the package the update moves along.
    let running = node.sys(|s| s.running(app).cloned()).ok_or_else(|| {
        ApiError::conflict("this app runs no package release: nothing to approve")
    })?;
    let target = PackageRef {
        package: running.package.clone(),
        version: version.to_owned(),
        service: running.service.clone(),
        compute: false,
    };
    // Before the passkey: an instance from a deployment names only its offer that still
    // serves; any other a release the approver may use, else answered as a missing one.
    if app.deployment.is_some() {
        if node.sys(|s| s.release_offered_to(app, &target, None).is_none()) {
            return Err(crate::node::outside_deployment());
        }
    } else if node.sys(|s| {
        s.release_used(&target.package, &target.version, &actor)
            .is_none()
    }) {
        return Err(ApiError::new(
            409,
            "NOT_INSTALLED",
            &format!("{} {} is not installed", target.package, target.version),
        ));
    }
    let targets = if unit {
        super::upgrades::unit_plan(node, &actor, &app.app_id, &target.version)
            .await?
            .targets
    } else {
        BTreeMap::new()
    };
    // A side the log no longer holds cannot be classified: approved as red, the reason
    // naming the release that is gone, as apply takes it (`SystemState::check_class`),
    // once every node decodes that reason (`Feature::UnclassifiedUpdates`).
    super::upgrades::unclassified_gate(node, app, Some(&target)).await?;
    let class = crate::egress::judged_update(node, app, &target.version, unit)
        .await?
        .and_then(super::upgrades::upgrade_class)
        .ok_or_else(|| ApiError::conflict("this update could not be classified"))?;
    if class.is_green() {
        return Err(crate::node::rejected(crate::system::ONE_APPROVAL));
    }
    let now = now_secs();
    // Another's live approval of this very move waits for its second, as `ApproveUpgrade`
    // refuses it where it applies.
    let pending = node.sys(|s| {
        s.orgs
            .get(&app.org)
            .and_then(|o| o.pending_upgrades.get(&app.app_id))
            .is_some_and(|p| {
                p.by != actor
                    && p.package == target.package
                    && p.version == target.version
                    && p.targets == targets
                    && s.approval_live(&app.org, &app.app_id, p, now)
            })
    });
    if pending {
        return Err(crate::node::rejected(crate::system::APPROVAL_PENDING));
    }
    let args =
        crate::system::approve_upgrade_args(&target.package, &target.version, &targets, &class);
    let consent = consent(node, p, &app.app_id, "__approve_upgrade", &args, confirm).await?;
    let logged = node
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
    // Answered as `pendingApprovals` lists it (the `PendingApproval` schema), not as the
    // log keeps it.
    let logged: crate::types::PendingApproval = serde_json::from_value(logged)
        .map_err(|e| ApiError::internal(&format!("the approval logged unread: {e}")))?;
    Ok(Json(json!(ApprovalLogged {
        approval: super::upgrades::approval_json(&app.app_id, &logged),
        app: app.app_id.clone(),
    })))
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
) -> ApiResult<Json<OrgUpdates>> {
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
    // The organization's own policy, else the defaults (`SystemState::policy_json`'s).
    let policy = node.sys(|s| {
        s.orgs
            .get(&org)
            .and_then(|o| o.upgrade_policy)
            .unwrap_or_default()
    });
    Ok(Json(OrgUpdates {
        org,
        policy,
        updates,
        next,
    }))
}

/// `POST /apps/{app}/upgrade-approvals`: the first approval logged.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ApprovalLogged {
    app: String,
    approval: super::upgrades::PendingApprovalView,
}

/// `GET /orgs/{org}/updates`: a page of its apps with an update or a pending approval, each
/// as `GET /apps/{app}/update` shows it with its `name`, and the organization's policy.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct OrgUpdates {
    org: String,
    policy: UpgradePolicy,
    updates: Vec<Value>,
    next: Option<String>,
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

/// `OrgUpdates` writes the bytes the `json!` it replaced wrote.
#[cfg(test)]
mod answer_tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_updates_page_answers_as_before() {
        let policy = UpgradePolicy::default();
        let updates = vec![json!({ "available": null, "name": "App One" })];
        for next in [Some("a9".to_owned()), None] {
            let typed = OrgUpdates {
                org: "o1".to_owned(),
                policy,
                updates: updates.clone(),
                next: next.clone(),
            };
            let before = json!({
                "org": "o1",
                "policy": json!({ "org": "o1", "policy": policy })["policy"],
                "updates": updates,
                "next": next,
            });
            assert_eq!(
                serde_json::to_string(&typed).expect("serializes"),
                serde_json::to_string(&before).expect("serializes")
            );
        }
    }
}
