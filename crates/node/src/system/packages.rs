//! System commands for bundles, package releases, publishers and package origins.
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

/// A release whose pipelines pin a module release that is gone.
const MODULE_GONE: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "a module the pipelines use is no longer installed",
);
/// A release of a package signed by another key than the one pinned first.
const OTHER_PUBLISHER_KEY: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "the package is published under another key",
);
/// A release whose version is installed with other bytes.
const OTHER_CONTENTS: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "that version is installed with other contents",
);
/// A release that gives upgrades back to the publisher after a lower one gave
/// them to owners.
const UPGRADES_OUT_OF_ORDER: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "a release may not give its package's upgrades back to the publisher once a lower one gave them to owners",
);
/// An uninstall by someone other than whoever installed the package.
const ONLY_INSTALLER_UNINSTALLS: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "only whoever installed the package (an account, or the operator) may uninstall it",
);
/// An uninstall of a version the package does not have.
const NO_SUCH_VERSION: Rejection = Rejection::new(RejectionKind::Rejected, "no such version");
/// An uninstall of a release a deployment offers.
const OFFERED_RELEASE: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "a deployment offers that release; it is uninstalled once no deployment offers it",
);
/// An uninstall of a release an app still runs.
const RELEASE_RUNS: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "an app still runs that release; delete or upgrade it first",
);
/// A binding whose signer id or cluster name passes [`MAX_BINDING_FIELD_BYTES`].
const BINDING_FIELD_TOO_LONG: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "a signer id or cluster name is at most 128 bytes",
);
/// A binding signed for another cluster.
const OTHER_CLUSTER: Rejection =
    Rejection::new(RejectionKind::Rejected, "the binding names another cluster");
/// A binding by someone without `APPROVE_RELEASES` in the organization.
const NEEDS_APPROVE_RELEASES: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "binding a publisher key needs APPROVE_RELEASES in the organization",
);
/// A binding whose proof is not the key's signature over it.
const BAD_BINDING_PROOF: Rejection = Rejection::new(
    RejectionKind::Rejected,
    "the proof is not that key's signature over this binding",
);

/// Judge the proof a command of this area carries ([`super::authorize`]).
pub(super) fn authorize(proofs: &Proofs<'_>, cmd: &SystemCmd) -> Result<Option<String>, Rejection> {
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
            let (statement, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
            let generation = proofs.s.binding_generation(signer_id);
            proofs.states(
                &statement,
                &format!("org:{org}"),
                "__bind_publisher",
                &|args| {
                    args["signerId"] == json!(signer_id) && args["generation"] == json!(generation)
                },
            )?;
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
        } => package_consent(
            proofs,
            actor,
            consent,
            package,
            "__add_origin",
            &crate::origins::add_args(origin, rp_id, *served),
        ),
        SystemCmd::RemovePackageOrigin {
            package,
            origin,
            actor,
            consent,
        } => package_consent(
            proofs,
            actor,
            consent,
            package,
            "__remove_origin",
            &json!({ "origin": origin }),
        ),
        _ => Ok(None),
    }
}

/// `actor`'s passkey consent, not used before, stating `method` on the
/// package (`package:<package>`) with exactly the arguments `stated`.
fn package_consent(
    proofs: &Proofs<'_>,
    actor: &str,
    consent: &Value,
    package: &str,
    method: &str,
    stated: &Value,
) -> Result<Option<String>, Rejection> {
    let (statement, used) = proofs.fresh(proofs.consent_of(actor, consent)?)?;
    proofs.states(&statement, &format!("package:{package}"), method, &|args| {
        args == stated
    })?;
    Ok(Some(used))
}

/// Apply a command of this area by its own rules, its proof already
/// judged ([`SystemMachine::run_authorized`]).
pub(super) fn apply(m: &mut SystemMachine, cmd: SystemCmd) -> Result<Value, Rejection> {
    let here = m.cluster.as_str();
    let state = &mut m.state;
    match cmd {
        SystemCmd::PutBundle {
            hash,
            size,
            actor,
            at,
        } => put_bundle(state, hash, size, actor, at),
        SystemCmd::InstallPackage {
            package,
            signer_id,
            release,
        } => install_package(state, package, signer_id, release),
        SystemCmd::UninstallPackage {
            package,
            version,
            actor,
        } => uninstall_package(state, &package, version.as_deref(), &actor),
        cmd @ SystemCmd::BindPublisher { .. } => bind_publisher(state, here, cmd),
        SystemCmd::AddPackageOrigin {
            package,
            origin,
            rp_id,
            served,
            actor,
            at,
            ..
        } => state.add_package_origin(package, origin, &rp_id, served, actor, at),
        SystemCmd::RemovePackageOrigin {
            package,
            origin,
            actor,
            ..
        } => state.remove_package_origin(&package, &origin, &actor),
        // SAFETY: `SystemCmd::area` sends a command only to the module of
        // its area, and this match has an arm for each command of it.
        _ => unreachable!("a command of another area reached packages::apply"),
    }
}

