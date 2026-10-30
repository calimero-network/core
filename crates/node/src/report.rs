//! Read-only reports of what each app runs and which mero-tee releases may
//! hold its data: for the operator (`GET /ops/apps`, `/ops/orgs`), for an
//! organization's members (`GET /orgs/{org}/apps`) and for an account's own
//! apps (`GET /auth/apps`). Built from the system state alone; nothing here
//! writes or decides anything.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;

use serde::Serialize;
use serde_json::{json, Value};

use crate::system::{compare_versions, SystemState, OPERATOR};
use crate::types::{Access, AppRecord, OrgRole, UpgradeRecord};

/// The most apps (or organizations) one page of a report answers.
pub const MAX_REPORT_PAGE: usize = 100;

/// The most newer releases a member's report lists per app (the newest).
pub const MAX_AVAILABLE_VERSIONS: usize = 16;

/// Who reads a report, which decides what it may name.
#[derive(Clone, Copy, Debug)]
pub enum Reader<'a> {
    /// The operator: ids, digests, releases and kinds. Never a name, an
    /// account, or a package release it could not see anyway
    /// ([`SystemState::sees_release`]).
    Operator,
    /// An account reading apps it reaches or manages.
    Account(&'a str),
}

impl Reader<'_> {
    fn who(&self) -> &str {
        match self {
            Self::Operator => OPERATOR,
            Self::Account(a) => a,
        }
    }
}

/// Who made an upgrade, by kind. The kind is judged against the app's and its
/// organization's owners as they stand when the report is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ActorKind {
    /// The app's owner, or, for an organization's app, the organization's owner.
    Owner,
    /// Another account (an organization's member holding `MANAGE_APPLICATION`).
    Member,
    /// The operator, through the Ops API.
    Operator,
}

/// The kind of the actor `by` (an account, or [`OPERATOR`]) of an upgrade of `app`.
#[must_use]
pub fn actor_kind(s: &SystemState, app: &AppRecord, by: &str) -> ActorKind {
    if by == OPERATOR {
        return ActorKind::Operator;
    }
    let org_owner = app
        .org
        .as_ref()
        .and_then(|o| s.orgs.get(o))
        .is_some_and(|o| o.role(by) == Some(OrgRole::Owner));
    if by == app.owner || org_owner {
        ActorKind::Owner
    } else {
        ActorKind::Member
    }
}

/// A package release, as a report names it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseName {
    pub package: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
}

/// The package release that ships bundle `hash`, if `reader` may see one: the
/// release the bundle was recorded from when visible, else the highest visible
/// release that ships it.
#[must_use]
pub fn release_of_bundle(s: &SystemState, hash: &str, reader: Reader<'_>) -> Option<ReleaseName> {
    let who = reader.who();
    let recorded = s.bundles.get(hash).and_then(|b| b.package.as_ref());
    if let Some(r) = recorded {
        let visible = s
            .packages
            .get(&r.package)
            .and_then(|p| p.versions.get(&r.version))
            .is_some_and(|v| v.services.values().any(|h| h == hash) && s.sees_release(v, who));
        if visible {
            return Some(ReleaseName {
                package: r.package.clone(),
                version: r.version.clone(),
                service: r.service.clone(),
            });
        }
    }
    s.packages
        .values()
        .flat_map(|p| p.versions.values().map(move |v| (p, v)))
        .filter(|(_, v)| s.sees_release(v, who))
        .filter_map(|(p, v)| {
            let service = v.services.iter().find(|(_, h)| *h == hash)?.0;
            Some((p, v, service))
        })
        .max_by(|a, b| compare_versions(&a.1.version, &b.1.version))
        .map(|(p, v, service)| ReleaseName {
            package: p.name.clone(),
            version: v.version.clone(),
            service: (!service.is_empty()).then(|| service.clone()),
        })
}

