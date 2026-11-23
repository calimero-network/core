//! The consent page's install and update pages (C10 of the "Apps, private services and
//! owner-approved updates" design): what an update is in plain words, the install sheet,
//! a change of update policy, and the node's updates page.
//!
//! - **Words** ([`reason_words`], [`update_summary`], [`policy_words`]): the classifier's
//!   verdict (`calimero_bundle::update`) and an update policy as a person reads them. The
//!   upgrade sheets, `GET /apps/{app}/update`'s `summary` and the updates page say the
//!   same sentences.
//! - **Install** (`install`): the sheet for creating an instance from a deployment or a
//!   release: what gets created (the app and the apps its release creates with it, with
//!   their versions), where data can go (the unit's destinations), and that updates wait
//!   for an approval (the policy that will apply). Approving runs `POST /apps`'s own code
//!   (`api::apps::create_installed`) behind an unlogged step-up over
//!   `account:<a> / __create_instance / { name, package, version, deployment, org }`, as
//!   `createOrg` does: the log entries are the route's, so no feature gate.
//! - **Update policy** (`updatePolicy`): an owner or admin sets or drops an
//!   organization's or an app's policy; approving runs the route's code
//!   (`api::set_upgrade_policy`), whose consent is logged.
//! - **Updates page** (`updates`): the node's page for an organization's owners and
//!   admins, served at `/updates/{id}`. It shows nothing until the person unlocks it with
//!   their passkey on the node's origin (an unlogged step-up over
//!   `account:<a> / __view_updates / { org }`), because the app that asked holds the
//!   page's address and could read an unlocked list itself. Unlocked, its model is read
//!   live ([`hub_model`]): each organization the person approves updates of, the updates
//!   waiting across its apps (class, what changes, approvals), its policy and the apps'
//!   own. From it the page opens an app's update sheet or a policy change
//!   (`POST /consent/{id}/open`), each an ordinary request returning to the page. It is
//!   kept apart from the account's one undecided request ([`super::ConsentRequests`]):
//!   it approves nothing itself, so a forged page has nothing to swap into it.

use std::collections::BTreeMap;
use std::sync::Arc;

use calimero_bundle::update::{Class, Classification, Reason};
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

use super::{hosts, phrase, release_facts, Action, UpgradeSubject};
use crate::api::Principal;
use crate::error::ApiError;
use crate::node::Node;
use crate::system::PackageRef;
use crate::types::{now_secs, AppRecord, AutoApply, UpgradePolicy};

/// The most apps the updates page reads across the person's organizations: each one with
/// an update is classified, which reads its releases.
pub const MAX_UPDATES_PAGE_APPS: usize = 100;

/// The most apps an install sheet names it would create (the instance and its unit).
const MAX_INSTALL_LIST: usize = 16;

// ---------------------------------------------------------------- words

fn number(v: Option<u64>) -> String {
    v.map_or_else(|| "none".to_owned(), |n| n.to_string())
}

/// One reason an update is red, as a sentence (a member's prefixed with its path).
#[must_use]
pub fn reason_words(r: &Reason) -> String {
    let of = |m: &Option<String>| m.as_ref().map_or_else(String::new, |m| format!("{m}: "));
    let to = |d: &[String]| phrase(&hosts(d.iter().map(String::as_str)));
    match r {
        Reason::SignerChanged { member, .. } => format!(
            "{}Signed by a different publisher key than the version running now.",
            of(member)
        ),
        Reason::EgressAdded { destinations } => format!(
            "This update can send data to a new place: {}.",
            to(destinations)
        ),
        Reason::StateVersionChanged { member, from, to } => format!(
            "{}Stored data changes shape (state version {} to {}), and no migration runs.",
            of(member),
            number(*from),
            number(*to)
        ),
        Reason::MemberAdded {
            member,
            package,
            egress,
        } => format!(
            "Adds {member} ({package}), which can send data to: {}.",
            to(egress)
        ),
        Reason::MemberRemoved {
            member,
            package,
            egress,
        } => format!(
            "Removes {member} ({package}), which could send data to: {}.",
            to(egress)
        ),
        Reason::DelegatedMethodsAdded {
            member,
            package,
            methods,
        } => format!(
            "{}Can now act as you in {package}: {}.",
            of(member),
            methods.join(", ")
        ),
        Reason::Unclassified {
            member,
            package,
            version,
        } => format!(
            "{}This update could not be checked: {package} {version}, which it runs, is no longer installed. Treat it as red.",
            of(member)
        ),
    }
}

