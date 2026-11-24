//! System commands for the app directory: instances, upgrades, transfers, moves, replicas, restores.
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
    let s = proofs.s;
    match cmd {
        // An update applying on its own carries no person's consent: the app's policy is
        // its authority, judged at apply (`SystemState::auto_update_gate`).
        SystemCmd::UpgradeApp {
            auto: true,
            by_operator: false,
            run: None,
            actor,
            ..
        } if actor == AUTO_UPDATE => Ok(None),
        SystemCmd::UpgradeApp { auto: true, .. } => Err(AUTO_UPDATE_REFUSED),
        SystemCmd::UpgradeApp {
            app_id,
            bundle,
            by_operator,
            actor,
            at,
            run: None,
            consent,
            ..
        } => {
            // An upgrade key's signature, judged against the key as the log
            // holds it now: revoked or expired, it proves nothing.
            if let (false, Some(key_id)) = (*by_operator, crate::proof::key_proof_id(consent)) {
                let key = s
                    .api_keys
                    .values()
                    .find(|k| k.key_id == key_id && k.account == *actor)
                    .ok_or("no such API key: it was revoked")?;
                let scope = s.key_may_upgrade(key, app_id, bundle, *at)?;
                let (_, st, used) = crate::proof::verify_key_upgrade(consent, &scope.proof_key)?;
                let (st, used) = proofs.fresh((st, used))?;
                proofs.states(&st, app_id, "__upgrade", &|a| a["bundle"] == json!(bundle))?;
                return Ok(Some(used));
            }
            let (st, used) = if *by_operator {
                proofs.fresh(proofs.operator(consent)?)?
            } else {
                proofs.fresh(proofs.consent_of(actor, consent)?)?
            };
            proofs.states(&st, app_id, "__upgrade", &|a| a["bundle"] == json!(bundle))?;
            Ok(Some(used))
        }
        SystemCmd::UpgradeApp {
            app_id,
            bundle,
            by_operator,
            actor,
            run: Some(run),
            release: target,
            ..
        } => {
            // One consent covers the run (`StartUpgradeRun`); each app's
            // upgrade must be one the run named, to the release it names.
            // The run is kept by the organization that started it: the app's
            // own, or the publisher's for a publisher's run over every
            // instance of its package (as `note_run` finds it).
            let r = s
                .orgs
                .values()
                .find_map(|o| o.upgrade_runs.get(run))
                .ok_or("no such upgrade run")?;
            let by = if *by_operator {
                OPERATOR
            } else {
                actor.as_str()
            };
            let (package, version) = r.target_of(app_id);
            let release = s
                .packages
                .get(package)
                .and_then(|p| p.versions.get(version));
            if r.by != by
                || r.finished
                || !r.apps.contains(app_id)
                || !release.is_some_and(|v| v.app_parts().values().any(|h| h == bundle))
                || target
                    .as_ref()
                    .is_some_and(|t| t.package != package || t.version != version)
            {
                return Err(NOT_IN_RUN);
            }
            Ok(None)
        }
        SystemCmd::StartUpgradeRun {
            org, run, consent, ..
        } => {
            let (st, used) = if run.by == OPERATOR {
                proofs.fresh(proofs.operator(consent)?)?
            } else {
                proofs.fresh(proofs.consent_of(&run.by, consent)?)?
            };
            let subject = format!("org:{org}");
            let same = |a: &Value| {
                a["package"] == json!(run.package)
                    && a["version"] == json!(run.version)
                    && a["team"] == json!(run.team)
                    && a["cascade"].as_bool().unwrap_or(false) == run.cascade
            };
            if st.method == "__upgrade_unit" {
                // A unit's run: the instance, its version and each member's target, as
                // the consent states them; which members it covers is the state's.
                proofs.states(&st, &subject, "__upgrade_unit", &|a| {
                    a["package"] == json!(run.package)
                        && a["version"] == json!(run.version)
                        && a["targets"] == json!(run.targets)
                })?;
                if run.team.is_some() || run.cascade {
                    return Err("the consent is for another change");
                }
            } else if st.method == "__upgrade_published" {
                // A publisher's run over every instance of its package: no
                // team; each app is still judged at apply (`may_upgrade`).
                proofs.states(&st, &subject, "__upgrade_published", &|a| {
                    a["package"] == json!(run.package) && a["version"] == json!(run.version)
                })?;
                if run.team.is_some() || run.cascade {
                    return Err("the consent is for another change");
                }
            } else if st.method == "__upgrade_org_retry" {
                // A retry repeats a run of the same org under its terms.
                let prev = st.args()["run"]
                    .as_str()
                    .and_then(|p| s.orgs.get(org)?.upgrade_runs.get(p))
                    .ok_or("the consent is for another change")?;
                let terms = json!({
                    "package": prev.package, "version": prev.version,
                    "team": prev.team, "cascade": prev.cascade,
                });
                proofs.states(&st, &subject, "__upgrade_org_retry", &|_| same(&terms))?;
            } else {
                proofs.states(&st, &subject, "__upgrade_org", &same)?;
            }
            Ok(Some(used))
        }
        SystemCmd::TransferApp {
            app_id,
            to,
            actor,
            consent,
            ..
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            proofs.states(&st, app_id, "__transfer", &|a| a["to"] == json!(to))?;
            Ok(Some(used))
        }
        SystemCmd::MoveApp {
            app_id,
            to_org,
            actor,
            consent,
            bring_members,
            ..
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = move_app_args(to_org, *bring_members);
            proofs.states(&st, app_id, "__move", &|a| *a == args)?;
            Ok(Some(used))
        }
        SystemCmd::SetDelegation {
            account,
            source,
            target,
            methods: Some(methods),
            consent,
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(account, consent)?)?;
            let args = delegation_args(target, methods);
            proofs.states(&st, source, "__delegate", &|a| *a == args)?;
            Ok(Some(used))
        }
        // Taking a grant back narrows what an app may do as the person: no consent.
        SystemCmd::SetDelegation { methods: None, .. } => Ok(None),
        SystemCmd::CreateAppInvite {
            invite_key,
            invite,
            actor,
            consent,
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = super::app_invite_args(invite_key, invite);
            proofs.states(&st, &invite.app, "__create_app_invite", &|a| *a == args)?;
            Ok(Some(used))
        }
        SystemCmd::CreatePublicLink {
            link_key,
            link,
            actor,
            consent,
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = super::public_link_args(link_key, link);
            proofs.states(&st, &link.app, "__create_public_link", &|a| *a == args)?;
            Ok(Some(used))
        }
        SystemCmd::RevokePublicLink {
            app_id,
            link_key,
            actor,
            consent,
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = json!({ "linkKey": link_key });
            proofs.states(&st, app_id, "__revoke_public_link", &|a| *a == args)?;
            Ok(Some(used))
        }
        SystemCmd::RevokeAppInvite {
            app_id,
            invite_key,
            actor,
            consent,
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = json!({ "inviteKey": invite_key });
            proofs.states(&st, app_id, "__revoke_app_invite", &|a| *a == args)?;
            Ok(Some(used))
        }
        // The token's key signs the redemption: only a node the redeemer gave the token
        // to can make it.
        SystemCmd::RedeemAppInvite {
            invite_key,
            account,
            now,
            signature,
        } => {
            if !s.app_invites.contains_key(invite_key) {
                return Err(super::APP_INVITE_GONE);
            }
            let used = crate::proof::verify_invite_redeem(invite_key, account, *now, signature)?;
            if s.used_proofs.contains(&used) {
                return Err("that redemption was used already");
            }
            Ok(Some(used))
        }
        SystemCmd::LinkApp {
            app_id,
            name,
            target,
            actor,
            consent,
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = link_app_args(name, target.as_deref());
            proofs.states(&st, app_id, "__link", &|a| *a == args)?;
            Ok(Some(used))
        }
        SystemCmd::SetReplicas {
            app_id,
            replicas,
            proof,
        } => {
            // A move writes the replicas twice under one signature over its
            // target (`api::move_replicas`): first the union of the current
            // set and the target, so every node of both starts the group,
            // then the target. The target spends the proof; the union, as
            // computed here from the state it applies to, needs it unspent
            // and leaves it so. Replaying the union after the move is
            // refused (the proof is spent), and so is any other set.
            let (st, used) = proofs.fresh(proofs.operator(proof)?)?;
            let target: BTreeSet<u64> = serde_json::from_value(st.args()["replicas"].clone())
                .map_err(|_| "the consent is for another change")?;
            let args = json!({ "replicas": target });
            proofs.states(&st, app_id, "__set_replicas", &|a| *a == args)?;
            let wanted: BTreeSet<u64> = replicas.iter().copied().collect();
            if wanted.len() != replicas.len() {
                return Err("a node is named twice");
            }
            if wanted == target {
                return Ok(Some(used));
            }
            let current = s.apps.get(app_id).ok_or("no such app")?.replicas.iter();
            let union: BTreeSet<u64> = current.copied().chain(target).collect();
            if wanted == union {
                return Ok(None);
            }
            Err("the consent is for another change")
        }
        SystemCmd::RestoreApp {
            app_id,
            source,
            proof,
            ..
        } => {
            let (st, used) = proofs.fresh(proofs.operator(proof)?)?;
            let args = json!({ "backup": source.name, "sha256": source.sha256 });
            proofs.states(&st, app_id, "__restore_app", &|a| *a == args)?;
            Ok(Some(used))
        }
        _ => Ok(None),
    }
}

