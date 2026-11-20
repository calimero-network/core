//! System commands for update policies and the first approval of a red update
//! (`SetUpgradePolicy`, `ApproveUpgrade`), and the rules `UpgradeApp` applies with them:
//! the class an update carries, judged against the log, and the second approval its
//! app's policy may ask for.
//!
//! [`authorize`] judges the proof each carries and [`apply`] applies it;
//! `system.rs` sends a command here by its [`super::Area`].

#[allow(
    clippy::wildcard_imports,
    reason = "the arms read `system.rs`'s items, as every area's module does"
)]
use super::*;

use calimero_bundle::update::{classify, Reason, Release, Unit};

use crate::types::{PendingApproval, RunTarget, UpgradeClass, UpgradePolicy, MAX_CLASS_REASONS};

/// Judge the proof a command of this area carries ([`super::authorize`]).
pub(super) fn authorize(
    proofs: &Proofs<'_>,
    cmd: &SystemCmd,
) -> Result<Option<String>, &'static str> {
    match cmd {
        SystemCmd::SetUpgradePolicy {
            org,
            app,
            policy,
            actor,
            consent,
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = upgrade_policy_args(app.as_deref(), policy.as_ref());
            proofs.states(&st, &format!("org:{org}"), "__set_upgrade_policy", &|a| {
                *a == args
            })?;
            Ok(Some(used))
        }
        SystemCmd::ApproveUpgrade {
            app_id,
            package,
            version,
            targets,
            class,
            actor,
            consent,
            ..
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = approve_upgrade_args(package, version, targets, class);
            proofs.states(&st, app_id, "__approve_upgrade", &|a| *a == args)?;
            Ok(Some(used))
        }
        _ => Ok(None),
    }
}

/// Apply a command of this area by its own rules, its proof already judged
/// ([`super::authorize`]).
pub(super) fn apply(m: &mut SystemMachine, cmd: SystemCmd) -> Result<Value, &'static str> {
    let s = &mut m.state;
    match cmd {
        SystemCmd::SetUpgradePolicy {
            org,
            app,
            policy,
            actor,
            ..
        } => {
            if !s.orgs.contains_key(&org) {
                return Err("no such organization");
            }
            if !s.approves_upgrades(&org, &actor) {
                return Err(NOT_AN_UPGRADE_APPROVER);
            }
            if let Some(id) = &app {
                if !s.apps.get(id).is_some_and(|a| a.org == org) {
                    return Err("no such app in that organization");
                }
            }
            if let Some(p) = &policy {
                p.check()?;
            }
            let here: BTreeSet<AppId> = s
                .apps
                .values()
                .filter(|a| a.org == org)
                .map(|a| a.app_id.clone())
                .collect();
            let o = s.orgs.get_mut(&org).ok_or("no such organization")?;
            match (&app, policy) {
                (None, p) => o.upgrade_policy = p,
                (Some(id), Some(p)) => {
                    let _prev = o.app_upgrade_policies.insert(id.clone(), p);
                }
                (Some(id), None) => {
                    let _gone = o.app_upgrade_policies.remove(id);
                }
            }
            // What deleted apps left behind goes with the next change.
            o.app_upgrade_policies.retain(|id, _| here.contains(id));
            o.pending_upgrades.retain(|id, _| here.contains(id));
            Ok(s.policy_json(&org, app.as_deref()))
        }
        SystemCmd::ApproveUpgrade {
            app_id,
            package,
            version,
            targets,
            class,
            actor,
            at,
            ..
        } => {
            let a = s.apps.get(&app_id).ok_or("no such app")?;
            if !s.approves_upgrades(&a.org, &actor) {
                return Err(NOT_AN_UPGRADE_APPROVER);
            }
            if !s.upgrade_policy_of(a).0.second_approver || class.is_green() {
                return Err(ONE_APPROVAL);
            }
            if s.logged_release(&package, &version).is_none() {
                return Err("that release is not installed");
            }
            if s.running(a).is_some_and(|r| r.package != package) {
                return Err("the app runs another package");
            }
            // A unit's members only, each to an installed release of the package it runs.
            for (member, t) in &targets {
                let runs = a
                    .links
                    .values()
                    .any(|id| id == member)
                    .then(|| s.apps.get(member))
                    .flatten()
                    .and_then(|m| s.running(m));
                if !runs.is_some_and(|r| r.package == t.package)
                    || s.logged_release(&t.package, &t.version).is_none()
                {
                    return Err(
                        "a target is not a member of the app's unit moving along its package",
                    );
                }
            }
            s.check_class(a, (&package, &version), &targets, &class)?;
            let org = a.org.clone();
            // Another's live approval of the same update waits for its second: an upgrade.
            let other = s
                .orgs
                .get(&org)
                .and_then(|o| o.pending_upgrades.get(&app_id));
            if let Some(p) = other {
                if p.by != actor
                    && p.package == package
                    && p.version == version
                    && p.targets == targets
                    && s.approval_live(&org, &app_id, p, at)
                {
                    return Err(APPROVAL_PENDING);
                }
            }
            let approval = PendingApproval {
                package,
                version,
                targets,
                by: actor,
                at,
                class,
            };
            let out = json!(approval);
            let o = s.orgs.get_mut(&org).ok_or("no such organization")?;
            let _prev = o.pending_upgrades.insert(app_id, approval);
            Ok(out)
        }
        _ => unreachable!("a command of another area reached updates::apply"),
    }
}

