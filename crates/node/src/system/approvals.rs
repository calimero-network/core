//! System commands for release approvals, release deadlines, outbound approvals and the
//! egress switch.
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

/// The operator's release approval on a cluster whose owners approve releases.
const RELEASES_BY_OWNERS: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "this cluster's releases are approved by each app's owner",
);
/// An owner's release approval on a cluster whose operator approves releases.
const RELEASES_BY_OPERATOR: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "this cluster's releases are approved by its operator",
);
/// Withdrawing the only approved release, which would leave the apps unrestricted.
const LAST_RELEASE: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "the last approved release cannot be withdrawn",
);
/// A deadline for something other than a semver release of at most
/// [`MAX_RELEASE_VERSION_BYTES`].
const BAD_DEADLINE_VERSION: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "a deadline names a semver release of at most 64 bytes",
);
/// A new deadline past [`MAX_RELEASE_DEADLINES`].
const TOO_MANY_DEADLINES: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "too many release deadlines; lift an old one first",
);
/// A release approval of the cluster's scope, which is `SetOperatorRelease`'s.
const OPERATOR_USES_ITS_OWN: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "the operator uses SetOperatorRelease",
);
/// A release approval by someone without `APPROVE_RELEASES` in the organization.
const NEEDS_APPROVE_RELEASES: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "approving releases needs APPROVE_RELEASES in the organization",
);
/// An owner's outbound approval on a cluster whose operator approves outbound hosts.
const OUTBOUND_BY_OPERATOR: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "this cluster's outbound hosts are approved by its operator",
);
/// The operator's outbound approval on a cluster whose owners approve outbound hosts.
const OUTBOUND_BY_OWNERS: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "this cluster's outbound hosts are approved by each app's owner",
);
/// An outbound approval by someone without org-wide `MANAGE_APPLICATION`.
const NEEDS_MANAGE_APPLICATION: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "approving outbound hosts needs MANAGE_APPLICATION in the organization",
);
/// Approved prefixes that are not https URL prefixes, or too many.
const BAD_PREFIXES: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "an approved prefix is not an https URL prefix, or too many",
);
/// The egress switch turned by someone neither the organization's owner nor an admin.
const ONLY_ADMINS_SWITCH_EGRESS: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "only the organization's owner or an admin turns its egress off or on",
);
/// An instance's egress switch for an app that is not in the organization.
const NO_SUCH_APP_IN_ORG: Rejection =
    Rejection::new(RejectionKind::Rejected, "no such app in the organization");

