//! System commands for bundles, package releases, publishers and package origins.
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
        SystemCmd::BindPublisher {
            signer_id,
            org,
            actor,
            consent,
            ..
        } => {
            // Over this key's binding as it stands when the command applies:
            // a consent for an earlier generation binds nothing again.
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let generation = s.binding_generation(signer_id);
            proofs.states(&st, &format!("org:{org}"), "__bind_publisher", &|a| {
                a["signerId"] == json!(signer_id) && a["generation"] == json!(generation)
            })?;
            Ok(Some(used))
        }
        SystemCmd::AddPackageOrigin {
            package,
            origin,
            rp_id,
            served,
            actor,
            consent,
            ..
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            proofs.states(&st, &format!("package:{package}"), "__add_origin", &|a| {
                *a == crate::origins::add_args(origin, rp_id, *served)
            })?;
            Ok(Some(used))
        }
        SystemCmd::RemovePackageOrigin {
            package,
            origin,
            actor,
            consent,
        } => {
            let (st, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            proofs.states(
                &st,
                &format!("package:{package}"),
                "__remove_origin",
                &|a| *a == json!({ "origin": origin }),
            )?;
            Ok(Some(used))
        }
        _ => Ok(None),
    }
}

/// Apply a command of this area by its own rules, its proof already
/// judged ([`SystemMachine::run_authorized`]).
#[allow(clippy::too_many_lines, reason = "one arm per command")]
pub(super) fn apply(m: &mut SystemMachine, cmd: SystemCmd) -> Result<Value, &'static str> {
    let here = m.cluster.as_str();
    let s = &mut m.state;
    match cmd {
        SystemCmd::PutBundle {
            hash,
            size,
            actor,
            at,
        } => {
            if !s.accounts.contains_key(&actor) {
                return Err("unknown account");
            }
            if !s.may_deploy(&actor, at) {
                return Err(NOT_A_DEPLOYER);
            }
            if !s.bundles.contains_key(&hash) {
                let used: u64 = s
                    .bundles
                    .values()
                    .filter(|b| b.uploaded_by == actor)
                    .map(|b| b.size)
                    .sum();
                if used + size > s.quotas.max_bundle_bytes_per_account {
                    return Err("quota exceeded: bundle storage");
                }
            }
            let _prev = s.bundles.entry(hash.clone()).or_insert(BundleRecord {
                hash: hash.clone(),
                size,
                uploaded_by: actor,
                package: None,
            });
            Ok(json!({ "hash": hash }))
        }
        SystemCmd::InstallPackage {
            package,
            signer_id,
            release,
        } => {
            let actor = release.installed_by.clone();
            let by_operator = actor == OPERATOR;
            if !by_operator && !s.accounts.contains_key(&actor) {
                return Err("unknown account");
            }
            // Installing a release again is an install too: it makes the
            // release the installer's to use.
            if !s.may_deploy(&actor, release.installed_at) {
                return Err(NOT_A_DEPLOYER);
            }
            if let Some(p) = s.packages.get(&package) {
                if p.signer_id != signer_id {
                    return Err("the package is published under another key");
                }
                if let Some(v) = p.versions.get(&release.version) {
                    // Other bytes under its version are a different,
                    // conflicting release. The same release again changes
                    // nothing but who installed it: it is theirs too.
                    if v.mpk != release.mpk {
                        return Err("that version is installed with other contents");
                    }
                    if !v.installed_by_any(&actor) {
                        if let Some(v) = s
                            .packages
                            .get_mut(&package)
                            .and_then(|p| p.versions.get_mut(&release.version))
                        {
                            let _new = v.installers.insert(actor);
                        }
                    }
                    return Ok(json!({ "package": package, "version": release.version }));
                }
            }
            if s.attributes_by_release && s.ships_in_other_package(&package, &signer_id, &release) {
                return Err(SHIPS_IN_OTHER_PACKAGE);
            }
            if s.packages
                .get(&package)
                .is_some_and(|p| !p.upgrades_in_order(&release.version, release.upgrades))
            {
                return Err(
                        "a release may not give its package's upgrades back to the publisher once a lower one gave them to owners",
                    );
            }
            let used: u64 = s
                .bundles
                .values()
                .filter(|b| b.uploaded_by == actor)
                .map(|b| b.size)
                .sum();
            let added = if release.part_sizes.is_empty() {
                release.mpk_size
            } else {
                release
                    .parts(&package)
                    .filter(|(hash, _)| !s.bundles.contains_key(*hash))
                    .map(|(hash, _)| release.part_size(hash))
                    .sum()
            };
            // The operator's installs count against no account's quota.
            if !by_operator && used + added > s.quotas.max_bundle_bytes_per_account {
                return Err("quota exceeded: bundle storage");
            }
            for (hash, at) in release.parts(&package) {
                let b = s.bundles.entry(hash.to_owned()).or_insert(BundleRecord {
                    hash: hash.to_owned(),
                    size: release.part_size(hash),
                    uploaded_by: actor.clone(),
                    package: None,
                });
                // A raw upload of the same WASM gains the release it ships in.
                if b.package.is_none() {
                    b.package = Some(at);
                }
            }
            let version = release.version.clone();
            let _prev = s
                .packages
                .entry(package.clone())
                .or_insert_with(|| PackageRecord {
                    name: package.clone(),
                    signer_id,
                    owner: actor,
                    versions: BTreeMap::new(),
                })
                .versions
                .insert(version.clone(), release);
            Ok(json!({ "package": package, "version": version }))
        }
        SystemCmd::UninstallPackage {
            package,
            version,
            actor,
        } => {
            let p = s.packages.get(&package).ok_or("no such package")?;
            if p.owner != actor {
                return Err("only whoever installed the package (an account, or the operator) may uninstall it");
            }
            let gone: Vec<String> = match &version {
                Some(v) if !p.versions.contains_key(v) => return Err("no such version"),
                Some(v) => vec![v.clone()],
                None => p.versions.keys().cloned().collect(),
            };
            // An offered release is the deployments' to keep: whoever
            // installed it first does not take it from them.
            if gone.iter().any(|v| s.offered(&package, v)) {
                return Err("a deployment offers that release; it is uninstalled once no deployment offers it");
            }
            let hashes: BTreeSet<String> = gone
                .iter()
                .filter_map(|v| p.versions.get(v))
                .flat_map(|v| v.parts(&package).map(|(h, _)| h.to_owned()))
                .collect();
            // Two releases may ship the same WASM: a hash another release
            // still provides stays, recorded under that release.
            let provider = |s: &SystemState, h: &str| -> Option<PackageRef> {
                s.packages.iter().find_map(|(name, p)| {
                    p.versions.iter().find_map(|(v, r)| {
                        let gone_here = *name == package && gone.contains(v);
                        (!gone_here)
                            .then(|| r.parts(name).find(|(x, _)| *x == h))
                            .flatten()
                            .map(|(_, at)| at)
                    })
                })
            };
            let mut keep = BTreeMap::new();
            for h in &hashes {
                if let Some(r) = provider(s, h) {
                    let _prev = keep.insert(h.clone(), r);
                }
            }
            let in_use = |h: &String| hashes.contains(h) && !keep.contains_key(h);
            if s.apps
                .values()
                .any(|a| in_use(&a.bundle) || a.compute_bundle.as_ref().is_some_and(in_use))
            {
                return Err("an app still runs that release; delete or upgrade it first");
            }
            for h in &hashes {
                match keep.remove(h) {
                    Some(r) => {
                        if let Some(b) = s.bundles.get_mut(h) {
                            b.package = Some(r);
                        }
                    }
                    None => {
                        let _prev = s.bundles.remove(h);
                    }
                }
            }
            let p = s.packages.get_mut(&package).ok_or("no such package")?;
            for v in &gone {
                let _prev = p.versions.remove(v);
            }
            if p.versions.is_empty() {
                let _prev = s.packages.remove(&package);
                s.forget_origins(&package);
            }
            Ok(json!({ "removed": gone }))
        }
        SystemCmd::BindPublisher {
            signer_id,
            org,
            cluster,
            proof,
            actor,
            consent: _,
        } => {
            if signer_id.len() > MAX_BINDING_FIELD_BYTES || cluster.len() > MAX_BINDING_FIELD_BYTES
            {
                return Err("a signer id or cluster name is at most 128 bytes");
            }
            // The signature is checked over this cluster's own name: one
            // the key made for another cluster binds nothing here.
            if cluster != here {
                return Err("the binding names another cluster");
            }
            if !s.holds_approve_releases(&org, &actor) {
                return Err("binding a publisher key needs APPROVE_RELEASES in the organization");
            }
            let generation = s.binding_generation(&signer_id);
            let message = publisher_binding_message(here, &org, &signer_id, generation);
            if !proof.proves(&signer_id, &message) {
                return Err("the proof is not that key's signature over this binding");
            }
            let _was = s.unbound_publishers.remove(&signer_id);
            let _prev = s.publishers.insert(
                signer_id,
                PublisherBinding {
                    org,
                    generation: generation + 1,
                },
            );
            Ok(json!({ "generation": generation + 1 }))
        }
        SystemCmd::AddPackageOrigin {
            package,
            origin,
            rp_id,
            served,
            actor,
            at,
            consent: _,
        } => s.add_package_origin(package, origin, &rp_id, served, actor, at),
        SystemCmd::RemovePackageOrigin {
            package,
            origin,
            actor,
            consent: _,
        } => s.remove_package_origin(&package, &origin, &actor),
        // SAFETY: `SystemCmd::area` sends a command only to the module of
        // its area, and this match has an arm for each command of it.
        _ => unreachable!("a command of another area reached packages::apply"),
    }
}
