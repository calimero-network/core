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

use crate::types::{
    AutoApply, PendingApproval, RunTarget, UpgradeClass, UpgradePolicy, MAX_CLASS_REASONS,
};

/// Judge the proof a command of this area carries ([`super::authorize`]).
pub(super) fn authorize(proofs: &Proofs<'_>, cmd: &SystemCmd) -> Result<Option<String>, Rejection> {
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
        SystemCmd::UndoUpgrade {
            app_id,
            apps,
            actor,
            consent,
            ..
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let args = proofs.s.undo_args(apps.keys());
            proofs.states(&st, app_id, "__undo_upgrade", &|a| *a == args)?;
            Ok(Some(used))
        }
        _ => Ok(None),
    }
}

/// Apply a command of this area by its own rules, its proof already judged
/// ([`super::authorize`]).
pub(super) fn apply(m: &mut SystemMachine, cmd: SystemCmd) -> Result<Value, Rejection> {
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
                return Err("no such organization".into());
            }
            if !s.approves_upgrades(&org, &actor) {
                return Err(NOT_AN_UPGRADE_APPROVER.into());
            }
            if let Some(id) = &app {
                if !s.apps.get(id).is_some_and(|a| a.org == org) {
                    return Err("no such app in that organization".into());
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
                return Err(NOT_AN_UPGRADE_APPROVER.into());
            }
            if !s.upgrade_policy_of(a).0.second_approver || class.is_green() {
                return Err(ONE_APPROVAL);
            }
            if s.logged_release(&package, &version).is_none() {
                return Err("that release is not installed".into());
            }
            if s.running(a).is_some_and(|r| r.package != package) {
                return Err("the app runs another package".into());
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
                        "a target is not a member of the app's unit moving along its package"
                            .into(),
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
        SystemCmd::UndoUpgrade {
            app_id,
            apps,
            actor,
            at,
            release_installed,
            ..
        } => {
            let a = s.apps.get(&app_id).ok_or("no such app")?;
            if !s.approves_upgrades(&a.org, &actor) {
                return Err(NOT_AN_UPGRADE_APPROVER.into());
            }
            let named: BTreeSet<&AppId> = apps.keys().collect();
            let expected = s.undo_set(&app_id)?;
            if named != expected.iter().collect() {
                return Err(UNDO_NOT_AS_NAMED);
            }
            // Every part is judged before any moves: all or nothing.
            for (id, replicas) in &apps {
                let a = s.apps.get(id).ok_or("no such app")?;
                let p = a.restore_point.as_ref().ok_or(NO_RESTORE_POINT)?;
                s.may_undo(a, p, at)?;
                match &p.file {
                    Some(f) => {
                        let distinct: BTreeSet<u64> = replicas.iter().copied().collect();
                        if replicas.first() != Some(&f.node)
                            || distinct.len() != replicas.len()
                            || replicas.iter().any(|n| s.replica_node(n).is_none())
                        {
                            return Err(UNDO_NOT_AS_NAMED);
                        }
                    }
                    None if !replicas.is_empty() => return Err(UNDO_NOT_AS_NAMED),
                    None => {}
                }
                if !s.bundles.contains_key(&p.from_bundle) {
                    return Err(RESTORE_BUNDLE_GONE.into());
                }
                if release_installed
                    || s.cluster_brings(cluster_version::Feature::UndoToInstalledRelease)
                {
                    s.undo_release_installed(p)?;
                }
            }
            let mut groups = serde_json::Map::new();
            for (id, replicas) in apps {
                let group = s.next_group.max(1) + 1;
                let a = s.apps.get_mut(&id).ok_or("no such app")?;
                let Some(mut p) = a.restore_point.take() else {
                    return Err(NO_RESTORE_POINT);
                };
                a.last_upgrade = Some(crate::types::UpgradeRecord {
                    from: a.bundle.clone(),
                    to: p.from_bundle.clone(),
                    at,
                    by: actor.clone(),
                    ok: true,
                    error: Value::Null,
                    run: p.run.clone(),
                    api_key: None,
                    from_release: p.to_release.clone(),
                    to_release: p.from_release.clone(),
                    class: None,
                    reasons: Vec::new(),
                    approved_by: None,
                    auto: false,
                    undo: true,
                });
                a.bundle = p.from_bundle.clone();
                a.compute_bundle = p.from_compute.clone();
                a.release = p.from_release.clone();
                if let Some(f) = &p.file {
                    // Into a new group the holder seeds from the restore point, as a
                    // restore does; the restore point stays until that restore finishes,
                    // so a seed that stopped half way may be sent again.
                    a.group = group;
                    a.replicas = replicas;
                    a.restore = Some(crate::types::PendingRestore {
                        seed: Some(f.node),
                        source: Some(f.source.clone()),
                    });
                    p.undone_at = Some(p.undone_at.unwrap_or(at));
                    a.restore_point = Some(p);
                    s.next_group = group;
                    let _prev = groups.insert(id, json!(group));
                } else {
                    let _prev = groups.insert(id, Value::Null);
                }
            }
            Ok(json!({ "groups": groups }))
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
    /// The apps an undo of `app_id` reverts: the app, and each direct member whose
    /// restore point is of the same unit's run as the app's.
    ///
    /// # Errors
    /// [`NO_RESTORE_POINT`] when the app has none.
    pub fn undo_set(&self, app_id: &str) -> Result<BTreeSet<AppId>, Rejection> {
        let a = self.apps.get(app_id).ok_or("no such app")?;
        let p = a.restore_point.as_ref().ok_or(NO_RESTORE_POINT)?;
        // A member its unit's run moved goes back with its instance, never alone.
        if p.run.is_some() && self.unit_parent(app_id).is_some() {
            return Err(UNDO_NOT_AS_NAMED);
        }
        let mut out: BTreeSet<AppId> = std::iter::once(a.app_id.clone()).collect();
        if let Some(run) = &p.run {
            for m in a.links.values() {
                let same = self.apps.get(m).and_then(|m| m.restore_point.as_ref());
                if same.is_some_and(|q| q.run.as_ref() == Some(run)) {
                    let _new = out.insert(m.clone());
                }
            }
        }
        Ok(out)
    }

    /// What an owner or admin consents to when undoing (`<app> / __undo_upgrade`): each
    /// app reverted, with the release it goes back to (`null`: a raw bundle, named in
    /// `bundle`), the one it leaves and the restore point's file (`null` when the upgrade
    /// ran nothing on its group).
    #[must_use]
    pub fn undo_args<'a>(&self, apps: impl IntoIterator<Item = &'a AppId>) -> Value {
        let parts: serde_json::Map<String, Value> = apps
            .into_iter()
            .map(|id| {
                let p = self.apps.get(id).and_then(|a| a.restore_point.as_ref());
                (
                    id.clone(),
                    p.map_or(Value::Null, |p| {
                        json!({
                            "to": p.from_release,
                            "bundle": p.from_bundle,
                            "from": p.to_release,
                            "restorePoint": p.file.as_ref().map(|f| &f.source.sha256),
                        })
                    }),
                )
            })
            .collect();
        json!({ "apps": parts })
    }

    /// Whether `app`'s restore point `p` may be undone at `at`: first within the window
    /// and while the app runs what the upgrade moved it to; once undone, again only while
    /// that restore is under way (a seed sent again).
    ///
    /// # Errors
    /// [`UNDO_WINDOW_PASSED`], [`UNDO_STALE`].
    pub fn may_undo(
        &self,
        app: &AppRecord,
        p: &crate::types::RestorePoint,
        at: u64,
    ) -> Result<(), Rejection> {
        if p.undone_at.is_some() {
            let restoring = app.restore.as_ref().and_then(|r| r.source.as_ref());
            return if restoring.is_some() && restoring == p.file.as_ref().map(|f| &f.source) {
                Ok(())
            } else {
                Err(UNDO_STALE)
            };
        }
        if !p.open_at(at) {
            return Err(UNDO_WINDOW_PASSED);
        }
        if app.bundle != p.to_bundle || app.release != p.to_release || app.restore.is_some() {
            return Err(UNDO_STALE);
        }
        Ok(())
    }

    /// Whether the release a restore point returns to is still installed and ships its bundle
    /// (another release's WASM may keep the bundle). A raw bundle or an undo already applied passes.
    ///
    /// # Errors
    /// [`UNDO_RELEASE_GONE`].
    pub fn undo_release_installed(&self, p: &crate::types::RestorePoint) -> Result<(), Rejection> {
        match &p.from_release {
            Some(r) if p.undone_at.is_none() && !self.release_ships(r, &p.from_bundle) => {
                Err(UNDO_RELEASE_GONE)
            }
            _ => Ok(()),
        }
    }

    /// What lets an update of `app` to `to` (its `bundle`), carrying `class`, apply on
    /// its own at `at` with restore point `taken` (`UpgradeApp::auto`): the app's policy
    /// says `noDestinations`; the class is green; it moves the app along its package to
    /// a newer release; the app is no unit's member (a member moves with its unit); its
    /// unit, as the log holds it, has no destination before or after; its hold has
    /// passed since the release was logged (its deployment's offer of exactly it for an
    /// instance of a deployment, else its install); and a restore point was taken where
    /// the upgrade runs on the app's group.
    ///
    /// # Errors
    /// [`AUTO_UPDATE_REFUSED`], [`HOLD_NOT_OVER`], [`RESTORE_POINT_REQUIRED`], or the
    /// class's own ([`Self::check_class`]).
    #[allow(
        clippy::too_many_arguments,
        reason = "one argument per part of the UpgradeApp entry it judges"
    )]
    pub fn auto_update_gate(
        &self,
        app: &AppRecord,
        to: Option<&PackageRef>,
        bundle: &str,
        class: Option<&UpgradeClass>,
        at: u64,
        taken: Option<&crate::types::TakenRestorePoint>,
        members_stay: bool,
    ) -> Result<(), Rejection> {
        self.auto_update_allows(app, to, class, at, members_stay)?;
        let runs_on_group = app.components.is_app() && bundle != app.bundle;
        match taken {
            Some(t) if t.file.is_some() || !runs_on_group => Ok(()),
            _ => Err(RESTORE_POINT_REQUIRED),
        }
    }

    /// The apps an update would apply to on their own at `at`, each with the releases it
    /// could move to, newest first: on a `noDestinations` policy, not being restored, the
    /// offer of their deployment (for its instances) or else the installed releases of
    /// the package they run, each where [`Self::auto_update_allows`] lets it through as
    /// green (the class itself is the leader's to classify, so a release the log shows as
    /// green may still be red for its code: the leader then tries the next one down).
    /// Never the release an update to failed or an undo left: an owner or admin decides
    /// on that one. An app with no such release is left out.
    #[must_use]
    pub fn auto_update_candidates(&self, at: u64) -> Vec<(AppRecord, Vec<PackageRef>)> {
        let green = UpgradeClass {
            class: calimero_bundle::update::Class::Green,
            reasons: Vec::new(),
        };
        self.apps
            .values()
            .filter(|a| a.restore.is_none())
            .filter(|a| self.upgrade_policy_of(a).0.auto_apply == AutoApply::NoDestinations)
            .filter_map(|a| {
                let running = self.running(a)?.clone();
                let mut versions: Vec<String> = match &a.deployment {
                    Some(d) => vec![self
                        .deployments
                        .get(d)?
                        .apps
                        .values()
                        .find(|o| o.package == running.package)?
                        .version
                        .clone()],
                    None => self
                        .packages
                        .get(&running.package)?
                        .versions
                        .keys()
                        .cloned()
                        .collect(),
                };
                // Newest first: a newer one that sends data out (or that the leader finds
                // red) waits for an owner or admin, and does not hold back a quiet one.
                versions.sort_by(|x, y| compare_versions(y, x));
                let to: Vec<PackageRef> = versions
                    .into_iter()
                    .map(|version| PackageRef {
                        package: running.package.clone(),
                        version,
                        service: running.service.clone(),
                        compute: false,
                    })
                    .filter(|to| {
                        let refused = a.last_upgrade.as_ref().is_some_and(|u| {
                            (!u.ok && u.to_release.as_ref() == Some(to))
                                || (u.undo && u.from_release.as_ref() == Some(to))
                        });
                        !refused
                            && self
                                .auto_update_allows(
                                    a,
                                    Some(to),
                                    Some(&green),
                                    at,
                                    self.members_stay(false),
                                )
                                .is_ok()
                    })
                    .collect();
                (!to.is_empty()).then(|| (a.clone(), to))
            })
            .collect()
    }

    /// [`Self::auto_update_gate`] but for its restore point: what the leader asks before
    /// it takes one.
    ///
    /// # Errors
    /// As [`Self::auto_update_gate`], but [`RESTORE_POINT_REQUIRED`].
    pub fn auto_update_allows(
        &self,
        app: &AppRecord,
        to: Option<&PackageRef>,
        class: Option<&UpgradeClass>,
        at: u64,
        members_stay: bool,
    ) -> Result<(), Rejection> {
        let (policy, _) = self.upgrade_policy_of(app);
        // A new owner's first update reaches the person on a sheet that names it.
        if policy.auto_apply != AutoApply::NoDestinations || self.handed_over_unseen(app) {
            return Err(AUTO_UPDATE_REFUSED);
        }
        let (Some(to), Some(class)) = (to, class) else {
            return Err(AUTO_UPDATE_REFUSED);
        };
        if !class.is_green() || self.unit_parent(&app.app_id).is_some() {
            return Err(AUTO_UPDATE_REFUSED);
        }
        self.check_class(app, (&to.package, &to.version), &BTreeMap::new(), class)?;
        let newer = self.running(app).is_some_and(|r| {
            r.package == to.package
                && compare_versions(&to.version, &r.version) == std::cmp::Ordering::Greater
        });
        if !newer || (members_stay && self.moves_members(app, to)) {
            return Err(AUTO_UPDATE_REFUSED);
        }
        let empty = BTreeMap::new();
        let destinations = [
            self.logged_unit(app, None, &empty),
            self.logged_unit(app, Some((&to.package, &to.version)), &empty),
        ];
        if !destinations
            .iter()
            .all(|u| u.as_ref().is_some_and(|u| u.egress().is_empty()))
        {
            return Err(AUTO_UPDATE_REFUSED);
        }
        let since = match &app.deployment {
            Some(d) => self
                .deployments
                .get(d)
                .and_then(|d| {
                    d.apps
                        .values()
                        .find(|o| o.package == to.package && o.version == to.version)
                })
                .map(|o| o.added_at)
                .ok_or(AUTO_UPDATE_REFUSED)?,
            None => self
                .packages
                .get(&to.package)
                .and_then(|p| p.versions.get(&to.version))
                .map(|v| v.installed_at)
                .ok_or(AUTO_UPDATE_REFUSED)?,
        };
        if at < since.saturating_add(u64::from(policy.hold_hours) * 3600) {
            return Err(HOLD_NOT_OVER);
        }
        Ok(())
    }

    /// Whether moving `app` to `to` would move a member of its unit: `to`'s `uses` resolve a
    /// direct member to another release than it runs, or name it no longer, or the log
    /// keeps no `uses` of `to` to tell ([`Self::release_of_use`]).
    pub fn moves_members(&self, app: &AppRecord, to: &PackageRef) -> bool {
        let uses = self
            .packages
            .get(&to.package)
            .and_then(|p| p.versions.get(&to.version))
            .map(|v| &v.uses);
        app.links.iter().any(|(name, id)| {
            let runs = self.apps.get(id).and_then(|m| self.running(m));
            let resolved = uses
                .and_then(|u| u.get(name))
                .and_then(|u| self.release_of_use(u));
            !matches!((runs, resolved), (Some(r), Some((p, v))) if r.package == p && r.version == v)
        })
    }

    /// The release a `uses` reference creates an app at, as creating an instance picks it:
    /// the one its lock pinned, else the newest installed inside its range; `None` when
    /// that release is not installed.
    pub fn release_of_use(&self, u: &ReleaseUse) -> Option<(String, String)> {
        let installed = &self.packages.get(&u.package)?.versions;
        let version = match &u.version {
            Some(v) => installed.contains_key(v).then(|| v.clone())?,
            None => {
                let range = semver::VersionReq::parse(&u.range).ok()?;
                installed
                    .keys()
                    .filter_map(|v| semver::Version::parse(v).ok().map(|p| (p, v)))
                    .filter(|(p, _)| range.matches(p))
                    .max_by(|a, b| a.0.cmp(&b.0))
                    .map(|(_, v)| v.clone())?
            }
        };
        Some((u.package.clone(), version))
    }

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

    /// Where an update of `app` on its own begins (`UpgradeFailed::from_release`): the
    /// release it runs as the candidates and the gate judge it ([`Self::running`]: its
    /// record's, else the one its bundle is governed by).
    #[must_use]
    pub fn auto_update_from(&self, app: &AppRecord) -> Option<PackageRef> {
        self.running(app).cloned()
    }

    /// The release `app` runs, as the log names it: its record's, else the release its
    /// bundle is governed by (a record without one).
    pub(crate) fn running<'a>(&'a self, app: &'a AppRecord) -> Option<&'a PackageRef> {
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
            secrets: v.secrets.clone(),
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
        let mut parts = self.unit_parts(app, root, targets)?.into_iter();
        let (_, package, version) = parts.next()?;
        let mut unit = Unit::of(self.logged_release(&package, &version)?);
        for (path, package, version) in parts {
            let _prev = unit
                .members
                .insert(path, self.logged_release(&package, &version)?);
        }
        Some(unit)
    }

    /// Each part of `app`'s unit with the release the log names for it, as
    /// [`Self::logged_unit`] reads them, installed or not: `(path, package, version)`, the
    /// app first (path `""`, at `root`, else the release it runs), then each member
    /// (`links`, transitively) at the release it runs, or a direct member at its entry in
    /// `targets`. `None` when the app runs no release.
    fn unit_parts(
        &self,
        app: &AppRecord,
        root: Option<(&str, &str)>,
        targets: &BTreeMap<AppId, RunTarget>,
    ) -> Option<Vec<(String, String, String)>> {
        let (package, version) = match root {
            Some((p, v)) => (p.to_owned(), v.to_owned()),
            None => {
                let r = self.running(app)?;
                (r.package.clone(), r.version.clone())
            }
        };
        let mut parts = vec![(String::new(), package, version)];
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
                let at = match targets.get(id).filter(|_| path.is_empty()) {
                    Some(t) if t.package == runs.package => (&t.package, &t.version),
                    _ => (&runs.package, &runs.version),
                };
                let path = if path.is_empty() {
                    name.clone()
                } else {
                    format!("{path}/{name}")
                };
                parts.push((path.clone(), at.0.clone(), at.1.clone()));
                stack.push((path, member, depth + 1));
            }
        }
        Some(parts)
    }

    /// Why `app`'s move to `to` (with a unit's member `targets`) cannot be classified from
    /// the log: an [`Reason::Unclassified`] for each part, before or after, whose release
    /// is not installed (the release it runs uninstalled while another ships its WASM), at
    /// most [`MAX_CLASS_REASONS`]. Empty when the log rebuilds both sides.
    #[must_use]
    pub fn unclassified_reasons(
        &self,
        app: &AppRecord,
        to: (&str, &str),
        targets: &BTreeMap<AppId, RunTarget>,
    ) -> Vec<Reason> {
        let mut reasons: Vec<Reason> = Vec::new();
        let sides = [
            self.unit_parts(app, None, &BTreeMap::new()),
            self.unit_parts(app, Some(to), targets),
        ];
        for (path, package, version) in sides.into_iter().flatten().flatten() {
            if self.logged_release(&package, &version).is_some() {
                continue;
            }
            let r = Reason::Unclassified {
                member: (!path.is_empty()).then_some(path),
                package,
                version,
            };
            if !reasons.contains(&r) {
                reasons.push(r);
            }
        }
        reasons.truncate(MAX_CLASS_REASONS);
        reasons
    }

    /// The class of `app`'s move to `to` (with a unit's member `targets`) when the log
    /// cannot classify it: red, with [`Self::unclassified_reasons`]. `None` when the log
    /// rebuilds both sides (the classifier's to judge).
    #[must_use]
    pub fn unclassified(
        &self,
        app: &AppRecord,
        to: (&str, &str),
        targets: &BTreeMap<AppId, RunTarget>,
    ) -> Option<UpgradeClass> {
        let reasons = self.unclassified_reasons(app, to, targets);
        (!reasons.is_empty()).then_some(UpgradeClass {
            class: calimero_bundle::update::Class::Red,
            reasons,
        })
    }

    /// Refuse a `claimed` class of `app`'s move to `to` (with a unit's member `targets`)
    /// that is not one the classifier answers (red without a reason, green with one, more
    /// than [`MAX_CLASS_REASONS`]) or that leaves out a reason the log shows: a new signer
    /// or a destination no release before it reached, judged over the unit. What the log
    /// cannot show (state versions, methods called as the person) is the proposer's to
    /// say. A move the log cannot rebuild on either side (a release the app or a member
    /// runs, or a target, no longer installed) is never green: the log cannot confirm
    /// it, so only a red class, gated as red, passes, its reasons the proposer's word.
    /// An `unclassified` reason must name a part the log lacks
    /// ([`Self::unclassified_reasons`]), so a move the log rebuilds never carries one.
    ///
    /// # Errors
    /// [`UPGRADE_CLASS_MISMATCH`].
    pub fn check_class(
        &self,
        app: &AppRecord,
        to: (&str, &str),
        targets: &BTreeMap<AppId, RunTarget>,
        claimed: &UpgradeClass,
    ) -> Result<(), Rejection> {
        if claimed.reasons.len() > MAX_CLASS_REASONS
            || claimed.is_green() != claimed.reasons.is_empty()
        {
            return Err(UPGRADE_CLASS_MISMATCH);
        }
        if claimed
            .reasons
            .iter()
            .any(|r| matches!(r, Reason::Unclassified { .. }))
        {
            let lacks = self.unclassified_reasons(app, to, targets);
            if claimed
                .reasons
                .iter()
                .any(|r| matches!(r, Reason::Unclassified { .. }) && !lacks.contains(r))
            {
                return Err(UPGRADE_CLASS_MISMATCH);
            }
        }
        let empty = BTreeMap::new();
        let (Some(before), Some(after)) = (
            self.logged_unit(app, None, &empty),
            self.logged_unit(app, Some(to), targets),
        ) else {
            return if claimed.is_green() {
                Err(UPGRADE_CLASS_MISMATCH)
            } else {
                Ok(())
            };
        };
        let derived = classify(&before, &after).reasons;
        if derived
            .iter()
            .all(|d| claimed.reasons.iter().any(|c| says_at_least(c, d)))
        {
            Ok(())
        } else {
            Err(UPGRADE_CLASS_MISMATCH)
        }
    }

    /// What lets `actor`'s upgrade of `app` to `release`, carrying `class`, at `at` apply,
    /// alone (`targets` empty) or as the instance of a unit whose members move to
    /// `targets`: a class the log agrees with, judged over the unit as it moves
    /// ([`Self::check_class`]; a raw upload carries none), and, where the app's policy
    /// asks for a second approver and the update is not green, another owner's or admin's
    /// live first approval of exactly this move ([`Self::approval_for`]; for a unit, of
    /// the same targets), whose approver it answers.
    ///
    /// # Errors
    /// [`UPGRADE_CLASS_MISMATCH`], [`SECOND_APPROVAL_REQUIRED`].
    pub fn update_gate(
        &self,
        app: &AppRecord,
        release: Option<&PackageRef>,
        class: Option<&UpgradeClass>,
        targets: &BTreeMap<AppId, RunTarget>,
        actor: &str,
        at: u64,
    ) -> Result<Option<AccountId>, Rejection> {
        self.class_agrees(app, release, class, targets)?;
        if !self.upgrade_policy_of(app).0.second_approver
            || class.is_some_and(UpgradeClass::is_green)
        {
            return Ok(None);
        }
        release
            .and_then(|to| self.approval_for(app, &to.package, &to.version, targets, actor, at))
            .map(|p| Some(p.by.clone()))
            .ok_or(SECOND_APPROVAL_REQUIRED)
    }

    /// [`Self::check_class`] of the class an `UpgradeApp` carries, if any: a raw upload
    /// (no release) carries none.
    fn class_agrees(
        &self,
        app: &AppRecord,
        release: Option<&PackageRef>,
        class: Option<&UpgradeClass>,
        targets: &BTreeMap<AppId, RunTarget>,
    ) -> Result<(), Rejection> {
        match (release, class) {
            (Some(to), Some(c)) => self.check_class(app, (&to.package, &to.version), targets, c),
            (None, Some(_)) => Err(UPGRADE_CLASS_MISMATCH),
            (_, None) => Ok(()),
        }
    }

    /// The unit's run `run` names, when it is one: only a unit's consent (`__upgrade_unit`)
    /// keeps member targets on a run (`StartUpgradeRun` drops any other run's).
    fn unit_run(&self, run: Option<&str>) -> Option<&crate::types::UpgradeRun> {
        let id = run?;
        self.orgs
            .values()
            .find_map(|o| o.upgrade_runs.get(id))
            .filter(|r| !r.targets.is_empty())
    }

    /// Whether `app` is the instance a unit's run `run` moves with its members: the app
    /// the run names first, which its consent named (`upgrade_run_apps`).
    #[must_use]
    pub fn is_unit_instance(&self, run: Option<&str>, app: &str) -> bool {
        self.unit_run(run)
            .is_some_and(|r| r.apps.first().is_some_and(|a| a == app))
    }

    /// [`Self::update_gate`] for an `UpgradeApp` logged by `run` (if any). A unit's run is
    /// judged once, as a whole, under its instance's policy: the instance's own entry
    /// carries the unit's class and is judged over the run's member targets, and a member
    /// the run moves takes that judgement once the instance moved in the same run (its
    /// own class is still checked against the log), so a member's own policy never stops
    /// a unit half way. A member whose instance has not moved in the run, an app of any
    /// other run and an app upgraded alone are judged on their own.
    ///
    /// # Errors
    /// [`UPGRADE_CLASS_MISMATCH`], [`SECOND_APPROVAL_REQUIRED`].
    pub fn run_update_gate(
        &self,
        app: &AppRecord,
        release: Option<&PackageRef>,
        class: Option<&UpgradeClass>,
        run: Option<&str>,
        actor: &str,
        at: u64,
    ) -> Result<Option<AccountId>, Rejection> {
        let alone = BTreeMap::new();
        let Some(r) = self.unit_run(run) else {
            return self.update_gate(app, release, class, &alone, actor, at);
        };
        if !r.targets.contains_key(&app.app_id) {
            let targets = if self.is_unit_instance(run, &app.app_id) {
                &r.targets
            } else {
                &alone
            };
            return self.update_gate(app, release, class, targets, actor, at);
        }
        let moved = r
            .apps
            .first()
            .filter(|root| r.done.contains(root))
            .and_then(|root| self.apps.get(root))
            .filter(|root| root.links.values().any(|m| *m == app.app_id));
        let Some(root) = moved else {
            return self.update_gate(app, release, class, &alone, actor, at);
        };
        self.class_agrees(app, release, class, &alone)?;
        Ok(root
            .last_upgrade
            .as_ref()
            .filter(|u| u.ok && u.run.as_deref() == run)
            .and_then(|u| u.approved_by.clone()))
    }

    /// The live first approval ([`Self::approval_live`]) by someone other than `actor`
    /// that lets `app` move to `package`@`version`: alone (`targets` empty), one of the app
    /// itself or of the unit it is a member of, to its own target; as the instance of a
    /// unit moving its members to `targets`, one of exactly that unit. Every live approval
    /// is looked at, so another unit's that covers the app too (units nest) never hides
    /// the one that matches.
    #[must_use]
    pub fn approval_for(
        &self,
        app: &AppRecord,
        package: &str,
        version: &str,
        targets: &BTreeMap<AppId, RunTarget>,
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
                    && (targets.is_empty() || (**approved == app.app_id && p.targets == *targets))
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
