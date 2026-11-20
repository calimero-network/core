//! Where an app's unit may send data, and whether an update changes it (C5): the unit's
//! releases read for the shared classifier (`calimero_bundle::update`), judged against the
//! cluster's sealed endpoints (`SystemState::sealed_endpoints`, `SetSealedEndpoints`).
//!
//! A unit is an app and the apps it created with it (`AppRecord::links`, transitively;
//! never an `attached` app, which its owner runs apart). Three reads use it:
//!
//! - [`release_unit`]: what an instance of a release would be, before any exists (the
//!   capability card): the release and each member release its `uses` would create.
//! - [`instance_unit`]: what an instance runs now (`GET /apps/{app}/egress`).
//! - [`update`]: an instance moved to another release of its package, alone or with its
//!   members (`GET /apps/{app}/update`, the consent page's upgrade sheet), classified.

use std::collections::BTreeMap;

use calimero_bundle::update::{classify, Classification, Release, Unit};
use serde_json::{json, Value};

use crate::api::apps::{unit_members, MAX_UNIT_DEPTH};
use crate::error::ApiError;
use crate::node::Node;
use crate::system::PackageRef;
use crate::types::AppRecord;

/// A unit with the app each part is: `""` for the instance, a member by its path
/// (`<name>`, a member's own `<name>/<name>`). Empty for [`release_unit`], which has no apps.
pub(crate) struct Placed {
    pub unit: Unit,
    pub apps: BTreeMap<String, String>,
}

/// Where each of an instance's own members moves, by its `uses` name: (package, version).
type Moves = BTreeMap<String, (String, String)>;

/// The cluster's sealed endpoints, sorted.
pub(crate) fn sealed_endpoints(node: &Node) -> Vec<String> {
    node.sys(|s| s.sealed_endpoints.clone())
}

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

/// The unit an instance of release `at` would be: the release, and each release its
/// `uses` (mode `create`) would create, transitively, as `create_instance` picks them
/// (`unit_members`), at most [`MAX_UNIT_DEPTH`] deep.
pub(crate) async fn release_unit(node: &Node, at: &PackageRef) -> Result<Unit, ApiError> {
    let mut unit = Unit::of(release_at(node, at).await?);
    let mut stack = vec![(String::new(), at.clone(), 0_usize)];
    while let Some((path, at, depth)) = stack.pop() {
        if depth >= MAX_UNIT_DEPTH {
            continue;
        }
        for (name, (package, version)) in unit_members(node, &at).await?.members {
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

/// The update of `app` to `version` of the package it runs, classified against the
/// cluster's sealed endpoints: with `whole_unit`, the instance's members move to the
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
        unit_members(node, &target).await?.members
    } else {
        BTreeMap::new()
    };
    let Some(after) = placed(node, app, Some((&target, &moves))).await? else {
        return Ok(None);
    };
    Ok(Some(classify(
        &before.unit,
        &after.unit,
        &sealed_endpoints(node),
    )))
}

/// A unit's egress as the API answers it: `egress` (sorted destinations over the app and
/// its members), `sealed`, and each member by path with its package, version, own egress
/// and (for an instance) app id.
pub(crate) fn describe(placed: &Placed, endpoints: &[String]) -> Value {
    let members: serde_json::Map<String, Value> = placed
        .unit
        .members
        .iter()
        .map(|(path, r)| {
            let mut m = json!({
                "package": r.package,
                "version": r.version,
                "signer": r.signer,
                "egress": r.egress(),
            });
            if let Some(id) = placed.apps.get(path) {
                m["app"] = json!(id);
            }
            (path.clone(), m)
        })
        .collect();
    json!({
        "egress": placed.unit.egress(),
        "sealed": placed.unit.sealed(endpoints),
        "members": members,
    })
}