/// Record an uploaded bundle: a deployer's, within its account's bundle bytes
/// (a bundle already recorded counts nothing again).
fn put_bundle(
    state: &mut SystemState,
    hash: String,
    size: u64,
    actor: AccountId,
    at: u64,
) -> Result<Value, Rejection> {
    if !state.accounts.contains_key(&actor) {
        return Err(UNKNOWN_ACCOUNT);
    }
    if !state.may_deploy(&actor, at) {
        return Err(NOT_A_DEPLOYER);
    }
    if !state.bundles.contains_key(&hash)
        && bundle_bytes_of(state, &actor) + size > state.quotas.max_bundle_bytes_per_account
    {
        return Err(Rejection::quota("bundle storage"));
    }
    let _prev = state.bundles.entry(hash.clone()).or_insert(BundleRecord {
        hash: hash.clone(),
        size,
        uploaded_by: actor,
        package: None,
    });
    Ok(json!({ "hash": hash }))
}

/// The bundle bytes an account uploaded.
fn bundle_bytes_of(state: &SystemState, account: &str) -> u64 {
    state
        .bundles
        .values()
        .filter(|bundle| bundle.uploaded_by == account)
        .map(|bundle| bundle.size)
        .sum()
}

/// Install a package release: a known account or the operator, a deployer,
/// every pinned module still there, the package's pinned key. A release
/// installed again changes only who installed it and what the record lacked.
fn install_package(
    state: &mut SystemState,
    package: String,
    signer_id: String,
    release: PackageVersion,
) -> Result<Value, Rejection> {
    let actor = release.installed_by.clone();
    let by_operator = actor == OPERATOR;
    if !by_operator && !state.accounts.contains_key(&actor) {
        return Err(UNKNOWN_ACCOUNT);
    }
    // Installing a release again is an install too: it makes the release the
    // installer's to use.
    if !state.may_deploy(&actor, release.installed_at) {
        return Err(NOT_A_DEPLOYER);
    }
    if !state.modules_installed(&release) {
        return Err(MODULE_GONE);
    }
    if let Some(record) = state.packages.get(&package) {
        if record.signer_id != signer_id {
            return Err(OTHER_PUBLISHER_KEY);
        }
        if record.versions.contains_key(&release.version) {
            return reinstall(state, package, actor, release);
        }
    }
    if state.attributes_by_release && state.ships_in_other_package(&package, &signer_id, &release) {
        return Err(SHIPS_IN_OTHER_PACKAGE.into());
    }
    if state
        .packages
        .get(&package)
        .is_some_and(|record| !record.upgrades_in_order(&release.version, release.upgrades))
    {
        return Err(UPGRADES_OUT_OF_ORDER);
    }
    // The operator's installs count against no account's quota.
    if !by_operator
        && bundle_bytes_of(state, &actor) + new_part_bytes(state, &package, &release)
            > state.quotas.max_bundle_bytes_per_account
    {
        return Err(Rejection::quota("bundle storage"));
    }
    for (hash, at) in release.parts(&package) {
        let bundle = state
            .bundles
            .entry(hash.to_owned())
            .or_insert(BundleRecord {
                hash: hash.to_owned(),
                size: release.part_size(hash),
                uploaded_by: actor.clone(),
                package: None,
            });
        // A raw upload of the same WASM gains the release it ships in.
        if bundle.package.is_none() {
            bundle.package = Some(at);
        }
    }
    let version = release.version.clone();
    let _prev = state
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

/// The bundle bytes a new release adds: each part not stored yet at its own
/// size (a release from before part sizes counts its whole `.mpk`).
fn new_part_bytes(state: &SystemState, package: &str, release: &PackageVersion) -> u64 {
    if release.part_sizes.is_empty() {
        return release.mpk_size;
    }
    release
        .parts(package)
        .filter(|(hash, _)| !state.bundles.contains_key(*hash))
        .map(|(hash, _)| release.part_size(hash))
        .sum()
}

/// A release installed again: other bytes under its version are a different,
/// conflicting release. The same release again changes nothing but who
/// installed it (it is theirs too), and what the record had not kept yet:
/// its `uses` (`Feature::DeploymentUsedSecrets`), its instance mode
/// (`Feature::OrdinaryOrgs`) and whether it takes guests (`Feature::Guests`).
fn reinstall(
    state: &mut SystemState,
    package: String,
    actor: AccountId,
    release: PackageVersion,
) -> Result<Value, Rejection> {
    let Some(recorded) = state
        .packages
        .get_mut(&package)
        .and_then(|record| record.versions.get_mut(&release.version))
    else {
        // SAFETY: `install_package` sends only a release whose version the
        // package records here.
        unreachable!("reinstall of a release the package does not record")
    };
    if recorded.mpk != release.mpk {
        return Err(OTHER_CONTENTS);
    }
    if !recorded.installed_by_any(&actor) {
        let _new = recorded.installers.insert(actor);
    }
    if recorded.uses.is_empty() && !release.uses.is_empty() {
        recorded.uses = release.uses;
    }
    if recorded.instance_org.is_none() && release.instance_org.is_some() {
        recorded.instance_org = release.instance_org;
    }
    if recorded.guests.is_none() && release.guests.is_some() {
        recorded.guests = release.guests;
    }
    Ok(json!({ "package": package, "version": release.version }))
}

/// Uninstall a package's version, or every version: whoever installed it,
/// never a release a deployment offers, a pipeline pins or an app runs. A
/// part another release still ships stays, recorded under that release.
fn uninstall_package(
    state: &mut SystemState,
    package: &str,
    version: Option<&str>,
    actor: &str,
) -> Result<Value, Rejection> {
    let record = state.packages.get(package).ok_or(NO_SUCH_PACKAGE)?;
    if record.owner != actor {
        return Err(ONLY_INSTALLER_UNINSTALLS);
    }
    let gone: Vec<String> = match version {
        Some(version) if !record.versions.contains_key(version) => return Err(NO_SUCH_VERSION),
        Some(version) => vec![version.to_owned()],
        None => record.versions.keys().cloned().collect(),
    };
    // An offered release is the deployments' to keep: whoever installed it
    // first does not take it from them.
    if gone.iter().any(|version| state.offered(package, version)) {
        return Err(OFFERED_RELEASE);
    }
    if gone
        .iter()
        .any(|version| state.module_pinned(package, version))
    {
        return Err(MODULE_IN_USE);
    }
    let hashes: BTreeSet<String> = gone
        .iter()
        .filter_map(|version| record.versions.get(version))
        .flat_map(|release| release.parts(package).map(|(hash, _)| hash.to_owned()))
        .collect();
    let mut keep = BTreeMap::new();
    for hash in &hashes {
        if let Some(provider) = other_provider(state, package, &gone, hash) {
            let _prev = keep.insert(hash.clone(), provider);
        }
    }
    let in_use = |hash: &String| hashes.contains(hash) && !keep.contains_key(hash);
    if state
        .apps
        .values()
        .any(|app| in_use(&app.bundle) || app.compute_bundle.as_ref().is_some_and(in_use))
    {
        return Err(RELEASE_RUNS);
    }
    for hash in &hashes {
        match keep.remove(hash) {
            Some(provider) => {
                if let Some(bundle) = state.bundles.get_mut(hash) {
                    bundle.package = Some(provider);
                }
            }
            None => {
                let _prev = state.bundles.remove(hash);
            }
        }
    }
    let record = state.packages.get_mut(package).ok_or(NO_SUCH_PACKAGE)?;
    for version in &gone {
        let _prev = record.versions.remove(version);
    }
    if record.versions.is_empty() {
        let _prev = state.packages.remove(package);
        state.forget_origins(package);
    }
    Ok(json!({ "removed": gone }))
}

/// A release outside the ones being removed that ships `hash`: two releases
/// may ship the same WASM.
fn other_provider(
    state: &SystemState,
    package: &str,
    gone: &[String],
    hash: &str,
) -> Option<PackageRef> {
    state.packages.iter().find_map(|(name, record)| {
        record.versions.iter().find_map(|(version, release)| {
            let gone_here = *name == package && gone.contains(version);
            (!gone_here)
                .then(|| release.parts(name).find(|(part, _)| *part == hash))
                .flatten()
                .map(|(_, at)| at)
        })
    })
}

/// Bind a publisher key to an organization: fields within bounds, signed for
/// this cluster (`here`, never the command's own claim), by an
/// `APPROVE_RELEASES` holder, the key's signature over this generation.
fn bind_publisher(state: &mut SystemState, here: &str, cmd: SystemCmd) -> Result<Value, Rejection> {
    let SystemCmd::BindPublisher {
        signer_id,
        org,
        cluster,
        proof,
        actor,
        ..
    } = cmd
    else {
        // SAFETY: `apply` sends only a `BindPublisher` here.
        unreachable!("bind_publisher applies a BindPublisher")
    };
    if signer_id.len() > MAX_BINDING_FIELD_BYTES || cluster.len() > MAX_BINDING_FIELD_BYTES {
        return Err(BINDING_FIELD_TOO_LONG);
    }
    // The signature is checked over this cluster's own name: one the key made
    // for another cluster binds nothing here.
    if cluster != here {
        return Err(OTHER_CLUSTER);
    }
    if !state.holds_approve_releases(&org, &actor) {
        return Err(NEEDS_APPROVE_RELEASES);
    }
    let generation = state.binding_generation(&signer_id);
    let message = publisher_binding_message(here, &org, &signer_id, generation);
    if !proof.proves(&signer_id, &message) {
        return Err(BAD_BINDING_PROOF);
    }
    let _was = state.unbound_publishers.remove(&signer_id);
    let _prev = state.publishers.insert(
        signer_id,
        PublisherBinding {
            org,
            generation: generation + 1,
        },
    );
    Ok(json!({ "generation": generation + 1 }))
}
