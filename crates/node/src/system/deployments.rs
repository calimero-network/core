//! System commands for deployers, the cluster's owner and deployments.
//!
//! [`authorize`] judges the proof each carries and [`apply`] applies it;
//! `system.rs` sends a command here by its [`super::Area`]. Each arm of either
//! is one call to a function of its own, in the order of the arms, and each
//! refusal of this area's own rules is a named [`Rejection`] below.

#[allow(
    clippy::wildcard_imports,
    reason = "the arms moved here from `system.rs` read its items"
)]
use super::*;

/// What `SetDeploymentsGranted` answers: the cluster's governance after.
#[derive(Debug, Serialize, Deserialize)]
pub struct GovernanceSet {
    pub governance: Governance,
}

/// What `SetClusterOwner` answers: the owner named.
#[derive(Debug, Serialize, Deserialize)]
pub struct ClusterOwnerNamed {
    pub owner: Deployer,
}

/// What `GrantDeployer` answers: the grant's key, whom it names and its terms.
#[derive(Debug, Serialize, Deserialize)]
pub struct DeployerGranted {
    pub key: String,
    pub deployer: Deployer,
    pub grant: DeployerGrant,
}

/// What `RevokeDeployer` answers: the key of the grant withdrawn.
#[derive(Debug, Serialize, Deserialize)]
pub struct DeployerRevoked {
    pub key: String,
}

/// What `CreateDeployment` answers: its id and record.
#[derive(Debug, Serialize, Deserialize)]
pub struct DeploymentMade {
    pub id: String,
    pub deployment: Deployment,
}

/// What `OfferApp` answers: the offer now in force.
#[derive(Debug, Serialize, Deserialize)]
pub struct AppOffered {
    pub deployment: String,
    pub app: String,
    pub offer: Offer,
}

/// What `WithdrawApp` answers.
#[derive(Debug, Serialize, Deserialize)]
pub struct AppWithdrawn {
    pub deployment: String,
    pub app: String,
}

/// What `DeleteDeployment` answers.
#[derive(Debug, Serialize, Deserialize)]
pub struct DeploymentDeleted {
    pub deployment: String,
}

/// What `OfferDeployment` answers: the offer as logged.
#[derive(Debug, Serialize, Deserialize)]
pub struct DeploymentOffered {
    pub deployment: String,
    pub transfer: PendingTransfer,
}

/// What `AcceptDeployment` answers: its new and previous owner, and the secret names dropped.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentAccepted {
    pub deployment: String,
    pub owner: Deployer,
    pub from: Deployer,
    pub secrets_dropped: Vec<String>,
}

/// What `CancelDeploymentTransfer` answers.
#[derive(Debug, Serialize, Deserialize)]
pub struct DeploymentTransferCancelled {
    pub deployment: String,
}

/// What `SetDeploymentSecret` answers: never the value, its fingerprint.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentSecretSet {
    pub deployment: String,
    pub name: String,
    pub fingerprint: String,
    pub set_by: String,
    pub set_at: u64,
}

/// What `ClearDeploymentSecret` answers.
#[derive(Debug, Serialize, Deserialize)]
pub struct DeploymentSecretCleared {
    pub deployment: String,
    pub name: String,
}

// This area's refusals. Each is `400 REJECTED`, the kind the bare message it
// names was answered with; the message is what a client reads.

/// A deployer, owner or cluster owner named that is neither an account nor
/// an organization.
const NO_SUCH_ACCOUNT_OR_ORG: Rejection =
    Rejection::new(RejectionKind::Rejected, "no such account or organization");
/// A deployer grant that ends no later than it is made.
const GRANT_ENDS_BEFORE_MADE: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "a deployer grant ends after it is made",
);
/// A new deployer grant past [`MAX_DEPLOYERS`].
const TOO_MANY_DEPLOYERS: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "too many deployer grants; revoke an old one first",
);
/// A revocation of a grant the cluster does not keep.
const NO_SUCH_GRANT: Rejection = Rejection::new(RejectionKind::Rejected, "no such deployer grant");
/// `SetDeploymentsGranted` on a cluster whose deployments are granted already.
const GRANTED_ALREADY: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "this cluster's deployments are granted already",
);
/// A cluster owner named where the operator grants deployers.
const NO_OWNER_TO_NAME: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "this cluster's operator grants its deployers; it has no owner to name",
);
/// A cluster owner named a second time.
const OWNER_NAMED: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "this cluster's owner is named already",
);
/// A deployment created by someone who does not act for its owner.
const NOT_FOR_OWNER: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "a deployment is created by its owner account, or by MANAGE_APPLICATION in its organization",
);
/// A deployment of that id exists already.
const DEPLOYMENT_EXISTS: Rejection =
    Rejection::new(RejectionKind::Rejected, "that deployment exists");
