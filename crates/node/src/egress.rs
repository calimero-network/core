//! Where an app's unit may send data, and whether an update changes it: the unit's releases
//! read for the shared classifier (`calimero_bundle::update`). Only destinations
//! are reported: every `outbound` prefix and secret host of the unit, an empty list meaning
//! no data leaves Calimero. The platform never vouches for an external endpoint, a TEE
//! included; an app that wants to trust one verifies it in its own code.
//!
//! A unit is an app and the apps it created with it (`AppRecord::links`, transitively;
//! never an `attached` app, which its owner runs apart). Three reads use it:
//!
//! - [`release_unit`]: what an instance of a release would be, before any exists (the
//!   capability card): the release and each member release its `uses` would create.
//! - [`instance_unit`]: what an instance runs now (`GET /apps/{app}/egress`).
//! - [`update`]: an instance moved to another release of its package, alone or with its
//!   members, classified; [`judged_update`] is the verdict shown and logged (`GET
//!   /apps/{app}/update`, the consent page's upgrade sheets, the upgrade's class), red and
//!   `unclassified` where the log lacks a part's release.

use std::collections::BTreeMap;

use calimero_bundle::update::{classify, Class, Classification, EgressDiff, Reason, Release, Unit};
use serde_json::{json, Value};

use crate::api::apps::{unit_members, MAX_UNIT_DEPTH};
use crate::error::ApiError;
use crate::node::Node;
use crate::system::PackageRef;
use crate::types::AppRecord;

/// What the capability card says of an instance whose egress is off
/// (`SystemState::egress_off`): whatever its releases declare, it sends nothing out.
pub(crate) const NO_EGRESS: &str =
    "This app cannot send data outside the cluster: its egress is off.";

/// A unit with the app each part is: `""` for the instance, a member by its path
/// (`<name>`, a member's own `<name>/<name>`). Empty for [`release_unit`], which has no apps.
pub(crate) struct Placed {
    pub unit: Unit,
    pub apps: BTreeMap<String, String>,
}

/// Where each of an instance's own members moves, by its `uses` name: (package, version).
type Moves = BTreeMap<String, (String, String)>;

/// The classifier's facts for the installed release `at`.
async fn release_at(node: &Node, at: &PackageRef) -> Result<Release, ApiError> {
    let version = node
        .sys(|s| {
            s.packages
                .get(&at.package)?
                .versions
                .get(&at.version)
                .cloned()
        })
        .ok_or_else(|| {
            ApiError::not_found(&format!(
                "{}@{} is not installed here",
                at.package, at.version
            ))
        })?;
    crate::consent_page::classifier_release(node, &at.package, &version, at.service.as_deref())
        .await
}

fn joined(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_owned()
    } else {
        format!("{path}/{name}")
    }
}

/// The unit an instance of release `at` would be, created by `creator` (from `deployment`):
/// the release, and each release its `uses` (mode `create`) would create, transitively, as
/// `create_instance` picks them (`unit_members`), at most [`MAX_UNIT_DEPTH`] deep.
pub(crate) async fn release_unit(
    node: &Node,
    at: &PackageRef,
    (creator, deployment): (&str, Option<&str>),
) -> Result<Unit, ApiError> {
    let mut unit = Unit::of(release_at(node, at).await?);
    let mut stack = vec![(String::new(), at.clone(), 0_usize)];
    while let Some((path, at, depth)) = stack.pop() {
        if depth >= MAX_UNIT_DEPTH {
            continue;
        }
        for (name, (package, version)) in
            unit_members(node, &at, creator, deployment).await?.members
        {
            let member = PackageRef {
                package,
                version,
                service: None,
                compute: false,
            };
            let path = joined(&path, &name);
            let _prev = unit
                .members
                .insert(path.clone(), release_at(node, &member).await?);
            stack.push((path, member, depth + 1));
        }
    }
    Ok(unit)
}

/// The unit `app` runs now: its release and each member's (`links`, transitively), each
/// with the release its record names. `None` when the instance runs no package release (a
/// development bundle, which declares no egress); a member that runs none is left out.
pub(crate) async fn instance_unit(
    node: &Node,
    app: &AppRecord,
) -> Result<Option<Placed>, ApiError> {
    placed(node, app, None).await
}

/// `app` and its members, each at the release it runs, or the root at `target` and, when
/// `moves` names them, its direct members at the releases given there (as `unit_plan`
/// moves a unit: only the instance's own members, never their members).
async fn placed(
    node: &Node,
    app: &AppRecord,
    target: Option<(&PackageRef, &Moves)>,
) -> Result<Option<Placed>, ApiError> {
    let Some(running) = node.release_of_app(app) else {
        return Ok(None);
    };
    let (root_at, moves) = match target {
        Some((at, moves)) => (at.clone(), moves.clone()),
        None => (running.at, BTreeMap::new()),
    };
    let mut out = Placed {
        unit: Unit::of(release_at(node, &root_at).await?),
        apps: BTreeMap::from([(String::new(), app.app_id.clone())]),
    };
    let mut stack = vec![(String::new(), app.clone(), 0_usize)];
    while let Some((path, rec, depth)) = stack.pop() {
        if depth >= MAX_UNIT_DEPTH {
            continue;
        }
        for (name, id) in &rec.links {
            let Some(member) = node.sys(|s| s.apps.get(id).cloned()) else {
                continue;
            };
            let Some(runs) = node.release_of_app(&member) else {
                continue;
            };
            let at = match moves.get(name).filter(|_| path.is_empty()) {
                Some((package, version)) if *package == runs.at.package => PackageRef {
                    package: package.clone(),
                    version: version.clone(),
                    service: runs.at.service.clone(),
                    compute: false,
                },
                _ => runs.at,
            };
            let path = joined(&path, name);
            let _prev = out
                .unit
                .members
                .insert(path.clone(), release_at(node, &at).await?);
            let _prev = out.apps.insert(path.clone(), member.app_id.clone());
            stack.push((path, member, depth + 1));
        }
    }
    // A member the target release would create that the instance lacks: shown as joining
    // (`unit_plan` refuses such an upgrade; a unit gains members only when created).
    for (name, (package, version)) in &moves {
        if out.unit.members.contains_key(name) {
            continue;
        }
        let at = PackageRef {
            package: package.clone(),
            version: version.clone(),
            service: None,
            compute: false,
        };
        let _prev = out
            .unit
            .members
            .insert(name.clone(), release_at(node, &at).await?);
    }
    Ok(Some(out))
}