/// An update in one line, for a banner or a list: the new destinations it adds, else that
/// nothing changes where data goes (green), else its first reason.
#[must_use]
pub fn update_summary(c: &Classification) -> String {
    if !c.egress.added.is_empty() {
        return format!(
            "This update can send data to a new place: {}",
            phrase(&hosts(c.egress.added.iter().map(String::as_str)))
        );
    }
    if c.class == Class::Green {
        return "Nothing changes where your data goes".to_owned();
    }
    c.reasons.first().map_or_else(
        || "Review it before approving".to_owned(),
        |r| reason_words(r).trim_end_matches('.').to_owned(),
    )
}

/// An update policy as sentences, ending with where it comes from (`source`: `app`,
/// `org`, `default`).
#[must_use]
pub fn policy_words(policy: &UpgradePolicy, source: &str) -> Vec<String> {
    let mut out = vec![match policy.auto_apply {
        AutoApply::Never => "You approve each update. Nothing changes until you do.".to_owned(),
        AutoApply::NoDestinations => format!(
            "An update that keeps all data in Calimero applies on its own after a {} h hold; \
             every other update waits for an approval.",
            policy.hold_hours
        ),
    }];
    if policy.second_approver {
        out.push(
            "A red update needs two owners or admins: one approves it, another applies it."
                .to_owned(),
        );
    }
    out.push(
        match source {
            "app" => "This is the app's own policy.",
            "org" => "This is the organization's policy.",
            _ => "This is the default policy.",
        }
        .to_owned(),
    );
    out
}

/// A policy as the pages show it: its fields, `source` and its `words`.
fn policy_view(policy: &UpgradePolicy, source: &str) -> Value {
    json!({
        "autoApply": policy.auto_apply,
        "holdHours": policy.hold_hours,
        "secondApprover": policy.second_approver,
        "source": source,
        "words": policy_words(policy, source),
    })
}

// ---------------------------------------------------------------- upgrade sheets

/// Where an upgrade sheet's approval stands under the app's policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ApprovalStep {
    /// One approval applies it.
    Only,
    /// A red update under `secondApprover`, nobody else's approval waiting: this one is the
    /// first (`POST /apps/{app}/upgrade-approvals`), another owner or admin applies it.
    First,
    /// Another owner's or admin's (`by`) first approval waits: this one applies it.
    Second { by: String },
}

/// The step `account`'s approval of `app`'s move to `to` would be (`green`: the class is
/// known and green). `409 SECOND_APPROVAL_REQUIRED` when `account` approved it first
/// already: another owner or admin applies it.
pub(super) fn approval_step(
    node: &Node,
    app: &AppRecord,
    to: &PackageRef,
    targets: &BTreeMap<String, crate::types::RunTarget>,
    green: bool,
    account: &str,
) -> Result<ApprovalStep, ApiError> {
    let now = now_secs();
    node.sys(|s| {
        if !s.upgrade_policy_of(app).0.second_approver || green {
            return Ok(ApprovalStep::Only);
        }
        if let Some(p) = s.approval_for(app, &to.package, &to.version, targets, account, now) {
            return Ok(ApprovalStep::Second { by: p.by.clone() });
        }
        let own = s.orgs.get(&app.org).is_some_and(|o| {
            o.pending_upgrades.iter().any(|(approved, p)| {
                p.by == account
                    && p.covers(approved, &app.app_id, &to.package, &to.version)
                    && s.approval_live(&app.org, approved, p, now)
            })
        });
        if own {
            return Err(ApiError::new(
                409,
                "SECOND_APPROVAL_REQUIRED",
                "you approved this update already: another owner or admin of the organization applies it",
            ));
        }
        Ok(ApprovalStep::First)
    })
}

