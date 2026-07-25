//! `cargo mero build`: compile an app to wasm32, copy it into `res/`, size-
//! optimize it (release only), and embed the full `res/abi.json` as the
//! `calimero_abi_v1` section so the node can read the state schema and
//! per-method flags off the bytecode.

use std::path::PathBuf;
use std::process::Command;

use camino::{Utf8Path, Utf8PathBuf};
use eyre::{bail, eyre, Context, Result};
use wasm_opt::{Feature, OptimizationOptions};

use crate::{meta, workspace, BuildArgs};

const TARGET: &str = "wasm32-unknown-unknown";

/// One built artifact: the crate, its optimized ABI-embedded wasm in `res/`, and
/// the `abi.json` beside it, which `bundle` ships as a sidecar.
#[derive(Debug, Clone)]
pub struct BuiltWasm {
    pub crate_name: String,
    pub wasm: PathBuf,
    pub abi_json: PathBuf,
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
    let metadata = workspace::metadata_for(args.manifest_path.as_deref())?;

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

    let mut built = Vec::with_capacity(targets.len());
    for target in targets {
        built.push(build_one(&metadata, &target, profile, !profiling, args)?);
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
        return selected
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

fn build_one(
    metadata: &cargo_metadata::Metadata,
    target: &Target,
    profile: &str,
    optimize: bool,
    args: &BuildArgs,
) -> Result<BuiltWasm> {
    let crate_name = &target.crate_name;
    let underscored = crate_name.replace('-', "_");

    println!("• building {crate_name} (--profile {profile})");
    cargo_build(crate_name, profile, args)?;

    let artifact = metadata
        .target_directory
        .join(TARGET)
        .join(profile)
        .join(format!("{underscored}.wasm"));
    if !artifact.exists() {
        bail!("expected wasm artifact not found at {artifact}");
    }

    let res_dir = target.crate_dir.join("res");
    std::fs::create_dir_all(&res_dir).wrap_err_with(|| format!("failed to create {res_dir}"))?;
    let wasm_path = res_dir.join(format!("{underscored}.wasm"));
    std::fs::copy(&artifact, &wasm_path)
        .wrap_err_with(|| format!("failed to copy {artifact} -> {wasm_path}"))?;
    println!("• copied wasm to {wasm_path}");

    // Checked before optimizing: a missing ABI is an early-onboarding mistake, and
    // failing here skips the expensive wasm-opt pass.
    let abi_json = res_dir.join("abi.json");
    if !abi_json.exists() {
        bail!(
            "app build did not emit {abi_json} - the app's build.rs must emit res/abi.json \
             (scaffold with `cargo mero new` or copy apps/kv-store/build.rs)"
        );
    }

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
    println!("• embedding {abi_json} into {wasm_path}");
    let mut manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&abi_json).wrap_err_with(|| format!("failed to read {abi_json}"))?,
    )
    .wrap_err_with(|| format!("failed to parse {abi_json} as JSON"))?;
    canonicalize_abi(&mut manifest);
    let canonical = tempfile::NamedTempFile::new().wrap_err("failed to create temp file")?;
    serde_json::to_writer(&canonical, &manifest).wrap_err("failed to write canonicalized ABI")?;
    mero_abi::run_embed(wasm_path.as_std_path(), canonical.path())?;

    Ok(BuiltWasm {
        crate_name: crate_name.clone(),
        wasm: wasm_path.into_std_path_buf(),
        abi_json: abi_json.into_std_path_buf(),
    })
}

/// Name-sort `methods` and `events` so `validate_manifest` accepts a manifest
/// an older SDK emitted in source order. Nothing else is touched.
fn canonicalize_abi(manifest: &mut serde_json::Value) {
    let name_of = |v: &serde_json::Value| {
        v.get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    for key in ["methods", "events"] {
        if let Some(arr) = manifest
            .get_mut(key)
            .and_then(serde_json::Value::as_array_mut)
        {
            arr.sort_by_key(&name_of);
        }
    }
}

fn cargo_build(crate_name: &str, profile: &str, args: &BuildArgs) -> Result<()> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let mut cmd = Command::new(cargo);
    cmd.args([
        "build",
        "--target",
        TARGET,
        "--profile",
        profile,
        "-p",
        crate_name,
    ]);
    if let Some(mp) = &args.manifest_path {
        cmd.arg("--manifest-path").arg(mp);
    }
    cmd.env("RUSTFLAGS", remapped_rustflags());

    let status = cmd.status().wrap_err("failed to spawn `cargo build`")?;
    if !status.success() {
        bail!("`cargo build` failed for `{crate_name}`");
    }
    Ok(())
}

/// Inherited RUSTFLAGS plus a `$HOME -> ~` remap so built wasm doesn't leak the
/// builder's home path. Appends, never clobbers, the user's flags.
fn remapped_rustflags() -> String {
    let inherited = std::env::var("RUSTFLAGS").unwrap_or_default();
    let Ok(home) = std::env::var("HOME") else {
        return inherited;
    };
    let remap = format!("--remap-path-prefix {home}=~");
    if inherited.trim().is_empty() {
        remap
    } else {
        format!("{inherited} {remap}")
    }
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
    use super::{canonicalize_abi, select_service_builds};
    use serde_json::json;

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
    fn selection_rejects_unmatched_manifest_path() {
        // --manifest-path given, not root, matched no member: error, not build-all.
        let err = select_service_builds(&services(), None, None, false).unwrap_err();
        assert!(err.to_string().contains("--manifest-path"));
    }

    fn names(v: &serde_json::Value, key: &str) -> Vec<String> {
        v[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["name"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn sorts_unsorted_methods_and_events() {
        // Mirrors the demo-app fixture: update_if_exists precedes get_or_insert.
        let mut m = json!({
            "methods": [
                { "name": "update_if_exists", "kind": "call" },
                { "name": "get_or_insert", "kind": "call" },
            ],
            "events": [
                { "name": "Updated" },
                { "name": "Created" },
            ],
        });
        canonicalize_abi(&mut m);
        assert_eq!(names(&m, "methods"), ["get_or_insert", "update_if_exists"]);
        assert_eq!(names(&m, "events"), ["Created", "Updated"]);
    }

    #[test]
    fn missing_keys_are_a_no_op() {
        let mut m = json!({ "types": { "State": {} } });
        let before = m.clone();
        canonicalize_abi(&mut m);
        assert_eq!(m, before);
    }

    #[test]
    fn only_the_two_arrays_are_reordered() {
        // `types` object and per-method inner content must be left untouched.
        let mut m = json!({
            "methods": [
                { "name": "b", "params": ["z", "a"] },
                { "name": "a" },
            ],
            "types": { "Zeta": {}, "Alpha": {} },
        });
        canonicalize_abi(&mut m);
        assert_eq!(names(&m, "methods"), ["a", "b"]);
        // Inner params of the (now second) method keep their original order.
        assert_eq!(m["methods"][1]["params"], json!(["z", "a"]));
        assert_eq!(m["types"], json!({ "Zeta": {}, "Alpha": {} }));
    }
}