/// The update of `app` to `version` of the package it runs, classified: with `whole_unit`, the instance's members move to the
/// releases the target's `uses` pick for them (`POST /apps/{app}/upgrade-unit`), else they
/// stay where they are (`POST /apps/{app}/upgrade`). `None` when the instance runs no
/// package release.
pub(crate) async fn update(
    node: &Node,
    app: &AppRecord,
    version: &str,
    whole_unit: bool,
) -> Result<Option<Classification>, ApiError> {
    let Some(before) = instance_unit(node, app).await? else {
        return Ok(None);
    };
    let Some(running) = node.release_of_app(app) else {
        return Ok(None);
    };
    let target = PackageRef {
        package: running.at.package.clone(),
        version: version.to_owned(),
        service: running.at.service.clone(),
        compute: false,
    };
    let moves = if whole_unit {
        unit_members(node, &target, &app.owner, app.deployment.as_deref())
            .await?
            .members
    } else {
        BTreeMap::new()
    };
    let Some(after) = placed(node, app, Some((&target, &moves))).await? else {
        return Ok(None);
    };
    Ok(Some(classify(&before.unit, &after.unit)))
}

/// [`update`] as every place that shows or judges an update takes it (`GET
/// /apps/{app}/update`, the consent page's sheets and approval step, `approve_upgrade`,
/// the class an upgrade logs): where the log cannot classify the move, a part's release
/// no longer installed ([`crate::system::SystemState::unclassified`]), red with those
/// `unclassified` reasons and no destinations, as apply judges it
/// (`SystemState::check_class`). [`update`] alone leaves out a part it cannot read, so
/// a member whose release is gone would not stop it showing green.
pub(crate) async fn judged_update(
    node: &Node,
    app: &AppRecord,
    version: &str,
    whole_unit: bool,
) -> Result<Option<Classification>, ApiError> {
    let unclassified = node.sys(|s| {
        let running = s.running(app)?;
        s.unclassified(app, (&running.package, version), &BTreeMap::new())
    });
    if let Some(c) = unclassified {
        return Ok(Some(Classification {
            class: c.class,
            reasons: c.reasons,
            egress: EgressDiff::default(),
            state_version_changed: false,
            new_methods: Vec::new(),
            removed_methods: Vec::new(),
            signer_changed: false,
        }));
    }
    update(node, app, version, whole_unit).await
}

/// Whether an update on its own may take this move, as far as the verdict goes: green and
/// removing no method, since another app may call one (the policy and destinations are the gate's).
pub(crate) fn moves_on_its_own(c: &Classification) -> bool {
    c.class == Class::Green && c.removed_methods.is_empty()
}

/// Whether `c` could not be classified ([`judged_update`]): its destinations are unknown,
/// not none.
pub(crate) fn unclassified(c: &Classification) -> bool {
    c.reasons
        .iter()
        .any(|r| matches!(r, Reason::Unclassified { .. }))
}

/// A verdict as the API and the consent page's model answer it: the [`Classification`]'s
/// fields, with every fact that compares the two sides (`egress`, `stateVersionChanged`,
/// `newMethods`, `removedMethods`, `signerChanged`) `null` when it is [`unclassified`]:
/// unknown, not unchanged.
pub(crate) fn verdict_json(c: &Classification) -> Value {
    let mut v = json!(c);
    if unclassified(c) {
        for fact in [
            "egress",
            "stateVersionChanged",
            "newMethods",
            "removedMethods",
            "signerChanged",
        ] {
            v[fact] = Value::Null;
        }
    }
    v
}

/// A unit's egress as the API answers it: `egress` (sorted destinations over the app and
/// its members; empty: no data leaves Calimero) as `destinations`, and each member by path with
/// its package, version, own destinations
/// and (for an instance) app id.
pub(crate) fn describe(placed: &Placed) -> Value {
    let members: serde_json::Map<String, Value> = placed
        .unit
        .members
        .iter()
        .map(|(path, r)| {
            let mut m = json!({
                "package": r.package,
                "version": r.version,
                "signer": r.signer,
                "destinations": r.egress(),
            });
            if let Some(id) = placed.apps.get(path) {
                m["app"] = json!(id);
            }
            (path.clone(), m)
        })
        .collect();
    json!({
        "destinations": placed.unit.egress(),
        "members": members,
    })
}