/// What the sheet says of a fact an unclassified update leaves unread.
const UNREAD: &str = "this update could not be checked against the version running now";

/// The sheet's `update`: the class and its reasons in words, the destinations before and
/// after, the state version, the policy and where the approval stands, and which button
/// leads (`keepByDefault`: an update that adds a destination keeps the current version
/// unless the person chooses otherwise). `members`: a line per member that moves.
pub(super) fn update_section(
    node: &Node,
    app: &AppRecord,
    c: Option<&Classification>,
    step: &ApprovalStep,
    (from, to): (&str, &str),
    members: Vec<String>,
) -> Value {
    let (policy, source) = node.sys(|s| s.upgrade_policy_of(app));
    // Not classified (a part's release unreadable, or no verdict at all): every fact
    // that compares the two sides is unknown, never "unchanged".
    let unknown = c.is_none_or(crate::egress::unclassified);
    let green = c.is_some_and(|c| c.class == Class::Green);
    let added = c.map(|c| c.egress.added.clone()).unwrap_or_default();
    let reasons: Vec<String> = c
        .map(|c| c.reasons.iter().map(reason_words).collect())
        .unwrap_or_else(|| vec!["This update could not be checked: treat it as red.".to_owned()]);
    let acts_as: Vec<String> = c
        .into_iter()
        .flat_map(|c| &c.reasons)
        .filter(|r| matches!(r, Reason::DelegatedMethodsAdded { .. }))
        .map(reason_words)
        .collect();
    let state = c
        .and_then(|c| {
            c.reasons
                .iter()
                .find(|r| matches!(r, Reason::StateVersionChanged { .. }))
        })
        .map_or_else(
            || "Same state version: the new code reads your data as it is.".to_owned(),
            reason_words,
        );
    // Unclassified: where data can go is unknown, not none.
    let destinations = c.filter(|c| !crate::egress::unclassified(c)).map_or(Value::Null, |c| {
        json!({
            "before": hosts(c.egress.before.iter().map(String::as_str)),
            "after": hosts(c.egress.after.iter().map(String::as_str)),
            "added": hosts(c.egress.added.iter().map(String::as_str)),
            "removed": hosts(c.egress.removed.iter().map(String::as_str)),
            "words": if !c.egress.added.is_empty() {
                format!("New: {}", phrase(&hosts(c.egress.added.iter().map(String::as_str))))
            } else if c.egress.after.is_empty() {
                "Unchanged: your data stays in Calimero".to_owned()
            } else {
                format!("Unchanged: {}", phrase(&hosts(c.egress.after.iter().map(String::as_str))))
            },
        })
    });
    let (approve_label, approval) = match step {
        ApprovalStep::Only if green => (
            "Update with passkey".to_owned(),
            json!({ "needed": 1, "step": "only" }),
        ),
        ApprovalStep::Only => (
            "Approve with passkey".to_owned(),
            json!({ "needed": 1, "step": "only" }),
        ),
        ApprovalStep::First => (
            "Approve (1 of 2)".to_owned(),
            json!({ "needed": 2, "step": "first" }),
        ),
        ApprovalStep::Second { by } => (
            "Approve and update (2 of 2)".to_owned(),
            json!({ "needed": 2, "step": "second", "approvedBy": by }),
        ),
    };
    let approval_words = match step {
        ApprovalStep::Only => None,
        ApprovalStep::First => Some(
            "This update needs a second owner or admin: your approval is the first, and \
             nothing changes until another one applies it."
                .to_owned(),
        ),
        ApprovalStep::Second { by } => Some(format!(
            "{by} approved this update first; your approval applies it."
        )),
    };
    let held = green
        && policy.auto_apply == AutoApply::NoDestinations
        && c.is_some_and(|c| c.egress.before.is_empty() && c.egress.after.is_empty());
    json!({
        "class": c.map(|c| c.class),
        "summary": c.map_or_else(|| "This update could not be checked".to_owned(), update_summary),
        "reasons": reasons,
        "destinations": destinations,
        "actsAs": if unknown {
            json!(format!("Unknown: {UNREAD}"))
        } else if acts_as.is_empty() {
            json!("Unchanged: none")
        } else {
            json!(acts_as)
        },
        "newMethods": c.filter(|_| !unknown).map(|c| c.new_methods.clone()),
        "removedMethods": c.filter(|_| !unknown).map(|c| c.removed_methods.clone()),
        "stateVersion": if unknown { format!("Unknown: {UNREAD}") } else { state },
        "signer": if unknown {
            format!("Unknown: {UNREAD}")
        } else if c.is_some_and(|c| c.signer_changed) {
            "A different publisher key signs it".to_owned()
        } else {
            "Same signer as the version running now".to_owned()
        },
        "members": members,
        "policy": policy_view(&policy, source),
        "approval": approval,
        "approvalWords": approval_words,
        "approveLabel": approve_label,
        "keepByDefault": !added.is_empty(),
        "keepLabel": if green { "Not now".to_owned() } else { format!("Keep {from}") },
        "keepWords": format!(
            "Keeping {from} is safe: it goes on working and your data stays where it is."
        ),
        "hold": held.then(|| format!(
            "With the automatic policy this would apply on its own after the {} h hold; you opened it early.",
            policy.hold_hours
        )),
        "to": to,
    })
}