/// An offer of a release the manager does not see installed.
const INSTALL_FIRST: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "install the release before offering it",
);
/// An offer that moves an app to another package.
const OFFER_KEEPS_PACKAGE: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "an app moves to another release of its own package; withdraw it to offer another package",
);
/// A withdrawal of an app the deployment does not offer.
const NO_SUCH_OFFER: Rejection =
    Rejection::new(RejectionKind::Rejected, "the deployment offers no such app");
/// A withdrawal while an instance from the deployment runs that app.
const OFFER_RUNS: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "an instance created from this deployment still runs that app; withdraw it once none does",
);
/// A deployment deleted while it offers apps.
const STILL_OFFERS: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "the deployment still offers apps; withdraw them first",
);
/// A deployment deleted while an instance from it exists.
const INSTANCE_EXISTS: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "an instance created from this deployment still exists",
);
/// An offer of a deployment to the owner it has.
const OWNS_IT_ALREADY: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "the deployment is that owner's already",
);
/// An acceptance by someone who does not act for the owner it is offered to.
const NOT_THE_RECEIVER: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "a deployment is accepted by the account it is offered to, or by MANAGE_APPLICATION in that organization",
);
/// A cancellation by someone who acts for neither end of the offer.
const NOT_AN_END_OF_THE_OFFER: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "an offer of a deployment is cancelled by a manager of its owner or of the owner it is offered to",
);
/// A secret's name outside `A-Z`, `0-9` and `_`, or not starting with a letter.
const BAD_SECRET_NAME: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "a secret's name is A-Z, 0-9 and _, starting with a letter",
);

/// Judge the proof a command of this area carries ([`super::authorize`]).
pub(super) fn authorize(proofs: &Proofs<'_>, cmd: &SystemCmd) -> Result<Option<String>, Rejection> {
    match cmd {
        SystemCmd::GrantDeployer {
            deployer,
            until,
            actor,
            proof,
            consent,
            ..
        } => {
            let (statement, used) =
                proofs.deployer_proof(actor, proof.as_ref(), consent.as_ref())?;
            proofs.states(&statement, "cluster", "__grant_deployer", &|args| {
                args["deployer"] == json!(deployer) && args["until"] == json!(until)
            })?;
            Ok(Some(used))
        }
        SystemCmd::RevokeDeployer {
            deployer,
            actor,
            proof,
            consent,
            ..
        } => {
            let (statement, used) =
                proofs.deployer_proof(actor, proof.as_ref(), consent.as_ref())?;
            proofs.states(&statement, "cluster", "__revoke_deployer", &|args| {
                args["deployer"] == json!(deployer)
            })?;
            Ok(Some(used))
        }
        SystemCmd::SetDeploymentsGranted { proof } => {
            operator_signed(proofs, proof, "__deployments_granted", &|_| true)
        }
        SystemCmd::SetClusterOwner { owner, proof } => {
            operator_signed(proofs, proof, "__cluster_owner", &|args| {
                args["owner"] == json!(owner)
            })
        }
        SystemCmd::CreateDeployment {
            id,
            name,
            owner,
            actor,
            consent,
            ..
        } => authorize_create(proofs, id, name, owner, actor, consent),
        SystemCmd::OfferApp {
            deployment,
            app,
            package,
            version,
            actor,
            consent,
            ..
        } => manager_consent(proofs, actor, consent, deployment, "__offer_app", &|args| {
            args["app"] == json!(app)
                && args["package"] == json!(package)
                && args["version"] == json!(version)
        }),
        SystemCmd::WithdrawApp {
            deployment,
            app,
            actor,
            consent,
        } => manager_consent(
            proofs,
            actor,
            consent,
            deployment,
            "__withdraw_app",
            &|args| args["app"] == json!(app),
        ),
        SystemCmd::OfferDeployment {
            deployment,
            to,
            actor,
            consent,
            ..
        } => manager_consent(
            proofs,
            actor,
            consent,
            deployment,
            "__offer_deployment",
            &|args| args["to"] == json!(to),
        ),
        // Bound to the one offer the receiver saw: a cancelled and repeated offer needs a
        // new acceptance.
        SystemCmd::AcceptDeployment {
            deployment,
            to,
            offered_at,
            actor,
            consent,
            ..
        } => manager_consent(
            proofs,
            actor,
            consent,
            deployment,
            "__accept_deployment",
            &|args| args["to"] == json!(to) && args["offeredAt"] == json!(offered_at),
        ),
        SystemCmd::CancelDeploymentTransfer {
            deployment,
            actor,
            consent,
        } => manager_consent(
            proofs,
            actor,
            consent,
            deployment,
            "__cancel_deployment_transfer",
            &|_| true,
        ),
        SystemCmd::DeleteDeployment {
            deployment,
            actor,
            consent,
        } => manager_consent(
            proofs,
            actor,
            consent,
            deployment,
            "__delete_deployment",
            &|_| true,
        ),
        SystemCmd::SetDeploymentSecret {
            deployment,
            name,
            value,
            actor,
            consent,
            ..
        } => {
            // The consent names the value's fingerprint, worked out here from the value
            // the entry carries: no node swaps in another value under it.
            let fingerprint = value.fingerprint();
            manager_consent(
                proofs,
                actor,
                consent,
                deployment,
                "__set_secret",
                &|args| args["name"] == json!(name) && args["fingerprint"] == json!(fingerprint),
            )
        }
        SystemCmd::ClearDeploymentSecret {
            deployment,
            name,
            actor,
            consent,
        } => manager_consent(
            proofs,
            actor,
            consent,
            deployment,
            "__clear_secret",
            &|args| args["name"] == json!(name),
        ),
        _ => Ok(None),
    }
}

