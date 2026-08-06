//! `cargo mero build`: take the ABI manifest the app itself builds, compile it
//! to wasm32, copy it into `res/`, size-optimize it (release only), and embed
//! the full `res/abi.json` as the `calimero_abi_v1` section so the node can read
//! the state schema and per-method flags off the bytecode.
//!
//! The ABI step lives here rather than in each app's `build.rs` so one
//! implementation owns it; an app needs no build script to carry an ABI - see
//! [`emit_abi`].

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

use calimero_wasm_abi::Manifest;
use camino::{Utf8Path, Utf8PathBuf};
use eyre::{bail, eyre, Context, Result};
use wasm_opt::{Feature, OptimizationOptions};

use crate::{meta, workspace, BuildArgs};

const TARGET: &str = "wasm32-unknown-unknown";

/// One built artifact: the crate, its optimized ABI-embedded wasm in `res/`, and
/// the `abi.json` beside it, which `bundle` ships as a sidecar. `abi_json` is
/// `None` for an app whose SDK generates no ABI entry point.
#[derive(Debug, Clone)]
pub struct BuiltWasm {
    pub crate_name: String,
    pub wasm: PathBuf,
    pub abi_json: Option<PathBuf>,
}

/// A crate the pipeline should build.
struct Target {
    crate_name: String,
    crate_dir: Utf8PathBuf,
}

/// CLI `cargo mero build`: honors `-p`/`--manifest-path` target selection.
pub fn run(args: &BuildArgs) -> Result<Vec<BuiltWasm>> {
    run_inner(args, false)
}

/// Build every declared service, ignoring `-p`/`--manifest-path`: `bundle`
/// stages one entry per service, so a filtered build would drop one silently.
pub fn run_all(args: &BuildArgs) -> Result<Vec<BuiltWasm>> {
    run_inner(args, true)
}

fn run_inner(args: &BuildArgs, all_services: bool) -> Result<Vec<BuiltWasm>> {
    let metadata = workspace::metadata_for(args.manifest_path.as_deref(), &args.features)?;

    let profiling =
        args.profiling || matches!(std::env::var("WASM_PROFILING").as_deref(), Ok("true"));
    let profile = if profiling {
        "app-profiling"
    } else {
        "app-release"
    };

    let targets = resolve_targets(&metadata, args, all_services)?;
    ensure_wasm_target()?;
    ensure_profile(&metadata.workspace_root, profile)?;

    cargo_build(&targets, profile, args)?;

    let mut built = Vec::with_capacity(targets.len());
    for target in targets {
        built.push(build_one(
            &metadata,
            &target,
            profile,
            !profiling,
            args.no_abi,
        )?);
    }
    Ok(built)
}

/// The crates to build: every declared service under `all_services`, otherwise
/// the selection in `select_service_builds`, else the single resolved package.
fn resolve_targets(
    metadata: &cargo_metadata::Metadata,
    args: &BuildArgs,
    all_services: bool,
) -> Result<Vec<Target>> {
    let manifest_dir = args.manifest_path.as_deref().map(workspace::manifest_dir);

    // Fall back to the workspace root so `cd workspace && cargo mero bundle`
    // still finds services. A single-crate app declares no table at all, so a
    // missing one means "no services", not an error.
    let discovery_dir = manifest_dir
        .clone()
        .unwrap_or_else(|| metadata.workspace_root.clone());
    let services = match meta::load(metadata, &discovery_dir) {
        Ok(m) => m.services,
        Err(e) if is_missing_table(&e) => Vec::new(),
        Err(e) => return Err(e),
    };

    if !services.is_empty() {
        let service_crates: Vec<String> = services.iter().map(|s| s.crate_name.clone()).collect();
        let selected = if all_services {
            service_crates
        } else {
            // A --manifest-path that names the workspace root (or is absent)
            // builds all services; one that names a member builds just it.
            let root_manifest = manifest_dir.as_ref().is_none_or(|dir| {
                workspace::same_dir(&metadata.workspace_root.join("Cargo.toml"), dir)
            });
            let matched_member = manifest_dir
                .as_deref()
                .and_then(|dir| workspace::package_in_dir(metadata, dir))
                .map(|p| p.name.to_string());
            select_service_builds(
                &service_crates,
                args.package.as_deref(),
                matched_member.as_deref(),
                root_manifest,
            )?
        };
        // Two service names may point at the same crate (e.g. one wasm
        // installed under two names): build it once, not once per service.
        return dedup(selected)
            .iter()
            .map(|name| target_for_named(metadata, name))
            .collect();
    }

    if let Some(pkg) = &args.package {
        return Ok(vec![target_for_named(metadata, pkg)?]);
    }

    // No services declared. An explicit --manifest-path must resolve to a real
    // package; a non-matching one is an error, NOT a silent root fallback (the
    // fallback is only for the no-flag case).
    let package = match &manifest_dir {
        Some(dir) => workspace::package_in_dir(metadata, dir).ok_or_else(|| {
            eyre!("--manifest-path `{dir}` does not match any package in the workspace")
        })?,
        None => metadata.root_package().ok_or_else(|| {
            eyre!("could not resolve an app package to build (pass -p or --manifest-path)")
        })?,
    };

    Ok(vec![target_from_package(package)?])
}