/// The releases of `current`'s package newer than it, shipping its service,
/// that `account` may see and so upgrade to: the newest
/// [`MAX_AVAILABLE_VERSIONS`], oldest first.
fn newer_versions(s: &SystemState, current: &ReleaseName, account: &str) -> Vec<String> {
    let Some(p) = s.packages.get(&current.package) else {
        return Vec::new();
    };
    let service = current.service.clone().unwrap_or_default();
    let mut newer: Vec<&str> = s
        .visible_versions(p, account)
        .filter(|v| v.services.contains_key(&service))
        .filter(|v| compare_versions(&v.version, &current.version).is_gt())
        .map(|v| v.version.as_str())
        .collect();
    newer.sort_by(|a, b| compare_versions(a, b));
    let skip = newer.len().saturating_sub(MAX_AVAILABLE_VERSIONS);
    newer.into_iter().skip(skip).map(str::to_owned).collect()
}

/// A bundle as a report names it: its digest, and its release if visible.
fn bundle_json(s: &SystemState, hash: &str, reader: Reader<'_>) -> Value {
    let mut v = json!({ "bundle": hash });
    if let Some(r) = release_of_bundle(s, hash, reader) {
        v["package"] = json!(r.package);
        v["version"] = json!(r.version);
    }
    v
}

fn last_upgrade_json(
    s: &SystemState,
    app: &AppRecord,
    u: &UpgradeRecord,
    reader: Reader<'_>,
) -> Value {
    let mut v = json!({
        "from": bundle_json(s, &u.from, reader),
        "to": bundle_json(s, &u.to, reader),
        "at": u.at,
        "ok": u.ok,
        "actorKind": actor_kind(s, app, &u.by),
    });
    // The account, an app's own error text and the run are its members'
    // to read, not the operator's.
    if let Reader::Account(_) = reader {
        v["actor"] = json!(u.by);
        if !u.error.is_null() {
            v["error"] = u.error.clone();
        }
        if let Some(run) = &u.run {
            v["run"] = json!(run);
        }
    }
    v
}

/// The scope whose approvals cover `app`: its organization, or its account.
fn scope_json(app: &AppRecord) -> Value {
    match &app.org {
        Some(org) => json!({ "kind": "org", "org": org }),
        None => json!({ "kind": "account" }),
    }
}

/// A set of approved releases; `null` when none was ever recorded (state
/// from before approvals), which leaves the apps unrestricted.
fn approved_json(set: Option<&BTreeSet<String>>) -> Value {
    set.map_or(Value::Null, |s| json!(s))
}

/// One app's entry. `releases` is each node's attested release, as known.
#[must_use]
pub fn app_json(
    s: &SystemState,
    app: &AppRecord,
    releases: &BTreeMap<u64, Option<String>>,
    reader: Reader<'_>,
) -> Value {
    let current = release_of_bundle(s, &app.bundle, reader);
    let replicas: Vec<Value> = app
        .replicas
        .iter()
        .map(|id| {
            let release = releases.get(id).cloned().flatten();
            let approved = s.release_may_hold(app, release.as_deref());
            json!({ "node": id, "release": release, "approved": approved })
        })
        .collect();
    let mut v = json!({
        "app": app.app_id,
        "group": app.group,
        "scope": scope_json(app),
        "bundle": app.bundle,
        "package": current.as_ref().map(|r| json!({ "package": r.package, "version": r.version })),
        "publisher": current
            .as_ref()
            .and_then(|r| s.packages.get(&r.package))
            .map(|p| json!({ "signer": p.signer_id })),
        "upgradesHeldBy": s.governance().app_upgrades,
        "lastUpgrade": app.last_upgrade.as_ref().map(|u| last_upgrade_json(s, app, u, reader)),
        "approvedReleases": approved_json(s.approved_releases(app)),
        "replicas": replicas,
    });
    if let Reader::Account(account) = reader {
        v["name"] = json!(app.name);
        if let Access::Team { team } = &app.access {
            v["team"] = json!(team);
        }
        if let Some(r) = &current {
            v["package"]["service"] = json!(r.service);
        }
        v["availableVersions"] = json!(current
            .as_ref()
            .map(|r| newer_versions(s, r, account))
            .unwrap_or_default());
    }
    v
}

