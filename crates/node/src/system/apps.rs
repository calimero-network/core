//! System commands for the app directory: instances, upgrades, transfers, moves, replicas, restores.
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

// This area's refusals. Each is `400 REJECTED`, the kind the bare message it
// names was answered with; the message is what a client reads.

/// An upgrade key's consent names a key the actor no longer holds.
const NO_SUCH_API_KEY: Rejection =
    Rejection::new(RejectionKind::Rejected, "no such API key: it was revoked");
/// An upgrade names a run no organization keeps.
const NO_SUCH_UPGRADE_RUN: Rejection =
    Rejection::new(RejectionKind::Rejected, "no such upgrade run");
/// A consent states another change than the command makes.
const CONSENT_FOR_ANOTHER_CHANGE: Rejection =
    Rejection::new(RejectionKind::Rejected, "the consent is for another change");
/// An instance invite's redemption was spent before.
const REDEMPTION_USED: Rejection =
    Rejection::new(RejectionKind::Rejected, "that redemption was used already");
/// A replica set names one node twice.
const NODE_NAMED_TWICE: Rejection =
    Rejection::new(RejectionKind::Rejected, "a node is named twice");
/// The command names an app the directory does not hold.
const NO_SUCH_APP: Rejection = Rejection::new(RejectionKind::Rejected, "no such app");
/// The command names a bundle the directory does not hold.
const UNKNOWN_BUNDLE: Rejection = Rejection::new(RejectionKind::Rejected, "unknown bundle");
/// An app of that id exists already.
const APP_EXISTS: Rejection = Rejection::new(RejectionKind::Rejected, "app exists");
/// An app names a deployment that does not exist.
const NO_SUCH_DEPLOYMENT: Rejection = Rejection::new(RejectionKind::Rejected, "no such deployment");
/// An app from a deployment would run a release the deployment does not offer.
const NOT_OFFERED: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "the deployment does not offer the release this app would run",
);
/// A unit member's offer is named without the deployment that makes it.
const UNIT_OFFER_WITHOUT_DEPLOYMENT: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "a unit's offer comes with the deployment that makes it",
);
/// An app's owner is not a member of its organization.
const OWNER_NOT_A_MEMBER: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "owner is not a member of the organization",
);
/// An app's owner lacks `CREATE_APP` where the app goes.
const NEEDS_CREATE_APP: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "creating apps here needs the CREATE_APP capability",
);
/// An app's owner does not write a team the app admits.
const NOT_IN_TEAM: Rejection = Rejection::new(RejectionKind::Rejected, "not a member of that team");
/// An owner's upgrade would move the app to a release its publisher upgrades.
const KEEPS_UPGRADE_AUTHORITY: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "an app whose owner upgrades it is not moved to a release its publisher upgrades",
);
/// A run's move to a release the app's owner may not use.
pub(super) const RELEASE_NOT_USABLE: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "an app is not moved to a release its owner may not use",
);
/// An owner's upgrade on a cluster whose operator upgrades apps.
const UPGRADED_BY_OPERATOR: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "this cluster's apps are upgraded by its operator",
);
/// The operator's upgrade on a cluster whose owners upgrade apps.
const UPGRADED_BY_OWNERS: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "this cluster's apps are upgraded by their owners",
);
/// An upgrade by someone the rule before `owner_approves` does not let upgrade.
const NOT_AN_UPGRADER: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "upgrading this app needs its owner, or MANAGE_APPLICATION in its organization",
);
/// The command names an organization that does not exist.
const NO_SUCH_ORG: Rejection = Rejection::new(RejectionKind::Rejected, "no such organization");
/// An upgrade run of that id exists already.
const RUN_EXISTS: Rejection = Rejection::new(RejectionKind::Rejected, "that run exists");
/// The organization keeps no run of that id.
const NO_SUCH_RUN: Rejection = Rejection::new(RejectionKind::Rejected, "no such run");
/// An alias outside 1 to [`MAX_LABEL_CHARS`] characters.
const BAD_ALIAS: Rejection =
    Rejection::new(RejectionKind::Rejected, "an alias is 1 to 64 characters");
/// The command names an account that does not exist.
const UNKNOWN_ACCOUNT: Rejection = Rejection::new(RejectionKind::Rejected, "unknown account");
/// A new alias past [`MAX_ALIASES`].
const TOO_MANY_ALIASES: Rejection = Rejection::new(RejectionKind::Rejected, "too many aliases");
/// A transfer, judged by ownership alone, by someone other than the owner.
const ONLY_OWNER_TRANSFERS: Rejection =
    Rejection::new(RejectionKind::Rejected, "only the owner may transfer");
/// A delete by someone neither the app's owner nor an admin of its organization.
const ONLY_OWNER_OR_ADMIN_DELETES: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "only the owner or an org admin may delete",
);
/// A unit's delete by someone who may not delete each of its apps.
const UNIT_DELETE_NEEDS_EACH: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "deleting a unit takes whoever may delete each of its apps",
);
/// A restore that names no replica.
const RESTORE_NAMES_REPLICAS: Rejection =
    Rejection::new(RejectionKind::Rejected, "a restore names its replicas");
/// A restore whose replicas repeat a node or name an unregistered one.
const RESTORE_REPLICAS_DISTINCT: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "a restore's replicas are distinct registered nodes",
);
/// A restore's end for an app and group that have none under way.
const NO_RESTORE_UNDER_WAY: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "no restore of that app into that group is under way",
);
/// An access change by someone who may not change the app's access.
const ONLY_MANAGERS_SET_ACCESS: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "only the app's owner, an organization admin, or whoever manages its team's apps (MANAGE_APPLICATION) may change its access",
);
/// An access change that would add or drop a public link.
const LINKS_KEEP_THEIR_CONSENT: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "a public link is made with its own consent, not by setting the instance's access",
);
/// A delegation to an app that does not exist.
const NO_DELEGATION_TARGET: Rejection =
    Rejection::new(RejectionKind::Rejected, "no such app to delegate to");
/// A delegation from an app to itself.
const SELF_DELEGATION: Rejection =
    Rejection::new(RejectionKind::Rejected, "an app is not delegated to itself");
/// A delegation by someone who does not reach both apps.
const DELEGATION_NEEDS_REACH: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "only who reaches both apps lets one act on the other as them",
);
/// A delegation whose methods are empty, too many, repeated or not methods.
const BAD_DELEGATED_METHODS: Rejection =
    Rejection::new(RejectionKind::Rejected, "not a list of methods to delegate");
/// An instance invite by someone who may not change the instance's access.
const ONLY_MANAGERS_INVITE: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "only whoever may change the instance's access invites its guests",
);
/// An instance invite whose scope does not fit.
const INVITE_SCOPE_TOO_LARGE: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "an invite's scope is at most 256 bytes of JSON",
);
/// An instance invite outside 1 to [`MAX_APP_INVITE_USES`] uses.
const BAD_INVITE_USES: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "an instance invite admits 1 to 1000 uses",
);
/// An instance invite that lasts past [`MAX_APP_INVITE_TTL_SECS`].
const INVITE_TOO_LONG: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "an instance invite lasts at most 90 days",
);
/// An invite of that key exists already.
const INVITE_EXISTS: Rejection = Rejection::new(RejectionKind::Rejected, "invite exists");
/// A public link made by someone who may not change the instance's access.
const ONLY_MANAGERS_MAKE_LINKS: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "only whoever may change the instance's access makes its public links",
);
/// A public link's rate outside 1 to [`MAX_PUBLIC_LINK_RATE`].
const BAD_LINK_RATE: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "a public link admits 1 to 600 calls a minute per visitor",
);
/// A public link's label that is too long or not one line.
const BAD_LINK_LABEL: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "a public link's label is one line of at most 64 characters",
);
/// A public link of that key exists already.
const LINK_EXISTS: Rejection = Rejection::new(RejectionKind::Rejected, "public link exists");
/// A public link revoked by someone neither its creator nor a manager of the access.
const ONLY_CREATOR_OR_MANAGERS_REVOKE_LINK: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "only the link's creator or whoever may change the instance's access revokes it",
);
/// An instance invite revoked by someone neither its creator nor a manager of the guests.
const ONLY_CREATOR_OR_MANAGERS_REVOKE_INVITE: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "only the invite's creator or whoever manages the instance's guests may revoke it",
);
/// An attachment under a name that is not a reference's.
const NOT_A_REFERENCE_NAME: Rejection =
    Rejection::new(RejectionKind::Rejected, "not a reference's name");
