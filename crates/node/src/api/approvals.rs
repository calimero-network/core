//! The API's routes for release, deadline and outbound approvals, and the egress switch.

#[allow(
    clippy::wildcard_imports,
    reason = "the handlers moved here from `api.rs` read its items"
)]
use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReleaseApprovalBody {
    /// The mero-tee release, as attestations name it.
    pub(crate) version: String,
    /// `false` withdraws an approval.
    #[serde(default = "yes")]
    pub(crate) approved: bool,
    #[serde(default)]
    pub(crate) confirm: Option<ConfirmBody>,
}

const fn yes() -> bool {
    true
}

/// Approve (or withdraw) a mero-tee release for the apps of `scope`, with a
/// passkey assertion over the exact change: an organization's by whoever
/// holds `APPROVE_RELEASES` there (`SystemState::approves_releases`), an
/// account's own by the account. Until it is approved, nodes on the release
/// get none of those apps' data.
///
/// An approval binds the version and the digest of the release's signed
/// measurements as this node reads them now (`consent_page::release_digest`),
/// so a release re-published under the same tag is another approval. A
/// withdrawal lets nothing in and binds the version alone. `shown` is the
/// digest the consent page displayed; another one now is `CONSENT_STALE`.
pub(crate) async fn set_release_approval(
    node: &Node,
    p: &Principal,
    scope: crate::system::ReleaseScope,
    body: ReleaseApprovalBody,
    shown: Option<&str>,
) -> ApiResult {
    let account = p.account()?;
    let subject = match &scope {
        crate::system::ReleaseScope::Org { org } => format!("org:{org}"),
        crate::system::ReleaseScope::Cluster => "cluster".to_owned(),
    };
    owner_holds(node, |g| g.releases, "release approvals")?;
    // Asked before the passkey ceremony, on fresh state, so a member whose
    // capability was revoked or who left is refused at once; `apply` asks
    // again.
    if let crate::system::ReleaseScope::Org { org } = &scope {
        node.sys_fresh_from(p.sys_index).await;
        let (member, approves) = node.sys(|s| {
            (
                s.orgs.get(org).is_some_and(|o| o.role(&account).is_some()),
                s.approves_releases(org, &account),
            )
        });
        if !member {
            return Err(ApiError::not_found("no such organization"));
        }
        if !approves {
            return Err(ApiError::forbidden(
                "approving releases needs APPROVE_RELEASES in the organization",
            ));
        }
    }
    let method = if body.approved {
        "__approve_release"
    } else {
        "__withdraw_release"
    };
    let args = if body.approved {
        let digest = crate::consent_page::release_digest(node, &body.version).await?;
        if shown.is_some_and(|s| s != digest) {
            return Err(crate::consent_page::stale(&format!(
                "the measurements of mero-tee {}",
                body.version
            )));
        }
        json!({ "version": body.version, "policyDigest": digest })
    } else {
        json!({ "version": body.version })
    };
    let consent = consent(node, p, &subject, method, &args, body.confirm).await?;
    Ok(Json(
        node.sys_write(&SystemCmd::SetReleaseApproval {
            scope,
            version: body.version,
            approved: body.approved,
            actor: account,
            consent,
        })
        .await?,
    ))
}

async fn org_release_approval(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<ReleaseApprovalBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    set_release_approval(
        &node,
        &p,
        crate::system::ReleaseScope::Org { org },
        body,
        None,
    )
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutboundApprovalBody {
    /// The package whose publisher (its pinned signer) the approval is for.
    pub(crate) package: String,
    /// `"manifest"` (the default: every prefix the latest release declares)
    /// or the prefixes to approve.
    #[serde(default = "manifest")]
    pub(crate) prefixes: calimero_primitives::outbound::OutboundSelection,
    /// Set exactly these (how a prefix is revoked) instead of adding them.
    #[serde(default)]
    pub(crate) replace: bool,
    #[serde(default)]
    pub(crate) confirm: Option<ConfirmBody>,
}

/// The release whose prefixes an outbound approval of `package` by `account` covers, shown
/// on the consent page and approved by the route alike: the newest one the account reads.
pub(crate) fn outbound_release<'s>(
    s: &'s crate::system::SystemState,
    package: &str,
    account: &str,
) -> Option<&'s crate::system::PackageVersion> {
    s.newest_read(package, account, false)
}