/// Which crates a CLI `build` compiles: a root (or absent) `--manifest-path`
/// builds every declared service, one at a member builds that member, and a
/// path matching no package errors rather than silently building all.
///
/// `-p` names a crate directly and is not restricted to the services table, so
/// it means the same thing whether or not the workspace declares services. A
/// crate that is not an app then fails on its missing wasm artifact.
fn select_service_builds(
    services: &[String],
    explicit_package: Option<&str>,
    matched_member: Option<&str>,
    root_manifest: bool,
) -> Result<Vec<String>> {
    if let Some(pkg) = explicit_package {
        return Ok(vec![pkg.to_owned()]);
    }
    if root_manifest {
        return Ok(services.to_vec());
    }
    if let Some(member) = matched_member {
        return Ok(vec![member.to_owned()]);
    }
    bail!(
        "--manifest-path matches no package in this workspace: point it at a member \
         crate, or at the workspace root to build all declared services, or pass `-p <crate>`"
    )
}

/// Drops repeat crate names, keeping first-seen order, so a crate named by
/// two services is only ever a single build `Target`.
fn dedup(names: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    names
        .into_iter()
        .filter(|n| seen.insert(n.clone()))
        .collect()
}

/// meta::load's "no calimero table / no package id" error, which `build`
/// tolerates for single-crate apps. Malformed-table and misplaced-services
/// errors are distinct types and still surface.
fn is_missing_table(e: &eyre::Report) -> bool {
    e.downcast_ref::<meta::MissingCalimeroPackage>().is_some()
}

fn target_for_named(metadata: &cargo_metadata::Metadata, name: &str) -> Result<Target> {
    let package = metadata
        .packages
        .iter()
        .find(|p| p.name.as_str() == name)
        .ok_or_else(|| eyre!("package `{name}` not found in the workspace"))?;
    target_from_package(package)
}

fn target_from_package(package: &cargo_metadata::Package) -> Result<Target> {
    let crate_dir = package
        .manifest_path
        .parent()
        .ok_or_else(|| eyre!("package `{}` has no parent directory", package.name))?
        .to_owned();
    Ok(Target {
        crate_name: package.name.to_string(),
        crate_dir,
    })
}

/// The features cargo resolves for `crate_name`, standing in for the
/// `CARGO_FEATURE_*` vars only a build script gets. Errors rather than returning
/// an empty set, which would quietly drop feature-gated items from the ABI.
fn resolved_features(
    metadata: &cargo_metadata::Metadata,
    crate_name: &str,
) -> Result<BTreeSet<String>> {
    let pkg = metadata
        .packages
        .iter()
        .find(|p| p.name.as_str() == crate_name)
        .ok_or_else(|| eyre!("package `{crate_name}` not found in cargo metadata"))?;

    let resolve = metadata
        .resolve
        .as_ref()
        .ok_or_else(|| eyre!("cargo metadata carries no resolve graph"))?;

    let node = resolve
        .nodes
        .iter()
        .find(|n| n.id == pkg.id)
        .ok_or_else(|| eyre!("no resolve node for `{crate_name}` ({})", pkg.id))?;

    Ok(node.features.iter().map(ToString::to_string).collect())
}

