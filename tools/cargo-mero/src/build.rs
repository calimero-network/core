//! `cargo mero build`: compile an app to wasm32, copy it into `res/`, size-
//! optimize it (release only), and embed the canonicalized full `res/abi.json`
//! as the `calimero_abi_v1` custom section so the node can read the app's ABI
//! (state schema plus per-method flags such as `xcall_callable`) off the
//! bytecode (calimero-network/core#3287). The manifest is canonicalized first
//! because the emitter writes methods/events in source order while core's
//! `validate_manifest` requires them name-sorted (see `canonicalize_abi`).

use std::path::PathBuf;
use std::process::Command;

use camino::{Utf8Path, Utf8PathBuf};
use eyre::{bail, eyre, Context, Result};
use wasm_opt::{Feature, OptimizationOptions};

use crate::meta;
use crate::BuildArgs;

const TARGET: &str = "wasm32-unknown-unknown";

/// One built artifact: the crate it came from, its (optimized, full-ABI-
/// embedded) wasm in `res/`, and the full `abi.json` beside it (the bundle
/// carries the as-emitted manifest as a separate file). `bundle` (Task 7)
/// consumes this.
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

pub fn run(args: &BuildArgs) -> Result<Vec<BuiltWasm>> {
    let mut cmd = cargo_metadata::MetadataCommand::new();
    if let Some(mp) = &args.manifest_path {
        let _ = cmd.manifest_path(mp);
    }
    let metadata = cmd.exec().wrap_err("failed to run `cargo metadata`")?;

    let profiling =
        args.profiling || matches!(std::env::var("WASM_PROFILING").as_deref(), Ok("true"));
    let profile = if profiling {
        "app-profiling"
    } else {
        "app-release"
    };

    let targets = resolve_targets(&metadata, args)?;
    ensure_wasm_target()?;
    ensure_profile(&metadata.workspace_root, profile)?;

    let mut built = Vec::with_capacity(targets.len());
    for target in targets {
        built.push(build_one(&metadata, &target, profile, !profiling, args)?);
    }
    Ok(built)
}

/// The crates to build: an explicit `-p`, else every declared workspace
/// service, else the single resolved package.
fn resolve_targets(metadata: &cargo_metadata::Metadata, args: &BuildArgs) -> Result<Vec<Target>> {
    if let Some(pkg) = &args.package {
        return Ok(vec![target_for_named(metadata, pkg)?]);
    }

    // Canonicalize so a relative `--manifest-path` matches cargo_metadata's
    // absolute package paths (otherwise it silently falls through to root_package).
    let manifest_dir = args.manifest_path.as_ref().map(|p| meta::canonical_dir(p));

    // Service discovery must also work from a workspace-root cwd with no
    // --manifest-path (`cd workspace && cargo mero bundle --dev`): fall back to
    // the metadata's workspace root so services are still consulted.
    //
    // `build` (unlike `bundle`) must also work for a single-crate app that never
    // declares `[package.metadata.calimero]`. Service discovery only needs the
    // table when a workspace lists services, so treat meta's missing-table error
    // as "no services" and fall back to the single resolved package.
    let discovery_dir = manifest_dir
        .clone()
        .unwrap_or_else(|| metadata.workspace_root.clone());
    let services = match meta::load(metadata, &discovery_dir) {
        Ok(m) => m.services,
        Err(e) if is_missing_table(&e) => Vec::new(),
        Err(e) => return Err(e),
    };

    if !services.is_empty() {
        return services
            .into_iter()
            .map(|s| target_for_named(metadata, &s.crate_name))
            .collect();
    }

    let package = manifest_dir
        .as_deref()
        .and_then(|dir| {
            metadata
                .packages
                .iter()
                .find(|p| p.manifest_path.parent() == Some(dir))
        })
        .or_else(|| metadata.root_package())
        .ok_or_else(|| {
            eyre!("could not resolve an app package to build (pass -p or --manifest-path)")
        })?;

    Ok(vec![target_from_package(package)?])
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

    if optimize {
        println!("• optimizing {wasm_path} (wasm-opt -Oz)");
        OptimizationOptions::new_optimize_for_size_aggressively()
            .enable_feature(Feature::BulkMemory)
            .run(&wasm_path, &wasm_path)
            .map_err(|e| eyre!("wasm-opt failed on {wasm_path}: {e}"))?;
    } else {
        println!("• skipping wasm-opt (profiling build)");
    }

    let abi_json = res_dir.join("abi.json");
    if !abi_json.exists() {
        bail!(
            "app build did not emit {abi_json} - the app's build.rs must emit res/abi.json \
             (scaffold with `cargo mero new` or copy apps/kv-store/build.rs)"
        );
    }

    // Embed the full abi.json (carries per-method flags like `xcall_callable`
    // the node's xcall gate reads) as the `calimero_abi_v1` section. The
    // emitter writes methods/events in source order but core's
    // `validate_manifest` requires them name-sorted, so we canonicalize into a
    // temp file first and let `run_embed` re-validate before writing. The
    // as-emitted abi.json still ships as a bundle sidecar via
    // `BuiltWasm.abi_json`.
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

/// Sort an ABI manifest's `methods` and `events` arrays by their `name` field
/// so core's `validate_manifest` (which requires name-sorted arrays) accepts
/// the emitter's source-order output. Stable sort, and only those two arrays
/// are touched - `types` and every other field are left as emitted.
///
/// This is a workaround for a core validator inconsistency (the emitter and the
/// validator disagree on ordering); it lives here at the call site, not in
/// mero-abi, so it is trivial to drop once core is fixed upstream.
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
    use super::canonicalize_abi;
    use serde_json::json;

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
