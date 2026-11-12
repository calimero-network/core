//! The API's routes for deployers and deployments.

#[allow(
    clippy::wildcard_imports,
    reason = "the handlers moved here from `api.rs` read its items"
)]
use super::*;

/// How recently, in seconds, a member counts as active in a deployment's
/// customer counts (`activeMembers30d`): 30 days.
const ACTIVE_WINDOW_SECS: u64 = 30 * 24 * 60 * 60;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DeployerBody {
    deployer: Deployer,
    /// RFC 3339: when the grant ends; absent, it does not.
    #[serde(default)]
    until: Option<String>,
    /// The operator's signature, once the node asked for it (`428`).
    #[serde(default)]
    operator_proof: Option<OperatorProofBody>,
    /// The owner's passkey, once the node asked for it (`412`).
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ClusterOwnerBody {
    owner: Deployer,
    #[serde(default)]
    operator_proof: Option<OperatorProofBody>,
}

/// The deployer a `{key}` path names (`account:<id>`, `org:<id>`).
fn deployer_of(key: &str) -> Result<Deployer, ApiError> {
    Deployer::from_key(key).ok_or_else(|| {
        ApiError::bad_request("BAD_REQUEST", "a deployer is `account:<id>` or `org:<id>`")
    })
}

/// Refused `403` once the cluster's owner grants its deployers: the
/// operator no longer may.
async fn operator_grants_deployers(node: &Node) -> Result<(), ApiError> {
    let _caught_up = node.sys_fresh().await;
    if node.sys(|s| s.deployers_owner().is_some()) {
        return Err(ApiError::forbidden(
            "this cluster's owner grants its deployers, not its operator",
        ));
    }
    Ok(())
}

/// The account that may grant deployers as the cluster's owner, judged on
/// fresh state: `403` while the operator grants them, or for anyone not
/// acting for the owner.
async fn owner_granting(node: &Node, headers: &HeaderMap) -> Result<Principal, ApiError> {
    let p = holder(node, headers).await?;
    let account = p.account()?;
    let _caught_up = node.sys_fresh_from(p.sys_index).await;
    node.sys(|s| match s.deployers_owner() {
        None => Err(ApiError::forbidden(
            "this cluster's operator grants its deployers",
        )),
        Some(owner) if s.acts_for(owner, &account) => Ok(()),
        Some(_) => Err(ApiError::forbidden(
            "only the cluster's owner grants its deployers",
        )),
    })?;
    Ok(p)
}

/// Let an account or organization install apps where deployments are
/// granted, signed with the operator key (`cluster / __grant_deployer /
/// { deployer, until }`), while the operator grants deployers.
async fn ops_deployer_grant(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<DeployerBody>,
) -> ApiResult {
    operator(&node, &headers)?;
    let until = unix_secs(body.until.as_deref(), "until")?;
    operator_grants_deployers(&node).await?;
    let proof = operator_proof(
        &node,
        "cluster",
        "__grant_deployer",
        &json!({ "deployer": body.deployer, "until": until }),
        body.operator_proof,
    )?;
    Ok(Json(
        node.sys_write(&SystemCmd::GrantDeployer {
            deployer: body.deployer,
            until,
            at: now_secs(),
            actor: crate::system::OPERATOR.to_owned(),
            proof: Some(proof),
            consent: None,
        })
        .await?,
    ))
}

/// End a deployer's grant (`__revoke_deployer`, `{ deployer }`): it installs
/// nothing new; what it installed keeps running.
async fn ops_deployer_revoke(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(key): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult {
    operator(&node, &headers)?;
    let body = ops_proof_body(&body)?;
    let deployer = deployer_of(&key)?;
    operator_grants_deployers(&node).await?;
    let proof = operator_proof(
        &node,
        "cluster",
        "__revoke_deployer",
        &json!({ "deployer": deployer }),
        body.operator_proof,
    )?;
    Ok(Json(
        node.sys_write(&SystemCmd::RevokeDeployer {
            deployer,
            at: now_secs(),
            actor: crate::system::OPERATOR.to_owned(),
            proof: Some(proof),
            consent: None,
        })
        .await?,
    ))
}

/// Move the cluster's deployments from `open` to `granted`, once and for
/// good (`__deployments_granted`): an existing cluster need not be
/// recreated. The attested governance changes with it.
async fn ops_deployments_granted(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> ApiResult {
    operator(&node, &headers)?;
    let body = ops_proof_body(&body)?;
    let _caught_up = node.sys_fresh().await;
    if node.sys(|s| s.governance().deployments == Deployments::Granted) {
        return Err(ApiError::conflict(
            "this cluster's deployments are granted already",
        ));
    }
    let proof = operator_proof(
        &node,
        "cluster",
        "__deployments_granted",
        &json!({}),
        body.operator_proof,
    )?;
    Ok(Json(
        node.sys_write(&SystemCmd::SetDeploymentsGranted { proof })
            .await?,
    ))
}

/// Name the cluster's owner, once, where governance says `deployers: owner`
/// (`__cluster_owner`, `{ owner }`): it grants deployers from then on.
async fn ops_cluster_owner(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<ClusterOwnerBody>,
) -> ApiResult {
    operator(&node, &headers)?;
    let _caught_up = node.sys_fresh().await;
    node.sys(|s| {
        if s.governance().deployers != Deployers::Owner {
            return Err(ApiError::conflict(
                "this cluster was deployed with its deployers granted by its operator",
            ));
        }
        if s.cluster_owner.is_some() {
            return Err(ApiError::conflict("this cluster's owner is named already"));
        }
        Ok(())
    })?;
    let proof = operator_proof(
        &node,
        "cluster",
        "__cluster_owner",
        &json!({ "owner": body.owner }),
        body.operator_proof,
    )?;
    Ok(Json(
        node.sys_write(&SystemCmd::SetClusterOwner {
            owner: body.owner,
            proof,
        })
        .await?,
    ))
}

/// The cluster's owner grants a deployer, with a passkey assertion over
/// `cluster / __grant_deployer / { deployer, until }` (a `412` asks for it).
async fn deployer_grant(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<DeployerBody>,
) -> ApiResult {
    let until = unix_secs(body.until.as_deref(), "until")?;
    let p = owner_granting(&node, &headers).await?;
    let consent = consent(
        &node,
        &p,
        "cluster",
        "__grant_deployer",
        &json!({ "deployer": body.deployer, "until": until }),
        body.confirm,
    )
    .await?;
    Ok(Json(
        node.sys_write(&SystemCmd::GrantDeployer {
            deployer: body.deployer,
            until,
            at: now_secs(),
            actor: p.account()?,
            proof: None,
            consent: Some(consent),
        })
        .await?,
    ))
}

/// The cluster's owner ends a deployer's grant, with a passkey assertion.
async fn deployer_revoke(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(key): Path<String>,
    body: Option<Json<ConfirmOnlyBody>>,
) -> ApiResult {
    let deployer = deployer_of(&key)?;
    let p = owner_granting(&node, &headers).await?;
    let confirm = body.and_then(|Json(b)| b.confirm);
    let consent = consent(
        &node,
        &p,
        "cluster",
        "__revoke_deployer",
        &json!({ "deployer": deployer }),
        confirm,
    )
    .await?;
    Ok(Json(
        node.sys_write(&SystemCmd::RevokeDeployer {
            deployer,
            at: now_secs(),
            actor: p.account()?,
            proof: None,
            consent: Some(consent),
        })
        .await?,
    ))
}

/// Who may install apps here: the deployments rule, who grants deployers,
/// the owner and every grant. For the operator (its token) and for whoever
/// acts for the cluster's owner; the governance itself is public
/// (`GET /releases`, and every receipt key's attestation).
async fn deployer_list(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    if operator(&node, &headers).is_err() {
        let p = signed_in(&node, &headers).await?;
        let account = p.account()?;
        let _caught_up = node.sys_fresh_from(p.sys_index).await;
        let owner = node.sys(|s| {
            s.cluster_owner
                .as_ref()
                .is_some_and(|o| s.acts_for(o, &account))
        });
        if !owner {
            return Err(ApiError::forbidden(
                "only the cluster's operator and owner read its deployers",
            ));
        }
    } else {
        let _caught_up = node.sys_fresh().await;
    }
    Ok(Json(node.sys(|s| {
        let g = s.governance();
        let grants: Vec<Value> = s
            .deployers
            .iter()
            .map(|(key, grant)| {
                json!({
                    "key": key,
                    "deployer": Deployer::from_key(key),
                    "until": grant.until.map(crate::system::rfc3339),
                    "untilSecs": grant.until,
                    "grantedAt": crate::system::rfc3339(grant.granted_at),
                    "grantedBy": grant.granted_by,
                })
            })
            .collect();
        json!({
            "deployments": g.deployments,
            "deployers": g.deployers,
            "owner": s.cluster_owner,
            "grants": grants,
        })
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DeploymentBody {
    name: String,
    owner: Deployer,
    /// The creator's passkey, once the node asked for it (`412`).
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OfferBody {
    app: String,
    package: String,
    version: String,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// Deployment `id` for `account` to manage, judged on fresh state: `404`
/// when there is none, `403` unless the account acts for its owner. `p` made
/// the request's round (`Node::sys_fresh_from`).
async fn managed_deployment(node: &Node, p: &Principal, id: &str) -> Result<Deployment, ApiError> {
    let account = p.account()?;
    let account = account.as_str();
    let _caught_up = node.sys_fresh_from(p.sys_index).await;
    node.sys(|s| {
        let d = s
            .deployments
            .get(id)
            .ok_or_else(|| ApiError::not_found("no such deployment"))?;
        if !s.acts_for(&d.owner, account) {
            return Err(ApiError::forbidden(
                "only the deployment's owner, or MANAGE_APPLICATION in its organization, manages it",
            ));
        }
        Ok(d.clone())
    })
}

/// A deployment as its managers read it.
fn deployment_json(node: &Node, d: &Deployment) -> Value {
    json!({
        "id": d.id,
        "host": node.deployment_host(&d.id),
        "name": d.name,
        "owner": d.owner,
        "apps": d.apps.iter().map(|(app, o)| json!({
            "app": app,
            "package": o.package,
            "version": o.version,
            "addedBy": o.added_by,
            "addedAt": crate::system::rfc3339(o.added_at),
        })).collect::<Vec<_>>(),
        "createdBy": d.created_by,
        "createdAt": crate::system::rfc3339(d.created_at),
    })
}

/// Create a deployment of `owner` (the caller's own account, or an
/// organization where it holds `MANAGE_APPLICATION` org-wide), with a passkey
/// assertion over `cluster / __create_deployment / { name, owner }` (a `412`
/// asks for it). Its id comes from that consent ([`deployment_id`]).
async fn deployment_create(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<DeploymentBody>,
) -> ApiResult {
    deployment_name_fits(&body.name).map_err(malformed)?;
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    let _caught_up = node.sys_fresh_from(p.sys_index).await;
    if !node.sys(|s| s.acts_for(&body.owner, &account)) {
        return Err(ApiError::forbidden(
            "a deployment is created by its owner account, or by MANAGE_APPLICATION in its organization",
        ));
    }
    node.refuse_undeployable(&account).await?;
    let consent = consent(
        &node,
        &p,
        "cluster",
        "__create_deployment",
        &json!({ "name": body.name, "owner": body.owner }),
        body.confirm,
    )
    .await?;
    let id = deployment_id(consent["nonce"].as_str().unwrap_or_default());
    let host = node.deployment_host(&id);
    let mut out = node
        .sys_write(&SystemCmd::CreateDeployment {
            id,
            name: body.name,
            owner: body.owner,
            actor: account,
            at: now_secs(),
            consent,
        })
        .await?;
    // Its frontend host, `app_host::deployment_label`, for the resolver.
    out["host"] = json!(host);
    Ok(Json(out))
}

/// Offer an installed release the manager sees as `app` (adding it, or
/// moving it to another release of its package), with a passkey assertion
/// over `deployment:{id} / __offer_app / { app, package, version }`.
async fn deployment_offer(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<OfferBody>,
) -> ApiResult {
    deployment_segment_fits(&body.app).map_err(malformed)?;
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    let _d = managed_deployment(&node, &p, &id).await?;
    node.refuse_undeployable(&account).await?;
    let seen = node.sys(|s| {
        s.packages
            .get(&body.package)
            .and_then(|p| p.versions.get(&body.version))
            .is_some_and(|v| s.sees_release(v, &account))
    });
    if !seen {
        return Err(ApiError::not_found(
            "no such release; install it before offering it",
        ));
    }
    let consent = consent(
        &node,
        &p,
        &format!("deployment:{id}"),
        "__offer_app",
        &json!({ "app": body.app, "package": body.package, "version": body.version }),
        body.confirm,
    )
    .await?;
    Ok(Json(
        node.sys_write(&SystemCmd::OfferApp {
            deployment: id,
            app: body.app,
            package: body.package,
            version: body.version,
            actor: account,
            at: now_secs(),
            consent,
        })
        .await?,
    ))
}

/// Withdraw `app` from the deployment, with a passkey assertion over
/// `deployment:{id} / __withdraw_app / { app }`; refused while an instance
/// created from the deployment runs it.
async fn deployment_withdraw(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((id, app)): Path<(String, String)>,
    body: Option<Json<ConfirmOnlyBody>>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    let d = managed_deployment(&node, &p, &id).await?;
    if !d.apps.contains_key(&app) {
        return Err(ApiError::not_found("the deployment offers no such app"));
    }
    let confirm = body.and_then(|Json(b)| b.confirm);
    let consent = consent(
        &node,
        &p,
        &format!("deployment:{id}"),
        "__withdraw_app",
        &json!({ "app": app }),
        confirm,
    )
    .await?;
    Ok(Json(
        node.sys_write(&SystemCmd::WithdrawApp {
            deployment: id,
            app,
            actor: account,
            consent,
        })
        .await?,
    ))
}

/// Delete a deployment that offers nothing, with a passkey assertion over
/// `deployment:{id} / __delete_deployment / {}`.
async fn deployment_delete(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Option<Json<ConfirmOnlyBody>>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    let _d = managed_deployment(&node, &p, &id).await?;
    let confirm = body.and_then(|Json(b)| b.confirm);
    let consent = consent(
        &node,
        &p,
        &format!("deployment:{id}"),
        "__delete_deployment",
        &json!({}),
        confirm,
    )
    .await?;
    Ok(Json(
        node.sys_write(&SystemCmd::DeleteDeployment {
            deployment: id,
            actor: account,
            consent,
        })
        .await?,
    ))
}

/// The deployments the caller manages: its own account's, and those of the
/// organizations where it holds `MANAGE_APPLICATION` org-wide.
async fn deployment_list(State(node): State<Arc<Node>>, headers: HeaderMap) -> ApiResult {
    let p = signed_in(&node, &headers).await?;
    p.unbound()?;
    let account = p.account()?;
    let _caught_up = node.sys_fresh_from(p.sys_index).await;
    let deployments: Vec<Value> = node.sys(|s| {
        s.deployments
            .values()
            .filter(|d| s.acts_for(&d.owner, &account))
            .map(|d| deployment_json(&node, d))
            .collect()
    });
    Ok(Json(json!({ "deployments": deployments })))
}

/// A deployment as anyone may read it, by id: its name, its owner's kind
/// and name, and the apps it offers with their releases; for frontends and
/// the customer's consent. Never its managers, members or customers.
/// Read on fresh state: a replica behind would still serve a deployment
/// deleted, or an app withdrawn, elsewhere.
async fn deployment_get(State(node): State<Arc<Node>>, Path(id): Path<String>) -> ApiResult {
    let _caught_up = node.sys_fresh().await;
    node.sys(|s| {
        let d = s.deployments.get(&id)?;
        let (kind, name) = match &d.owner {
            Deployer::Account(a) => ("account", s.accounts.get(a).map(|a| a.name.clone())),
            Deployer::Org(o) => ("org", s.orgs.get(o).map(|o| o.name.clone())),
        };
        Some(json!({
            "id": d.id,
            "host": node.deployment_host(&d.id),
            "name": d.name,
            "owner": { "kind": kind, "name": name },
            "apps": d.apps.iter().map(|(app, o)| json!({
                "app": app, "package": o.package, "version": o.version,
            })).collect::<Vec<_>>(),
        }))
    })
    .map(Json)
    .ok_or_else(|| ApiError::not_found("no such deployment"))
}

/// One customer organization of a deployment, as its listing counts it.
struct Customer<'a> {
    instances: u64,
    /// (app name in the deployment, the release its instance runs)
    apps: std::collections::BTreeSet<(&'a str, &'a str)>,
    /// When its first instance from the deployment was created.
    first: u64,
}

/// The organizations with instances created from the deployment, for its
/// managers, on fresh state: each organization's id and name, how many
/// instances it has from here, which of the deployment's apps they are and
/// the releases they run, when its first was created, and how many members
/// it has and how many of them were active in the last 30 days. Never who
/// the members are, teams, instance names or app data; instances outside an
/// organization are counted only (`outsideOrgs`).
async fn deployment_orgs(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult {
    let p = signed_in(&node, &headers).await?;
    p.unbound()?;
    let d = managed_deployment(&node, &p, &id).await?;
    let now = now_secs();
    Ok(Json(node.sys(|s| {
        let used = session_use(s, now);
        let mut orgs: BTreeMap<&str, Customer<'_>> = BTreeMap::new();
        let mut outside = 0u64;
        for a in s
            .apps
            .values()
            .filter(|a| a.deployment.as_deref() == Some(id.as_str()))
        {
            // A personal organization is one person's: its instances are
            // only counted (`outsideOrgs`), never named.
            let org = a.org.as_str();
            if s.orgs.get(org).is_some_and(|o| o.personal) {
                outside += 1;
                continue;
            }
            let entry = orgs.entry(org).or_insert(Customer {
                instances: 0,
                apps: std::collections::BTreeSet::new(),
                first: u64::MAX,
            });
            entry.instances += 1;
            entry.first = entry.first.min(a.created_at);
            for (app, o) in &d.apps {
                let version = s.packages.get(&o.package).and_then(|p| {
                    p.versions
                        .values()
                        .find(|v| v.app_parts().values().any(|h| *h == a.bundle))
                });
                if let Some(v) = version {
                    let _new = entry.apps.insert((app.as_str(), v.version.as_str()));
                }
            }
        }
        let orgs: Vec<Value> = orgs
            .into_iter()
            .map(|(org, c)| {
                // Counts only: how many people, and how many were active in
                // the last 30 days; never who.
                let members = s.orgs.get(org).map(|o| o.members.keys());
                let (count, active) = members.into_iter().flatten().fold((0, 0), |(n, a), m| {
                    let recent =
                        last_active(s, m, &used).is_some_and(|t| t + ACTIVE_WINDOW_SECS > now);
                    (n + 1, a + u64::from(recent))
                });
                json!({
                    "orgId": org,
                    "name": s.orgs.get(org).map(|o| o.name.clone()),
                    "memberCount": count,
                    "activeMembers30d": active,
                    "instances": c.instances,
                    "apps": c.apps.iter().map(|(app, version)| json!({
                        "app": app, "version": version,
                    })).collect::<Vec<_>>(),
                    "firstCreatedAt": crate::system::rfc3339(c.first),
                })
            })
            .collect();
        json!({ "deployment": id, "orgs": orgs, "outsideOrgs": outside })
    })))
}

/// This area's routes, merged into [`super::router`].
pub(super) fn routes() -> Router<Arc<Node>> {
    Router::new()
        .route("/ops/deployers", post(ops_deployer_grant))
        .route("/ops/deployers/{key}", delete(ops_deployer_revoke))
        // The same revoke for an HTTP client whose DELETE carries no body
        // (calimero-client-py's), which the operator's proof needs.
        .route("/ops/deployers/{key}/revoke", post(ops_deployer_revoke))
        .route("/ops/deployments/granted", post(ops_deployments_granted))
        .route("/ops/cluster-owner", post(ops_cluster_owner))
        .route("/deployers", post(deployer_grant).get(deployer_list))
        .route("/deployers/{key}", delete(deployer_revoke))
        .route("/deployments", post(deployment_create).get(deployment_list))
        .route("/deployments/{id}", get(deployment_get).delete(deployment_delete))
        .route("/deployments/{id}/apps", post(deployment_offer))
        .route("/deployments/{id}/apps/{app}", delete(deployment_withdraw))
        .route("/deployments/{id}/orgs", get(deployment_orgs))
}
