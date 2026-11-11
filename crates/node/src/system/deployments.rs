//! System commands for deployers, the cluster's owner and deployments.
//!
//! [`authorize`] judges the proof each carries and [`apply`] applies it;
//! `system.rs` sends a command here by its [`super::Area`].

#[allow(
    clippy::wildcard_imports,
    reason = "the arms moved here from `system.rs` read its items"
)]
use super::*;

/// Judge the proof a command of this area carries ([`super::authorize`]).
pub(super) fn authorize(
    proofs: &Proofs<'_>,
    cmd: &SystemCmd,
) -> Result<Option<String>, &'static str> {
    match cmd {
        SystemCmd::GrantDeployer {
            deployer,
            until,
            actor,
            proof,
            consent,
            ..
        } => {
            let (st, used) = proofs.deployer_proof(actor, proof.as_ref(), consent.as_ref())?;
            proofs.states(&st, "cluster", "__grant_deployer", &|a| {
                a["deployer"] == json!(deployer) && a["until"] == json!(until)
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
            let (st, used) = proofs.deployer_proof(actor, proof.as_ref(), consent.as_ref())?;
            proofs.states(&st, "cluster", "__revoke_deployer", &|a| {
                a["deployer"] == json!(deployer)
            })?;
            Ok(Some(used))
        }
        SystemCmd::SetDeploymentsGranted { proof } => {
            let (st, used) = proofs.fresh(proofs.operator(proof)?)?;
            proofs.states(&st, "cluster", "__deployments_granted", &|_| true)?;
            Ok(Some(used))
        }
        SystemCmd::SetClusterOwner { owner, proof } => {
            let (st, used) = proofs.fresh(proofs.operator(proof)?)?;
            proofs.states(&st, "cluster", "__cluster_owner", &|a| {
                a["owner"] == json!(owner)
            })?;
            Ok(Some(used))
        }
        SystemCmd::CreateDeployment {
            id,
            name,
            owner,
            actor,
            consent,
            ..
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            proofs.states(&st, "cluster", "__create_deployment", &|a| {
                a["name"] == json!(name) && a["owner"] == json!(owner)
            })?;
            // The id is the consent's own, so a node cannot pick another.
            if *id != deployment_id(&st.nonce) {
                return Err("the consent is for another change");
            }
            Ok(Some(used))
        }
        SystemCmd::OfferApp {
            deployment,
            app,
            package,
            version,
            actor,
            consent,
            ..
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            proofs.states(
                &st,
                &format!("deployment:{deployment}"),
                "__offer_app",
                &|a| {
                    a["app"] == json!(app)
                        && a["package"] == json!(package)
                        && a["version"] == json!(version)
                },
            )?;
            Ok(Some(used))
        }
        SystemCmd::WithdrawApp {
            deployment,
            app,
            actor,
            consent,
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            proofs.states(
                &st,
                &format!("deployment:{deployment}"),
                "__withdraw_app",
                &|a| a["app"] == json!(app),
            )?;
            Ok(Some(used))
        }
        SystemCmd::DeleteDeployment {
            deployment,
            actor,
            consent,
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            proofs.states(
                &st,
                &format!("deployment:{deployment}"),
                "__delete_deployment",
                &|_| true,
            )?;
            Ok(Some(used))
        }
        _ => Ok(None),
    }
}

/// Apply a command of this area by its own rules, its proof already
/// judged ([`SystemMachine::run_authorized`]).
#[allow(clippy::too_many_lines, reason = "one arm per command")]
pub(super) fn apply(m: &mut SystemMachine, cmd: SystemCmd) -> Result<Value, &'static str> {
    let s = &mut m.state;
    match cmd {
        SystemCmd::GrantDeployer {
            deployer,
            until,
            at,
            actor,
            ..
        } => {
            s.grants_deployers(&actor)?;
            if !s.exists(&deployer) {
                return Err("no such account or organization");
            }
            if until.is_some_and(|u| u <= at) {
                return Err("a deployer grant ends after it is made");
            }
            let key = deployer.key();
            if !s.deployers.contains_key(&key) && s.deployers.len() >= MAX_DEPLOYERS {
                return Err("too many deployer grants; revoke an old one first");
            }
            let grant = DeployerGrant {
                until,
                granted_at: at,
                granted_by: actor,
            };
            let _prev = s.deployers.insert(key.clone(), grant.clone());
            Ok(json!({ "key": key, "deployer": deployer, "grant": grant }))
        }
        SystemCmd::RevokeDeployer {
            deployer, actor, ..
        } => {
            s.grants_deployers(&actor)?;
            let key = deployer.key();
            let _gone = s.deployers.remove(&key).ok_or("no such deployer grant")?;
            Ok(json!({ "key": key }))
        }
        SystemCmd::SetDeploymentsGranted { proof: _ } => {
            let g = s
                .governance
                .as_mut()
                .ok_or("the cluster has no governance yet")?;
            if g.deployments == Deployments::Granted {
                return Err("this cluster's deployments are granted already");
            }
            g.deployments = Deployments::Granted;
            Ok(json!({ "governance": g }))
        }
        SystemCmd::SetClusterOwner { owner, proof: _ } => {
            if s.governance().deployers != Deployers::Owner {
                return Err(
                    "this cluster's operator grants its deployers; it has no owner to name",
                );
            }
            if s.cluster_owner.is_some() {
                return Err("this cluster's owner is named already");
            }
            if !s.exists(&owner) {
                return Err("no such account or organization");
            }
            s.cluster_owner = Some(owner.clone());
            Ok(json!({ "owner": owner }))
        }
        SystemCmd::CreateDeployment {
            id,
            name,
            owner,
            actor,
            at,
            consent: _,
        } => {
            if !s.accounts.contains_key(&actor) {
                return Err("unknown account");
            }
            deployment_segment_fits(&id)?;
            deployment_name_fits(&name)?;
            if !s.exists(&owner) {
                return Err("no such account or organization");
            }
            if !s.acts_for(&owner, &actor) {
                return Err("a deployment is created by its owner account, or by MANAGE_APPLICATION in its organization");
            }
            if !s.may_deploy(&actor, at) {
                return Err(NOT_A_DEPLOYER);
            }
            if s.deployments.contains_key(&id) {
                return Err("that deployment exists");
            }
            if s.deployments.values().filter(|d| d.owner == owner).count()
                >= MAX_DEPLOYMENTS_PER_OWNER
            {
                return Err("quota exceeded: deployments per owner");
            }
            if s.deployments.len() >= MAX_DEPLOYMENTS {
                return Err("quota exceeded: deployments");
            }
            let d = Deployment {
                id: id.clone(),
                name,
                owner,
                apps: BTreeMap::new(),
                created_by: actor,
                created_at: at,
            };
            let out = json!({ "id": id, "deployment": d });
            let _prev = s.deployments.insert(id, d);
            Ok(out)
        }
        SystemCmd::OfferApp {
            deployment,
            app,
            package,
            version,
            actor,
            at,
            consent: _,
        } => {
            let d = s.deployments.get(&deployment).ok_or("no such deployment")?;
            s.manages_deployment(d, &actor)?;
            if !s.may_deploy(&actor, at) {
                return Err(NOT_A_DEPLOYER);
            }
            deployment_segment_fits(&app)?;
            // A release the manager may not see is answered as missing.
            let seen = s
                .packages
                .get(&package)
                .and_then(|p| p.versions.get(&version))
                .is_some_and(|v| s.sees_release(v, &actor));
            if !seen {
                return Err("install the release before offering it");
            }
            match d.apps.get(&app) {
                Some(o) if o.package != package => {
                    return Err("an app moves to another release of its own package; withdraw it to offer another package");
                }
                Some(_) => {}
                None if d.apps.len() >= MAX_APPS_PER_DEPLOYMENT => {
                    return Err("quota exceeded: apps per deployment");
                }
                None => {}
            }
            let offer = Offer {
                package,
                version,
                added_by: actor,
                added_at: at,
            };
            let out = json!({ "deployment": deployment, "app": app, "offer": offer });
            if let Some(d) = s.deployments.get_mut(&deployment) {
                let _prev = d.apps.insert(app, offer);
            }
            Ok(out)
        }
        SystemCmd::WithdrawApp {
            deployment,
            app,
            actor,
            consent: _,
        } => {
            let d = s.deployments.get(&deployment).ok_or("no such deployment")?;
            s.manages_deployment(d, &actor)?;
            let offer = d
                .apps
                .get(&app)
                .ok_or("the deployment offers no such app")?;
            let running = s.apps.values().any(|a| {
                a.deployment.as_deref() == Some(deployment.as_str())
                    && s.ships(&offer.package, &a.bundle)
            });
            if running {
                return Err("an instance created from this deployment still runs that app; withdraw it once none does");
            }
            if let Some(d) = s.deployments.get_mut(&deployment) {
                let _gone = d.apps.remove(&app);
            }
            Ok(json!({ "deployment": deployment, "app": app }))
        }
        SystemCmd::DeleteDeployment {
            deployment,
            actor,
            consent: _,
        } => {
            let d = s.deployments.get(&deployment).ok_or("no such deployment")?;
            s.manages_deployment(d, &actor)?;
            if !d.apps.is_empty() {
                return Err("the deployment still offers apps; withdraw them first");
            }
            if s.apps
                .values()
                .any(|a| a.deployment.as_deref() == Some(deployment.as_str()))
            {
                return Err("an instance created from this deployment still exists");
            }
            let _gone = s.deployments.remove(&deployment);
            Ok(json!({ "deployment": deployment }))
        }
        // SAFETY: `SystemCmd::area` sends a command only to the module of
        // its area, and this match has an arm for each command of it.
        _ => unreachable!("a command of another area reached deployments::apply"),
    }
}
