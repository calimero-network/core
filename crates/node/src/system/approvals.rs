//! System commands for release approvals, release deadlines and outbound approvals.
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
        SystemCmd::SetReleaseApproval {
            scope,
            version,
            approved,
            actor,
            consent,
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let method = if *approved {
                "__approve_release"
            } else {
                "__withdraw_release"
            };
            proofs.states(&st, &scope_subject(scope, actor), method, &|a| {
                a["version"] == json!(version)
            })?;
            Ok(Some(used))
        }
        SystemCmd::SetOperatorRelease {
            version,
            approved,
            proof,
        } => {
            let (st, used) = proofs.fresh(proofs.operator(proof)?)?;
            let method = if *approved {
                "__approve_release"
            } else {
                "__withdraw_release"
            };
            proofs.states(&st, "cluster", method, &|a| a["version"] == json!(version))?;
            Ok(Some(used))
        }
        SystemCmd::SetOutboundApproval {
            scope,
            package,
            selection,
            replace,
            actor,
            consent,
        } => {
            let (st, used) = if *scope == ReleaseScope::Cluster {
                proofs.fresh(proofs.operator(consent)?)?
            } else {
                proofs.fresh(proofs.consent_of(actor, consent)?)?
            };
            proofs.states(
                &st,
                &scope_subject(scope, actor),
                "__approve_outbound",
                &|a| {
                    a["package"] == json!(package)
                        && a["prefixes"] == json!(selection)
                        && a["replace"].as_bool().unwrap_or(false) == *replace
                },
            )?;
            Ok(Some(used))
        }
        SystemCmd::SetReleaseDeadline {
            version,
            after,
            proof,
        } => {
            let (st, used) = proofs.fresh(proofs.operator(proof)?)?;
            proofs.states(&st, "cluster", "__release_deadline", &|a| {
                a["version"] == json!(version) && a["after"] == json!(after)
            })?;
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
        SystemCmd::SetOperatorRelease {
            version,
            approved,
            proof: _,
        } => {
            if s.governance().releases != Authority::Operator {
                return Err("this cluster's releases are approved by each app's owner");
            }
            let set = &mut s.operator_releases;
            if approved {
                let _new = set.insert(version);
            } else {
                if set.len() == 1 && set.contains(&version) {
                    return Err("the last approved release cannot be withdrawn");
                }
                let _gone = set.remove(&version);
            }
            Ok(json!({ "approvedReleases": set }))
        }
        SystemCmd::SetReleaseDeadline { version, after, .. } => {
            // Closed input: a semver release, so it orders against the
            // approved ones; a bounded number of them.
            if version.len() > MAX_RELEASE_VERSION_BYTES
                || semver::Version::parse(&version).is_err()
            {
                return Err("a deadline names a semver release of at most 64 bytes");
            }
            match after {
                Some(t) => {
                    if !s.release_deadlines.contains_key(&version)
                        && s.release_deadlines.len() >= MAX_RELEASE_DEADLINES
                    {
                        return Err("too many release deadlines; lift an old one first");
                    }
                    let _prev = s.release_deadlines.insert(version, t);
                }
                None => {
                    let _gone = s.release_deadlines.remove(&version);
                }
            }
            Ok(json!({ "deadlines": s.deadlines_json() }))
        }
        SystemCmd::SetReleaseApproval {
            scope,
            version,
            approved,
            actor,
            consent: _,
        } => {
            if s.governance().releases == Authority::Operator {
                return Err("this cluster's releases are approved by its operator");
            }
            let set = match &scope {
                ReleaseScope::Cluster => return Err("the operator uses SetOperatorRelease"),
                ReleaseScope::Org { org } => {
                    if !s.approves_releases(org, &actor) {
                        return Err(
                            "approving releases needs APPROVE_RELEASES in the organization",
                        );
                    }
                    let o = s.orgs.get_mut(org).ok_or("no such organization")?;
                    &mut o.approved_releases
                }
            };
            if approved {
                let _new = set.insert(version);
            } else {
                // An empty set would leave the apps unrestricted.
                if set.len() == 1 && set.contains(&version) {
                    return Err("the last approved release cannot be withdrawn");
                }
                let _gone = set.remove(&version);
            }
            Ok(json!({ "approvedReleases": set }))
        }
        SystemCmd::SetOutboundApproval {
            scope,
            package,
            selection,
            replace,
            actor,
            consent: _,
        } => {
            let record = s.packages.get(&package).ok_or("no such package")?;
            let signer = record.signer_id.clone();
            let declared = record
                .latest()
                .map(|v| v.outbound.clone())
                .unwrap_or_default();
            let operator = s.governance().outbound == Authority::Operator;
            if operator != (scope == ReleaseScope::Cluster) {
                return Err(if operator {
                    "this cluster's outbound hosts are approved by its operator"
                } else {
                    "this cluster's outbound hosts are approved by each app's owner"
                });
            }
            let approvals = match &scope {
                ReleaseScope::Cluster => &mut s.operator_outbound,
                ReleaseScope::Org { org } => {
                    // One approval covers every app of the organization,
                    // so only an org-wide grant reaches it.
                    if !s.manages_apps(org, &actor, None) {
                        return Err(
                            "approving outbound hosts needs MANAGE_APPLICATION in the organization",
                        );
                    }
                    let o = s.orgs.get_mut(org).ok_or("no such organization")?;
                    &mut o.outbound_approvals
                }
            };
            let existing = approvals.get(&signer).map(|a| calimero_outbound::Approval {
                prefixes: a.prefixes.clone(),
                reviewed: a.reviewed.clone(),
            });
            let merged = calimero_outbound::merge_approval(
                existing.as_ref(),
                &declared,
                &selection,
                replace,
            )
            .map_err(|_| "an approved prefix is not an https URL prefix, or too many")?;
            let approval = crate::types::OutboundApproval {
                prefixes: merged.prefixes,
                reviewed: merged.reviewed,
            };
            let _prev = approvals.insert(signer.clone(), approval.clone());
            Ok(json!({ "signer": signer, "approval": approval }))
        }
        // SAFETY: `SystemCmd::area` sends a command only to the module of
        // its area, and this match has an arm for each command of it.
        _ => unreachable!("a command of another area reached approvals::apply"),
    }
}