/// Emit `res/abi.json` and `res/state-schema.json` from the manifest the app
/// itself builds, so an app needs no build script of its own. Returns the
/// `abi.json` path the embed step consumes.
///
/// `Ok(None)` when the app's SDK generates no `__calimero_abi` entry point: the
/// wasm still builds, it just carries no ABI. Degrading rather than failing is
/// what lets an app pinned to a released SDK keep building.
fn emit_abi(
    crate_dir: &Utf8Path,
    res_dir: &Utf8Path,
    features: &BTreeSet<String>,
) -> Result<Option<Utf8PathBuf>> {
    let manifest_path = crate_dir.join("Cargo.toml");
    let abi_json = res_dir.join("abi.json");

    let Some(manifest) = crate::abi::extract(&manifest_path, features)? else {
        remove_stale(&abi_json)?;
        remove_stale(&res_dir.join("state-schema.json"))?;
        eprintln!(
            "warning: {manifest_path}: SDK provides no ABI entry point; no ABI embedded\n\
             warning: res/abi.json and res/state-schema.json were not written, and the \
             wasm carries no calimero_abi_v1 section"
        );
        return Ok(None);
    };

    std::fs::write(&abi_json, serde_json::to_string_pretty(&manifest)?)
        .wrap_err_with(|| format!("failed to write {abi_json}"))?;
    println!("• emitted {abi_json}");

    write_state_schema(crate_dir, res_dir, &manifest)?;

    Ok(Some(abi_json))
}

/// res/ is not cleaned between builds, so an artifact an earlier build left
/// behind would otherwise linger and describe a wasm it no longer matches -
/// and `bundle` ships res/ as-is.
fn remove_stale(path: &Utf8Path) -> Result<()> {
    if path.is_file() {
        std::fs::remove_file(path).wrap_err_with(|| format!("failed to remove stale {path}"))?;
    }
    Ok(())
}

/// Write `res/state-schema.json`, the pair the node's upgrade gates read
/// alongside the ABI.
///
/// Tolerated but never silent: without a state schema those gates fail open, so
/// say so loudly rather than shipping a quietly incomplete pair.
fn write_state_schema(crate_dir: &Utf8Path, res_dir: &Utf8Path, manifest: &Manifest) -> Result<()> {
    let schema_path = res_dir.join("state-schema.json");
    match manifest.extract_state_schema() {
        Err(e) => {
            remove_stale(&schema_path)?;
            eprintln!(
                "warning: no state schema for {crate_dir}: {e}\n\
                 warning: res/state-schema.json was not written; the node cannot check \
                 upgrades against this app's state"
            );
        }
        Ok(mut state_schema) => {
            state_schema.schema_version = "wasm-abi/1".to_owned();
            std::fs::write(&schema_path, serde_json::to_string_pretty(&state_schema)?)
                .wrap_err_with(|| format!("failed to write {schema_path}"))?;
            println!("• emitted {schema_path}");
        }
    }

    Ok(())
}

fn build_one(
    metadata: &cargo_metadata::Metadata,
    target: &Target,
    profile: &str,
    optimize: bool,
    no_abi: bool,
) -> Result<BuiltWasm> {
    let crate_name = &target.crate_name;
    let underscored = crate_name.replace('-', "_");

    let artifact = metadata
        .target_directory
        .join(TARGET)
        .join(profile)
        .join(format!("{underscored}.wasm"));
    if !artifact.exists() {
        bail!("expected wasm artifact not found at {artifact}");
    }

    // Before anything lands in res/: emitting later would leave an un-embedded
    // wasm behind on failure, which trips the CI ABI guard.
    let res_dir = target.crate_dir.join("res");
    std::fs::create_dir_all(&res_dir).wrap_err_with(|| format!("failed to create {res_dir}"))?;
    let abi_json = if no_abi {
        None
    } else {
        let features = resolved_features(metadata, crate_name)?;
        emit_abi(&target.crate_dir, &res_dir, &features)?
    };

    let wasm_path = res_dir.join(format!("{underscored}.wasm"));
    std::fs::copy(&artifact, &wasm_path)
        .wrap_err_with(|| format!("failed to copy {artifact} -> {wasm_path}"))?;
    println!("• copied wasm to {wasm_path}");

    if optimize {
        println!("• optimizing {wasm_path} (wasm-opt -Oz)");
        OptimizationOptions::new_optimize_for_size_aggressively()
            .enable_feature(Feature::BulkMemory)
            .run(&wasm_path, &wasm_path)
            .map_err(|e| eyre!("wasm-opt failed on {wasm_path}: {e}"))?;
    } else {
        println!("• skipping wasm-opt (profiling build)");
    }

    // Embed the full abi.json: it carries the per-method flags the node's xcall
    // gate reads, which a state schema alone would drop.
    if let Some(abi_json) = &abi_json {
        println!("• embedding {abi_json} into {wasm_path}");
        mero_abi::run_embed(wasm_path.as_std_path(), abi_json.as_std_path())?;
    }

    Ok(BuiltWasm {
        crate_name: crate_name.clone(),
        wasm: wasm_path.into_std_path_buf(),
        abi_json: abi_json.map(Utf8PathBuf::into_std_path_buf),
    })
}

