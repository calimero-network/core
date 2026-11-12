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
            let release = s
                .packages
                .get(&r.package)
                .and_then(|p| p.versions.get(&r.version));
            if r.by != by
                || r.finished
                || !r.apps.contains(app_id)
                || !release.is_some_and(|v| v.app_parts().values().any(|h| h == bundle))
                || target
                    .as_ref()
                    .is_some_and(|t| t.package != r.package || t.version != r.version)
            {
                return Err(NOT_IN_RUN);
            }
            Ok(None)
        }
        SystemCmd::StartUpgradeRun { org, run, consent } => {
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
            if st.method == "__upgrade_published" {
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
        SystemCmd::CreateApp { app } => {
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
                let offers_release = app.release.as_ref().is_none_or(|r| {
                    d.apps
                        .values()
                        .any(|o| o.package == r.package && o.version == r.version)
                });
                if !s.offers_bundle(d, &app.bundle) || !offers_release {
                    return Err("the deployment does not offer the release this app would run");
                }
            }
            if let Some(org) = &app.org {
                let o = s.orgs.get(org);
                let member = o.and_then(|o| o.role(&app.owner));
                if !member.is_some_and(|r| r >= OrgRole::Member) {
                    return Err("owner is not a member of the organization");
                }
                // CREATE_APP: admins and the team's leads hold it,
                // a member only when granted (not by default).
                let team = match &app.access {
                    Access::Team { team } => Some(team.as_str()),
                    _ => None,
                };
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
            if let Access::Team { team } = &app.access {
                let o = app.org.as_ref().and_then(|o| s.orgs.get(o));
                // A team's app is created by someone who writes the team.
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
            let from = s.apps.get(&app_id).map(|a| a.bundle.clone());
            if !by_operator && from.is_some_and(|f| !s.upgrade_keeps_authority(&f, &bundle)) {
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
            let allowed = if by_operator {
                s.apps.contains_key(&app_id)
            } else {
                s.apps.get(&app_id).is_some_and(|a| {
                    s.may_upgrade_as_logged(
                        a,
                        &actor,
                        OwnerRule::logged(owner_in_org, owner_in_team),
                    )
                })
            };
            if !allowed {
                return Err(
                    "upgrading this app needs its owner, or MANAGE_APPLICATION in its organization",
                );
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
            let by = if by_operator {
                OPERATOR.to_owned()
            } else {
                actor
            };
            if let Some(a) = s.apps.get_mut(&app_id) {
                a.last_upgrade = Some(crate::types::UpgradeRecord {
                    from: a.bundle.clone(),
                    to: bundle.clone(),
                    at,
                    by,
                    ok: true,
                    error: Value::Null,
                    run: run.clone(),
                    api_key: crate::proof::key_proof_id(&consent)
                        .filter(|_| !by_operator)
                        .map(str::to_owned),
                    from_release: a.release.clone(),
                    to_release: release.clone(),
                });
                a.bundle = bundle;
                a.compute_bundle = compute_bundle;
                a.release = release;
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
        } => {
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
            });
            s.note_run(run.as_deref(), &app_id, Some(error));
            Ok(Value::Null)
        }
        SystemCmd::StartUpgradeRun {
            org,
            mut run,
            consent,
        } => {
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
                run.apps = s.upgrade_run_apps(&org, &run, &consent);
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
            // An org's app stays with a member of the org (as `CreateApp`).
            if let Some(org) = org {
                let role = s.orgs.get(&org).and_then(|o| o.role(&to));
                if !role.is_some_and(|r| r >= OrgRole::Member) {
                    return Err("the new owner is not a member of the app's organization");
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
            // A personal app's files move to the new owner's quota.
            let held = s.app_file_bytes.get(&app_id).copied().unwrap_or(0);
            let new_payer = format!("account:{to}");
            if app.org.is_none()
                && s.file_bytes(&new_payer).saturating_add(held) > s.payer_quotas(&new_payer).2
            {
                return Err("quota exceeded: file bytes");
            }
            s.set_app_file_bytes(&app_id, 0);
            if let Some(app) = s.apps.get_mut(&app_id) {
                app.owner = to;
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
            },
        ),
        SystemCmd::DeleteApp {
            app_id,
            actor,
            owner_in_org,
            owner_in_team,
        } => {
            let app = s.apps.get(&app_id).ok_or("no such app")?;
            let admin = app.org.as_ref().is_some_and(|o| s.is_org_admin(o, &actor));
            if !s.owner_acts_as_logged(app, &actor, OwnerRule::logged(owner_in_org, owner_in_team))
                && !admin
            {
                return Err("only the owner or an org admin may delete");
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
            app.restore = None;
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
            if let Access::Team { team } = &access {
                let in_org = app
                    .org
                    .as_ref()
                    .and_then(|o| s.orgs.get(o))
                    .is_some_and(|o| o.teams.contains_key(team));
                if !in_org {
                    return Err("no such team in the app's organization");
                }
            }
            let app = s.apps.get_mut(&app_id).ok_or("no such app")?;
            app.access = access;
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
            let admin = app.org.as_ref().is_some_and(|o| s.is_org_admin(o, &actor));
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