// ---------------------------------------------------------------- install

/// `newOrg` in an `install` subject: the organization made with the instance.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewOrgSubject {
    name: String,
}

/// An `install` request: create an instance of `package` from `deployment`'s offer, or of
/// the release `version` (the newest the person sees when absent), named `name`, as
/// `POST /apps` takes them.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallSubject {
    package: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    deployment: Option<String>,
    name: String,
    #[serde(default)]
    org: Option<String>,
    #[serde(default)]
    new_org: Option<NewOrgSubject>,
    #[serde(default)]
    init_args: Value,
}

/// The install sheet: judged as `POST /apps` would judge the session (a frontend makes
/// instances of its own installed package only), the release resolved without installing
/// anything, and the unit it would make read from the signed manifests.
pub(super) async fn install_request(
    node: &Node,
    p: &Principal,
    account: &str,
    subject: InstallSubject,
) -> Result<(Action, Value), ApiError> {
    let package = subject.package;
    let frontend = p.bound_app() == Some(package.as_str());
    if p.bound_app().is_some() && !frontend {
        return Err(ApiError::forbidden(
            "a session opened on an app's frontend creates instances of that app only",
        ));
    }
    if subject.org.is_some() && subject.new_org.is_some() {
        return Err(ApiError::bad_request(
            "BAD_REQUEST",
            "name an organization or a new one, not both",
        ));
    }
    let release = match &subject.deployment {
        Some(d) => {
            crate::api::apps::offered_bundle(node, d, &package, subject.version.as_deref(), None)
                .await?
                .2
        }
        None => {
            let version = node
                .sys(|s| {
                    s.releases_used(&package, account, frontend)
                        .filter(|v| subject.version.as_ref().is_none_or(|w| &v.version == w))
                        .max_by(|a, b| crate::system::compare_versions(&a.version, &b.version))
                        .map(|v| v.version.clone())
                })
                .ok_or_else(|| {
                    ApiError::new(
                        409,
                        "NOT_INSTALLED",
                        &format!(
                            "{package} {} is not installed here; install it first",
                            subject.version.as_deref().unwrap_or("")
                        ),
                    )
                })?;
            PackageRef {
                package: package.clone(),
                version,
                service: None,
                compute: false,
            }
        }
    };
    let target = node
        .sys(|s| {
            s.packages
                .get(&release.package)?
                .versions
                .get(&release.version)
                .cloned()
        })
        .ok_or_else(|| ApiError::not_found("no such release"))?;
    let facts = release_facts(node, &package, &target).await?;
    let app_name = facts
        .manifest
        .metadata
        .as_ref()
        .map_or_else(|| package.clone(), |m| m.name.clone());
    let author = facts
        .manifest
        .metadata
        .as_ref()
        .and_then(|m| m.author.clone());
    let unit =
        crate::egress::release_unit(node, &release, (account, subject.deployment.as_deref()))
            .await?;
    let mut creates = vec![json!({
        "name": app_name, "what": "the app", "package": package, "version": release.version,
    })];
    for (path, r) in unit.members.iter().take(MAX_INSTALL_LIST) {
        let named = node.sys(|s| {
            s.packages
                .get(&r.package)
                .and_then(|p| p.versions.get(&r.version))
                .cloned()
        });
        let name = match named {
            Some(v) => release_facts(node, &r.package, &v)
                .await
                .ok()
                .and_then(|f| f.manifest.metadata.as_ref().map(|m| m.name.clone())),
            None => None,
        };
        let only = if r.egress().is_empty() {
            format!("only {app_name} and you can reach it; it sends nothing out")
        } else {
            format!(
                "only {app_name} and you can reach it; it can send data to {}",
                phrase(&hosts(r.egress().iter().map(String::as_str)))
            )
        };
        creates.push(json!({
            "name": name.unwrap_or_else(|| r.package.clone()),
            "as": path,
            "what": only,
            "package": r.package,
            "version": r.version,
        }));
    }
    let destinations: Vec<String> = unit.egress().into_iter().collect();
    let members = !unit.members.is_empty();
    let where_words = if destinations.is_empty() {
        "Your data stays in Calimero.".to_owned()
    } else {
        format!(
            "This app can send data to: {}.",
            phrase(&hosts(destinations.iter().map(String::as_str)))
        )
    };
    // The policy the instance will follow: its organization's where the person is in it,
    // else the defaults (a new organization starts with them).
    let (policy, source, org_name) = node.sys(|s| {
        let o = subject
            .org
            .as_ref()
            .and_then(|org| s.orgs.get(org))
            .filter(|o| o.role(account).is_some());
        match o.and_then(|o| o.upgrade_policy.map(|p| (p, o.name.clone()))) {
            Some((p, name)) => (p, "org", Some(name)),
            None => (
                UpgradePolicy::default(),
                "default",
                o.map(|o| o.name.clone()),
            ),
        }
    });
    let mut update_words = policy_words(&policy, source);
    update_words.pop();
    if destinations.is_empty() && policy.auto_apply == AutoApply::Never {
        update_words.push(format!(
            "Because {app_name} sends no data out of Calimero, you can later add a policy that \
             applies its updates on their own after a 24 h hold, as long as they keep it that way."
        ));
    }
    let payer = subject.deployment.as_ref().and_then(|d| {
        node.sys(|s| {
            let d = s.deployments.get(d)?;
            Some(match &d.owner {
                crate::system::Deployer::Account(a) => s.accounts.get(a).map(|a| a.name.clone()),
                crate::system::Deployer::Org(o) => s.orgs.get(o).map(|o| o.name.clone()),
            })
        })
        .flatten()
    });
    let mut points = vec![json!(format!(
        "{} is created for you{}: {app_name} {}{}.",
        subject.name,
        org_name
            .as_ref()
            .map_or_else(String::new, |o| format!(" in {o}")),
        release.version,
        if members {
            " and the apps it creates with it"
        } else {
            ""
        }
    ))];
    points.push(json!(if members {
        format!("{where_words} That includes the apps it creates.")
    } else {
        where_words.clone()
    }));
    points.push(json!(update_words[0]));
    if let Some(who) = &payer {
        points.push(json!(format!(
            "{who} pays for {app_name}; you have no Calimero bill for it."
        )));
    }
    points.push(json!("Nothing is created until you approve."));
    let model = json!({
        "kind": "install",
        "headline": format!("Create {} from {app_name} {}", subject.name, release.version),
        "title": format!("Start using {app_name}"),
        "points": points,
        "attention": [],
        "app": { "name": app_name, "package": package },
        "publisher": { "name": author, "signer": facts.signer },
        "install": {
            "name": subject.name,
            "package": package,
            "version": release.version,
            "deployment": subject.deployment,
            "org": subject.org,
            "orgName": org_name,
            "newOrg": subject.new_org.as_ref().map(|o| o.name.clone()),
            "creates": creates,
            "destinations": hosts(destinations.iter().map(String::as_str)),
            "where": where_words,
            "updates": update_words,
            "policy": policy_view(&policy, source),
            "payer": payer,
        },
        "before": "Until you approve, nothing is created.",
        "after": format!(
            "Once you approve, {} runs {app_name} {} and the apps it creates, and you own them.",
            subject.name, release.version
        ),
        "notes": [],
    });
    Ok((
        Action::Install {
            install: crate::api::apps::Install {
                name: subject.name,
                package,
                version: Some(release.version),
                deployment: subject.deployment,
                org: subject.org,
                new_org: subject.new_org.map(|o| o.name),
                init_args: subject.init_args,
                from_frontend: frontend,
            },
            mpk: target.mpk,
        },
        model,
    ))
}