/// Compile every target in one invocation. Cargo rejects a plain `--features`
/// naming a feature the one selected package lacks, but accepts it whenever some
/// selected package has it - and a multi-service workspace routinely gates only
/// one service's schema.
fn cargo_build(targets: &[Target], profile: &str, args: &BuildArgs) -> Result<()> {
    let names: Vec<&str> = targets.iter().map(|t| t.crate_name.as_str()).collect();
    println!("• building {} (--profile {profile})", names.join(", "));

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let mut cmd = Command::new(cargo);
    cmd.args(["build", "--target", TARGET, "--profile", profile]);
    for name in &names {
        cmd.args(["-p", name]);
    }
    args.features.apply_to(&mut cmd);
    if let Some(mp) = &args.manifest_path {
        cmd.arg("--manifest-path").arg(mp);
    }
    cmd.env("RUSTFLAGS", remapped_rustflags(""));

    let status = cmd.status().wrap_err("failed to spawn `cargo build`")?;
    if !status.success() {
        bail!("`cargo build` failed for `{}`", names.join(", "));
    }
    Ok(())
}

/// Inherited RUSTFLAGS plus a `$HOME -> ~` remap so built wasm doesn't leak the
/// builder's home path, plus whatever `extra` the caller needs. Appends, never
/// clobbers, the user's flags.
pub fn remapped_rustflags(extra: &str) -> String {
    let home = std::env::var("HOME").ok();
    compose_rustflags(
        &std::env::var("RUSTFLAGS").unwrap_or_default(),
        home.as_deref(),
        extra,
    )
}

/// A missing `$HOME` drops only the remap: it must never swallow `extra` too.
fn compose_rustflags(inherited: &str, home: Option<&str>, extra: &str) -> String {
    let remap = home.map(|h| format!("--remap-path-prefix {h}=~"));
    [inherited, remap.as_deref().unwrap_or_default(), extra]
        .iter()
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn ensure_wasm_target() -> Result<()> {
    let listed = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output();
    match listed {
        Ok(out) if out.status.success() => {
            let installed = String::from_utf8_lossy(&out.stdout);
            if installed.lines().any(|l| l.trim() == TARGET) {
                println!("• target {TARGET} present");
            } else {
                println!("• installing target {TARGET}");
                let status = Command::new("rustup")
                    .args(["target", "add", TARGET])
                    .status()
                    .wrap_err("failed to run `rustup target add`")?;
                if !status.success() {
                    bail!("`rustup target add {TARGET}` failed");
                }
            }
        }
        // No rustup (e.g. distro toolchain): assume the target is provided some
        // other way and let `cargo build` produce the real error if it isn't.
        _ => println!("• skipping target check (rustup unavailable)"),
    }
    Ok(())
}

/// The `[profile.<profile>]` table must live in the workspace-root manifest
/// (cargo ignores profiles declared in non-root members). Fail early with a
/// paste-able snippet instead of cargo's terse "profile is not defined".
fn ensure_profile(workspace_root: &Utf8Path, profile: &str) -> Result<()> {
    let manifest = workspace_root.join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest)
        .wrap_err_with(|| format!("failed to read {manifest}"))?;
    let header = format!("[profile.{profile}]");
    if text.lines().any(|l| l.trim() == header) {
        return Ok(());
    }
    bail!(
        "missing `{header}` in {manifest}. Add these profiles to the workspace-root Cargo.toml:\n\n\
{PROFILES_SNIPPET}"
    );
}