/// Judge the proof a command of this area carries ([`super::authorize`]).
pub(super) fn authorize(proofs: &Proofs<'_>, cmd: &SystemCmd) -> Result<Option<String>, Rejection> {
    match cmd {
        SystemCmd::SetReleaseApproval {
            scope,
            version,
            approved,
            actor,
            consent,
        } => {
            let proven = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            release_stated(
                proofs,
                proven,
                &scope_subject(scope, actor),
                version,
                *approved,
            )
        }
        SystemCmd::SetOperatorRelease {
            version,
            approved,
            proof,
        } => {
            let proven = proofs.fresh(proofs.operator(proof)?)?;
            release_stated(proofs, proven, "cluster", version, *approved)
        }
        SystemCmd::SetOutboundApproval {
            scope,
            package,
            selection,
            replace,
            actor,
            consent,
        } => authorize_outbound(proofs, scope, package, selection, *replace, actor, consent),
        SystemCmd::SetEgress {
            org,
            app,
            egress,
            actor,
            consent,
        } => {
            let (statement, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            proofs.states(&statement, &format!("org:{org}"), "__set_egress", &|args| {
                *args == egress_args(app.as_deref(), *egress)
            })?;
            Ok(Some(used))
        }
        SystemCmd::SetReleaseDeadline {
            version,
            after,
            proof,
        } => {
            let (statement, used) = proofs.fresh(proofs.operator(proof)?)?;
            proofs.states(&statement, "cluster", "__release_deadline", &|args| {
                args["version"] == json!(version) && args["after"] == json!(after)
            })?;
            Ok(Some(used))
        }
        _ => Ok(None),
    }
}

/// A proven statement approves (or withdraws) `version` on `subject`.
fn release_stated(
    proofs: &Proofs<'_>,
    (statement, used): (crate::proof::Statement, String),
    subject: &str,
    version: &str,
    approved: bool,
) -> Result<Option<String>, Rejection> {
    let method = if approved {
        "__approve_release"
    } else {
        "__withdraw_release"
    };
    proofs.states(&statement, subject, method, &|args| {
        args["version"] == json!(version)
    })?;
    Ok(Some(used))
}

/// A `SetOutboundApproval`: the operator's signature for the cluster's scope,
/// else the actor's consent, over the package, the prefixes and whether they
/// replace the approval.
fn authorize_outbound(
    proofs: &Proofs<'_>,
    scope: &ReleaseScope,
    package: &str,
    selection: &calimero_primitives::outbound::OutboundSelection,
    replace: bool,
    actor: &str,
    consent: &Value,
) -> Result<Option<String>, Rejection> {
    let (statement, used) = if *scope == ReleaseScope::Cluster {
        proofs.fresh(proofs.operator(consent)?)?
    } else {
        proofs.fresh(proofs.consent_of(actor, consent)?)?
    };
    proofs.states(
        &statement,
        &scope_subject(scope, actor),
        "__approve_outbound",
        &|args| {
            args["package"] == json!(package)
                && args["prefixes"] == json!(selection)
                && args["replace"].as_bool().unwrap_or(false) == replace
        },
    )?;
    Ok(Some(used))
}

/// Apply a command of this area by its own rules, its proof already
/// judged ([`SystemMachine::run_authorized`]).
pub(super) fn apply(m: &mut SystemMachine, cmd: SystemCmd) -> Result<Value, Rejection> {
    let state = &mut m.state;
    match cmd {
        SystemCmd::SetOperatorRelease {
            version, approved, ..
        } => set_operator_release(state, version, approved),
        SystemCmd::SetReleaseDeadline { version, after, .. } => {
            set_release_deadline(state, version, after)
        }
        SystemCmd::SetReleaseApproval {
            scope,
            version,
            approved,
            actor,
            ..
        } => set_release_approval(state, &scope, version, approved, &actor),
        SystemCmd::SetOutboundApproval {
            scope,
            package,
            selection,
            replace,
            actor,
            ..
        } => set_outbound_approval(state, &scope, &package, &selection, replace, &actor),
        SystemCmd::SetEgress {
            org,
            app,
            egress,
            actor,
            ..
        } => set_egress(state, org, app, egress, &actor),
        // SAFETY: `SystemCmd::area` sends a command only to the module of
        // its area, and this match has an arm for each command of it.
        _ => unreachable!("a command of another area reached approvals::apply"),
    }
}

/// The operator approves (or withdraws) a release, where it approves them.
fn set_operator_release(
    state: &mut SystemState,
    version: String,
    approved: bool,
) -> Result<Value, Rejection> {
    if state.governance().releases != Authority::Operator {
        return Err(RELEASES_BY_OWNERS);
    }
    let releases = &mut state.operator_releases;
    approve_or_withdraw(releases, version, approved)?;
    Ok(json!({ "approvedReleases": releases }))
}

/// Add `version` to an approved set, or take it out; never the last one, as
/// an empty set would leave the apps unrestricted.
fn approve_or_withdraw(
    releases: &mut BTreeSet<String>,
    version: String,
    approved: bool,
) -> Result<(), Rejection> {
    if approved {
        let _new = releases.insert(version);
        return Ok(());
    }
    if releases.len() == 1 && releases.contains(&version) {
        return Err(LAST_RELEASE);
    }
    let _gone = releases.remove(&version);
    Ok(())
}

/// Set (or, with no time, lift) a release's deadline. Closed input: a semver
/// release, so it orders against the approved ones; a bounded number of them.
fn set_release_deadline(
    state: &mut SystemState,
    version: String,
    after: Option<u64>,
) -> Result<Value, Rejection> {
    if version.len() > MAX_RELEASE_VERSION_BYTES || semver::Version::parse(&version).is_err() {
        return Err(BAD_DEADLINE_VERSION);
    }
    match after {
        Some(at) => {
            if !state.release_deadlines.contains_key(&version)
                && state.release_deadlines.len() >= MAX_RELEASE_DEADLINES
            {
                return Err(TOO_MANY_DEADLINES);
            }
            let _prev = state.release_deadlines.insert(version, at);
        }
        None => {
            let _gone = state.release_deadlines.remove(&version);
        }
    }
    Ok(json!({ "deadlines": state.deadlines_json() }))
}

/// An organization approves (or withdraws) a release, where owners approve
/// them: an `APPROVE_RELEASES` holder.
fn set_release_approval(
    state: &mut SystemState,
    scope: &ReleaseScope,
    version: String,
    approved: bool,
    actor: &str,
) -> Result<Value, Rejection> {
    if state.governance().releases == Authority::Operator {
        return Err(RELEASES_BY_OPERATOR);
    }
    let releases = match scope {
        ReleaseScope::Cluster => return Err(OPERATOR_USES_ITS_OWN),
        ReleaseScope::Org { org } => {
            if !state.approves_releases(org, actor) {
                return Err(NEEDS_APPROVE_RELEASES);
            }
            let organization = state.orgs.get_mut(org).ok_or(NO_SUCH_ORG)?;
            &mut organization.approved_releases
        }
    };
    approve_or_withdraw(releases, version, approved)?;
    Ok(json!({ "approvedReleases": releases }))
}

/// Approve outbound prefixes for a package's signer, merged into the
/// approval the scope holds: the party governance names, an org-wide
/// `MANAGE_APPLICATION` holder for an organization whose egress is on.
fn set_outbound_approval(
    state: &mut SystemState,
    scope: &ReleaseScope,
    package: &str,
    selection: &calimero_primitives::outbound::OutboundSelection,
    replace: bool,
    actor: &str,
) -> Result<Value, Rejection> {
    let record = state.packages.get(package).ok_or(NO_SUCH_PACKAGE)?;
    let signer = record.signer_id.clone();
    let declared = record
        .latest()
        .map(|version| version.outbound.clone())
        .unwrap_or_default();
    let operator = state.governance().outbound == Authority::Operator;
    if operator != (*scope == ReleaseScope::Cluster) {
        return Err(if operator {
            OUTBOUND_BY_OPERATOR
        } else {
            OUTBOUND_BY_OWNERS
        });
    }
    let approvals = outbound_approvals_of(state, scope, actor)?;
    let existing = approvals
        .get(&signer)
        .map(|approval| calimero_outbound::Approval {
            prefixes: approval.prefixes.clone(),
            reviewed: approval.reviewed.clone(),
        });
    let merged =
        calimero_outbound::merge_approval(existing.as_ref(), &declared, selection, replace)
            .map_err(|_| BAD_PREFIXES)?;
    let approval = crate::types::OutboundApproval {
        prefixes: merged.prefixes,
        reviewed: merged.reviewed,
    };
    let _prev = approvals.insert(signer.clone(), approval.clone());
    Ok(json!({ "signer": signer, "approval": approval }))
}

/// The outbound approvals `scope` holds, for an actor who may change them:
/// the operator's, or an organization's (egress on, an org-wide
/// `MANAGE_APPLICATION` holder, as one approval covers every app of it).
fn outbound_approvals_of<'s>(
    state: &'s mut SystemState,
    scope: &ReleaseScope,
    actor: &str,
) -> Result<&'s mut BTreeMap<String, crate::types::OutboundApproval>, Rejection> {
    match scope {
        ReleaseScope::Cluster => Ok(&mut state.operator_outbound),
        ReleaseScope::Org { org } if state.orgs.get(org).is_some_and(|o| o.egress_off) => {
            Err(EGRESS_OFF)
        }
        ReleaseScope::Org { org } => {
            if !state.manages_apps(org, actor, None) {
                return Err(NEEDS_MANAGE_APPLICATION);
            }
            let organization = state.orgs.get_mut(org).ok_or(NO_SUCH_ORG)?;
            Ok(&mut organization.outbound_approvals)
        }
    }
}

