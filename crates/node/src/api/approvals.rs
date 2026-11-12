//! The API's routes for release, deadline and outbound approvals.

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
    // A package none of whose releases the caller may see is not installed
    // for them: answered before the passkey ceremony, as a missing one.
    node.sys_fresh_from(p.sys_index).await;
    if !node.sys(|s| {
        s.packages
            .get(&body.package)
            .is_some_and(|pkg| s.visible_versions(pkg, &account).next().is_some())
    }) {
        return Err(ApiError::not_found("no such package"));
    }
    let subject = match &scope {
        crate::system::ReleaseScope::Org { org } => format!("org:{org}"),
        crate::system::ReleaseScope::Cluster => "cluster".to_owned(),
    };
    owner_holds(node, |g| g.outbound, "outbound approvals")?;
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
async fn releases_public(State(node): State<Arc<Node>>) -> ApiResult {
    node.sys_fresh().await;
    Ok(Json(node.sys(|s| {
        json!({
            "governance": s.governance(),
            "deadlines": s.deadlines_json(),
        })
    })))
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
        .route("/ops/releases", post(ops_release))
        .route(
            "/ops/releases/{version}/deadline",
            post(ops_release_deadline).delete(ops_release_deadline_lift),
        )
        .route("/releases", get(releases_public))
        .route("/ops/outbound", post(ops_outbound))
}