/// Whether the log's `claimed` reason says at least what `derived` says: the same reason,
/// or for an added egress, every destination of it.
fn says_at_least(claimed: &Reason, derived: &Reason) -> bool {
    match (claimed, derived) {
        (Reason::EgressAdded { destinations: c }, Reason::EgressAdded { destinations: d }) => {
            d.iter().all(|x| c.contains(x))
        }
        _ => claimed == derived,
    }
}

impl SystemState {
    /// The update policy `app` follows, and where it comes from: its own (`app`), its
    /// organization's (`org`), or the defaults (`default`: nothing updates on its own, one
    /// approval applies an update).
    #[must_use]
    pub fn upgrade_policy_of(&self, app: &AppRecord) -> (UpgradePolicy, &'static str) {
        let o = self.orgs.get(&app.org);
        if let Some(p) = o.and_then(|o| o.app_upgrade_policies.get(&app.app_id)) {
            return (*p, "app");
        }
        if let Some(p) = o.and_then(|o| o.upgrade_policy) {
            return (p, "org");
        }
        (UpgradePolicy::default(), "default")
    }

    /// An update policy as the API answers it: the organization's (`app` absent) or an
    /// app's, `policy` the one in force, `source` where it comes from, `own` the one set
    /// at that scope (`null`: none).
    #[must_use]
    pub fn policy_json(&self, org: &str, app: Option<&str>) -> Value {
        let o = self.orgs.get(org);
        match app.and_then(|id| self.apps.get(id)) {
            Some(a) => {
                let (policy, source) = self.upgrade_policy_of(a);
                json!({
                    "org": org,
                    "app": a.app_id,
                    "policy": policy,
                    "source": source,
                    "own": o.and_then(|o| o.app_upgrade_policies.get(&a.app_id)),
                })
            }
            None => {
                let own = o.and_then(|o| o.upgrade_policy);
                json!({
                    "org": org,
                    "policy": own.unwrap_or_default(),
                    "source": if own.is_some() { "org" } else { "default" },
                    "own": own,
                })
            }
        }
    }

    /// The release `app` runs, as the log names it.
    fn running<'a>(&'a self, app: &'a AppRecord) -> Option<&'a PackageRef> {
        app.release.as_ref().or_else(|| self.governing_release(app))
    }

    /// What the classifier reads of an installed release that the log holds: its package's
    /// pinned signer, its `outbound` prefixes and its secrets' hosts. Its ABI (state
    /// version, methods) and its `uses` methods are in the `.mpk`, not the log: the API
    /// judges those, and apply takes its word for them.
    fn logged_release(&self, package: &str, version: &str) -> Option<Release> {
        let p = self.packages.get(package)?;
        let v = p.versions.get(version)?;
        Some(Release {
            package: package.to_owned(),
            version: version.to_owned(),
            signer: p.signer_id.clone(),
            outbound: v.outbound.iter().cloned().collect(),
            secrets: v
                .secrets
                .iter()
                .map(|(name, s)| (name.clone(), s.hosts.iter().cloned().collect()))
                .collect(),
            ..Release::default()
        })
    }

    /// `app`'s unit as the log holds it: the app at `root` (else the release it runs) and
    /// each member (`links`, transitively, as deep as a release may nest them) at the
    /// release it runs, or a direct member at its entry in `targets`. `None` when a part
    /// it needs is not an installed release.
    fn logged_unit(
        &self,
        app: &AppRecord,
        root: Option<(&str, &str)>,
        targets: &BTreeMap<AppId, RunTarget>,
    ) -> Option<Unit> {
        let at = match root {
            Some((p, v)) => self.logged_release(p, v)?,
            None => {
                let r = self.running(app)?;
                self.logged_release(&r.package, &r.version)?
            }
        };
        let mut unit = Unit::of(at);
        let mut stack = vec![(String::new(), app, 0_usize)];
        while let Some((path, rec, depth)) = stack.pop() {
            if depth >= crate::api::apps::MAX_UNIT_DEPTH {
                continue;
            }
            for (name, id) in &rec.links {
                let Some(member) = self.apps.get(id) else {
                    continue;
                };
                let Some(runs) = self.running(member) else {
                    continue;
                };
                let release = match targets.get(id).filter(|_| path.is_empty()) {
                    Some(t) if t.package == runs.package => {
                        self.logged_release(&t.package, &t.version)?
                    }
                    _ => self.logged_release(&runs.package, &runs.version)?,
                };
                let path = if path.is_empty() {
                    name.clone()
                } else {
                    format!("{path}/{name}")
                };
                let _prev = unit.members.insert(path.clone(), release);
                stack.push((path, member, depth + 1));
            }
        }
        Some(unit)
    }