/// Turn egress off or on for an organization, or for one instance of it: its
/// owner's or an admin's.
fn set_egress(
    state: &mut SystemState,
    org: OrgId,
    app: Option<AppId>,
    egress: bool,
    actor: &str,
) -> Result<Value, Rejection> {
    if !state.is_org_admin(&org, actor) {
        return Err(ONLY_ADMINS_SWITCH_EGRESS);
    }
    match &app {
        None => {
            let organization = state.orgs.get_mut(&org).ok_or(NO_SUCH_ORG)?;
            organization.egress_off = !egress;
        }
        Some(app_id) => {
            let record = state
                .apps
                .get_mut(app_id)
                .filter(|record| record.org == org)
                .ok_or(NO_SUCH_APP_IN_ORG)?;
            record.egress_off = !egress;
        }
    }
    let org_off = state.orgs.get(&org).is_some_and(|o| o.egress_off);
    let mut out = json!({ "org": org, "egress": egress, "orgEgressOff": org_off });
    if let Some(app_id) = app {
        out["app"] = json!(app_id);
    }
    Ok(out)
}

/// What a `SetEgress` consent states (`org:<org> / __set_egress`): the instance, or `null`
/// for the whole organization, and the switch.
#[must_use]
pub fn egress_args(app: Option<&str>, egress: bool) -> Value {
    json!({ "app": app, "egress": egress })
}