/// One page of `s.apps` after `after`: those `keep` admits, at most `limit`,
/// and the id to continue after when more remain.
pub fn page<'a>(
    s: &'a SystemState,
    after: Option<&str>,
    limit: usize,
    keep: impl Fn(&AppRecord) -> bool,
) -> (Vec<&'a AppRecord>, Option<String>) {
    let start = after.map_or(Bound::Unbounded, |a| Bound::Excluded(a.to_owned()));
    let mut apps: Vec<&AppRecord> = s
        .apps
        .range((start, Bound::Unbounded))
        .map(|(_, a)| a)
        .filter(|a| keep(a))
        .take(limit.saturating_add(1))
        .collect();
    let next = (apps.len() > limit).then(|| {
        apps.truncate(limit);
        apps.last().map(|a| a.app_id.clone())
    });
    (apps, next.flatten())
}

/// The releases the cluster's nodes run, each with how many nodes run it and
/// whether `approved` holds it (`None`: unrestricted).
#[must_use]
pub fn cluster_releases(
    releases: &BTreeMap<u64, Option<String>>,
    approved: Option<&BTreeSet<String>>,
) -> Vec<Value> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for r in releases.values().flatten() {
        *counts.entry(r.as_str()).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(release, nodes)| {
            json!({
                "release": release,
                "nodes": nodes,
                "approved": approved.is_none_or(|set| set.contains(release)),
            })
        })
        .collect()
}

/// An approved set as a report shows it.
#[must_use]
pub fn approved_releases(set: Option<&BTreeSet<String>>) -> Value {
    approved_json(set)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Org;

    fn app(owner: &str, org: Option<&str>) -> AppRecord {
        serde_json::from_value(json!({
            "appId": "a", "name": "n", "group": 2, "bundle": "b", "owner": owner,
            "org": org, "replicas": [1], "createdAt": 0,
        }))
        .unwrap()
    }

    #[test]
    fn actor_kinds_follow_the_apps_and_organizations_owners() {
        let mut s = SystemState::default();
        let mut o: Org = serde_json::from_value(json!({
            "orgId": "o", "name": "Acme", "members": { "boss": "owner", "m": "member" },
            "teams": {},
        }))
        .unwrap();
        o.org_id = "o".into();
        let _prev = s.orgs.insert("o".into(), o);
        let personal = app("alice", None);
        assert_eq!(actor_kind(&s, &personal, "alice"), ActorKind::Owner);
        assert_eq!(actor_kind(&s, &personal, OPERATOR), ActorKind::Operator);
        let org_app = app("m", Some("o"));
        assert_eq!(actor_kind(&s, &org_app, "boss"), ActorKind::Owner);
        assert_eq!(actor_kind(&s, &org_app, "m"), ActorKind::Owner);
        assert_eq!(actor_kind(&s, &org_app, "other"), ActorKind::Member);
    }

    #[test]
    fn pages_are_bounded_and_continue_after_the_last() {
        let mut s = SystemState::default();
        for id in ["a", "b", "c"] {
            let mut a = app("x", None);
            a.app_id = id.into();
            let _prev = s.apps.insert(id.into(), a);
        }
        let (apps, next) = page(&s, None, 2, |_| true);
        assert_eq!(apps.len(), 2);
        assert_eq!(next.as_deref(), Some("b"));
        let (apps, next) = page(&s, next.as_deref(), 2, |_| true);
        assert_eq!(
            apps.iter().map(|a| a.app_id.as_str()).collect::<Vec<_>>(),
            ["c"]
        );
        assert_eq!(next, None);
    }
}