/// The operator's signature, not used before, stating `method` on the
/// cluster with arguments `accepts` takes.
fn operator_signed(
    proofs: &Proofs<'_>,
    proof: &Value,
    method: &str,
    accepts: &dyn Fn(&Value) -> bool,
) -> Result<Option<String>, Rejection> {
    let (statement, used) = proofs.fresh(proofs.operator(proof)?)?;
    proofs.states(&statement, "cluster", method, accepts)?;
    Ok(Some(used))
}

/// `actor`'s passkey consent, not used before, stating `method` on the
/// deployment (`deployment:<id>`) with arguments `accepts` takes.
fn manager_consent(
    proofs: &Proofs<'_>,
    actor: &str,
    consent: &Value,
    deployment: &str,
    method: &str,
    accepts: &dyn Fn(&Value) -> bool,
) -> Result<Option<String>, Rejection> {
    let (statement, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
    proofs.states(
        &statement,
        &format!("deployment:{deployment}"),
        method,
        accepts,
    )?;
    Ok(Some(used))
}

/// A `CreateDeployment`: the actor's consent over its name and owner, and
/// the id is the consent's own, so a node cannot pick another.
fn authorize_create(
    proofs: &Proofs<'_>,
    id: &str,
    name: &str,
    owner: &Deployer,
    actor: &str,
    consent: &Value,
) -> Result<Option<String>, Rejection> {
    let (statement, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
    proofs.states(&statement, "cluster", "__create_deployment", &|args| {
        args["name"] == json!(name) && args["owner"] == json!(owner)
    })?;
    if id != deployment_id(&statement.nonce) {
        return Err(CONSENT_FOR_ANOTHER_CHANGE);
    }
    Ok(Some(used))
}

/// Apply a command of this area by its own rules, its proof already
/// judged ([`SystemMachine::run_authorized`]).
pub(super) fn apply(m: &mut SystemMachine, cmd: SystemCmd) -> Result<Value, Rejection> {
    let state = &mut m.state;
    match cmd {
        SystemCmd::GrantDeployer {
            deployer,
            until,
            at,
            actor,
            ..
        } => grant_deployer(state, deployer, until, at, actor),
        SystemCmd::RevokeDeployer {
            deployer, actor, ..
        } => {
            state.grants_deployers(&actor)?;
            let key = deployer.key();
            let _gone = state.deployers.remove(&key).ok_or(NO_SUCH_GRANT)?;
            Ok(json!(DeployerRevoked { key }))
        }
        SystemCmd::SetDeploymentsGranted { .. } => {
            let governance = state.governance.as_mut().ok_or(NO_GOVERNANCE)?;
            if governance.deployments == Deployments::Granted {
                return Err(GRANTED_ALREADY);
            }
            governance.deployments = Deployments::Granted;
            Ok(json!(GovernanceSet {
                governance: governance.clone()
            }))
        }
        SystemCmd::SetClusterOwner { owner, .. } => set_cluster_owner(state, owner),
        cmd @ SystemCmd::CreateDeployment { .. } => create_deployment(state, cmd),
        SystemCmd::OfferApp {
            deployment,
            app,
            package,
            version,
            actor,
            at,
            serving,
            ..
        } => offer_app(state, deployment, app, package, version, actor, at, serving),
        SystemCmd::WithdrawApp {
            deployment,
            app,
            actor,
            ..
        } => withdraw_app(state, deployment, app, &actor),
        SystemCmd::OfferDeployment {
            deployment,
            to,
            actor,
            at,
            ..
        } => offer_deployment(state, deployment, to, actor, at),
        cmd @ SystemCmd::AcceptDeployment { .. } => accept_deployment(state, cmd),
        SystemCmd::CancelDeploymentTransfer {
            deployment, actor, ..
        } => cancel_deployment_transfer(state, deployment, &actor),
        SystemCmd::DeleteDeployment {
            deployment, actor, ..
        } => delete_deployment(state, deployment, &actor),
        SystemCmd::SetDeploymentSecret {
            deployment,
            name,
            value,
            actor,
            at,
            used,
            ..
        } => set_deployment_secret(state, deployment, name, value, &actor, at, used),
        SystemCmd::ClearDeploymentSecret {
            deployment,
            name,
            actor,
            ..
        } => clear_deployment_secret(state, deployment, name, &actor),
        // SAFETY: `SystemCmd::area` sends a command only to the module of
        // its area, and this match has an arm for each command of it.
        _ => unreachable!("a command of another area reached deployments::apply"),
    }
}

/// Grant (or renew) a deployer: whoever grants deployers, for an account or
/// organization that exists, ending after it is made, within [`MAX_DEPLOYERS`].
fn grant_deployer(
    state: &mut SystemState,
    deployer: Deployer,
    until: Option<u64>,
    at: u64,
    actor: AccountId,
) -> Result<Value, Rejection> {
    state.grants_deployers(&actor)?;
    if !state.exists(&deployer) {
        return Err(NO_SUCH_ACCOUNT_OR_ORG);
    }
    if until.is_some_and(|end| end <= at) {
        return Err(GRANT_ENDS_BEFORE_MADE);
    }
    let key = deployer.key();
    if !state.deployers.contains_key(&key) && state.deployers.len() >= MAX_DEPLOYERS {
        return Err(TOO_MANY_DEPLOYERS);
    }
    let grant = DeployerGrant {
        until,
        granted_at: at,
        granted_by: actor,
    };
    let _prev = state.deployers.insert(key.clone(), grant.clone());
    Ok(json!(DeployerGranted {
        key,
        deployer,
        grant
    }))
}

/// Name the cluster's owner, once, where the owner grants deployers.
fn set_cluster_owner(state: &mut SystemState, owner: Deployer) -> Result<Value, Rejection> {
    if state.governance().deployers != Deployers::Owner {
        return Err(NO_OWNER_TO_NAME);
    }
    if state.cluster_owner.is_some() {
        return Err(OWNER_NAMED);
    }
    if !state.exists(&owner) {
        return Err(NO_SUCH_ACCOUNT_OR_ORG);
    }
    state.cluster_owner = Some(owner.clone());
    Ok(json!(ClusterOwnerNamed { owner }))
}

/// Create a deployment: by someone who acts for its owner and may deploy, an
/// organization owner holding a recovery key where that is required, under a new id
/// and a name that fit, within the bounds per owner and in all.
fn create_deployment(state: &mut SystemState, cmd: SystemCmd) -> Result<Value, Rejection> {
    let SystemCmd::CreateDeployment {
        id,
        name,
        owner,
        actor,
        at,
        deployment_needs_recovery,
        ..
    } = cmd
    else {
        // SAFETY: `apply` sends only a `CreateDeployment` here.
        unreachable!("create_deployment applies a CreateDeployment")
    };
    if !state.accounts.contains_key(&actor) {
        return Err(UNKNOWN_ACCOUNT);
    }
    deployment_segment_fits(&id)?;
    deployment_name_fits(&name)?;
    if !state.exists(&owner) {
        return Err(NO_SUCH_ACCOUNT_OR_ORG);
    }
    if !state.acts_for(&owner, &actor) {
        return Err(NOT_FOR_OWNER);
    }
    if !state.may_deploy(&actor, at) {
        return Err(NOT_A_DEPLOYER);
    }
    if !state.keeps_recoverable(&owner, deployment_needs_recovery) {
        return Err(RECOVERY_KEY_REQUIRED);
    }
    if state.deployments.contains_key(&id) {
        return Err(DEPLOYMENT_EXISTS);
    }
    if state
        .deployments
        .values()
        .filter(|deployment| deployment.owner == owner)
        .count()
        >= MAX_DEPLOYMENTS_PER_OWNER
    {
        return Err(Rejection::quota("deployments per owner"));
    }
    if state.deployments.len() >= MAX_DEPLOYMENTS {
        return Err(Rejection::quota("deployments"));
    }
    let deployment = Deployment {
        id: id.clone(),
        name,
        owner,
        apps: BTreeMap::new(),
        created_by: actor,
        created_at: at,
        transfer: None,
        transferred: None,
    };
    let out = json!(DeploymentMade {
        id: id.clone(),
        deployment: deployment.clone(),
    });
    let _prev = state.deployments.insert(id, deployment);
    Ok(out)
}

/// The deployment, for an actor who manages it.
fn managed_deployment<'s>(
    state: &'s SystemState,
    deployment: &str,
    actor: &str,
) -> Result<&'s Deployment, Rejection> {
    let record = state
        .deployments
        .get(deployment)
        .ok_or(NO_SUCH_DEPLOYMENT)?;
    state.manages_deployment(record, actor)?;
    Ok(record)
}