/// Approve URL prefixes a package's publisher may call out to from the apps
/// of `scope`, with a passkey assertion over the exact change. The owner
/// decides, never the operator: the operator only bounds the hosts.
pub(crate) async fn set_outbound_approval(
    node: &Node,
    p: &Principal,
    scope: crate::system::ReleaseScope,
    body: OutboundApprovalBody,
) -> ApiResult {
    let account = p.account()?;
    // Apply approves the prefixes of the newest release the caller reads: with none, it is
    // answered before the passkey, as for a package that is not installed.
    node.sys_fresh_from(p.sys_index).await;
    if node.sys(|s| outbound_release(s, &body.package, &account).is_none()) {
        return Err(ApiError::not_found("no such package"));
    }
    let subject = match &scope {
        crate::system::ReleaseScope::Org { org } => format!("org:{org}"),
        crate::system::ReleaseScope::Cluster => "cluster".to_owned(),
    };
    owner_holds(node, |g| g.outbound, "outbound approvals")?;
    if let crate::system::ReleaseScope::Org { org } = &scope {
        refuse_egress_off(node, org)?;
    }
    let consent = consent(
        node,
        p,
        &subject,
        "__approve_outbound",
        &json!({
            "package": body.package,
            "prefixes": body.prefixes,
            "replace": body.replace,
        }),
        body.confirm,
    )
    .await?;
    Ok(Json(
        node.sys_write(&SystemCmd::SetOutboundApproval {
            scope,
            package: body.package,
            selection: body.prefixes,
            replace: body.replace,
            actor: account,
            consent,
        })
        .await?,
    ))
}

async fn org_outbound_approval(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<OutboundApprovalBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    set_outbound_approval(&node, &p, crate::system::ReleaseScope::Org { org }, body).await
}

/// `409 EGRESS_OFF` while `org` turned egress off: it approves no outbound host until an
/// owner or admin turns egress on again. Asked before the passkey ceremony; the log asks
/// again (`SetOutboundApproval`).
pub(crate) fn refuse_egress_off(node: &Node, org: &str) -> Result<(), ApiError> {
    if node.sys(|s| s.orgs.get(org).is_some_and(|o| o.egress_off)) {
        return Err(crate::node::rejected(crate::system::EGRESS_OFF));
    }
    Ok(())
}

/// `403`: egress is turned back on only on the node's consent page.
pub(crate) const CONSENT_PAGE_REQUIRED: &str = "CONSENT_PAGE_REQUIRED";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EgressBody {
    /// `false` turns egress off: at once, from any signed-in client of an owner or admin.
    /// `true` turns it back on: only through the consent page (`setEgress`), else `403
    /// CONSENT_PAGE_REQUIRED`.
    pub(crate) egress: bool,
    #[serde(default)]
    pub(crate) confirm: Option<ConfirmBody>,
}

/// Turn egress off or on for every app of `org`, or with `app` for that instance alone
/// (`SystemCmd::SetEgress`): its owner or an admin, with a passkey over `org:<org> /
/// __set_egress / { app, egress }`, as approving outbound hosts takes one. Off, no outbound
/// request leaves the cluster for the apps it covers, whatever their releases declare and
/// whatever was approved, from their next request on; the organization approves no new
/// outbound host while its own switch is off. On again is a red change: the consent page
/// alone runs it (`Principal::consent_page`), so the person sees what it lets out. `404` to
/// anyone outside the organization (or for an app not in it), `403` to a member below admin.
pub(crate) async fn set_egress(
    node: &Node,
    p: &Principal,
    org: String,
    app: Option<String>,
    body: EgressBody,
) -> ApiResult {
    let account = p.account()?;
    node.sys_fresh_from(p.sys_index).await;
    let (member, admin, app_in_org) = node.sys(|s| {
        (
            s.orgs.get(&org).is_some_and(|o| o.role(&account).is_some()),
            s.is_org_admin_self(&org, &account),
            app.as_ref()
                .is_none_or(|id| s.apps.get(id).is_some_and(|a| a.org == org)),
        )
    });
    if !member {
        return Err(ApiError::not_found("no such organization"));
    }
    if !app_in_org {
        return Err(ApiError::not_found("no such app"));
    }
    if !admin {
        return Err(ApiError::forbidden(
            "only the organization's owner or an admin turns its egress off or on",
        ));
    }
    if body.egress && !p.consent_page {
        return Err(ApiError::new(
            403,
            CONSENT_PAGE_REQUIRED,
            "egress is turned back on only on the node's consent page: ask \
             `POST /consent/requests { kind: \"setEgress\" }` and approve it there",
        ));
    }
    let args = crate::system::egress_args(app.as_deref(), body.egress);
    let consent = consent(
        node,
        p,
        &format!("org:{org}"),
        "__set_egress",
        &args,
        body.confirm,
    )
    .await?;
    Ok(Json(
        node.sys_write(&SystemCmd::SetEgress {
            org,
            app,
            egress: body.egress,
            actor: account,
            consent,
        })
        .await?,
    ))
}

async fn org_egress(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<EgressBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    set_egress(&node, &p, org, None, body).await
}

/// `POST /apps/{app}/egress`: [`set_egress`] for one instance, in its organization.
pub(super) async fn app_set_egress(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<EgressBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    node.sys_fresh_from(p.sys_index).await;
    let app_id = node.canonical_app_id(&app_id);
    let org_rec = node
        .sys(|s| s.apps.get(&app_id).map(|a| a.org.clone()))
        .and_then(|o| node.org_of(&o));
    let org = node
        .sys(|s| {
            s.apps
                .get(&app_id)
                .filter(|a| s.may_reach(a, Some(&account), org_rec.as_ref()))
                .map(|a| a.org.clone())
        })
        .ok_or_else(|| ApiError::not_found("no such app"))?;
    set_egress(&node, &p, org, Some(app_id), body).await
}

