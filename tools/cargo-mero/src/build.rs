//! `cargo mero build`: emit the app's ABI from its sources, compile it to wasm32,
//! copy it into `res/`, size-optimize it (release only), and embed the full
//! `res/abi.json` as the `calimero_abi_v1` section so the node can read the state
//! schema and per-method flags off the bytecode.
//!
//! The ABI emit lives here rather than in each app's `build.rs` so one
//! implementation owns it; an app needs no build script to carry an ABI.

use std::collections::BTreeSet;
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

/// The `#[path = "..."]` override on a `mod` declaration, if present.
fn mod_path_attr(attrs: &[syn::Attribute]) -> Option<String> {
    attrs.iter().find_map(|attr| {
        let syn::Meta::NameValue(nv) = &attr.meta else {
            return None;
        };
        if !nv.path.is_ident("path") {
            return None;
        }
        match &nv.value {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) => Some(s.value()),
            _ => None,
        }
    })
}

/// Where `mod <name>;` declared in a file whose module directory is `dir`
/// resolves. `name.rs` and `name/mod.rs` are both accepted.
fn module_file(dir: &Utf8Path, name: &str, path_attr: Option<&str>) -> Option<Utf8PathBuf> {
    if let Some(rel) = path_attr {
        let candidate = dir.join(rel);
        return candidate.is_file().then_some(candidate);
    }
    [
        dir.join(format!("{name}.rs")),
        dir.join(name).join("mod.rs"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// Queue every file `items` pulls in via `mod name;`, as (file, its module dir).
/// Descends into inline `mod name { .. }` bodies, whose own `mod x;` children
/// resolve one directory deeper.
fn queue_module_files(
    items: &[syn::Item],
    dir: &Utf8Path,
    features: &BTreeSet<String>,
    out: &mut Vec<(Utf8PathBuf, Utf8PathBuf)>,
) {
    for item in items {
        let syn::Item::Mod(m) = item else { continue };
        if !calimero_wasm_abi::emitter::item_cfg_active(item, features) {
            continue;
        }
        let name = m.ident.to_string();
        let path_attr = mod_path_attr(&m.attrs);
        match &m.content {
            // Inline body: already part of the enclosing file's contents, but its
            // own `mod x;` children still resolve one level down.
            Some((_, inner)) => {
                let child_dir = path_attr
                    .as_ref()
                    .map_or_else(|| dir.join(&name), |p| dir.join(p));
                queue_module_files(inner, &child_dir, features, out);
            }
            // A declared module with no file on disk would not compile at all, so
            // cargo's own error is the better one to surface - don't mask it here.
            None => {
                if let Some(file) = module_file(dir, &name, path_attr.as_deref()) {
                    out.push((file, dir.join(&name)));
                }
            }
        }
    }
}

/// The crate's sources, found by following `mod` declarations from `src/lib.rs`.
/// Not a glob: a file the module tree does not pull in is never compiled into the
/// wasm, so its types must not reach the ABI either.
fn crate_sources(src_dir: &Utf8Path, features: &BTreeSet<String>) -> Result<Vec<(String, String)>> {
    let root = src_dir.join("lib.rs");
    if !root.is_file() {
        bail!("no lib.rs found under {src_dir}");
    }

    let mut sources = Vec::new();
    let mut queue = vec![(root, src_dir.to_owned())];
    let mut seen = std::collections::BTreeSet::new();

    while let Some((path, dir)) = queue.pop() {
        if !seen.insert(path.clone()) {
            continue;
        }
        let content =
            std::fs::read_to_string(&path).wrap_err_with(|| format!("failed to read {path}"))?;
        let parsed = syn::parse_file(&content)
            .map_err(|e| eyre!("failed to parse {path} while collecting sources: {e}"))?;

        queue_module_files(&parsed.items, &dir, features, &mut queue);

        let name = path
            .strip_prefix(src_dir)
            .map(Utf8Path::as_str)
            .unwrap_or(path.as_str())
            .to_owned();
        sources.push((name, content));
    }

    // Deterministic order regardless of traversal order.
    sources.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(sources)
}

/// Emit `res/abi.json` and `res/state-schema.json` from the crate's own sources,
/// so an app needs no build script of its own. Returns the `abi.json` path the
/// embed step consumes.
fn emit_abi(
    crate_dir: &Utf8Path,
    res_dir: &Utf8Path,
    features: &BTreeSet<String>,
) -> Result<Utf8PathBuf> {
    let src_dir = crate_dir.join("src");
    let sources = crate_sources(&src_dir, features)?;
    if sources.is_empty() {
        bail!("no .rs sources found under {src_dir}");
    }

    let manifest =
        calimero_wasm_abi::emitter::emit_manifest_from_crate_with_features(&sources, features)
            .map_err(|e| eyre!("failed to emit ABI manifest for {crate_dir}: {e}"))?;

    let abi_json = res_dir.join("abi.json");
    std::fs::write(&abi_json, serde_json::to_string_pretty(&manifest)?)
        .wrap_err_with(|| format!("failed to write {abi_json}"))?;
    println!("• emitted {abi_json}");

    // Tolerated but never silent: without a state schema the node's upgrade gates
    // fail open, so say so loudly rather than shipping a quietly incomplete pair.
    match manifest.extract_state_schema() {
        Err(e) => eprintln!(
            "warning: no state schema for {crate_dir}: {e}\n\
             warning: res/state-schema.json was not written; the node cannot check \
             upgrades against this app's state"
        ),
        Ok(mut state_schema) => {
            state_schema.schema_version = "wasm-abi/1".to_owned();
            let path = res_dir.join("state-schema.json");
            std::fs::write(&path, serde_json::to_string_pretty(&state_schema)?)
                .wrap_err_with(|| format!("failed to write {path}"))?;
            println!("• emitted {path}");
        }
    }

    Ok(abi_json)
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

    // Before anything lands in res/: emitting later would leave an un-embedded
    // wasm behind on failure, which trips the CI ABI guard.
    let res_dir = target.crate_dir.join("res");
    std::fs::create_dir_all(&res_dir).wrap_err_with(|| format!("failed to create {res_dir}"))?;
    let features = resolved_features(metadata, crate_name)?;
    let abi_json = emit_abi(&target.crate_dir, &res_dir, &features)?;

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
    use std::collections::BTreeSet;

    use camino::Utf8PathBuf;
    use serde_json::json;

    use super::{canonicalize_abi, emit_abi, select_service_builds};

    const MINIMAL_APP: &str = r#"
        #[app::state(version = 1)]
        pub struct State { x: u32 }
        #[app::logic]
        impl State {
            #[app::init] pub fn init() -> State { State { x: 0 } }
            pub fn bump(&mut self) {}
        }
    "#;

    /// Write `sources` into `<root>/src` and emit into `<root>/res`.
    fn emit_into(root: &Utf8PathBuf, sources: &[(&str, &str)]) -> Utf8PathBuf {
        std::fs::create_dir_all(root.join("src")).unwrap();
        for (name, body) in sources {
            std::fs::write(root.join("src").join(name), body).unwrap();
        }
        let res = root.join("res");
        std::fs::create_dir_all(&res).unwrap();
        emit_abi(root, &res, &BTreeSet::new()).expect("emit_abi");
        res
    }

    #[test]
    fn emit_abi_writes_both_artifacts() {
        let tmp = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        let res = emit_into(&root, &[("lib.rs", MINIMAL_APP)]);

        let abi: serde_json::Value =
            serde_json::from_slice(&std::fs::read(res.join("abi.json")).unwrap()).unwrap();
        assert!(
            abi["methods"]
                .as_array()
                .is_some_and(|m| m.iter().any(|x| x["name"] == "bump")),
            "abi.json must carry the app's methods"
        );

        let schema: serde_json::Value =
            serde_json::from_slice(&std::fs::read(res.join("state-schema.json")).unwrap()).unwrap();
        assert_eq!(
            schema["schema_version"], "wasm-abi/1",
            "state schema must be stamped with the version the node reads"
        );
    }

    fn collected(root: &Utf8PathBuf, features: &BTreeSet<String>) -> Vec<String> {
        super::crate_sources(&root.join("src"), features)
            .unwrap()
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }

    /// A module declared with `mod x;` is part of the crate, so it is collected.
    #[test]
    fn crate_sources_follows_mod_declarations() {
        let tmp = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            format!("pub mod extra;\n{MINIMAL_APP}"),
        )
        .unwrap();
        std::fs::write(root.join("src/extra.rs"), "pub struct Extra { pub a: u32 }").unwrap();

        let names = collected(&root, &BTreeSet::new());
        assert!(names.contains(&"extra.rs".to_owned()), "got {names:?}");
    }

    /// `src/foo.rs` declaring `mod bar;` resolves to `src/foo/bar.rs`.
    #[test]
    fn crate_sources_follows_nested_modules() {
        let tmp = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        std::fs::create_dir_all(root.join("src/outer")).unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            format!("pub mod outer;\n{MINIMAL_APP}"),
        )
        .unwrap();
        std::fs::write(root.join("src/outer.rs"), "pub mod deep;").unwrap();
        std::fs::write(
            root.join("src/outer/deep.rs"),
            "pub struct Deep { pub a: u32 }",
        )
        .unwrap();

        let names = collected(&root, &BTreeSet::new());
        assert!(
            names.iter().any(|n| n.ends_with("deep.rs")),
            "nested module must be collected, got {names:?}"
        );
    }

    /// `mod outer { mod inner; }` still loads `src/outer/inner.rs`, so the walk has
    /// to descend through the inline body rather than skip it.
    #[test]
    fn crate_sources_follows_mods_nested_in_inline_blocks() {
        let tmp = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        std::fs::create_dir_all(root.join("src/outer")).unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            format!("pub mod outer {{ pub mod inner; }}\n{MINIMAL_APP}"),
        )
        .unwrap();
        std::fs::write(
            root.join("src/outer/inner.rs"),
            "pub struct Inner { pub a: u32 }",
        )
        .unwrap();

        let names = collected(&root, &BTreeSet::new());
        assert!(
            names.iter().any(|n| n.ends_with("inner.rs")),
            "a mod declared inside an inline block must still be followed, got {names:?}"
        );
    }

    /// A `#[cfg(test)]` inline block must not drag its file-backed children in.
    #[test]
    fn crate_sources_skips_mods_under_a_cfg_test_inline_block() {
        let tmp = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        std::fs::create_dir_all(root.join("src/harness")).unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            format!("#[cfg(test)]\nmod harness {{ pub mod fixtures; }}\n{MINIMAL_APP}"),
        )
        .unwrap();
        std::fs::write(root.join("src/harness/fixtures.rs"), "pub struct Fixture;").unwrap();

        let names = collected(&root, &BTreeSet::new());
        assert_eq!(names, vec!["lib.rs".to_owned()], "got {names:?}");
    }

    /// `cfg(test)` also has to be decided inside `all(..)` / `any(..)`, and
    /// `not(test)` must stay included.
    #[test]
    fn crate_sources_decides_cfg_test_in_compound_predicates() {
        let tmp = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            format!(
                "#[cfg(all(test, feature = \"extras\"))]\nmod all_test;\n\
                 #[cfg(any(test, feature = \"absent\"))]\nmod any_test;\n\
                 #[cfg(not(test))]\nmod shipped;\n{MINIMAL_APP}"
            ),
        )
        .unwrap();
        for m in ["all_test", "any_test", "shipped"] {
            std::fs::write(root.join(format!("src/{m}.rs")), "pub struct T;").unwrap();
        }

        // `extras` on: all(test, extras) is still test-only, and any(test, absent)
        // has no other way in, so only `shipped` joins lib.rs.
        let on: BTreeSet<String> = ["extras".to_owned()].into_iter().collect();
        let mut names = collected(&root, &on);
        names.sort();
        assert_eq!(
            names,
            vec!["lib.rs".to_owned(), "shipped.rs".to_owned()],
            "got {names:?}"
        );
    }

    /// A file the module tree never pulls in is not compiled into the wasm, so its
    /// types must not reach the ABI. Same for a module gated off by features.
    #[test]
    fn crate_sources_ignores_unreferenced_and_gated_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            format!(
                "#[cfg(feature = \"extras\")]\npub mod gated;\n#[cfg(test)]\nmod harness;\n{MINIMAL_APP}"
            ),
        )
        .unwrap();
        std::fs::write(root.join("src/stray.rs"), "pub struct Stray;").unwrap();
        std::fs::write(root.join("src/gated.rs"), "pub struct Gated;").unwrap();
        std::fs::write(root.join("src/harness.rs"), "pub struct Harness;").unwrap();

        let names = collected(&root, &BTreeSet::new());
        assert_eq!(
            names,
            vec!["lib.rs".to_owned()],
            "only lib.rs is reachable with `extras` off"
        );

        let on: BTreeSet<String> = ["extras".to_owned()].into_iter().collect();
        let names = collected(&root, &on);
        assert!(names.contains(&"gated.rs".to_owned()), "got {names:?}");
        assert!(!names.contains(&"stray.rs".to_owned()), "got {names:?}");
    }

    #[test]
    fn emit_abi_fails_without_a_lib_rs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let res = root.join("res");
        std::fs::create_dir_all(&res).unwrap();
        assert!(emit_abi(&root, &res, &BTreeSet::new()).is_err());
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