/// Offer an app (a release the manager sees installed) in a deployment, or
/// move it to another release of its package, within [`MAX_APPS_PER_DEPLOYMENT`].
#[allow(
    clippy::too_many_arguments,
    reason = "an offer names its deployment, app, package and version, its actor and time, and the serving orgs baked for the sharded visibility check"
)]
fn offer_app(
    state: &mut SystemState,
    deployment: String,
    app: String,
    package: String,
    version: String,
    actor: AccountId,
    at: u64,
    serving: Vec<crate::types::Org>,
) -> Result<Value, Rejection> {
    let record = managed_deployment(state, &deployment, &actor)?;
    if !state.may_deploy(&actor, at) {
        return Err(NOT_A_DEPLOYER);
    }
    deployment_segment_fits(&app)?;
    // A release the manager may not see is answered as missing. The
    // organizations a private release serves are baked from their own shards
    // (their tenant groups), so the visibility check judges membership against
    // them rather than root's copy
    // (<https://github.com/calimero-network/core/blob/master/docs/design/system-group-sharding.md>).
    let extra: std::collections::BTreeMap<crate::types::OrgId, crate::types::Org> =
        serving.into_iter().map(|o| (o.org_id.clone(), o)).collect();
    let seen = state
        .packages
        .get(&package)
        .and_then(|recorded| recorded.versions.get(&version))
        .is_some_and(|release| state.sees_release_with(release, &actor, &extra));
    if !seen {
        return Err(INSTALL_FIRST);
    }
    match record.apps.get(&app) {
        Some(offer) if offer.package != package => return Err(OFFER_KEEPS_PACKAGE),
        Some(_) => {}
        None if record.apps.len() >= MAX_APPS_PER_DEPLOYMENT => {
            return Err(Rejection::quota("apps per deployment"));
        }
        None => {}
    }
    let offer = Offer {
        package,
        version,
        added_by: actor,
        added_at: at,
    };
    let out = json!(AppOffered {
        deployment: deployment.clone(),
        app: app.clone(),
        offer: offer.clone(),
    });
    if let Some(record) = state.deployments.get_mut(&deployment) {
        let _prev = record.apps.insert(app, offer);
    }
    Ok(out)
}