// ---------------------------------------------------------------- update policy

/// An `updatePolicy` request: `policy` (`null` drops it) for the app `app`, else for the
/// organization `org`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdatePolicySubject {
    #[serde(default)]
    org: Option<String>,
    #[serde(default)]
    app: Option<String>,
    #[serde(default)]
    policy: Option<UpgradePolicy>,
}

/// The policy change's page, judged as the route judges it before the passkey.
pub(super) async fn policy_request(
    node: &Node,
    p: &Principal,
    account: &str,
    subject: UpdatePolicySubject,
) -> Result<(Action, Value), ApiError> {
    let (org, app) = match (&subject.app, subject.org) {
        (Some(a), _) => {
            let app = node.app(&node.canonical_app_id(a)).await?;
            p.within(node, &app)?;
            (app.org.clone(), Some(app))
        }
        (None, Some(org)) => (org, None),
        (None, None) => {
            return Err(ApiError::bad_request(
                "BAD_REQUEST",
                "name the organization or the app whose update policy changes",
            ))
        }
    };
    crate::api::approver_of(node, &org, account)?;
    if let Some(Err(why)) = subject.policy.as_ref().map(UpgradePolicy::check) {
        return Err(crate::node::rejected(why));
    }
    node.require_features(&[crate::system::cluster_version::Feature::UpgradePolicies])
        .await?;
    let app_id = app.as_ref().map(|a| a.app_id.clone());
    let (now, org_name) = node.sys(|s| {
        (
            s.policy_json(&org, app_id.as_deref()),
            s.orgs.get(&org).map(|o| o.name.clone()),
        )
    });
    let org_name = org_name.unwrap_or_else(|| org.clone());
    let current: UpgradePolicy = serde_json::from_value(now["policy"].clone()).unwrap_or_default();
    let source = now["source"].as_str().unwrap_or("default").to_owned();
    let whose = app
        .as_ref()
        .map_or_else(|| org_name.clone(), |a| a.name.clone());
    let after = match (&subject.policy, &app) {
        (Some(p), Some(_)) => policy_words(p, "app"),
        (Some(p), None) => policy_words(p, "org"),
        (None, Some(_)) => {
            let inherited = node.sys(|s| {
                s.orgs
                    .get(&org)
                    .and_then(|o| o.upgrade_policy)
                    .map_or((UpgradePolicy::default(), "default"), |p| (p, "org"))
            });
            policy_words(&inherited.0, inherited.1)
        }
        (None, None) => policy_words(&UpgradePolicy::default(), "default"),
    };
    let mut points = vec![
        json!(format!(
            "Now: {}",
            policy_words(&current, &source).join(" ")
        )),
        json!(format!("Once you approve: {}", after.join(" "))),
    ];
    if app.is_none() {
        points.push(json!(
            "It applies to every app of the organization that has no policy of its own."
        ));
    }
    points.push(json!("Nothing changes until you approve."));
    let model = json!({
        "kind": "updatePolicy",
        "headline": format!("Change how {whose} updates"),
        "title": format!("Change how {whose} updates"),
        "points": points,
        "attention": [],
        "scope": match &app {
            Some(a) => json!({ "kind": "app", "app": a.app_id, "name": a.name, "org": org }),
            None => json!({ "kind": "org", "org": org, "name": org_name }),
        },
        "policy": {
            "now": policy_view(&current, &source),
            "after": subject.policy.as_ref().map(|p| policy_view(p, if app.is_some() { "app" } else { "org" })),
            "afterWords": after,
        },
        "before": "Until you approve, the policy stays as it is.",
        "after": "Once you approve, the new policy applies to every update from then on.",
        "notes": [],
    });
    Ok((
        Action::UpdatePolicy {
            org,
            app: app_id,
            policy: subject.policy,
            own: now["own"].clone(),
        },
        model,
    ))
}