    /// Refuse a `claimed` class of `app`'s move to `to` (with a unit's member `targets`)
    /// that is not one the classifier answers (red without a reason, green with one, more
    /// than [`MAX_CLASS_REASONS`]) or that leaves out a reason the log shows: a new signer
    /// or a destination no release before it reached, judged over the unit. What the log
    /// cannot show (state versions, methods called as the person) is the proposer's to
    /// say.
    ///
    /// # Errors
    /// [`UPGRADE_CLASS_MISMATCH`].
    pub fn check_class(
        &self,
        app: &AppRecord,
        to: (&str, &str),
        targets: &BTreeMap<AppId, RunTarget>,
        claimed: &UpgradeClass,
    ) -> Result<(), &'static str> {
        if claimed.reasons.len() > MAX_CLASS_REASONS
            || claimed.is_green() != claimed.reasons.is_empty()
        {
            return Err(UPGRADE_CLASS_MISMATCH);
        }
        let empty = BTreeMap::new();
        let (Some(before), Some(after)) = (
            self.logged_unit(app, None, &empty),
            self.logged_unit(app, Some(to), targets),
        ) else {
            return Ok(());
        };
        let derived = classify(&before, &after, &self.sealed_endpoints).reasons;
        if derived
            .iter()
            .all(|d| claimed.reasons.iter().any(|c| says_at_least(c, d)))
        {
            Ok(())
        } else {
            Err(UPGRADE_CLASS_MISMATCH)
        }
    }

    /// What lets `actor`'s upgrade of `app` to `release`, carrying `class`, at `at` apply:
    /// a class the log agrees with ([`Self::check_class`]; a raw upload carries none),
    /// and, where the app's policy asks for a second approver and the update is not green,
    /// another owner's or admin's live first approval of exactly this move
    /// ([`Self::approval_for`]), whose approver it answers.
    ///
    /// # Errors
    /// [`UPGRADE_CLASS_MISMATCH`], [`SECOND_APPROVAL_REQUIRED`].
    pub fn update_gate(
        &self,
        app: &AppRecord,
        release: Option<&PackageRef>,
        class: Option<&UpgradeClass>,
        actor: &str,
        at: u64,
    ) -> Result<Option<AccountId>, &'static str> {
        match (release, class) {
            (Some(to), Some(c)) => {
                self.check_class(app, (&to.package, &to.version), &BTreeMap::new(), c)?;
            }
            (None, Some(_)) => return Err(UPGRADE_CLASS_MISMATCH),
            (_, None) => {}
        }
        if !self.upgrade_policy_of(app).0.second_approver
            || class.is_some_and(UpgradeClass::is_green)
        {
            return Ok(None);
        }
        release
            .and_then(|to| self.approval_for(app, &to.package, &to.version, actor, at))
            .map(|p| Some(p.by.clone()))
            .ok_or(SECOND_APPROVAL_REQUIRED)
    }

    /// The live first approval ([`Self::approval_live`]) by someone other than `actor`
    /// that lets `app` move to `package`@`version`: of the app itself, or of the unit it is
    /// a member of, to its own target.
    #[must_use]
    pub fn approval_for(
        &self,
        app: &AppRecord,
        package: &str,
        version: &str,
        actor: &str,
        at: u64,
    ) -> Option<&PendingApproval> {
        self.orgs
            .get(&app.org)?
            .pending_upgrades
            .iter()
            .find(|(approved, p)| {
                p.by != actor
                    && p.covers(approved, &app.app_id, package, version)
                    && self.approval_live(&app.org, approved, p, at)
            })
            .map(|(_, p)| p)
    }

    /// Whether first approval `p` of `approved` (an app of `org`) may still let an upgrade
    /// logged at `at` through: no older than [`crate::types::MAX_APPROVAL_AGE_SECS`], its
    /// approver still an owner or admin, and some app it covers not yet at its target.
    #[must_use]
    pub fn approval_live(&self, org: &str, approved: &str, p: &PendingApproval, at: u64) -> bool {
        let runs = |id: &str, package: &str, version: &str| {
            self.apps.get(id).is_none_or(|a| {
                a.release
                    .as_ref()
                    .is_some_and(|r| r.package == package && r.version == version)
            })
        };
        let spent = runs(approved, &p.package, &p.version)
            && p.targets
                .iter()
                .all(|(id, t)| runs(id, &t.package, &t.version));
        p.fresh_at(at) && self.approves_upgrades(org, &p.by) && !spent
    }

    /// Drop `org`'s first approvals that can let nothing through any more at `at`.
    pub fn drop_spent_approvals(&mut self, org: &str, at: u64) {
        let Some(o) = self.orgs.get(org) else {
            return;
        };
        let dead: Vec<AppId> = o
            .pending_upgrades
            .iter()
            .filter(|(approved, p)| !self.approval_live(org, approved, p, at))
            .map(|(id, _)| id.clone())
            .collect();
        if let Some(o) = self.orgs.get_mut(org) {
            for id in dead {
                let _gone = o.pending_upgrades.remove(&id);
            }
        }
    }
}