/// Withdraw an offered app, once no instance from the deployment runs it.
fn withdraw_app(
    state: &mut SystemState,
    deployment: String,
    app: String,
    actor: &str,
) -> Result<Value, Rejection> {
    let record = managed_deployment(state, &deployment, actor)?;
    let offer = record.apps.get(&app).ok_or(NO_SUCH_OFFER)?;
    let running = state.apps.values().any(|instance| {
        instance.deployment.as_deref() == Some(deployment.as_str())
            && state.ships(&offer.package, &instance.bundle)
    });
    if running {
        return Err(OFFER_RUNS);
    }
    if let Some(record) = state.deployments.get_mut(&deployment) {
        let _gone = record.apps.remove(&app);
    }
    Ok(json!(AppWithdrawn { deployment, app }))
}

/// Offer a deployment to another owner: a manager who may hand it over, to an
/// account or organization that exists and is not its owner, while no offer is live.
fn offer_deployment(
    state: &mut SystemState,
    deployment: String,
    to: Deployer,
    actor: AccountId,
    at: u64,
) -> Result<Value, Rejection> {
    let record = managed_deployment(state, &deployment, &actor)?;
    if !state.hands_over(&record.owner, &actor) {
        return Err(NOT_HANDING_OVER);
    }
    if !state.exists(&to) {
        return Err(NO_SUCH_ACCOUNT_OR_ORG);
    }
    if to == record.owner {
        return Err(OWNS_IT_ALREADY);
    }
    if record.transfer.as_ref().is_some_and(|offer| offer.live(at)) {
        return Err(TRANSFER_PENDING);
    }
    let transfer = PendingTransfer {
        to,
        offered_by: actor,
        offered_at: at,
    };
    let out = json!(DeploymentOffered {
        deployment: deployment.clone(),
        transfer: transfer.clone(),
    });
    if let Some(record) = state.deployments.get_mut(&deployment) {
        record.transfer = Some(transfer);
    }
    Ok(out)
}