#[derive(Deserialize)]
pub(super) struct OpsReleaseBody {
    version: String,
    #[serde(default = "yes")]
    approved: bool,
    #[serde(default, rename = "operatorProof")]
    operator_proof: Option<OperatorProofBody>,
}

/// Approve (or withdraw) a mero-tee release for every app, on a cluster whose
/// operator manages upgrades.
async fn ops_release(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<OpsReleaseBody>,
) -> ApiResult {
    operator(&node, &headers)?;
    operator_holds(&node, |g| g.releases, "release approvals")?;
    let method = if body.approved {
        "__approve_release"
    } else {
        "__withdraw_release"
    };
    let proof = operator_proof(
        &node,
        "cluster",
        method,
        &json!({ "version": body.version }),
        body.operator_proof,
    )?;
    Ok(Json(
        node.sys_write(&SystemCmd::SetOperatorRelease {
            proof,
            version: body.version,
            approved: body.approved,
        })
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OpsDeadlineBody {
    /// RFC 3339: from then on, a scope that has not approved the release
    /// starts no new work. `null` lifts the deadline.
    after: Option<String>,
    /// The operator's signature, once the node asked for it (`428`).
    #[serde(default, rename = "operatorProof")]
    operator_proof: Option<OperatorProofBody>,
}

/// Set (or, with `after: null`, lift) the end-of-new-work deadline of a
/// mero-tee release, in any governance: it decides availability, which is the
/// operator's already. It moves no data and blocks no approval; a scope that
/// approves the release (or a newer one) is past it at once. An operator
/// decision: it carries the operator's signature, checked where it applies.
async fn ops_release_deadline(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(version): Path<String>,
    Json(body): Json<OpsDeadlineBody>,
) -> ApiResult {
    operator(&node, &headers)?;
    let after = unix_secs(body.after.as_deref(), "after")?;
    set_release_deadline(&node, version, after, body.operator_proof).await
}

/// Lift a release's deadline (`POST` with `after: null` does the same).
async fn ops_release_deadline_lift(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(version): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    operator(&node, &headers)?;
    let body = ops_proof_body(&body)?;
    set_release_deadline(&node, version, None, body.operator_proof).await
}

async fn set_release_deadline(
    node: &Node,
    version: String,
    after: Option<u64>,
    given: Option<OperatorProofBody>,
) -> ApiResult {
    let proof = operator_proof(
        node,
        "cluster",
        "__release_deadline",
        &json!({ "version": version, "after": after }),
        given,
    )?;
    Ok(Json(
        node.sys_write(&SystemCmd::SetReleaseDeadline {
            version,
            after,
            proof,
        })
        .await?,
    ))
}

/// The cluster's release rules, for anyone, signed in or not (inside the
/// sealed channel, so from an attested node): the governance, and each
/// release's end-of-new-work deadline. Users see the rules before they rely
/// on them.
async fn releases_public(State(node): State<Arc<Node>>) -> ApiResult<Json<ClusterReleases>> {
    node.sys_fresh().await;
    Ok(Json(node.sys(|s| ClusterReleases {
        governance: s.governance(),
        deadlines: s.deadlines_json(),
    })))
}

/// `GET /releases`: the cluster's governance and the deadlines set on mero-tee releases,
/// oldest release first.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ClusterReleases {
    governance: crate::system::Governance,
    deadlines: Vec<crate::system::VersionDeadline>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OpsOutboundBody {
    package: String,
    #[serde(default = "manifest")]
    prefixes: calimero_primitives::outbound::OutboundSelection,
    #[serde(default)]
    replace: bool,
    #[serde(default)]
    operator_proof: Option<OperatorProofBody>,
}

/// Approve URL prefixes a package's publisher may call out to, for every app,
/// where the deployment gave the operator outbound approvals.
async fn ops_outbound(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<OpsOutboundBody>,
) -> ApiResult {
    operator(&node, &headers)?;
    operator_holds(&node, |g| g.outbound, "outbound approvals")?;
    let consent = operator_proof(
        &node,
        "cluster",
        "__approve_outbound",
        &json!({
            "package": body.package,
            "prefixes": body.prefixes,
            "replace": body.replace,
        }),
        body.operator_proof,
    )?;
    Ok(Json(
        node.sys_write(&SystemCmd::SetOutboundApproval {
            scope: crate::system::ReleaseScope::Cluster,
            package: body.package,
            selection: body.prefixes,
            replace: body.replace,
            actor: String::new(),
            consent,
        })
        .await?,
    ))
}

/// This area's routes, merged into [`super::router`].
pub(super) fn routes() -> Router<Arc<Node>> {
    Router::new()
        .route("/orgs/{org}/releases", post(org_release_approval))
        .route("/orgs/{org}/outbound", post(org_outbound_approval))
        .route("/orgs/{org}/egress", post(org_egress))
        .route("/ops/releases", post(ops_release))
        .route(
            "/ops/releases/{version}/deadline",
            post(ops_release_deadline).delete(ops_release_deadline_lift),
        )
        .route("/releases", get(releases_public))
        .route("/ops/outbound", post(ops_outbound))
}