/// An attachment under the name of a unit member's link.
const UNIT_MEMBER_LINKED: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "a unit member is linked at create and stays",
);
/// An attachment of an app that does not exist.
const NO_ATTACH_TARGET: Rejection =
    Rejection::new(RejectionKind::Rejected, "no such app to attach");
/// An attachment of the app itself or of another organization's app.
const ATTACH_SAME_ORG: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "only another app of the same organization is attached",
);
/// A new attachment past [`MAX_ATTACHED`].
const TOO_MANY_ATTACHED: Rejection =
    Rejection::new(RejectionKind::Rejected, "too many attached apps");
/// A metadata change by someone neither the app's owner nor an admin of its organization.
const ONLY_OWNER_OR_ADMIN_SETS_METADATA: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "only the owner or an org admin sets an app's metadata",
);

/// Judge the proof a command of this area carries ([`super::authorize`]).
pub(super) fn authorize(proofs: &Proofs<'_>, cmd: &SystemCmd) -> Result<Option<String>, Rejection> {
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
        } => authorize_upgrade(proofs, app_id, bundle, *by_operator, actor, *at, consent),
        SystemCmd::UpgradeApp {
            app_id,
            bundle,
            by_operator,
            actor,
            run: Some(run),
            release: target,
            ..
        } => authorize_run_upgrade(
            proofs,
            app_id,
            bundle,
            *by_operator,
            actor,
            run,
            target.as_ref(),
        ),
        SystemCmd::StartUpgradeRun {
            org, run, consent, ..
        } => authorize_start_upgrade_run(proofs, org, run, consent),
        SystemCmd::TransferApp {
            app_id,
            to,
            actor,
            consent,
            ..
        } => {
            let (statement, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            proofs.states(&statement, app_id, "__transfer", &|a| a["to"] == json!(to))?;
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
            let (statement, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = move_app_args(to_org, *bring_members);
            proofs.states(&statement, app_id, "__move", &|a| *a == args)?;
            Ok(Some(used))
        }
        SystemCmd::SetDelegation {
            account,
            source,
            target,
            methods: Some(methods),
            consent,
        } => {
            let (statement, used) = proofs.fresh(proofs.consent_of(account, consent)?)?;
            let args = delegation_args(target, methods);
            proofs.states(&statement, source, "__delegate", &|a| *a == args)?;
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
            let (statement, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = super::app_invite_args(invite_key, invite);
            proofs.states(&statement, &invite.app, "__create_app_invite", &|a| {
                *a == args
            })?;
            Ok(Some(used))
        }
        SystemCmd::CreatePublicLink {
            link_key,
            link,
            actor,
            consent,
        } => {
            let (statement, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = super::public_link_args(link_key, link);
            proofs.states(&statement, &link.app, "__create_public_link", &|a| {
                *a == args
            })?;
            Ok(Some(used))
        }
        SystemCmd::RevokePublicLink {
            app_id,
            link_key,
            actor,
            consent,
        } => {
            let (statement, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = json!({ "linkKey": link_key });
            proofs.states(&statement, app_id, "__revoke_public_link", &|a| *a == args)?;
            Ok(Some(used))
        }
        SystemCmd::RevokeAppInvite {
            app_id,
            invite_key,
            actor,
            consent,
        } => {
            let (statement, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = json!({ "inviteKey": invite_key });
            proofs.states(&statement, app_id, "__revoke_app_invite", &|a| *a == args)?;
            Ok(Some(used))
        }
        SystemCmd::RedeemAppInvite {
            invite_key,
            account,
            now,
            signature,
        } => authorize_invite_redemption(proofs, invite_key, account, *now, signature),
        SystemCmd::LinkApp {
            app_id,
            name,
            target,
            actor,
            consent,
        } => {
            let (statement, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = link_app_args(name, target.as_deref());
            proofs.states(&statement, app_id, "__link", &|a| *a == args)?;
            Ok(Some(used))
        }
        SystemCmd::SetReplicas {
            app_id,
            replicas,
            proof,
        } => authorize_replicas(proofs, app_id, replicas, proof),
        SystemCmd::RestoreApp {
            app_id,
            source,
            proof,
            ..
        } => {
            let (statement, used) = proofs.fresh(proofs.operator(proof)?)?;
            let args = json!({ "backup": source.name, "sha256": source.sha256 });
            proofs.states(&statement, app_id, "__restore_app", &|a| *a == args)?;
            Ok(Some(used))
        }
        _ => Ok(None),
    }
}

/// An `UpgradeApp` outside a run: an upgrade key's signature, the operator's,
/// or the actor's consent, each over this bundle.
fn authorize_upgrade(
    proofs: &Proofs<'_>,
    app_id: &str,
    bundle: &str,
    by_operator: bool,
    actor: &str,
    at: u64,
    consent: &Value,
) -> Result<Option<String>, Rejection> {
    let state = proofs.s;
    // An upgrade key's signature, judged against the key as the log
    // holds it now: revoked or expired, it proves nothing.
    if let (false, Some(key_id)) = (by_operator, crate::proof::key_proof_id(consent)) {
        let key = state
            .api_keys
            .values()
            .find(|k| k.key_id == key_id && k.account == actor)
            .ok_or(NO_SUCH_API_KEY)?;
        let scope = state.key_may_upgrade(key, app_id, bundle, at)?;
        let (_, statement, used) = crate::proof::verify_key_upgrade(consent, &scope.proof_key)?;
        let (statement, used) = proofs.fresh((statement, used))?;
        proofs.states(&statement, app_id, "__upgrade", &|a| {
            a["bundle"] == json!(bundle)
        })?;
        return Ok(Some(used));
    }
    let (statement, used) = if by_operator {
        proofs.fresh(proofs.operator(consent)?)?
    } else {
        proofs.fresh(proofs.consent_of(actor, consent)?)?
    };
    proofs.states(&statement, app_id, "__upgrade", &|a| {
        a["bundle"] == json!(bundle)
    })?;
    Ok(Some(used))
}

/// An `UpgradeApp` of a run: one the run's consent named, to the release it names.
fn authorize_run_upgrade(
    proofs: &Proofs<'_>,
    app_id: &str,
    bundle: &str,
    by_operator: bool,
    actor: &str,
    run: &str,
    target: Option<&PackageRef>,
) -> Result<Option<String>, Rejection> {
    // One consent covers the run (`StartUpgradeRun`); each app's
    // upgrade must be one the run named, to the release it names.
    // The run is kept by the organization that started it: the app's
    // own, or the publisher's for a publisher's run over every
    // instance of its package (as `note_run` finds it).
    let state = proofs.s;
    let upgrade_run = state
        .orgs
        .values()
        .find_map(|o| o.upgrade_runs.get(run))
        .ok_or(NO_SUCH_UPGRADE_RUN)?;
    let by = if by_operator { OPERATOR } else { actor };
    let (package, version) = upgrade_run.target_of(app_id);
    let release = state
        .packages
        .get(package)
        .and_then(|p| p.versions.get(version));
    if upgrade_run.by != by
        || upgrade_run.finished
        || !upgrade_run.apps.iter().any(|id| id == app_id)
        || !release.is_some_and(|v| v.app_parts().values().any(|h| h == bundle))
        || target.is_some_and(|t| t.package != package || t.version != version)
    {
        return Err(NOT_IN_RUN.into());
    }
    Ok(None)
}

/// A `StartUpgradeRun`: the starter's (or the operator's) consent over the
/// run's terms, as the kind of run it is states them.
fn authorize_start_upgrade_run(
    proofs: &Proofs<'_>,
    org: &str,
    run: &crate::types::UpgradeRun,
    consent: &Value,
) -> Result<Option<String>, Rejection> {
    let state = proofs.s;
    let (statement, used) = if run.by == OPERATOR {
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
    if statement.method == "__upgrade_unit" {
        // A unit's run: the instance, its version and each member's target, as
        // the consent states them; which members it covers is the state's.
        proofs.states(&statement, &subject, "__upgrade_unit", &|a| {
            a["package"] == json!(run.package)
                && a["version"] == json!(run.version)
                && a["targets"] == json!(run.targets)
        })?;
        if run.team.is_some() || run.cascade {
            return Err(CONSENT_FOR_ANOTHER_CHANGE);
        }
    } else if statement.method == "__upgrade_published" {
        // A publisher's run over every instance of its package: no
        // team; each app is still judged at apply (`may_upgrade`).
        proofs.states(&statement, &subject, "__upgrade_published", &|a| {
            a["package"] == json!(run.package) && a["version"] == json!(run.version)
        })?;
        if run.team.is_some() || run.cascade {
            return Err(CONSENT_FOR_ANOTHER_CHANGE);
        }
    } else if statement.method == "__upgrade_org_retry" {
        // A retry repeats a run of the same org under its terms.
        let previous = statement.args()["run"]
            .as_str()
            .and_then(|p| state.orgs.get(org)?.upgrade_runs.get(p))
            .ok_or(CONSENT_FOR_ANOTHER_CHANGE)?;
        let terms = json!({
            "package": previous.package, "version": previous.version,
            "team": previous.team, "cascade": previous.cascade,
        });
        proofs.states(&statement, &subject, "__upgrade_org_retry", &|_| {
            same(&terms)
        })?;
    } else {
        proofs.states(&statement, &subject, "__upgrade_org", &same)?;
    }
    Ok(Some(used))
}

/// A `RedeemAppInvite`: the token's key signs the redemption, so only a node
/// the redeemer gave the token to can make it, and only once.
fn authorize_invite_redemption(
    proofs: &Proofs<'_>,
    invite_key: &str,
    account: &str,
    now: u64,
    signature: &str,
) -> Result<Option<String>, Rejection> {
    let state = proofs.s;
    if !state.app_invites.contains_key(invite_key) {
        return Err(super::APP_INVITE_GONE.into());
    }
    let used = crate::proof::verify_invite_redeem(invite_key, account, now, signature)?;
    if state.used_proofs.contains(&used) {
        return Err(REDEMPTION_USED);
    }
    Ok(Some(used))
}

/// A `SetReplicas`: the operator's signature over the move's target, which
/// the target spends and the union before it needs unspent.
fn authorize_replicas(
    proofs: &Proofs<'_>,
    app_id: &str,
    replicas: &[u64],
    proof: &Value,
) -> Result<Option<String>, Rejection> {
    // A move writes the replicas twice under one signature over its
    // target (`api::move_replicas`): first the union of the current
    // set and the target, so every node of both starts the group,
    // then the target. The target spends the proof; the union, as
    // computed here from the state it applies to, needs it unspent
    // and leaves it so. Replaying the union after the move is
    // refused (the proof is spent), and so is any other set.
    let state = proofs.s;
    let (statement, used) = proofs.fresh(proofs.operator(proof)?)?;
    let target: BTreeSet<u64> = serde_json::from_value(statement.args()["replicas"].clone())
        .map_err(|_| CONSENT_FOR_ANOTHER_CHANGE)?;
    let args = json!({ "replicas": target });
    proofs.states(&statement, app_id, "__set_replicas", &|a| *a == args)?;
    let wanted: BTreeSet<u64> = replicas.iter().copied().collect();
    if wanted.len() != replicas.len() {
        return Err(NODE_NAMED_TWICE);
    }
    if wanted == target {
        return Ok(Some(used));
    }
    let current = state.apps.get(app_id).ok_or(NO_SUCH_APP)?.replicas.iter();
    let union: BTreeSet<u64> = current.copied().chain(target).collect();
    if wanted == union {
        return Ok(None);
    }
    Err(CONSENT_FOR_ANOTHER_CHANGE)
}

/// Apply a command of this area by its own rules, its proof already
/// judged ([`SystemMachine::run_authorized`]).
///
/// A command with more fields than a function takes arguments (`UpgradeApp`,
/// `UpgradeFailed`) goes whole to its function, which takes its fields by name.
pub(super) fn apply(m: &mut SystemMachine, cmd: SystemCmd) -> Result<Value, Rejection> {
    let index = m.index;
    let state = &mut m.state;
    match cmd {
        SystemCmd::CreateApp {
            app,
            unit_offer,
            offers_only,
        } => create_app(state, *app, unit_offer, offers_only),
        cmd @ SystemCmd::UpgradeApp { .. } => upgrade_app(state, index, cmd),
        cmd @ SystemCmd::UpgradeFailed { .. } => upgrade_failed(state, cmd),
        SystemCmd::StartUpgradeRun {
            org,
            run,
            consent,
            by_release,
            owner_approves,
        } => start_upgrade_run(
            state,
            index,
            org,
            *run,
            &consent,
            by_release,
            owner_approves,
        ),
        SystemCmd::FinishUpgradeRun { org, run_id } => {
            let organization = state.orgs.get_mut(&org).ok_or(NO_SUCH_ORG)?;
            let upgrade_run = organization
                .upgrade_runs
                .get_mut(&run_id)
                .ok_or(NO_SUCH_RUN)?;
            upgrade_run.finished = true;
            Ok(serde_json::to_value(&*upgrade_run).unwrap_or_default())
        }
        SystemCmd::SetAlias {
            account,
            alias,
            app,
        } => set_alias(state, account, alias, app),
        SystemCmd::TransferApp {
            app_id,
            to,
            actor,
            owner_in_org,
            owner_in_team,
            ..
        } => transfer_app(state, app_id, to, actor, owner_in_org, owner_in_team),
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
        } => state.move_app(
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
        } => delete_app(state, app_id, actor, owner_in_org, owner_in_team, members),
        SystemCmd::SetReplicas {
            app_id, replicas, ..
        } => {
            let app = state.apps.get_mut(&app_id).ok_or(NO_SUCH_APP)?;
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
        } => restore_app(state, app_id, replicas, bundle, compute_bundle, source),
        SystemCmd::FinishRestore { app_id, group } => finish_restore(state, app_id, group),
        SystemCmd::SetAccess {
            app_id,
            access,
            actor,
            owner_in_org,
            owner_in_team,
        } => set_access(state, app_id, access, actor, owner_in_org, owner_in_team),
        SystemCmd::SetDelegation {
            account,
            source,
            target,
            methods,
            ..
        } => set_delegation(state, account, source, target, methods),
        SystemCmd::CreateAppInvite {
            invite_key,
            invite,
            actor,
            ..
        } => create_app_invite(state, invite_key, invite, actor),
        SystemCmd::CreatePublicLink {
            link_key,
            link,
            actor,
            ..
        } => create_public_link(state, link_key, link, actor),
        SystemCmd::RevokePublicLink {
            app_id,
            link_key,
            actor,
            ..
        } => revoke_public_link(state, app_id, link_key, actor),
        SystemCmd::RevokeAppInvite {
            app_id,
            invite_key,
            actor,
            ..
        } => revoke_app_invite(state, app_id, invite_key, actor),
        SystemCmd::RedeemAppInvite {
            invite_key,
            account,
            now,
            ..
        } => redeem_app_invite(state, invite_key, account, now),
        SystemCmd::LinkApp {
            app_id,
            name,
            target,
            actor,
            ..
        } => link_app(state, app_id, name, target, actor),
        SystemCmd::SetAppMetadata {
            app_id,
            metadata,
            actor,
            owner_in_org,
            owner_in_team,
        } => set_app_metadata(state, app_id, metadata, actor, owner_in_org, owner_in_team),
        // SAFETY: `SystemCmd::area` sends a command only to the module of
        // its area, and this match has an arm for each command of it.
        _ => unreachable!("a command of another area reached apps::apply"),
    }
}

/// Create an app: the rules it must meet, then its record in a group of its own.
fn create_app(
    state: &mut SystemState,
    mut app: AppRecord,
    unit_offer: Option<PackageRef>,
    offers_only: bool,
) -> Result<Value, Rejection> {
    check_bundle(
        state,
        &app.bundle,
        app.compute_bundle.as_deref(),
        app.components,
    )?;
    check_release_ships(state, app.release.as_ref(), &app.bundle)?;
    check_components_are_releases(state, &app)?;
    check_quota(state, &app)?;
    check_new_app_id(state, &app)?;
    check_deployment_offer(state, &app, unit_offer.as_ref())?;
    check_outside_offers(state, &app, unit_offer.as_ref(), offers_only)?;
    check_org_membership(state, &app)?;
    check_create_capability(state, &app)?;
    check_new_package_installer(state, &app)?;
    state.check_grants(&app.org, app.access.grants())?;
    check_team_writes(state, &app)?;
    state.next_group = state.next_group.max(1) + 1;
    app.group = state.next_group;
    app.restore = None;
    let out = json!({ "appId": app.app_id, "group": app.group, "replicas": app.replicas });
    let _prev = state.apps.insert(app.app_id.clone(), app);
    Ok(out)
}

/// The bundle is known, is not a compute component where an app component is
/// due, and ships the compute component named beside it.
fn check_bundle(
    state: &SystemState,
    bundle: &str,
    compute_bundle: Option<&str>,
    components: Components,
) -> Result<(), Rejection> {
    if !state.bundles.contains_key(bundle) {
        return Err(UNKNOWN_BUNDLE);
    }
    if components.is_app() && state.is_compute_bundle(bundle) {
        return Err(COMPUTE_NOT_AN_APP.into());
    }
    if !state.ships_compute(bundle, compute_bundle) {
        return Err(COMPUTE_NOT_SHIPPED.into());
    }
    Ok(())
}

/// A release named for the app ships its bundle.
fn check_release_ships(
    state: &SystemState,
    release: Option<&PackageRef>,
    bundle: &str,
) -> Result<(), Rejection> {
    if release.is_some_and(|r| !state.release_ships(r, bundle)) {
        return Err(RELEASE_NOT_SHIPPED.into());
    }
    Ok(())
}

/// The app has the components its release has.
fn check_components_are_releases(state: &SystemState, app: &AppRecord) -> Result<(), Rejection> {
    if state.components_of(app.release.as_ref()) != app.components {
        return Err(COMPONENTS_NOT_RELEASES.into());
    }
    Ok(())
}

/// The owner holds fewer apps than the per-account quota allows.
fn check_quota(state: &SystemState, app: &AppRecord) -> Result<(), Rejection> {
    let owned = state.apps.values().filter(|a| a.owner == app.owner).count();
    if owned >= state.quotas.max_apps_per_account as usize {
        return Err(Rejection::quota("apps"));
    }
    Ok(())
}

/// No app has the new app's id.
fn check_new_app_id(state: &SystemState, app: &AppRecord) -> Result<(), Rejection> {
    if state.apps.contains_key(&app.app_id) {
        return Err(APP_EXISTS);
    }
    Ok(())
}

/// An app from a deployment runs a release the deployment offers (a unit's
/// member, one the offer of its instance's release covers).
fn check_deployment_offer(
    state: &SystemState,
    app: &AppRecord,
    unit_offer: Option<&PackageRef>,
) -> Result<(), Rejection> {
    let Some(id) = &app.deployment else {
        return Ok(());
    };
    let deployment = state.deployments.get(id).ok_or(NO_SUCH_DEPLOYMENT)?;
    let offered = |r: &PackageRef| {
        deployment
            .apps
            .values()
            .any(|o| o.package == r.package && o.version == r.version)
    };
    let covered = match unit_offer {
        // A unit's member runs a release of its own: the deployment's offer of
        // its instance's release covers it.
        Some(offer) => app.release.is_some() && offered(offer),
        None => {
            state.offers_bundle(deployment, &app.bundle) && app.release.as_ref().is_none_or(offered)
        }
    };
    if !covered {
        return Err(NOT_OFFERED);
    }
    Ok(())
}

/// An app from no deployment names no unit offer, and where only offers
/// create, its owner may create outside them.
fn check_outside_offers(
    state: &SystemState,
    app: &AppRecord,
    unit_offer: Option<&PackageRef>,
    offers_only: bool,
) -> Result<(), Rejection> {
    if app.deployment.is_some() {
        return Ok(());
    }
    if unit_offer.is_some() {
        return Err(UNIT_OFFER_WITHOUT_DEPLOYMENT);
    }
    if (offers_only || state.cluster_brings(cluster_version::Feature::OffersOnly))
        && !state.creates_outside_offers(&app.owner, app.release.as_ref(), app.created_at)
    {
        return Err(NOT_A_DEPLOYER);
    }
    Ok(())
}

/// The owner is a member (or above) of the app's organization.
fn check_org_membership(state: &SystemState, app: &AppRecord) -> Result<(), Rejection> {
    let role = state
        .orgs
        .get(&app.org)
        .and_then(|org| org.role(&app.owner));
    if !role.is_some_and(|r| r >= OrgRole::Member) {
        return Err(OWNER_NOT_A_MEMBER);
    }
    Ok(())
}

/// The owner holds `CREATE_APP` where the app goes: admins and the team's
/// leads do, a member only when granted (not by default).
fn check_create_capability(state: &SystemState, app: &AppRecord) -> Result<(), Rejection> {
    let team = app.access.the_team().map(String::as_str);
    let org = state.orgs.get(&app.org);
    if !org.is_some_and(|org| org.may(&app.owner, team, crate::types::caps::CREATE_APP)) {
        return Err(NEEDS_CREATE_APP);
    }
    Ok(())
}

/// In an organization whose apps have app teams, a package with none is a
/// new app there: only its owner or an admin installs it.
fn check_new_package_installer(state: &SystemState, app: &AppRecord) -> Result<(), Rejection> {
    if let (Some(org), Some(package)) = (state.orgs.get(&app.org), state.package_of(app)) {
        if org.mode() != crate::types::OrgMode::Legacy
            && org.app_team(package).is_none()
            && !org.role(&app.owner).is_some_and(|r| r >= OrgRole::Admin)
        {
            return Err(ONLY_ADMINS_INSTALL.into());
        }
    }
    Ok(())
}

/// A team's app is created by someone who writes each team it admits.
fn check_team_writes(state: &SystemState, app: &AppRecord) -> Result<(), Rejection> {
    let org = state.orgs.get(&app.org);
    for team in app.access.teams() {
        if !org.is_some_and(|org| org.writes_team(&app.owner, team)) {
            return Err(NOT_IN_TEAM);
        }
    }
    Ok(())
}

/// Upgrade an app: the bundle and authority rules, the app's policy for an
/// update on its own, who may upgrade it, then its new release and record.
fn upgrade_app(state: &mut SystemState, index: u64, cmd: SystemCmd) -> Result<Value, Rejection> {
    let SystemCmd::UpgradeApp {
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
    } = cmd
    else {
        // SAFETY: `apply` sends only an `UpgradeApp` here.
        unreachable!("upgrade_app applies an UpgradeApp")
    };
    let components = state.components_of(release.as_ref());
    check_bundle(state, &bundle, compute_bundle.as_deref(), components)?;
    check_release_ships(state, release.as_ref(), &bundle)?;
    if state
        .apps
        .get(&app_id)
        .is_some_and(|app| app.components != components)
    {
        return Err(COMPONENTS_CHANGED.into());
    }
    check_upgrade_authority(state, &app_id, release.as_ref(), &bundle, at, by_operator)?;
    let owner_approves = state.owner_approves(owner_approves);
    // An update on its own is its app's policy's, judged from the log alone.
    if auto {
        let app = state.apps.get(&app_id).ok_or(NO_SUCH_APP)?;
        state.auto_update_gate(
            app,
            release.as_ref(),
            &bundle,
            class.as_ref(),
            at,
            restore_point.as_ref(),
            state.members_stay(members_stay),
        )?;
    }
    let allowed = if by_operator {
        state.apps.contains_key(&app_id)
    } else if auto {
        true
    } else {
        state.apps.get(&app_id).is_some_and(|app| {
            state.may_upgrade_as_logged(
                app,
                &actor,
                OwnerRule::logged(owner_in_org, owner_in_team),
                owner_approves,
            )
        })
    };
    if !allowed {
        return Err(if owner_approves {
            NOT_AN_UPGRADE_APPROVER.into()
        } else {
            NOT_AN_UPGRADER
        });
    }
    // Only a deployment's managers decide which release its instances
    // run: an owner, an organization's or publisher's run and a unit's
    // upgrade alike move one only to the release it offers. Judged
    // from the flag, or a cluster version that brings the rule to
    // every node, so an earlier release's entry replays as it applied.
    if in_deployment || state.cluster_brings(cluster_version::Feature::UpgradeInDeployment) {
        if let Some(app) = state.apps.get(&app_id) {
            state.within_deployment(app, release.as_ref(), run.as_deref())?;
        }
    }
    // An instance from no deployment enters another package only as its upgrader
    // could create one there; read from the logged version alone.
    if !by_operator && state.cluster_brings(cluster_version::Feature::OffersOnly) {
        if let Some(app) = state.apps.get(&app_id) {
            if !state.upgrades_outside_offers(app, &actor, release.as_ref(), &bundle, at) {
                return Err(NOT_A_DEPLOYER);
            }
        }
    }
    // A run moves each app it names once, along its package and
    // never back: its consent covers that, not whatever an entry
    // naming it says.
    if let (Some(run_id), true) = (&run, state.holds_current_rules(index)) {
        let upgrade_run = state
            .orgs
            .values()
            .find_map(|o| o.upgrade_runs.get(run_id))
            .ok_or(NO_SUCH_UPGRADE_RUN)?;
        let app = state.apps.get(&app_id).ok_or(NO_SUCH_APP)?;
        state.run_may_move(upgrade_run, app)?;
    }
    // A run moves an app only to a release its owner may use; entries logged below the
    // version that brings private releases replay as they applied.
    if run.is_some() && state.cluster_brings(cluster_version::Feature::PrivateReleases) {
        if let (Some(app), Some(to)) = (state.apps.get(&app_id), release.as_ref()) {
            let usable = state.release_offered_to(app, to, run.as_deref()).is_some()
                || state
                    .release_used(&to.package, &to.version, &app.owner)
                    .is_some()
                || (by_operator
                    && state
                        .release_used(&to.package, &to.version, OPERATOR)
                        .is_some());
            if !usable {
                return Err(RELEASE_NOT_USABLE);
            }
        }
    }
    // The class it carries is judged against the log (a red update is never
    // logged as green), and where the app's policy asks for a second approver, a
    // red or unclassified update needs another owner's or admin's first approval
    // (`SystemState::update_gate`); a unit's run is judged once, under its
    // instance's policy (`SystemState::run_update_gate`). An entry without a class
    // from before the policies replays as it applied: no policy existed yet.
    let approved_by = match state.apps.get(&app_id) {
        Some(app) if !by_operator => state.run_update_gate(
            app,
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
    let org = state.apps.get(&app_id).map(|app| app.org.clone());
    if let Some(app) = state.apps.get_mut(&app_id) {
        app.last_upgrade = Some(crate::types::UpgradeRecord {
            from: app.bundle.clone(),
            to: bundle.clone(),
            at,
            by: by.clone(),
            ok: true,
            error: Value::Null,
            run: run.clone(),
            api_key: crate::proof::key_proof_id(&consent)
                .filter(|_| !by_operator)
                .map(str::to_owned),
            from_release: app.release.clone(),
            to_release: release.clone(),
            class: class.as_ref().map(|c| c.class),
            reasons: class.map(|c| c.reasons).unwrap_or_default(),
            approved_by,
            auto,
            undo: false,
        });
        // The restore point it took is the app's one undo, replacing any before;
        // without one, the app keeps none (an undo would skip this upgrade).
        app.restore_point = restore_point.map(|taken| crate::types::RestorePoint {
            from_bundle: app.bundle.clone(),
            from_compute: app.compute_bundle.clone(),
            from_release: app.release.clone(),
            to_bundle: bundle.clone(),
            to_release: release.clone(),
            at,
            by,
            run: run.clone(),
            file: taken.file,
            undone_at: None,
        });
        app.bundle = bundle;
        app.compute_bundle = compute_bundle;
        app.release = release;
    }
    if let Some(org) = org {
        state.drop_spent_approvals(&org, at);
    }
    state.note_run(run.as_deref(), &app_id, None);
    Ok(Value::Null)
}

/// An owner's upgrade keeps an app its owner upgrades off a release its
/// publisher upgrades, and only the party governance names upgrades apps.
fn check_upgrade_authority(
    state: &SystemState,
    app_id: &str,
    release: Option<&PackageRef>,
    bundle: &str,
    at: u64,
    by_operator: bool,
) -> Result<(), Rejection> {
    let operator = state.governance().app_upgrades == Authority::Operator;
    let to = state.governing(release, bundle, at);
    let keeps = state
        .apps
        .get(app_id)
        .is_none_or(|app| state.upgrade_keeps_authority(state.governing_release(app), to));
    if !by_operator && !keeps {
        return Err(KEEPS_UPGRADE_AUTHORITY);
    }
    if by_operator != operator {
        return Err(if operator {
            UPGRADED_BY_OPERATOR
        } else {
            UPGRADED_BY_OWNERS
        });
    }
    Ok(())
}

/// Record an upgrade that failed, unless it was an update on its own that
/// another move of the app made stale.
fn upgrade_failed(state: &mut SystemState, cmd: SystemCmd) -> Result<Value, Rejection> {
    let SystemCmd::UpgradeFailed {
        app_id,
        bundle,
        by,
        at,
        error,
        run,
        release,
        from_release,
    } = cmd
    else {
        // SAFETY: `apply` sends only an `UpgradeFailed` here.
        unreachable!("upgrade_failed applies an UpgradeFailed")
    };
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
        let app = state.apps.get(&app_id).ok_or(NO_SUCH_APP)?;
        let runs = state.running(app);
        let done = app.bundle == bundle || (release.is_some() && runs == release.as_ref());
        let moved = from_release.as_ref().is_some_and(|f| runs != Some(f));
        if done || moved {
            return Ok(json!({ "stale": true }));
        }
    }
    let app = state.apps.get_mut(&app_id).ok_or(NO_SUCH_APP)?;
    app.last_upgrade = Some(crate::types::UpgradeRecord {
        from_release: app.release.clone(),
        to_release: release,
        from: app.bundle.clone(),
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
    state.note_run(run.as_deref(), &app_id, Some(error));
    Ok(Value::Null)
}

/// Start an organization's upgrade run: its starter's authority, then the
/// run as the state derives it, kept with the organization's latest runs.
fn start_upgrade_run(
    state: &mut SystemState,
    index: u64,
    org: OrgId,
    mut run: crate::types::UpgradeRun,
    consent: &Value,
    by_release: bool,
    owner_approves: bool,
) -> Result<Value, Rejection> {
    let owner_approves = state.owner_approves(owner_approves);
    // Only an owner or admin of the organization starts a run there; a
    // publisher's run then covers the organizations the starter is one of.
    if owner_approves && run.by != OPERATOR && !state.approves_upgrades(&org, &run.by) {
        return Err(NOT_AN_UPGRADE_APPROVER.into());
    }
    // Member targets are a unit's, stated by its consent (`__upgrade_unit`, held to
    // them in `authorize`); any other run keeps none, whatever its entry lists, so
    // no app of it is moved off the run's version or judged as a unit's member
    // (`SystemState::run_update_gate`). No entry an honest node logged carries them
    // outside a unit's run, so every such entry replays as it applied.
    let unit_consent = serde_json::from_value::<crate::proof::Statement>(consent.clone())
        .is_ok_and(|statement| statement.method == "__upgrade_unit");
    if !unit_consent {
        run.targets.clear();
    }
    if state.holds_current_rules(index) {
        // What the run covers and how far it got are the state's,
        // not the proposer's: its consent names only the package,
        // version and scope. Its id names one run in the cluster,
        // as `UpgradeApp { run }` finds it in any organization.
        if !state.orgs.contains_key(&org) {
            return Err(NO_SUCH_ORG);
        }
        if state
            .orgs
            .values()
            .any(|o| o.upgrade_runs.contains_key(&run.run_id))
        {
            return Err(RUN_EXISTS);
        }
        run.apps = state.upgrade_run_apps(&org, &run, consent, by_release, owner_approves);
        run.done.clear();
        run.failed.clear();
        run.finished = false;
    }
    let organization = state.orgs.get_mut(&org).ok_or(NO_SUCH_ORG)?;
    if organization.upgrade_runs.contains_key(&run.run_id) {
        return Err(RUN_EXISTS);
    }
    while organization.upgrade_runs.len() >= crate::types::UPGRADE_RUNS_KEPT {
        let oldest = organization
            .upgrade_runs
            .values()
            .min_by_key(|r| (r.started_at, r.run_id.clone()))
            .map(|r| r.run_id.clone());
        match oldest {
            Some(id) => {
                let _gone = organization.upgrade_runs.remove(&id);
            }
            None => break,
        }
    }
    let out = serde_json::to_value(&run).unwrap_or_default();
    let _prev = organization.upgrade_runs.insert(run.run_id.clone(), run);
    Ok(out)
}

/// Set or clear one of an account's aliases for an app.
fn set_alias(
    state: &mut SystemState,
    account: AccountId,
    alias: String,
    app: Option<AppId>,
) -> Result<Value, Rejection> {
    if alias.is_empty() || alias.chars().count() > MAX_LABEL_CHARS {
        return Err(BAD_ALIAS);
    }
    let record = state.accounts.get_mut(&account).ok_or(UNKNOWN_ACCOUNT)?;
    match app {
        Some(app) => {
            if record.aliases.len() >= MAX_ALIASES && !record.aliases.contains_key(&alias) {
                return Err(TOO_MANY_ALIASES);
            }
            let _prev = record.aliases.insert(alias, app);
        }
        None => {
            let _gone = record.aliases.remove(&alias);
        }
    }
    Ok(Value::Null)
}

/// Transfer an app to a new owner, who keeps it in its organization (or, before
/// organizations are ordinary, takes it into their personal one).
fn transfer_app(
    state: &mut SystemState,
    app_id: AppId,
    to: AccountId,
    actor: AccountId,
    owner_in_org: bool,
    owner_in_team: bool,
) -> Result<Value, Rejection> {
    if !state.accounts.contains_key(&to) {
        return Err(UNKNOWN_ACCOUNT);
    }
    let org = state.apps.get(&app_id).ok_or(NO_SUCH_APP)?.org.clone();
    // An org's app stays with a member of the org (as `CreateApp`),
    // except that, before organizations are ordinary
    // (`SystemState::ordinary_orgs`), one in a personal organization handed
    // to someone outside it goes to theirs, as a personal app went with its
    // new owner.
    let mut to_org = None;
    let organization = state.orgs.get(&org);
    let role = organization.and_then(|o| o.role(&to));
    if !role.is_some_and(|r| r >= OrgRole::Member) {
        match (
            organization.is_some_and(|o| state.acts_personal(o)),
            state.personal_org_of(&to),
        ) {
            (true, Some(theirs)) => to_org = Some(theirs.clone()),
            (true, None) => return Err(NO_PERSONAL_ORG.into()),
            _ => return Err(super::NEW_OWNER_NOT_A_MEMBER.into()),
        }
    }
    let app = state.apps.get(&app_id).ok_or(NO_SUCH_APP)?;
    if !owner_in_org && app.owner != actor {
        return Err(ONLY_OWNER_TRANSFERS);
    }
    if owner_in_org
        && !state.manages_app_as_logged(app, &actor, OwnerRule::logged(true, owner_in_team))
    {
        return Err(ONLY_MANAGERS_TRANSFER.into());
    }
    // An app going to its new owner's personal organization takes its
    // files to that organization's quota.
    let held = state.app_file_bytes.get(&app_id).copied().unwrap_or(0);
    if let Some(new_payer) = &to_org {
        if state.file_bytes(new_payer).saturating_add(held) > state.payer_quotas(new_payer).2 {
            return Err(Rejection::quota("file bytes"));
        }
    }
    state.set_app_file_bytes(&app_id, 0);
    if let Some(app) = state.apps.get_mut(&app_id) {
        app.owner = to;
        if let Some(org) = to_org {
            app.org = org;
            // A team of the organization it left means nothing there.
            app.access = app.access.teams_to_org();
        }
    }
    state.set_app_file_bytes(&app_id, held);
    let payer = state.apps.get(&app_id).map(crate::usage::payer);
    if let (Some(payer), Some(runs)) = (payer, state.compute.get_mut(&app_id)) {
        for slot in runs.values_mut() {
            slot.payer.clone_from(&payer);
        }
        state.grant_runs();
    }
    Ok(Value::Null)
}

/// Delete an app, or an instance with its whole unit, by whoever may delete each.
fn delete_app(
    state: &mut SystemState,
    app_id: AppId,
    actor: AccountId,
    owner_in_org: bool,
    owner_in_team: bool,
    members: Option<Vec<AppId>>,
) -> Result<Value, Rejection> {
    let rule = OwnerRule::logged(owner_in_org, owner_in_team);
    let may_delete = |state: &SystemState, app: &AppRecord| {
        state.owner_acts_as_logged(app, &actor, rule) || state.is_org_admin(&app.org, &actor)
    };
    let app = state.apps.get(&app_id).ok_or(NO_SUCH_APP)?;
    if !may_delete(state, app) {
        return Err(ONLY_OWNER_OR_ADMIN_DELETES);
    }
    // An entry from before the whole-unit rule deletes the app alone.
    let unit = match members {
        None => Vec::new(),
        Some(named) => {
            if state.unit_parent(&app_id).is_some() {
                return Err(UNIT_MEMBER);
            }
            let unit = state.unit_of(&app_id);
            let named: BTreeSet<AppId> = named.into_iter().collect();
            if named.len() != unit.len() || !unit.iter().all(|m| named.contains(m)) {
                return Err(UNIT_NOT_AS_NAMED.into());
            }
            if unit
                .iter()
                .filter_map(|m| state.apps.get(m))
                .any(|m| !may_delete(state, m))
            {
                return Err(UNIT_DELETE_NEEDS_EACH);
            }
            unit
        }
    };
    for member in &unit {
        state.remove_app(member);
    }
    state.remove_app(&app_id);
    Ok(Value::Null)
}

/// Restore an app from a backup into a new group on the replicas named.
fn restore_app(
    state: &mut SystemState,
    app_id: AppId,
    replicas: Vec<u64>,
    bundle: String,
    compute_bundle: Option<String>,
    source: crate::types::RestoreSource,
) -> Result<Value, Rejection> {
    if !state.bundles.contains_key(&bundle) {
        return Err(RESTORE_BUNDLE_GONE.into());
    }
    if !state.ships_compute(&bundle, compute_bundle.as_deref()) {
        return Err(COMPUTE_NOT_SHIPPED.into());
    }
    let distinct: BTreeSet<u64> = replicas.iter().copied().collect();
    let Some(&seed) = replicas.first() else {
        return Err(RESTORE_NAMES_REPLICAS);
    };
    if distinct.len() != replicas.len()
        || replicas.iter().any(|id| state.replica_node(id).is_none())
    {
        return Err(RESTORE_REPLICAS_DISTINCT);
    }
    if !state.apps.contains_key(&app_id) {
        return Err(NO_SUCH_APP);
    }
    let group = state.next_group.max(1) + 1;
    state.next_group = group;
    let app = state.apps.get_mut(&app_id).ok_or(NO_SUCH_APP)?;
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

/// End an app's restore into `group`, spending an undo's restore point.
fn finish_restore(state: &mut SystemState, app_id: AppId, group: u64) -> Result<Value, Rejection> {
    let app = state.apps.get_mut(&app_id).ok_or(NO_SUCH_APP)?;
    if app.group != group || app.restore.is_none() {
        return Err(NO_RESTORE_UNDER_WAY);
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

/// Set an app's access grants, keeping its public links as they are.
fn set_access(
    state: &mut SystemState,
    app_id: AppId,
    access: Access,
    actor: AccountId,
    owner_in_org: bool,
    owner_in_team: bool,
) -> Result<Value, Rejection> {
    let app = state.apps.get(&app_id).ok_or(NO_SUCH_APP)?;
    if !state.owner_acts_as_logged(app, &actor, OwnerRule::logged(owner_in_org, owner_in_team))
        && !state.may_set_access(app, &access, &actor)
    {
        return Err(ONLY_MANAGERS_SET_ACCESS);
    }
    // The instance's public links stay as they are: made and revoked with their own
    // consent, never here.
    let access = app
        .access
        .keeping_links(access)
        .ok_or(LINKS_KEEP_THEIR_CONSENT)?;
    state.check_grants(&app.org, access.without_links().grants())?;
    let app = state.apps.get_mut(&app_id).ok_or(NO_SUCH_APP)?;
    app.access = access;
    Ok(Value::Null)
}

/// Let `source` call `target` as `account` for `methods`, or take that back.
fn set_delegation(
    state: &mut SystemState,
    account: AccountId,
    source: AppId,
    target: AppId,
    methods: Option<Vec<String>>,
) -> Result<Value, Rejection> {
    let Some(methods) = methods else {
        if let Some(sources) = state.delegations.get_mut(&account) {
            if let Some(targets) = sources.get_mut(&source) {
                let _was = targets.remove(&target);
                if targets.is_empty() {
                    let _gone = sources.remove(&source);
                }
            }
            if sources.is_empty() {
                let _gone = state.delegations.remove(&account);
            }
        }
        return Ok(Value::Null);
    };
    let from = state.apps.get(&source).ok_or(NO_SUCH_APP)?;
    let to = state.apps.get(&target).ok_or(NO_DELEGATION_TARGET)?;
    if source == target {
        return Err(SELF_DELEGATION);
    }
    if !state.may_reach(from, Some(&account)) || !state.may_reach(to, Some(&account)) {
        return Err(DELEGATION_NEEDS_REACH);
    }
    let distinct: BTreeSet<&String> = methods.iter().collect();
    if methods.is_empty()
        || methods.len() > calimero_bundle::MAX_USE_METHODS
        || distinct.len() != methods.len()
        || !methods.iter().all(|m| calimero_bundle::is_use_method(m))
    {
        return Err(BAD_DELEGATED_METHODS);
    }
    let _was = state
        .delegations
        .entry(account)
        .or_default()
        .entry(source)
        .or_default()
        .insert(target, methods);
    Ok(Value::Null)
}

/// Make an invite to an instance, within its bounds.
fn create_app_invite(
    state: &mut SystemState,
    invite_key: String,
    invite: crate::types::AppInvite,
    actor: AccountId,
) -> Result<Value, Rejection> {
    let app = state.apps.get(&invite.app).ok_or(NO_SUCH_APP)?;
    if !state.manages_grants(app, &actor) {
        return Err(ONLY_MANAGERS_INVITE);
    }
    // Whether guests are allowed is judged when someone redeems it (a member always
    // may): an invite is made whatever the switches say now.
    if invite
        .scope
        .as_ref()
        .is_some_and(|v| !crate::types::scope_fits(v))
    {
        return Err(INVITE_SCOPE_TOO_LARGE);
    }
    if invite.uses_left == 0 || invite.uses_left > super::MAX_APP_INVITE_USES {
        return Err(BAD_INVITE_USES);
    }
    if invite.expires_at
        > invite
            .created_at
            .saturating_add(super::MAX_APP_INVITE_TTL_SECS)
    {
        return Err(INVITE_TOO_LONG);
    }
    if state.app_invites.contains_key(&invite_key) || state.invites.contains_key(&invite_key) {
        return Err(INVITE_EXISTS);
    }
    if state
        .app_invites
        .values()
        .filter(|i| i.app == invite.app)
        .count()
        >= super::MAX_APP_INVITES_PER_APP
        || state.app_invites.len() >= super::MAX_APP_INVITES
    {
        return Err(Rejection::quota("instance invites"));
    }
    let invite = crate::types::AppInvite {
        created_by: actor,
        ..invite
    };
    let _prev = state.app_invites.insert(invite_key, invite);
    Ok(Value::Null)
}

/// Make a public link to an instance: one more grant of it, within its bounds.
fn create_public_link(
    state: &mut SystemState,
    link_key: String,
    link: crate::types::PublicLink,
    actor: AccountId,
) -> Result<Value, Rejection> {
    let app = state.apps.get(&link.app).ok_or(NO_SUCH_APP)?;
    if !state.manages_grants(app, &actor) {
        return Err(ONLY_MANAGERS_MAKE_LINKS);
    }
    if !state
        .orgs
        .get(&app.org)
        .is_some_and(|o| o.allow_public_links)
    {
        return Err(super::PUBLIC_LINKS_OFF.into());
    }
    if link.rate_per_minute == 0 || link.rate_per_minute > super::MAX_PUBLIC_LINK_RATE {
        return Err(BAD_LINK_RATE);
    }
    if link.label.as_ref().is_some_and(|l| {
        l.chars().count() > super::MAX_PUBLIC_LINK_LABEL_CHARS || l.chars().any(char::is_control)
    }) {
        return Err(BAD_LINK_LABEL);
    }
    if state.public_link(&link_key).is_some() {
        return Err(LINK_EXISTS);
    }
    if app.access.links().len() >= super::MAX_PUBLIC_LINKS_PER_APP
        || state.public_link_count() >= super::MAX_PUBLIC_LINKS
    {
        return Err(Rejection::quota("public links"));
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
    if let Some(app) = state.apps.get_mut(&app_id) {
        app.access = app.access.with_link(grant);
    }
    Ok(Value::Null)
}

/// Revoke a public link, by its creator or whoever may change the access.
fn revoke_public_link(
    state: &mut SystemState,
    app_id: AppId,
    link_key: String,
    actor: AccountId,
) -> Result<Value, Rejection> {
    let app = state.apps.get(&app_id).ok_or(super::PUBLIC_LINK_GONE)?;
    let link = app.access.link(&link_key).ok_or(super::PUBLIC_LINK_GONE)?;
    if link.created_by != actor && !state.manages_grants(app, &actor) {
        return Err(ONLY_CREATOR_OR_MANAGERS_REVOKE_LINK);
    }
    let access = app.access.without_link(&link_key);
    if let Some(app) = state.apps.get_mut(&app_id) {
        app.access = access;
    }
    Ok(Value::Null)
}

/// Revoke an instance invite, by its creator or whoever manages the guests.
fn revoke_app_invite(
    state: &mut SystemState,
    app_id: AppId,
    invite_key: String,
    actor: AccountId,
) -> Result<Value, Rejection> {
    let invite = state
        .app_invites
        .get(&invite_key)
        .filter(|i| i.app == app_id)
        .ok_or(super::APP_INVITE_GONE)?;
    let app = state.apps.get(&app_id).ok_or(NO_SUCH_APP)?;
    if invite.created_by != actor && !state.manages_grants(app, &actor) {
        return Err(ONLY_CREATOR_OR_MANAGERS_REVOKE_INVITE);
    }
    let _gone = state.app_invites.remove(&invite_key);
    Ok(Value::Null)
}

/// Redeem an instance invite: name the account on the instance, as a member
/// or, while the instance takes them, a guest, spending one use.
fn redeem_app_invite(
    state: &mut SystemState,
    invite_key: String,
    account: AccountId,
    now: u64,
) -> Result<Value, Rejection> {
    if !state.accounts.contains_key(&account) {
        return Err(UNKNOWN_ACCOUNT);
    }
    let invite = state
        .app_invites
        .get(&invite_key)
        .ok_or(super::APP_INVITE_GONE)?;
    if invite.expires_at <= now || invite.uses_left == 0 {
        return Err(INVITE_EXPIRED.into());
    }
    let app = state.apps.get(&invite.app).ok_or(super::APP_INVITE_GONE)?;
    // From the logged version that brings it, an instance invite gives only what its creator
    // could still give, as an organization invite: one whose creator no longer manages the
    // instance's grants (removed, demoted, out of its team, or unknown) names nobody, so a
    // manager who lost the instance cannot hand it out through invites made before.
    if state.cluster_brings(super::cluster_version::Feature::InviteCreatorStanding)
        && !state.manages_grants(app, &invite.created_by)
    {
        return Err(super::INVITE_CREATOR_LACKS);
    }
    // A member of its organization is named on it (a member grant); anyone else is a
    // guest, only while the instance takes guests.
    let member = state
        .orgs
        .get(&app.org)
        .and_then(|o| o.role(&account))
        .is_some();
    if !member && !state.guests_allowed(app) {
        return Err(super::GUESTS_NOT_ALLOWED);
    }
    let answer = json!(super::InviteRedeemed {
        app: Some(app.app_id.clone()),
        org: app.org.clone(),
        member: Some(member),
    });
    // Named already: nothing changes, and the use is left for someone else.
    if app.access.names(&account) {
        return Ok(answer);
    }
    if app.access.named_count() >= crate::types::MAX_ACCOUNT_GRANTS {
        return Err(Rejection::quota("an instance names at most 1000 accounts"));
    }
    let access = app.access.naming(&account, invite.scope.clone());
    let app_id = invite.app.clone();
    let invite = state
        .app_invites
        .get_mut(&invite_key)
        .ok_or(super::APP_INVITE_GONE)?;
    invite.uses_left -= 1;
    if invite.uses_left == 0 {
        let _gone = state.app_invites.remove(&invite_key);
    }
    if let Some(app) = state.apps.get_mut(&app_id) {
        app.access = access;
    }
    Ok(answer)
}

/// Attach another app of the same organization under a reference's name, or detach it.
fn link_app(
    state: &mut SystemState,
    app_id: AppId,
    name: String,
    target: Option<AppId>,
    actor: AccountId,
) -> Result<Value, Rejection> {
    let app = state.apps.get(&app_id).ok_or(NO_SUCH_APP)?;
    if !state.manages_app(app, &actor) {
        return Err(ONLY_MANAGERS_LINK.into());
    }
    if !calimero_bundle::is_use_name(&name) {
        return Err(NOT_A_REFERENCE_NAME);
    }
    if app.links.contains_key(&name) {
        return Err(UNIT_MEMBER_LINKED);
    }
    if let Some(target_id) = &target {
        let attached = state.apps.get(target_id).ok_or(NO_ATTACH_TARGET)?;
        if *target_id == app_id || attached.org != app.org {
            return Err(ATTACH_SAME_ORG);
        }
        if !state.manages_app(attached, &actor) {
            return Err(ONLY_MANAGERS_LINK.into());
        }
        if !app.attached.contains_key(&name) && app.attached.len() >= MAX_ATTACHED {
            return Err(TOO_MANY_ATTACHED);
        }
    }
    let app = state.apps.get_mut(&app_id).ok_or(NO_SUCH_APP)?;
    match target {
        Some(target_id) => {
            let _was = app.attached.insert(name, target_id);
        }
        None => {
            let _was = app.attached.remove(&name);
        }
    }
    Ok(Value::Null)
}

/// Set an app's metadata, by its owner or an admin of its organization.
fn set_app_metadata(
    state: &mut SystemState,
    app_id: AppId,
    metadata: Value,
    actor: AccountId,
    owner_in_org: bool,
    owner_in_team: bool,
) -> Result<Value, Rejection> {
    metadata_fits(&metadata)?;
    let app = state.apps.get(&app_id).ok_or(NO_SUCH_APP)?;
    let admin = state.is_org_admin(&app.org, &actor);
    if !state.owner_acts_as_logged(app, &actor, OwnerRule::logged(owner_in_org, owner_in_team))
        && !admin
    {
        return Err(ONLY_OWNER_OR_ADMIN_SETS_METADATA);
    }
    if let Some(app) = state.apps.get_mut(&app_id) {
        app.metadata = metadata;
    }
    Ok(Value::Null)
}