/// Accept the live offer of a deployment the receiver saw, while whoever made it may
/// still hand it over: the receiver a deployer holding what it needs, every offer still
/// serving under its new owner. The previous owner's secret values stay theirs: an
/// offer the new owner makes could otherwise declare a name and send its value to a
/// host of its own.
fn accept_deployment(state: &mut SystemState, cmd: SystemCmd) -> Result<Value, Rejection> {
    let SystemCmd::AcceptDeployment {
        deployment,
        to,
        offered_at,
        actor,
        at,
        ..
    } = cmd
    else {
        // SAFETY: `apply` sends only an `AcceptDeployment` here.
        unreachable!("accept_deployment applies an AcceptDeployment")
    };
    let record = state
        .deployments
        .get(&deployment)
        .ok_or(NO_SUCH_DEPLOYMENT)?;
    if !state.acts_for(&to, &actor) {
        return Err(NOT_THE_RECEIVER);
    }
    let offer = record
        .transfer
        .as_ref()
        .filter(|offer| offer.to == to && offer.offered_at == offered_at)
        .ok_or(NO_TRANSFER)?;
    if !state.hands_over(&record.owner, &offer.offered_by) {
        return Err(TRANSFER_STALE);
    }
    if !offer.live(at) {
        return Err(TRANSFER_LAPSED);
    }
    if !state.granted(&to, at) {
        return Err(NOT_A_DEPLOYER);
    }
    if !state.keeps_recoverable(&to, true) {
        return Err(RECOVERY_KEY_REQUIRED);
    }
    if state
        .deployments
        .values()
        .filter(|other| other.owner == to)
        .count()
        >= MAX_DEPLOYMENTS_PER_OWNER
    {
        return Err(Rejection::quota("deployments per owner"));
    }
    let theirs = Deployment {
        owner: to.clone(),
        ..record.clone()
    };
    let serves = |app_offer| {
        state
            .offered_release(app_offer)
            .is_none_or(|release| state.offer_serves(&theirs, release))
    };
    if !record.apps.values().all(serves) {
        return Err(OFFERS_NOT_USABLE);
    }
    let from = record.owner.clone();
    let dropped: Vec<String> = state
        .deployment_secrets
        .remove(&deployment)
        .map(|held| held.into_keys().collect())
        .unwrap_or_default();
    let out = json!(DeploymentAccepted {
        deployment: deployment.clone(),
        owner: to.clone(),
        from: from.clone(),
        secrets_dropped: dropped,
    });
    if let Some(record) = state.deployments.get_mut(&deployment) {
        record.owner = to;
        record.transfer = None;
        record.transferred = Some(Transferred { from, at });
    }
    Ok(out)
}