// ---------------------------------------------------------------- updates page

/// An `updates` request: the node's updates page, for every organization whose updates
/// the person approves, or only `org`.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdatesSubject {
    #[serde(default)]
    org: Option<String>,
}

/// The updates page before it is unlocked: no list, only what unlocking does.
pub(super) fn updates_request(
    node: &Node,
    account: &str,
    subject: UpdatesSubject,
) -> Result<(Action, Value), ApiError> {
    if let Some(org) = &subject.org {
        crate::api::approver_of(node, org, account)?;
    }
    let model = json!({
        "kind": "updates",
        "headline": "Updates for apps you approve",
        "title": "Updates for apps you approve",
        "points": [
            "Confirm it is you with your passkey to see the updates waiting for your approval and your organizations' update policies.",
            "Showing them changes nothing. Each update and each policy change asks you again.",
        ],
        "attention": [],
        "locked": true,
        "org": subject.org,
        "before": "This page lists nothing until you confirm it is you.",
        "after": "Once you confirm, this page lists the updates waiting across the apps you approve.",
        "notes": [],
    });
    Ok((Action::Updates { org: subject.org }, model))
}

/// What the unlocked updates page shows `account`, read now: each organization they
/// approve updates of (only `only`, when given), with its policy and its apps' own, and
/// the updates waiting across them, at most [`MAX_UPDATES_PAGE_APPS`] apps read.
pub(super) async fn hub_model(node: &Node, account: &str, only: Option<&str>) -> Value {
    let orgs: Vec<(String, String, Vec<AppRecord>)> = node.sys(|s| {
        s.orgs
            .values()
            .filter(|o| {
                only.is_none_or(|w| o.org_id == w) && s.approves_upgrades(&o.org_id, account)
            })
            .map(|o| {
                let mut apps: Vec<AppRecord> = s
                    .apps
                    .values()
                    .filter(|a| a.org == o.org_id)
                    .cloned()
                    .collect();
                apps.sort_by(|a, b| a.app_id.cmp(&b.app_id));
                (o.org_id.clone(), o.name.clone(), apps)
            })
            .collect()
    });
    let mut read = 0_usize;
    let mut truncated = false;
    let mut rows = Vec::new();
    let mut policies = Vec::new();
    for (org, org_name, apps) in &orgs {
        let (own, overrides) = node.sys(|s| {
            let o = s.orgs.get(org);
            (
                o.and_then(|o| o.upgrade_policy),
                o.map(|o| o.app_upgrade_policies.clone())
                    .unwrap_or_default(),
            )
        });
        let names: BTreeMap<&str, &str> = apps
            .iter()
            .map(|a| (a.app_id.as_str(), a.name.as_str()))
            .collect();
        policies.push(json!({
            "org": org,
            "name": org_name,
            "policy": policy_view(&own.unwrap_or_default(), if own.is_some() { "org" } else { "default" }),
            "own": own,
            "apps": overrides.iter().map(|(id, p)| json!({
                "app": id,
                "name": names.get(id.as_str()),
                "policy": policy_view(p, "app"),
            })).collect::<Vec<_>>(),
        }));
        for app in apps {
            if read >= MAX_UPDATES_PAGE_APPS {
                truncated = true;
                break;
            }
            read += 1;
            let view = crate::api::update_view(node, app, Some(account), None).await;
            let available = &view["available"];
            let pending: Vec<Value> = view["pendingApprovals"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if available.is_null() && pending.is_empty() {
                continue;
            }
            let reasons: Vec<String> = available["reasons"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|r| serde_json::from_value::<Reason>(r.clone()).ok())
                .map(|r| reason_words(&r))
                .collect();
            let first = pending.iter().find(|p| p["app"] == json!(app.app_id));
            rows.push(json!({
                "app": app.app_id,
                "name": app.name,
                "org": org,
                "orgName": org_name,
                "from": view["running"]["version"],
                "to": available["version"],
                "class": available["class"],
                "summary": available["summary"],
                "reasons": reasons,
                "approvalsNeeded": available["approvalsNeeded"],
                "approvedBy": available["approvedBy"],
                "approvedByYou": available["approvedBy"] == json!(account),
                "waitingSince": first.map(|p| p["at"].clone()),
                "unit": !app.links.is_empty(),
                "mayApprove": view["mayApprove"],
            }));
        }
    }
    json!({
        "orgs": orgs.iter().map(|(id, name, _)| json!({ "org": id, "name": name })).collect::<Vec<_>>(),
        "pending": rows,
        "policies": policies,
        "truncated": truncated,
    })
}

/// `POST /consent/{id}/open`: a request the unlocked updates page opens for its account,
/// returning to the page.
#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(super) enum OpenBody {
    Upgrade { subject: UpgradeSubject },
    UpgradeUnit { subject: UpgradeSubject },
    UpdatePolicy { subject: UpdatePolicySubject },
}

/// Open `body` for the account of the unlocked updates page `hub` (its `account`, `org`
/// filter and `back`, the page's own address): an ordinary request, as an unbound session
/// of the account would ask it.
pub(super) async fn open(
    node: &Arc<Node>,
    account: &str,
    back: Url,
    body: OpenBody,
) -> Result<Value, ApiError> {
    let (sys_index, _caught_up) = node.sys_fresh_index().await;
    let p = super::page_principal(node, account, sys_index);
    let (action, model) = match body {
        OpenBody::Upgrade { subject } => {
            super::upgrade_request(node, &p, account, subject, super::Part::Alone).await?
        }
        OpenBody::UpgradeUnit { subject } => {
            super::unit_upgrade_request(node, &p, account, subject).await?
        }
        OpenBody::UpdatePolicy { subject } => policy_request(node, &p, account, subject).await?,
    };
    let axum::Json(v) = super::hold(node, account.to_owned(), None, action, model, back).await?;
    Ok(v)
}
