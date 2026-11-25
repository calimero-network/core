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
async fn deployer_list(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
) -> ApiResult<Json<DeployerList>> {
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
        let grants = s
            .deployers
            .iter()
            .map(|(key, grant)| DeployerGrantView {
                key: key.clone(),
                deployer: Deployer::from_key(key),
                until: grant.until.map(crate::system::rfc3339),
                until_secs: grant.until,
                granted_at: crate::system::rfc3339(grant.granted_at),
                granted_by: grant.granted_by.clone(),
            })
            .collect();
        DeployerList {
            deployments: g.deployments,
            deployers: g.deployers,
            owner: s.cluster_owner.clone(),
            grants,
        }
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
fn deployment_json(node: &Node, d: &Deployment) -> DeploymentView {
    DeploymentView {
        id: d.id.clone(),
        host: node.deployment_host(&d.id),
        name: d.name.clone(),
        owner: d.owner.clone(),
        apps: d
            .apps
            .iter()
            .map(|(app, o)| DeploymentOffer {
                app: app.clone(),
                package: o.package.clone(),
                version: o.version.clone(),
                added_by: o.added_by.clone(),
                added_at: crate::system::rfc3339(o.added_at),
            })
            .collect(),
        created_by: d.created_by.clone(),
        created_at: crate::system::rfc3339(d.created_at),
    }
}

/// `GET /deployers`: the cluster's deployment rules, its owner and every grant.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct DeployerList {
    deployments: crate::system::Deployments,
    deployers: crate::system::Deployers,
    owner: Option<Deployer>,
    grants: Vec<DeployerGrantView>,
}

/// One grant of [`DeployerList`]: `until` RFC 3339, `untilSecs` Unix seconds.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DeployerGrantView {
    key: String,
    deployer: Option<Deployer>,
    until: Option<String>,
    until_secs: Option<u64>,
    granted_at: String,
    granted_by: String,
}

/// A deployment as its managers read it (`#Deployment`): its offers, who added each and when.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DeploymentView {
    id: String,
    host: Option<String>,
    name: String,
    owner: Deployer,
    apps: Vec<DeploymentOffer>,
    created_by: String,
    created_at: String,
}

/// One offer of a [`DeploymentView`].
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DeploymentOffer {
    app: String,
    package: String,
    version: String,
    added_by: String,
    added_at: String,
}

/// `GET /deployments`.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct DeploymentList {
    deployments: Vec<DeploymentView>,
}

/// `POST /deployments`: the record the log made, and its frontend host.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct DeploymentCreated {
    #[serde(flatten)]
    made: crate::system::DeploymentMade,
    host: Option<String>,
}

/// `GET /deployments/{id}`, public: its name, whose it is, and what it offers.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct PublicDeployment {
    id: String,
    host: Option<String>,
    name: String,
    owner: DeploymentOwner,
    apps: Vec<PublicDeploymentApp>,
}

/// The owner of a [`PublicDeployment`]: its kind and name.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct DeploymentOwner {
    kind: String,
    name: Option<String>,
}

/// One offer of a [`PublicDeployment`].
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct PublicDeploymentApp {
    app: String,
    package: String,
    version: String,
}

/// `GET /deployments/{id}/secrets`: each name its offers declare or it holds a value for.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct DeploymentSecrets {
    deployment: String,
    secrets: BTreeMap<String, DeploymentSecretView>,
}

/// One secret of [`DeploymentSecrets`]: never its value.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DeploymentSecretView {
    apps: Vec<String>,
    declared_by: Vec<SecretDeclarer>,
    set: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    set_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    set_at: Option<u64>,
}

/// A release that declares a secret.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SecretDeclarer {
    package: String,
    version: String,
}

/// `GET /deployments/{id}/orgs`: the customer organizations, by counts only.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DeploymentOrgs {
    deployment: String,
    orgs: Vec<DeploymentCustomer>,
    outside_orgs: u64,
}

/// One customer organization of [`DeploymentOrgs`].
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DeploymentCustomer {
    org_id: String,
    name: Option<String>,
    member_count: u64,
    #[serde(rename = "activeMembers30d")]
    active_members_30d: u64,
    instances: u64,
    apps: Vec<CustomerApp>,
    first_created_at: String,
}