const PROFILES_SNIPPET: &str = r#"[profile.app-release]
inherits = "release"
codegen-units = 1
opt-level = "z"
lto = true
debug = false
strip = "symbols"
panic = "abort"
overflow-checks = true

[profile.app-profiling]
inherits = "release"
opt-level = 2
debug = true
strip = false"#;

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;

    use super::{compose_rustflags, dedup, select_service_builds, write_state_schema};

    /// res/ is not cleaned between builds, so a schema left by an earlier build
    /// must not survive a build that can extract none - `bundle` ships res/ as-is,
    /// and it would describe an abi.json it no longer matches.
    #[test]
    fn a_stale_state_schema_does_not_survive_a_rootless_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let res = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        let stale = res.join("state-schema.json");
        std::fs::write(&stale, "{\"stale\":true}").unwrap();

        // No state root, so there is no schema to extract.
        let manifest = calimero_wasm_abi::Manifest {
            schema_version: "wasm-abi/1".to_owned(),
            ..Default::default()
        };
        write_state_schema(&res, &res, &manifest).expect("a missing schema is not an error");

        assert!(
            !stale.exists(),
            "a stale state-schema.json must not survive a failed extraction"
        );
    }

    /// Every part is optional, and a missing `$HOME` must not take the caller's
    /// flags down with the remap - the ABI extraction rides on `extra`.
    #[test]
    fn rustflags_compose_from_whatever_is_present() {
        assert_eq!(
            compose_rustflags("-C opt-level=1", Some("/home/me"), "--cfg calimero_abi"),
            "-C opt-level=1 --remap-path-prefix /home/me=~ --cfg calimero_abi"
        );
        assert_eq!(
            compose_rustflags("", None, "--cfg calimero_abi"),
            "--cfg calimero_abi"
        );
        assert_eq!(
            compose_rustflags("  ", Some("/home/me"), ""),
            "--remap-path-prefix /home/me=~"
        );
        assert_eq!(compose_rustflags("", None, ""), "");
    }

    fn services() -> Vec<String> {
        vec!["api".to_string(), "worker".to_string()]
    }

    #[test]
    fn selection_builds_all_services_at_root_or_no_manifest_path() {
        // no -p, no/root --manifest-path -> every declared service.
        assert_eq!(
            select_service_builds(&services(), None, None, true).unwrap(),
            services()
        );
    }

    #[test]
    fn selection_builds_only_the_member_named_by_manifest_path() {
        assert_eq!(
            select_service_builds(&services(), None, Some("worker"), false).unwrap(),
            vec!["worker".to_string()]
        );
    }

    #[test]
    fn selection_honors_explicit_package() {
        // -p wins even when a member --manifest-path also matched.
        assert_eq!(
            select_service_builds(&services(), Some("api"), Some("worker"), false).unwrap(),
            vec!["api".to_string()]
        );
    }

    #[test]
    fn selection_allows_a_package_outside_the_services_table() {
        // `-p` means the named crate, in every workspace shape; restricting it to
        // declared services here would make the same flag behave differently in a
        // multi-service workspace than in a single-crate one.
        assert_eq!(
            select_service_builds(&services(), Some("tooling-helper"), None, false).unwrap(),
            vec!["tooling-helper".to_string()]
        );
    }

    #[test]
    fn dedup_builds_a_shared_crate_once_keeping_first_seen_order() {
        // Two services ("store-a", "store-b") both named `suite` as their crate.
        assert_eq!(
            dedup(vec!["suite".into(), "other".into(), "suite".into()]),
            vec!["suite".to_string(), "other".to_string()]
        );
    }

    #[test]
    fn selection_rejects_unmatched_manifest_path() {
        // --manifest-path given, not root, matched no member: error, not build-all.
        let err = select_service_builds(&services(), None, None, false).unwrap_err();
        assert!(err.to_string().contains("--manifest-path"));
    }
}