/// Cancel a deployment's offer: whoever acts for its owner or for the owner it is
/// offered to.
fn cancel_deployment_transfer(
    state: &mut SystemState,
    deployment: String,
    actor: &str,
) -> Result<Value, Rejection> {
    let record = state
        .deployments
        .get(&deployment)
        .ok_or(NO_SUCH_DEPLOYMENT)?;
    let offer = record.transfer.as_ref().ok_or(NO_TRANSFER)?;
    if !state.acts_for(&record.owner, actor) && !state.acts_for(&offer.to, actor) {
        return Err(NOT_AN_END_OF_THE_OFFER);
    }
    if let Some(record) = state.deployments.get_mut(&deployment) {
        record.transfer = None;
    }
    Ok(json!(DeploymentTransferCancelled { deployment }))
}

/// Delete a deployment that offers nothing and no instance came from, with
/// its secrets.
fn delete_deployment(
    state: &mut SystemState,
    deployment: String,
    actor: &str,
) -> Result<Value, Rejection> {
    let record = managed_deployment(state, &deployment, actor)?;
    if !record.apps.is_empty() {
        return Err(STILL_OFFERS);
    }
    if state
        .apps
        .values()
        .any(|instance| instance.deployment.as_deref() == Some(deployment.as_str()))
    {
        return Err(INSTANCE_EXISTS);
    }
    let _gone = state.deployments.remove(&deployment);
    let _secrets = state.deployment_secrets.remove(&deployment);
    Ok(json!(DeploymentDeleted { deployment }))
}

/// Set a deployment's secret: a well-formed name a release it offers (or,
/// with `used`, one an offered release uses) declares, a value that fits,
/// within the bounds per deployment and in all. The answer never holds the value.
fn set_deployment_secret(
    state: &mut SystemState,
    deployment: String,
    name: String,
    value: crate::component::SecretValue,
    actor: &str,
    at: u64,
    used: bool,
) -> Result<Value, Rejection> {
    let record = managed_deployment(state, &deployment, actor)?;
    if !calimero_bundle::is_secret_name(&name) {
        return Err(BAD_SECRET_NAME);
    }
    if !value.fits() {
        return Err(crate::component::SECRET_VALUE_SIZE.into());
    }
    // An entry from before `used` is judged by the offered releases alone, as it was.
    let declared = state.deployment_declares(record, &name)
        || (used && state.deployment_uses_declare(record, &name));
    if !declared {
        return Err(UNDECLARED_SECRET);
    }
    check_secret_room(state, &deployment, &name)?;
    let secret = crate::component::InstanceSecret {
        fingerprint: value.fingerprint(),
        value,
        set_by: format!("account:{actor}"),
        set_at: at,
    };
    // What the routes answer of it: never the value.
    let out = json!(DeploymentSecretSet {
        deployment: deployment.clone(),
        name: name.clone(),
        fingerprint: secret.fingerprint.clone(),
        set_by: secret.set_by.clone(),
        set_at: at,
    });
    let _prev = state
        .deployment_secrets
        .entry(deployment)
        .or_default()
        .insert(name, secret);
    Ok(out)
}

/// A new secret name fits within [`MAX_SECRETS_PER_DEPLOYMENT`] and
/// [`MAX_DEPLOYMENT_SECRETS`]; a name already held replaces its value.
fn check_secret_room(state: &SystemState, deployment: &str, name: &str) -> Result<(), Rejection> {
    let held = state.deployment_secrets.get(deployment);
    if held.is_some_and(|secrets| secrets.contains_key(name)) {
        return Ok(());
    }
    if held.is_some_and(|secrets| secrets.len() >= MAX_SECRETS_PER_DEPLOYMENT) {
        return Err(Rejection::quota("secrets per deployment"));
    }
    let all: usize = state.deployment_secrets.values().map(BTreeMap::len).sum();
    if all >= MAX_DEPLOYMENT_SECRETS {
        return Err(Rejection::quota("deployment secrets"));
    }
    Ok(())
}

/// Clear a deployment's secret it holds.
fn clear_deployment_secret(
    state: &mut SystemState,
    deployment: String,
    name: String,
    actor: &str,
) -> Result<Value, Rejection> {
    let _managed = managed_deployment(state, &deployment, actor)?;
    let held = state.deployment_secrets.get_mut(&deployment);
    if held.and_then(|secrets| secrets.remove(&name)).is_none() {
        return Err(NO_DEPLOYMENT_SECRET);
    }
    if state
        .deployment_secrets
        .get(&deployment)
        .is_some_and(BTreeMap::is_empty)
    {
        let _empty = state.deployment_secrets.remove(&deployment);
    }
    Ok(json!(DeploymentSecretCleared { deployment, name }))
}