/// Apply a command of this area by its own rules, its proof already
/// judged ([`SystemMachine::run_authorized`]).
#[allow(clippy::too_many_lines, reason = "one arm per command")]
pub(super) fn apply(m: &mut SystemMachine, cmd: SystemCmd) -> Result<Value, &'static str> {
    let index = m.index;
    let s = &mut m.state;
    match cmd {
        SystemCmd::CreateApp {
            app,
            unit_offer,
            offers_only,
        } => {
            let mut app = *app;
            if !s.bundles.contains_key(&app.bundle) {
                return Err("unknown bundle");
            }
            if app.components.is_app() && s.is_compute_bundle(&app.bundle) {
                return Err(COMPUTE_NOT_AN_APP);
            }
            if !s.ships_compute(&app.bundle, app.compute_bundle.as_deref()) {
                return Err(COMPUTE_NOT_SHIPPED);
            }
            if app
                .release
                .as_ref()
                .is_some_and(|r| !s.release_ships(r, &app.bundle))
            {
                return Err(RELEASE_NOT_SHIPPED);
            }
            if s.components_of(app.release.as_ref()) != app.components {
                return Err(COMPONENTS_NOT_RELEASES);
            }
            let owned = s.apps.values().filter(|a| a.owner == app.owner).count();
            if owned >= s.quotas.max_apps_per_account as usize {
                return Err("quota exceeded: apps");
            }
            if s.apps.contains_key(&app.app_id) {
                return Err("app exists");
            }
            if let Some(id) = &app.deployment {
                let d = s.deployments.get(id).ok_or("no such deployment")?;
                let offered = |r: &PackageRef| {
                    d.apps
                        .values()
                        .any(|o| o.package == r.package && o.version == r.version)
                };
                let covered = match &unit_offer {
                    // A unit's member runs a release of its own: the deployment's offer of
                    // its instance's release covers it.
                    Some(offer) => app.release.is_some() && offered(offer),
                    None => {
                        s.offers_bundle(d, &app.bundle) && app.release.as_ref().is_none_or(offered)
                    }
                };
                if !covered {
                    return Err("the deployment does not offer the release this app would run");
                }
            } else if unit_offer.is_some() {
                return Err("a unit's offer comes with the deployment that makes it");
            } else if (offers_only || s.cluster_brings(cluster_version::Feature::OffersOnly))
                && !s.creates_outside_offers(&app.owner, app.release.as_ref(), app.created_at)
            {
                return Err(NOT_A_DEPLOYER);
            }
            {
                let o = s.orgs.get(&app.org);
                let member = o.and_then(|o| o.role(&app.owner));
                if !member.is_some_and(|r| r >= OrgRole::Member) {
                    return Err("owner is not a member of the organization");
                }
                // CREATE_APP: admins and the team's leads hold it,
                // a member only when granted (not by default).
                let team = app.access.the_team().map(String::as_str);
                if !o.is_some_and(|o| o.may(&app.owner, team, crate::types::caps::CREATE_APP)) {
                    return Err("creating apps here needs the CREATE_APP capability");
                }
                // In an organization whose apps have app teams, a package
                // with none is a new app there: an admin installs it.
                if let (Some(o), Some(package)) = (o, s.package_of(&app)) {
                    if o.mode() != crate::types::OrgMode::Legacy
                        && o.app_team(package).is_none()
                        && !o.role(&app.owner).is_some_and(|r| r >= OrgRole::Admin)
                    {
                        return Err(ONLY_ADMINS_INSTALL);
                    }
                }
            }
            s.check_grants(&app.org, app.access.grants())?;
            let o = s.orgs.get(&app.org);
            // A team's app is created by someone who writes the team (each team it admits).
            for team in app.access.teams() {
                if !o.is_some_and(|o| o.writes_team(&app.owner, team)) {
                    return Err("not a member of that team");
                }
            }
            s.next_group = s.next_group.max(1) + 1;
            app.group = s.next_group;
            app.restore = None;
            let out = json!({ "appId": app.app_id, "group": app.group, "replicas": app.replicas });
            let _prev = s.apps.insert(app.app_id.clone(), app);
            Ok(out)
        }
        SystemCmd::UpgradeApp {
            app_id,
            bundle,
            compute_bundle,
            by_operator,
            actor,
            at,
            run,
            consent,
            release,
            owner_in_org,
            owner_in_team,
            in_deployment,
            owner_approves,
            class,
            auto,
            members_stay,
            restore_point,
        } => {
            if !s.bundles.contains_key(&bundle) {
                return Err("unknown bundle");
            }
            let components = s.components_of(release.as_ref());
            if components.is_app() && s.is_compute_bundle(&bundle) {
                return Err(COMPUTE_NOT_AN_APP);
            }
            if !s.ships_compute(&bundle, compute_bundle.as_deref()) {
                return Err(COMPUTE_NOT_SHIPPED);
            }
            if release
                .as_ref()
                .is_some_and(|r| !s.release_ships(r, &bundle))
            {
                return Err(RELEASE_NOT_SHIPPED);
            }
            if s.apps
                .get(&app_id)
                .is_some_and(|a| a.components != components)
            {
                return Err(COMPONENTS_CHANGED);
            }
            let operator = s.governance().app_upgrades == Authority::Operator;
            let to = s.governing(release.as_ref(), &bundle, at);
            let keeps = s
                .apps
                .get(&app_id)
                .is_none_or(|a| s.upgrade_keeps_authority(s.governing_release(a), to));
            if !by_operator && !keeps {
                return Err(
                        "an app whose owner upgrades it is not moved to a release its publisher upgrades",
                    );
            }
            if by_operator != operator {
                return Err(if operator {
                    "this cluster's apps are upgraded by its operator"
                } else {
                    "this cluster's apps are upgraded by their owners"
                });
            }
            let owner_approves = s.owner_approves(owner_approves);
            // An update on its own is its app's policy's, judged from the log alone.
            if auto {
                let a = s.apps.get(&app_id).ok_or("no such app")?;
                s.auto_update_gate(
                    a,
                    release.as_ref(),
                    &bundle,
                    class.as_ref(),
                    at,
                    restore_point.as_ref(),
                    s.members_stay(members_stay),
                )?;
            }
            let allowed = if by_operator {
                s.apps.contains_key(&app_id)
            } else if auto {
                true
            } else {
                s.apps.get(&app_id).is_some_and(|a| {
                    s.may_upgrade_as_logged(
                        a,
                        &actor,
                        OwnerRule::logged(owner_in_org, owner_in_team),
                        owner_approves,
                    )
                })
            };
            if !allowed {
                return Err(if owner_approves {
                    NOT_AN_UPGRADE_APPROVER
                } else {
                    "upgrading this app needs its owner, or MANAGE_APPLICATION in its organization"
                });
            }
            // Only a deployment's managers decide which release its instances
            // run: an owner, an organization's or publisher's run and a unit's
            // upgrade alike move one only to the release it offers. Judged
            // from the flag, or a cluster version that brings the rule to
            // every node, so an earlier release's entry replays as it applied.
            if in_deployment || s.cluster_brings(cluster_version::Feature::UpgradeInDeployment) {
                if let Some(a) = s.apps.get(&app_id) {
                    s.within_deployment(a, release.as_ref(), run.as_deref())?;
                }
            }
            // An instance from no deployment enters another package only as its upgrader
            // could create one there; read from the logged version alone.
            if !by_operator && s.cluster_brings(cluster_version::Feature::OffersOnly) {
                if let Some(a) = s.apps.get(&app_id) {
                    if !s.upgrades_outside_offers(a, &actor, release.as_ref(), &bundle, at) {
                        return Err(NOT_A_DEPLOYER);
                    }
                }
            }
            // A run moves each app it names once, along its package and
            // never back: its consent covers that, not whatever an entry
            // naming it says.
            if let (Some(run_id), true) = (&run, s.holds_current_rules(index)) {
                let r = s
                    .orgs
                    .values()
                    .find_map(|o| o.upgrade_runs.get(run_id))
                    .ok_or("no such upgrade run")?;
                let a = s.apps.get(&app_id).ok_or("no such app")?;
                s.run_may_move(r, a)?;
            }
            // The class it carries is judged against the log (a red update is never
            // logged as green), and where the app's policy asks for a second approver, a
            // red or unclassified update needs another owner's or admin's first approval
            // (`SystemState::update_gate`); a unit's run is judged once, under its
            // instance's policy (`SystemState::run_update_gate`). An entry without a class
            // from before the policies replays as it applied: no policy existed yet.
            let approved_by = match s.apps.get(&app_id) {
                Some(a) if !by_operator => s.run_update_gate(
                    a,
                    release.as_ref(),
                    class.as_ref(),
                    run.as_deref(),
                    &actor,
                    at,
                )?,
                _ => None,
            };
            let by = if by_operator {
                OPERATOR.to_owned()
            } else {
                actor
            };
            let org = s.apps.get(&app_id).map(|a| a.org.clone());
            if let Some(a) = s.apps.get_mut(&app_id) {
                a.last_upgrade = Some(crate::types::UpgradeRecord {
                    from: a.bundle.clone(),
                    to: bundle.clone(),
                    at,
                    by: by.clone(),
                    ok: true,
                    error: Value::Null,
                    run: run.clone(),
                    api_key: crate::proof::key_proof_id(&consent)
                        .filter(|_| !by_operator)
                        .map(str::to_owned),
                    from_release: a.release.clone(),
                    to_release: release.clone(),
                    class: class.as_ref().map(|c| c.class),
                    reasons: class.map(|c| c.reasons).unwrap_or_default(),
                    approved_by,
                    auto,
                    undo: false,
                });
                // The restore point it took is the app's one undo, replacing any before;
                // without one, the app keeps none (an undo would skip this upgrade).
                a.restore_point = restore_point.map(|t| crate::types::RestorePoint {
                    from_bundle: a.bundle.clone(),
                    from_compute: a.compute_bundle.clone(),
                    from_release: a.release.clone(),
                    to_bundle: bundle.clone(),
                    to_release: release.clone(),
                    at,
                    by,
                    run: run.clone(),
                    file: t.file,
                    undone_at: None,
                });
                a.bundle = bundle;
                a.compute_bundle = compute_bundle;
                a.release = release;
            }
            if let Some(org) = org {
                s.drop_spent_approvals(&org, at);
            }
            s.note_run(run.as_deref(), &app_id, None);
            Ok(Value::Null)
        }
        SystemCmd::UpgradeFailed {
            app_id,
            bundle,
            by,
            at,
            error,
            run,
            release,
            from_release,
        } => {
            // An update on its own that lost to another move of the app (a pass on another
            // node after a failover, a person's upgrade) is not the release's failure: the
            // app runs the target already, or no longer runs what the update began from.
            // Such entries exist only from the version that brings `Feature::AutoUpdates`,
            // so every replica judges them by this rule.
            // The release it runs is judged as the candidates and the gate judge it
            // (`SystemState::running`: its record's, else the one its bundle is governed
            // by), so an app without `release` is judged alike. An entry without
            // `from_release` (logged before the field) is judged on its target alone.
            if by == super::AUTO_UPDATE {
                let a = s.apps.get(&app_id).ok_or("no such app")?;
                let runs = s.running(a);
                let done = a.bundle == bundle || (release.is_some() && runs == release.as_ref());
                let moved = from_release.as_ref().is_some_and(|f| runs != Some(f));
                if done || moved {
                    return Ok(json!({ "stale": true }));
                }
            }
            let a = s.apps.get_mut(&app_id).ok_or("no such app")?;
            a.last_upgrade = Some(crate::types::UpgradeRecord {
                from_release: a.release.clone(),
                to_release: release,
                from: a.bundle.clone(),
                to: bundle,
                at,
                by,
                ok: false,
                error: error.clone(),
                run: run.clone(),
                api_key: None,
                class: None,
                reasons: Vec::new(),
                approved_by: None,
                auto: false,
                undo: false,
            });
            s.note_run(run.as_deref(), &app_id, Some(error));
            Ok(Value::Null)
        }
        SystemCmd::StartUpgradeRun {
            org,
            mut run,
            consent,
            by_release,
            owner_approves,
        } => {
            let owner_approves = s.owner_approves(owner_approves);
            // Only an owner or admin of the organization starts a run there; a
            // publisher's run then covers the organizations the starter is one of.
            if owner_approves && run.by != OPERATOR && !s.approves_upgrades(&org, &run.by) {
                return Err(NOT_AN_UPGRADE_APPROVER);
            }
            // Member targets are a unit's, stated by its consent (`__upgrade_unit`, held to
            // them in `authorize`); any other run keeps none, whatever its entry lists, so
            // no app of it is moved off the run's version or judged as a unit's member
            // (`SystemState::run_update_gate`). No entry an honest node logged carries them
            // outside a unit's run, so every such entry replays as it applied.
            let unit_consent = serde_json::from_value::<crate::proof::Statement>(consent.clone())
                .is_ok_and(|st| st.method == "__upgrade_unit");
            if !unit_consent {
                run.targets.clear();
            }
            if s.holds_current_rules(index) {
                // What the run covers and how far it got are the state's,
                // not the proposer's: its consent names only the package,
                // version and scope. Its id names one run in the cluster,
                // as `UpgradeApp { run }` finds it in any organization.
                if !s.orgs.contains_key(&org) {
                    return Err("no such organization");
                }
                if s.orgs
                    .values()
                    .any(|o| o.upgrade_runs.contains_key(&run.run_id))
                {
                    return Err("that run exists");
                }
                run.apps = s.upgrade_run_apps(&org, &run, &consent, by_release, owner_approves);
                run.done.clear();
                run.failed.clear();
                run.finished = false;
            }
            let o = s.orgs.get_mut(&org).ok_or("no such organization")?;
            if o.upgrade_runs.contains_key(&run.run_id) {
                return Err("that run exists");
            }
            while o.upgrade_runs.len() >= crate::types::UPGRADE_RUNS_KEPT {
                let oldest = o
                    .upgrade_runs
                    .values()
                    .min_by_key(|r| (r.started_at, r.run_id.clone()))
                    .map(|r| r.run_id.clone());
                match oldest {
                    Some(id) => {
                        let _gone = o.upgrade_runs.remove(&id);
                    }
                    None => break,
                }
            }
            let out = serde_json::to_value(&*run).unwrap_or_default();
            let _prev = o.upgrade_runs.insert(run.run_id.clone(), *run);
            Ok(out)
        }
        SystemCmd::FinishUpgradeRun { org, run_id } => {
            let o = s.orgs.get_mut(&org).ok_or("no such organization")?;
            let r = o.upgrade_runs.get_mut(&run_id).ok_or("no such run")?;
            r.finished = true;
            Ok(serde_json::to_value(&*r).unwrap_or_default())
        }
        SystemCmd::SetAlias {
            account,
            alias,
            app,
        } => {
            if alias.is_empty() || alias.chars().count() > MAX_LABEL_CHARS {
                return Err("an alias is 1 to 64 characters");
            }
            let a = s.accounts.get_mut(&account).ok_or("unknown account")?;
            match app {
                Some(app) => {
                    if a.aliases.len() >= MAX_ALIASES && !a.aliases.contains_key(&alias) {
                        return Err("too many aliases");
                    }
                    let _prev = a.aliases.insert(alias, app);
                }
                None => {
                    let _gone = a.aliases.remove(&alias);
                }
            }
            Ok(Value::Null)
        }
        SystemCmd::TransferApp {
            app_id,
            to,
            actor,
            owner_in_org,
            owner_in_team,
            ..
        } => {
            if !s.accounts.contains_key(&to) {
                return Err("unknown account");
            }
            let org = s.apps.get(&app_id).ok_or("no such app")?.org.clone();
            // An org's app stays with a member of the org (as `CreateApp`),
            // except that, before organizations are ordinary
            // (`SystemState::ordinary_orgs`), one in a personal organization handed
            // to someone outside it goes to theirs, as a personal app went with its
            // new owner.
            let mut to_org = None;
            let o = s.orgs.get(&org);
            let role = o.and_then(|o| o.role(&to));
            if !role.is_some_and(|r| r >= OrgRole::Member) {
                match (
                    o.is_some_and(|o| s.acts_personal(o)),
                    s.personal_org_of(&to),
                ) {
                    (true, Some(theirs)) => to_org = Some(theirs.clone()),
                    (true, None) => return Err(NO_PERSONAL_ORG),
                    _ => return Err(super::NEW_OWNER_NOT_A_MEMBER),
                }
            }
            let app = s.apps.get(&app_id).ok_or("no such app")?;
            if !owner_in_org && app.owner != actor {
                return Err("only the owner may transfer");
            }
            if owner_in_org
                && !s.manages_app_as_logged(app, &actor, OwnerRule::logged(true, owner_in_team))
            {
                return Err(ONLY_MANAGERS_TRANSFER);
            }
            // An app going to its new owner's personal organization takes its
            // files to that organization's quota.
            let held = s.app_file_bytes.get(&app_id).copied().unwrap_or(0);
            if let Some(new_payer) = &to_org {
                if s.file_bytes(new_payer).saturating_add(held) > s.payer_quotas(new_payer).2 {
                    return Err("quota exceeded: file bytes");
                }
            }
            s.set_app_file_bytes(&app_id, 0);
            if let Some(app) = s.apps.get_mut(&app_id) {
                app.owner = to;
                if let Some(org) = to_org {
                    app.org = org;
                    // A team of the organization it left means nothing there.
                    app.access = app.access.teams_to_org();
                }
            }
            s.set_app_file_bytes(&app_id, held);
            let payer = s.apps.get(&app_id).map(crate::usage::payer);
            if let (Some(payer), Some(runs)) = (payer, s.compute.get_mut(&app_id)) {
                for slot in runs.values_mut() {
                    slot.payer.clone_from(&payer);
                }
                s.grant_runs();
            }
            Ok(Value::Null)
        }
        SystemCmd::MoveApp {
            app_id,
            to_org,
            actor,
            bring_members,
            nonce,
            all_apps_rows,
            accounts_in_app_team,
            org_quotas,
            ..
        } => s.move_app(
            &app_id,
            &to_org,
            bring_members,
            &nonce,
            &actor,
            MoveRows {
                all_apps_rows,
                accounts_in_app_team,
                org_quotas,
            },
        ),
        SystemCmd::DeleteApp {
            app_id,
            actor,
            owner_in_org,
            owner_in_team,
            members,
        } => {
            let rule = OwnerRule::logged(owner_in_org, owner_in_team);
            let may_delete = |s: &SystemState, a: &AppRecord| {
                s.owner_acts_as_logged(a, &actor, rule) || s.is_org_admin(&a.org, &actor)
            };
            let app = s.apps.get(&app_id).ok_or("no such app")?;
            if !may_delete(s, app) {
                return Err("only the owner or an org admin may delete");
            }
            // An entry from before the whole-unit rule deletes the app alone.
            let unit = match members {
                None => Vec::new(),
                Some(named) => {
                    if s.unit_parent(&app_id).is_some() {
                        return Err(UNIT_MEMBER);
                    }
                    let unit = s.unit_of(&app_id);
                    let named: BTreeSet<AppId> = named.into_iter().collect();
                    if named.len() != unit.len() || !unit.iter().all(|m| named.contains(m)) {
                        return Err(UNIT_NOT_AS_NAMED);
                    }
                    if unit
                        .iter()
                        .filter_map(|m| s.apps.get(m))
                        .any(|m| !may_delete(s, m))
                    {
                        return Err("deleting a unit takes whoever may delete each of its apps");
                    }
                    unit
                }
            };
            for m in &unit {
                s.remove_app(m);
            }
            s.remove_app(&app_id);
            Ok(Value::Null)
        }
        SystemCmd::SetReplicas {
            app_id, replicas, ..
        } => {
            let app = s.apps.get_mut(&app_id).ok_or("no such app")?;
            app.replicas = replicas;
            Ok(Value::Null)
        }
        SystemCmd::RestoreApp {
            app_id,
            replicas,
            bundle,
            compute_bundle,
            source,
            ..
        } => {
            if !s.bundles.contains_key(&bundle) {
                return Err(RESTORE_BUNDLE_GONE);
            }
            if !s.ships_compute(&bundle, compute_bundle.as_deref()) {
                return Err(COMPUTE_NOT_SHIPPED);
            }
            let distinct: BTreeSet<u64> = replicas.iter().copied().collect();
            let Some(&seed) = replicas.first() else {
                return Err("a restore names its replicas");
            };
            if distinct.len() != replicas.len()
                || replicas.iter().any(|id| s.replica_node(id).is_none())
            {
                return Err("a restore's replicas are distinct registered nodes");
            }
            if !s.apps.contains_key(&app_id) {
                return Err("no such app");
            }
            let group = s.next_group.max(1) + 1;
            s.next_group = group;
            let app = s.apps.get_mut(&app_id).ok_or("no such app")?;
            if app.bundle != bundle {
                // Inferred from the bundle again (`report::release_of_bundle`).
                app.release = None;
            }
            app.bundle = bundle;
            app.compute_bundle = compute_bundle;
            app.group = group;
            app.replicas = replicas;
            // The operator's restore replaces the state an undo would go back from.
            app.restore_point = None;
            app.restore = Some(crate::types::PendingRestore {
                seed: Some(seed),
                source: Some(source),
            });
            Ok(json!({ "group": group }))
        }
        SystemCmd::FinishRestore { app_id, group } => {
            let app = s.apps.get_mut(&app_id).ok_or("no such app")?;
            if app.group != group || app.restore.is_none() {
                return Err("no restore of that app into that group is under way");
            }
            // An undo's restore is done: its restore point is spent. Only an app an
            // `UndoUpgrade` reverted has one marked undone, so earlier logs replay alike.
            let source = app.restore.take().and_then(|r| r.source);
            let spent = app.restore_point.as_ref().is_some_and(|p| {
                p.undone_at.is_some() && p.file.as_ref().map(|f| &f.source) == source.as_ref()
            });
            if spent {
                app.restore_point = None;
            }
            Ok(Value::Null)
        }
        SystemCmd::SetAccess {
            app_id,
            access,
            actor,
            owner_in_org,
            owner_in_team,
        } => {
            let app = s.apps.get(&app_id).ok_or("no such app")?;
            if !s.owner_acts_as_logged(app, &actor, OwnerRule::logged(owner_in_org, owner_in_team))
                && !s.may_set_access(app, &access, &actor)
            {
                return Err("only the app's owner, an organization admin, or whoever manages its team's apps (MANAGE_APPLICATION) may change its access");
            }
            // The instance's public links stay as they are: made and revoked with their own
            // consent, never here.
            let access = app.access.keeping_links(access).ok_or(
                "a public link is made with its own consent, not by setting the instance's access",
            )?;
            s.check_grants(&app.org, access.without_links().grants())?;
            let app = s.apps.get_mut(&app_id).ok_or("no such app")?;
            app.access = access;
            Ok(Value::Null)
        }
        SystemCmd::SetDelegation {
            account,
            source,
            target,
            methods,
            ..
        } => {
            let Some(methods) = methods else {
                if let Some(sources) = s.delegations.get_mut(&account) {
                    if let Some(targets) = sources.get_mut(&source) {
                        let _was = targets.remove(&target);
                        if targets.is_empty() {
                            let _gone = sources.remove(&source);
                        }
                    }
                    if sources.is_empty() {
                        let _gone = s.delegations.remove(&account);
                    }
                }
                return Ok(Value::Null);
            };
            let from = s.apps.get(&source).ok_or("no such app")?;
            let to = s.apps.get(&target).ok_or("no such app to delegate to")?;
            if source == target {
                return Err("an app is not delegated to itself");
            }
            if !s.may_reach(from, Some(&account)) || !s.may_reach(to, Some(&account)) {
                return Err("only who reaches both apps lets one act on the other as them");
            }
            let distinct: BTreeSet<&String> = methods.iter().collect();
            if methods.is_empty()
                || methods.len() > calimero_bundle::MAX_USE_METHODS
                || distinct.len() != methods.len()
                || !methods.iter().all(|m| calimero_bundle::is_use_method(m))
            {
                return Err("not a list of methods to delegate");
            }
            let _was = s
                .delegations
                .entry(account)
                .or_default()
                .entry(source)
                .or_default()
                .insert(target, methods);
            Ok(Value::Null)
        }
        SystemCmd::CreateAppInvite {
            invite_key,
            invite,
            actor,
            ..
        } => {
            let app = s.apps.get(&invite.app).ok_or("no such app")?;
            if !s.manages_grants(app, &actor) {
                return Err("only whoever may change the instance's access invites its guests");
            }
            // Whether guests are allowed is judged when someone redeems it (a member always
            // may): an invite is made whatever the switches say now.
            if invite
                .scope
                .as_ref()
                .is_some_and(|v| !crate::types::scope_fits(v))
            {
                return Err("an invite's scope is at most 256 bytes of JSON");
            }
            if invite.uses_left == 0 || invite.uses_left > super::MAX_APP_INVITE_USES {
                return Err("an instance invite admits 1 to 1000 uses");
            }
            if invite.expires_at
                > invite
                    .created_at
                    .saturating_add(super::MAX_APP_INVITE_TTL_SECS)
            {
                return Err("an instance invite lasts at most 90 days");
            }
            if s.app_invites.contains_key(&invite_key) || s.invites.contains_key(&invite_key) {
                return Err("invite exists");
            }
            if s.app_invites
                .values()
                .filter(|i| i.app == invite.app)
                .count()
                >= super::MAX_APP_INVITES_PER_APP
                || s.app_invites.len() >= super::MAX_APP_INVITES
            {
                return Err("quota exceeded: instance invites");
            }
            let invite = crate::types::AppInvite {
                created_by: actor,
                ..invite
            };
            let _prev = s.app_invites.insert(invite_key, invite);
            Ok(Value::Null)
        }
        SystemCmd::CreatePublicLink {
            link_key,
            link,
            actor,
            ..
        } => {
            let app = s.apps.get(&link.app).ok_or("no such app")?;
            if !s.manages_grants(app, &actor) {
                return Err("only whoever may change the instance's access makes its public links");
            }
            if !s.orgs.get(&app.org).is_some_and(|o| o.allow_public_links) {
                return Err(super::PUBLIC_LINKS_OFF);
            }
            if link.rate_per_minute == 0 || link.rate_per_minute > super::MAX_PUBLIC_LINK_RATE {
                return Err("a public link admits 1 to 600 calls a minute per visitor");
            }
            if link.label.as_ref().is_some_and(|l| {
                l.chars().count() > super::MAX_PUBLIC_LINK_LABEL_CHARS
                    || l.chars().any(char::is_control)
            }) {
                return Err("a public link's label is one line of at most 64 characters");
            }
            if s.public_link(&link_key).is_some() {
                return Err("public link exists");
            }
            if app.access.links().len() >= super::MAX_PUBLIC_LINKS_PER_APP
                || s.public_link_count() >= super::MAX_PUBLIC_LINKS
            {
                return Err("quota exceeded: public links");
            }
            // One more grant of the instance.
            let app_id = link.app.clone();
            let grant = crate::types::LinkGrant::of(
                &link_key,
                crate::types::PublicLink {
                    created_by: actor,
                    ..link
                },
            );
            if let Some(app) = s.apps.get_mut(&app_id) {
                app.access = app.access.with_link(grant);
            }
            Ok(Value::Null)
        }
        SystemCmd::RevokePublicLink {
            app_id,
            link_key,
            actor,
            ..
        } => {
            let app = s.apps.get(&app_id).ok_or(super::PUBLIC_LINK_GONE)?;
            let link = app.access.link(&link_key).ok_or(super::PUBLIC_LINK_GONE)?;
            if link.created_by != actor && !s.manages_grants(app, &actor) {
                return Err("only the link's creator or whoever may change the instance's access revokes it");
            }
            let access = app.access.without_link(&link_key);
            if let Some(app) = s.apps.get_mut(&app_id) {
                app.access = access;
            }
            Ok(Value::Null)
        }
        SystemCmd::RevokeAppInvite {
            app_id,
            invite_key,
            actor,
            ..
        } => {
            let inv = s
                .app_invites
                .get(&invite_key)
                .filter(|i| i.app == app_id)
                .ok_or(super::APP_INVITE_GONE)?;
            let app = s.apps.get(&app_id).ok_or("no such app")?;
            if inv.created_by != actor && !s.manages_grants(app, &actor) {
                return Err("only the invite's creator or whoever manages the instance's guests may revoke it");
            }
            let _gone = s.app_invites.remove(&invite_key);
            Ok(Value::Null)
        }
        SystemCmd::RedeemAppInvite {
            invite_key,
            account,
            now,
            ..
        } => {
            if !s.accounts.contains_key(&account) {
                return Err("unknown account");
            }
            let inv = s
                .app_invites
                .get(&invite_key)
                .ok_or(super::APP_INVITE_GONE)?;
            if inv.expires_at <= now || inv.uses_left == 0 {
                return Err(INVITE_EXPIRED);
            }
            let app = s.apps.get(&inv.app).ok_or(super::APP_INVITE_GONE)?;
            // A member of its organization is named on it (a member grant); anyone else is a
            // guest, only while the instance takes guests.
            let member = s
                .orgs
                .get(&app.org)
                .and_then(|o| o.role(&account))
                .is_some();
            if !member && !s.guests_allowed(app) {
                return Err(super::GUESTS_NOT_ALLOWED);
            }
            let answer = json!({ "app": app.app_id, "org": app.org, "member": member });
            // Named already: nothing changes, and the use is left for someone else.
            if app.access.names(&account) {
                return Ok(answer);
            }
            if app.access.named_count() >= crate::types::MAX_ACCOUNT_GRANTS {
                return Err("quota exceeded: an instance names at most 1000 accounts");
            }
            let access = app.access.naming(&account, inv.scope.clone());
            let app_id = inv.app.clone();
            let inv = s
                .app_invites
                .get_mut(&invite_key)
                .ok_or(super::APP_INVITE_GONE)?;
            inv.uses_left -= 1;
            if inv.uses_left == 0 {
                let _gone = s.app_invites.remove(&invite_key);
            }
            if let Some(app) = s.apps.get_mut(&app_id) {
                app.access = access;
            }
            Ok(answer)
        }
        SystemCmd::LinkApp {
            app_id,
            name,
            target,
            actor,
            ..
        } => {
            let app = s.apps.get(&app_id).ok_or("no such app")?;
            if !s.manages_app(app, &actor) {
                return Err(ONLY_MANAGERS_LINK);
            }
            if !calimero_bundle::is_use_name(&name) {
                return Err("not a reference's name");
            }
            if app.links.contains_key(&name) {
                return Err("a unit member is linked at create and stays");
            }
            if let Some(t) = &target {
                let to = s.apps.get(t).ok_or("no such app to attach")?;
                if *t == app_id || to.org != app.org {
                    return Err("only another app of the same organization is attached");
                }
                if !s.manages_app(to, &actor) {
                    return Err(ONLY_MANAGERS_LINK);
                }
                if !app.attached.contains_key(&name) && app.attached.len() >= MAX_ATTACHED {
                    return Err("too many attached apps");
                }
            }
            let a = s.apps.get_mut(&app_id).ok_or("no such app")?;
            match target {
                Some(t) => {
                    let _was = a.attached.insert(name, t);
                }
                None => {
                    let _was = a.attached.remove(&name);
                }
            }
            Ok(Value::Null)
        }
        SystemCmd::SetAppMetadata {
            app_id,
            metadata,
            actor,
            owner_in_org,
            owner_in_team,
        } => {
            metadata_fits(&metadata)?;
            let app = s.apps.get(&app_id).ok_or("no such app")?;
            let admin = s.is_org_admin(&app.org, &actor);
            if !s.owner_acts_as_logged(app, &actor, OwnerRule::logged(owner_in_org, owner_in_team))
                && !admin
            {
                return Err("only the owner or an org admin sets an app's metadata");
            }
            if let Some(a) = s.apps.get_mut(&app_id) {
                a.metadata = metadata;
            }
            Ok(Value::Null)
        }
        // SAFETY: `SystemCmd::area` sends a command only to the module of
        // its area, and this match has an arm for each command of it.
        _ => unreachable!("a command of another area reached apps::apply"),
    }
}