/// An app a customer runs, and its version.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct CustomerApp {
    app: String,
    version: String,
}

/// Create a deployment of `owner` (the caller's own account, or an
/// organization where it holds `MANAGE_APPLICATION` org-wide), with a passkey
/// assertion over `cluster / __create_deployment / { name, owner }` (a `412`
/// asks for it). Its id comes from that consent ([`deployment_id`]).
async fn deployment_create(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Json(body): Json<DeploymentBody>,
) -> ApiResult<Json<DeploymentCreated>> {
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
    // Never `deployment_id("")`, which every consent without a nonce would share.
    let nonce = consent["nonce"]
        .as_str()
        .ok_or_else(|| ApiError::internal("the consent names no nonce"))?;
    let id = deployment_id(nonce);
    let host = node.deployment_host(&id);
    let out = node
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
    let made = serde_json::from_value(out).map_err(|e| ApiError::internal(&e.to_string()))?;
    Ok(Json(DeploymentCreated { made, host }))
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
        s.release_used(&body.package, &body.version, &account)
            .is_some()
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
async fn deployment_list(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
) -> ApiResult<Json<DeploymentList>> {
    let p = signed_in(&node, &headers).await?;
    p.unbound()?;
    let account = p.account()?;
    let _caught_up = node.sys_fresh_from(p.sys_index).await;
    let deployments = node.sys(|s| {
        s.deployments
            .values()
            .filter(|d| s.acts_for(&d.owner, &account))
            .map(|d| deployment_json(&node, d))
            .collect()
    });
    Ok(Json(DeploymentList { deployments }))
}

/// A deployment as anyone may read it, by id: its name, its owner's kind
/// and name, and the apps it offers with their releases; for frontends and
/// the customer's consent. Never its managers, members or customers.
/// Read on fresh state: a replica behind would still serve a deployment
/// deleted, or an app withdrawn, elsewhere.
async fn deployment_get(
    State(node): State<Arc<Node>>,
    Path(id): Path<String>,
) -> ApiResult<Json<PublicDeployment>> {
    let _caught_up = node.sys_fresh().await;
    node.sys(|s| {
        let d = s.deployments.get(&id)?;
        let (kind, name) = match &d.owner {
            Deployer::Account(a) => ("account", s.accounts.get(a).map(|a| a.name.clone())),
            Deployer::Org(o) => ("org", s.orgs.get(o).map(|o| o.name.clone())),
        };
        Some(PublicDeployment {
            id: d.id.clone(),
            host: node.deployment_host(&d.id),
            name: d.name.clone(),
            owner: DeploymentOwner {
                kind: kind.to_owned(),
                name,
            },
            apps: d
                .apps
                .iter()
                .map(|(app, o)| PublicDeploymentApp {
                    app: app.clone(),
                    package: o.package.clone(),
                    version: o.version.clone(),
                })
                .collect(),
        })
    })
    .map(Json)
    .ok_or_else(|| ApiError::not_found("no such deployment"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DeploymentSecretBody {
    value: String,
    /// The manager's passkey, once the node asked for it (`412`).
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// A deployment's secrets as its managers read them: every name a release it
/// offers declares, or a release one of those references (`uses`), and every
/// name it holds a value of, each with the apps whose release declares it or
/// references one that does (none: the value is kept, unused, until cleared),
/// the releases that declare it (`declaredBy`), and, when it holds a value,
/// its fingerprint, who set it and when. Never a value.
fn deployment_secrets_json(s: &crate::system::SystemState, d: &Deployment) -> DeploymentSecrets {
    let held = s.deployment_secrets.get(&d.id);
    let mut rows: BTreeMap<String, DeploymentSecretView> = s
        .deployment_declarers(d)
        .into_iter()
        .map(|(name, by)| {
            let declared_by = by
                .declared_by
                .into_iter()
                .map(|(package, version)| SecretDeclarer { package, version })
                .collect();
            let row = DeploymentSecretView {
                apps: by.apps.into_iter().collect(),
                declared_by,
                ..DeploymentSecretView::default()
            };
            (name, row)
        })
        .collect();
    for (name, secret) in held.into_iter().flatten() {
        let row = rows.entry(name.clone()).or_default();
        row.set = true;
        row.fingerprint = Some(secret.fingerprint.clone());
        row.set_by = Some(secret.set_by.clone());
        row.set_at = Some(secret.set_at);
    }
    DeploymentSecrets {
        deployment: d.id.clone(),
        secrets: rows,
    }
}

/// The deployment's secrets, for its managers ([`deployment_secrets_json`]),
/// on fresh state: fingerprints, never values.
async fn deployment_secrets(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<DeploymentSecrets>> {
    let p = signed_in(&node, &headers).await?;
    p.unbound()?;
    let d = managed_deployment(&node, &p, &id).await?;
    Ok(Json(node.sys(|s| deployment_secrets_json(s, &d))))
}

/// Set the deployment's value of a secret a release it offers declares, or a
/// release one of those references (`uses`, for the apps it creates; then
/// `Feature::DeploymentUsedSecrets` too): every instance created from it uses
/// the value unless it set its own, only toward that secret's declared hosts,
/// and only where the instance's own release declares it. With a passkey
/// assertion over `deployment:{id} / __set_secret / { name, fingerprint }`
/// (never the value).
async fn deployment_secret_set(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((id, name)): Path<(String, String)>,
    Json(body): Json<DeploymentSecretBody>,
) -> ApiResult {
    if !calimero_bundle::is_secret_name(&name) {
        return Err(ApiError::bad_request(
            "BAD_SECRET",
            "a secret's name is A-Z, 0-9 and _, starting with a letter",
        ));
    }
    let value = crate::component::SecretValue(body.value);
    if !value.fits() {
        return Err(bad_secret_value());
    }
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    let d = managed_deployment(&node, &p, &id).await?;
    // Judged as apply judges it: the offered releases first, and only past them the
    // releases they reference, which an older node does not apply.
    let (offered, used) = node.sys(|s| {
        let offered = s.deployment_declares(&d, &name);
        (offered, !offered && s.deployment_uses_declare(&d, &name))
    });
    if !offered && !used {
        return Err(crate::node::undeclared_secret());
    }
    let mut features = vec![Feature::DeploymentSecrets];
    if used {
        features.push(Feature::DeploymentUsedSecrets);
    }
    node.require_features(&features).await?;
    let consent = consent(
        &node,
        &p,
        &format!("deployment:{id}"),
        "__set_secret",
        &json!({ "name": name, "fingerprint": value.fingerprint() }),
        body.confirm,
    )
    .await?;
    Ok(Json(
        node.sys_write(&SystemCmd::SetDeploymentSecret {
            deployment: id,
            name,
            value,
            actor: account,
            at: now_secs(),
            consent,
            used,
        })
        .await?,
    ))
}

/// Remove the deployment's value of a secret, with a passkey assertion over
/// `deployment:{id} / __clear_secret / { name }`: its instances that set none
/// of their own fall back to the operator's.
async fn deployment_secret_clear(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((id, name)): Path<(String, String)>,
    body: Option<Json<ConfirmOnlyBody>>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    let _d = managed_deployment(&node, &p, &id).await?;
    let held = node.sys(|s| {
        s.deployment_secrets
            .get(&id)
            .is_some_and(|h| h.contains_key(&name))
    });
    if !held {
        return Err(crate::node::rejected(crate::system::NO_DEPLOYMENT_SECRET));
    }
    node.require_features(&[Feature::DeploymentSecrets]).await?;
    let confirm = body.and_then(|Json(b)| b.confirm);
    let consent = consent(
        &node,
        &p,
        &format!("deployment:{id}"),
        "__clear_secret",
        &json!({ "name": name }),
        confirm,
    )
    .await?;
    Ok(Json(
        node.sys_write(&SystemCmd::ClearDeploymentSecret {
            deployment: id,
            name,
            actor: account,
            consent,
        })
        .await?,
    ))
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
) -> ApiResult<Json<DeploymentOrgs>> {
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
            // A personal organization is one person's: before organizations are
            // ordinary its instances are only counted (`outsideOrgs`), never named.
            // After, it is an organization like any other.
            let org = a.org.as_str();
            if s.orgs.get(org).is_some_and(|o| s.acts_personal(o)) {
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
            // The release the record names: a release found by its WASM could be a private
            // sibling of the one the instance runs.
            for (app, o) in &d.apps {
                if let Some(r) = a.release.as_ref().filter(|r| r.package == o.package) {
                    let _new = entry.apps.insert((app.as_str(), r.version.as_str()));
                }
            }
        }
        let orgs = orgs
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
                DeploymentCustomer {
                    org_id: org.to_owned(),
                    name: s.orgs.get(org).map(|o| o.name.clone()),
                    member_count: count,
                    active_members_30d: active,
                    instances: c.instances,
                    apps: c
                        .apps
                        .iter()
                        .map(|(app, version)| CustomerApp {
                            app: (*app).to_owned(),
                            version: (*version).to_owned(),
                        })
                        .collect(),
                    first_created_at: crate::system::rfc3339(c.first),
                }
            })
            .collect();
        DeploymentOrgs {
            deployment: id.clone(),
            orgs,
            outside_orgs: outside,
        }
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
        .route("/deployments/{id}/secrets", get(deployment_secrets))
        .route(
            "/deployments/{id}/secrets/{name}",
            put(deployment_secret_set).delete(deployment_secret_clear),
        )
}

/// The typed answers write the bytes the `json!` they replaced wrote (review finding E7): each
/// test keeps the replaced expression and compares the serialized text, key order included.
#[cfg(test)]
mod answer_tests {
    use serde::Serialize;
    use serde_json::{json, Value};

    use super::*;

    fn same<T: Serialize>(typed: &T, before: &Value) {
        assert_eq!(
            serde_json::to_string(typed).expect("serializes"),
            serde_json::to_string(before).expect("serializes")
        );
    }

    fn deployment() -> Deployment {
        serde_json::from_value(json!({
            "id": "d1", "name": "acme", "owner": { "org": "o1" },
            "apps": { "kv": { "package": "p1", "version": "1.0.0", "addedBy": "alice", "addedAt": 5 } },
            "createdBy": "alice", "createdAt": 4
        }))
        .expect("a deployment")
    }

    #[test]
    fn deployers_answer_as_before() {
        for until in [Some(1_800_000_000_u64), None] {
            let typed = DeployerList {
                deployments: crate::system::Deployments::Granted,
                deployers: crate::system::Deployers::Owner,
                owner: None,
                grants: vec![DeployerGrantView {
                    key: "account:alice".to_owned(),
                    deployer: Deployer::from_key("account:alice"),
                    until: until.map(crate::system::rfc3339),
                    until_secs: until,
                    granted_at: crate::system::rfc3339(7),
                    granted_by: "operator".to_owned(),
                }],
            };
            let grant = json!({
                "key": "account:alice",
                "deployer": Deployer::from_key("account:alice"),
                "until": until.map(crate::system::rfc3339),
                "untilSecs": until,
                "grantedAt": crate::system::rfc3339(7),
                "grantedBy": "operator",
            });
            let before = json!({
                "deployments": crate::system::Deployments::Granted,
                "deployers": crate::system::Deployers::Owner,
                "owner": Option::<Deployer>::None,
                "grants": [grant],
            });
            same(&typed, &before);
        }
    }

    #[test]
    fn deployments_answer_as_before() {
        let d = deployment();
        for host in [Some("d1.apps.example.com".to_owned()), None] {
            // `deployment_json`, with the host the node would name.
            let view = DeploymentView {
                id: d.id.clone(),
                host: host.clone(),
                name: d.name.clone(),
                owner: d.owner.clone(),
                apps: d
                    .apps
                    .iter()
                    .map(|(app, o)| DeploymentOffer {
                        app: app.clone(),
                        package: o.package.clone(),
                        version: o.version.clone(),
                        added_by: o.added_by.clone(),
                        added_at: crate::system::rfc3339(o.added_at),
                    })
                    .collect(),
                created_by: d.created_by.clone(),
                created_at: crate::system::rfc3339(d.created_at),
            };
            let before = json!({
                "id": d.id,
                "host": host,
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
            });
            same(&view, &before);
            same(
                &DeploymentList {
                    deployments: vec![view],
                },
                &json!({ "deployments": [before] }),
            );
            let public = PublicDeployment {
                id: d.id.clone(),
                host: host.clone(),
                name: d.name.clone(),
                owner: DeploymentOwner {
                    kind: "org".to_owned(),
                    name: None,
                },
                apps: d
                    .apps
                    .iter()
                    .map(|(app, o)| PublicDeploymentApp {
                        app: app.clone(),
                        package: o.package.clone(),
                        version: o.version.clone(),
                    })
                    .collect(),
            };
            let name: Option<String> = None;
            same(
                &public,
                &json!({
                    "id": d.id,
                    "host": host,
                    "name": d.name,
                    "owner": { "kind": "org", "name": name },
                    "apps": d.apps.iter().map(|(app, o)| json!({
                        "app": app, "package": o.package, "version": o.version,
                    })).collect::<Vec<_>>(),
                }),
            );
            // `POST /deployments`: the log's answer, then the host.
            let mut out = json!(crate::system::DeploymentMade {
                id: d.id.clone(),
                deployment: d.clone(),
            });
            let mut before = json!({ "id": d.id, "deployment": d });
            assert_eq!(out, before);
            let made = serde_json::from_value(out.clone()).expect("reads back");
            out["host"] = json!(host);
            before["host"] = json!(host);
            same(
                &DeploymentCreated {
                    made,
                    host: host.clone(),
                },
                &before,
            );
        }
    }

    #[test]
    fn secrets_and_customers_answer_as_before() {
        // A declared name with no value, and one with a value.
        let mut rows = BTreeMap::new();
        let _new = rows.insert(
            "API_KEY".to_owned(),
            DeploymentSecretView {
                apps: vec!["kv".to_owned()],
                declared_by: vec![SecretDeclarer {
                    package: "p1".to_owned(),
                    version: "1.0.0".to_owned(),
                }],
                ..DeploymentSecretView::default()
            },
        );
        let _new = rows.insert(
            "TOKEN".to_owned(),
            DeploymentSecretView {
                set: true,
                fingerprint: Some("fp".to_owned()),
                set_by: Some("account:alice".to_owned()),
                set_at: Some(9),
                ..DeploymentSecretView::default()
            },
        );
        let mut before_rows: BTreeMap<String, Value> = BTreeMap::new();
        let declared: Vec<Value> = vec![json!({ "package": "p1", "version": "1.0.0" })];
        let _new = before_rows.insert(
            "API_KEY".to_owned(),
            json!({ "apps": ["kv"], "declaredBy": declared, "set": false }),
        );
        let mut held = json!({ "apps": [], "declaredBy": [], "set": false });
        held["set"] = json!(true);
        held["fingerprint"] = json!("fp");
        held["setBy"] = json!("account:alice");
        held["setAt"] = json!(9_u64);
        let _new = before_rows.insert("TOKEN".to_owned(), held);
        same(
            &DeploymentSecrets {
                deployment: "d1".to_owned(),
                secrets: rows,
            },
            &json!({ "deployment": "d1", "secrets": before_rows }),
        );
        let (count, active) = (3, 1_u64);
        let instances = 2_u64;
        same(
            &DeploymentOrgs {
                deployment: "d1".to_owned(),
                orgs: vec![DeploymentCustomer {
                    org_id: "o2".to_owned(),
                    name: Some("Customer".to_owned()),
                    member_count: count,
                    active_members_30d: active,
                    instances,
                    apps: vec![CustomerApp {
                        app: "kv".to_owned(),
                        version: "1.0.0".to_owned(),
                    }],
                    first_created_at: crate::system::rfc3339(4),
                }],
                outside_orgs: 1,
            },
            &json!({ "deployment": "d1", "orgs": [json!({
                "orgId": "o2",
                "name": Some("Customer"),
                "memberCount": count,
                "activeMembers30d": active,
                "instances": instances,
                "apps": [json!({ "app": "kv", "version": "1.0.0" })],
                "firstCreatedAt": crate::system::rfc3339(4),
            })], "outsideOrgs": 1_u64 }),
        );
    }
}
