//! `cargo mero bundle`: build every service, stage the wasm/abi files, render
//! and sign `manifest.json`, and package everything into a `.mpk` (tar.gz). This
//! replaces the hand-written `build-bundle.sh` heredocs each app used to carry.

use std::fs;
use std::fs::File;
use std::path::PathBuf;

use camino::{Utf8Path, Utf8PathBuf};
use ed25519_dalek::SigningKey;
use eyre::{bail, eyre, Context, Result};
use flate2::write::GzEncoder;
use flate2::Compression;

use crate::build::{self, BuiltWasm};
use crate::manifest::{self, Artifact, StagedArtifact};
use crate::meta::{self, BundleMeta};
use crate::workspace;
use crate::{BuildArgs, BundleArgs};

/// Environment variable naming a signing-key JSON file, used when none of
/// `--key`/`--dev` is passed.
const SIGN_KEY_ENV: &str = "MERO_SIGN_KEY";

/// How the manifest will be signed, resolved from the flags + `MERO_SIGN_KEY`.
enum SignMode {
    Key(SigningKey),
    Dev(SigningKey),
}

pub fn run(args: &BundleArgs) -> Result<PathBuf> {
    // Resolve signing first: a missing or bad key must fail before a long build.
    let sign_mode = resolve_sign_mode(args)?;

    let metadata = workspace::metadata_for(args.manifest_path.as_deref())?;
    let base = base_dir(&metadata, args);
    let bundle_meta = resolve_meta(&metadata, &base, args)?;

    // Always build every declared service: `stage` writes one entry per service
    // in the manifest, so a filtered build would silently drop one.
    let built = build::run_all(&BuildArgs {
        profiling: args.profiling,
        package: None,
        manifest_path: args.manifest_path.clone(),
    })?;

    let staging = prepare_staging(&base)?;
    println!("• staging bundle files in {staging}");
    let staged = stage(&bundle_meta, &built, &staging)?;

    println!("• writing manifest.json");
    let manifest_path = write_manifest(&staging, &bundle_meta, &staged)?;
    let signer_id = sign(&manifest_path, &sign_mode)?;

    let output = output_path(args, &base, &bundle_meta.package);
    println!("• packaging {output}");
    package(&output, &manifest_path, &staged)?;
    fs::remove_dir_all(&staging).wrap_err_with(|| format!("failed to remove {staging}"))?;

    println!("\nbundle:     {output}");
    println!("package:    {}", bundle_meta.package);
    println!("appVersion: {}", bundle_meta.app_version);
    println!("signerId:   {signer_id}");

    Ok(output.into_std_path_buf())
}

/// Bundle metadata with the `--package` / `--app-version` overrides applied.
fn resolve_meta(
    metadata: &cargo_metadata::Metadata,
    base: &Utf8Path,
    args: &BundleArgs,
) -> Result<BundleMeta> {
    let mut resolved = meta::load(metadata, base)?;
    if let Some(package) = &args.package {
        meta::validate_package_id(package)?;
        resolved.package = package.clone();
    }
    if let Some(version) = &args.app_version {
        resolved.app_version = version.clone();
    }
    if resolved.app_version.is_empty() {
        bail!(
            "no app version resolved for `{}`. In a multi-service (virtual) workspace, either add\n\n\
             [workspace.package]\nversion = \"0.1.0\"\n\nto the workspace-root Cargo.toml, or pass `--app-version <version>`.",
            resolved.package
        );
    }
    Ok(resolved)
}

/// A clean `res/bundle-temp/`, discarding anything a previous run left behind.
fn prepare_staging(base: &Utf8Path) -> Result<Utf8PathBuf> {
    let staging = base.join("res").join("bundle-temp");
    if staging.exists() {
        fs::remove_dir_all(&staging)
            .wrap_err_with(|| format!("failed to clear stale {staging}"))?;
    }
    fs::create_dir_all(&staging).wrap_err_with(|| format!("failed to create {staging}"))?;
    Ok(staging)
}

fn write_manifest(
    staging: &Utf8Path,
    meta: &BundleMeta,
    staged: &[StagedArtifact],
) -> Result<Utf8PathBuf> {
    let path = staging.join("manifest.json");
    let value = manifest::render(meta, staged)?;
    fs::write(&path, serde_json::to_string_pretty(&value)?)
        .wrap_err_with(|| format!("failed to write {path}"))?;
    Ok(path)
}

/// Turn the mutually-exclusive flags (plus the `MERO_SIGN_KEY` fallback) into a
/// concrete signing decision. Erroring here, before the build, keeps a mistyped
/// key path from wasting a full compile.
fn resolve_sign_mode(args: &BundleArgs) -> Result<SignMode> {
    if let Some(path) = &args.key {
        return Ok(SignMode::Key(mero_sign::load_signing_key(path)?));
    }
    if args.dev {
        return Ok(SignMode::Dev(mero_sign::dev_signing_key()));
    }
    if let Some(path) = std::env::var_os(SIGN_KEY_ENV) {
        return Ok(SignMode::Key(mero_sign::load_signing_key(
            PathBuf::from(path).as_path(),
        )?));
    }
    bail!(
        "no signing method given. Pass one of:\n  \
         --key <FILE>   sign with a production key (cargo mero key generate)\n  \
         --dev          sign with the well-known dev key (not publishable)\n\
         or set {SIGN_KEY_ENV} to a key-file path."
    );
}

/// Sign `manifest.json` in place; returns the signerId.
fn sign(manifest_path: &Utf8Path, mode: &SignMode) -> Result<String> {
    match mode {
        SignMode::Key(key) => {
            println!("• signing manifest.json");
            mero_sign::sign_manifest(manifest_path.as_std_path(), key)?;
            Ok(signer_id_of(key))
        }
        SignMode::Dev(key) => {
            print_dev_warning();
            println!("• signing manifest.json with the DEV key");
            mero_sign::sign_manifest(manifest_path.as_std_path(), key)?;
            Ok(signer_id_of(key))
        }
    }
}

fn signer_id_of(key: &SigningKey) -> String {
    mero_sign::derive_signer_id_did_key(key.verifying_key().as_bytes())
}

fn print_dev_warning() {
    eprintln!("============================================================");
    eprintln!("  WARNING: signing with the DEVELOPMENT key");
    eprintln!();
    eprintln!("  The dev key is a well-known, public seed. It is fine for");
    eprintln!("  local testing but is REFUSED by the registry.");
    eprintln!();
    eprintln!("  For a publishable bundle, generate a real key:");
    eprintln!("      cargo mero key generate --output my-key.json");
    eprintln!("      cargo mero bundle --key my-key.json");
    eprintln!("============================================================");
}

/// Base directory of the app: the manifest's parent when `--manifest-path` is
/// given, else the workspace root. Canonicalized so a relative `--manifest-path`
/// still matches cargo_metadata's absolute package paths.
fn base_dir(metadata: &cargo_metadata::Metadata, args: &BundleArgs) -> Utf8PathBuf {
    args.manifest_path
        .as_ref()
        .map(|p| workspace::manifest_dir(p))
        .unwrap_or_else(|| metadata.workspace_root.clone())
}

/// Copy built artifacts into staging and describe them for the manifest: a single
/// service lands at `app.wasm` + `abi.json`, multi-service under `services/<name>.*`.
fn stage(
    meta: &BundleMeta,
    built: &[BuiltWasm],
    staging: &Utf8Path,
) -> Result<Vec<StagedArtifact>> {
    if meta.services.is_empty() {
        let [one] = built else {
            bail!(
                "expected exactly one built artifact for a single-service app, got {}",
                built.len()
            );
        };
        let wasm = stage_file(&one.wasm, staging, "app.wasm")?;
        let abi = stage_file(&one.abi_json, staging, "abi.json")?;
        return Ok(vec![StagedArtifact {
            service_name: None,
            wasm,
            abi,
        }]);
    }

    fs::create_dir_all(staging.join("services"))
        .wrap_err_with(|| format!("failed to create {staging}/services"))?;

    let mut staged = Vec::with_capacity(meta.services.len());
    for service in &meta.services {
        let build = built
            .iter()
            .find(|b| b.crate_name == service.crate_name)
            .ok_or_else(|| {
                eyre!(
                    "service `{}` maps to crate `{}`, which was not built",
                    service.name,
                    service.crate_name
                )
            })?;
        let wasm = stage_file(
            &build.wasm,
            staging,
            &format!("services/{}.wasm", service.name),
        )?;
        let abi = stage_file(
            &build.abi_json,
            staging,
            &format!("services/{}-abi.json", service.name),
        )?;
        staged.push(StagedArtifact {
            service_name: Some(service.name.clone()),
            wasm,
            abi,
        });
    }
    Ok(staged)
}

/// Copy one source file to `staging/<rel>` and describe it (path, size, hash).
fn stage_file(src: &std::path::Path, staging: &Utf8Path, rel: &str) -> Result<Artifact> {
    let dest = staging.join(rel);
    let bytes = fs::read(src).wrap_err_with(|| format!("failed to read {}", src.display()))?;
    fs::write(&dest, &bytes).wrap_err_with(|| format!("failed to write {dest}"))?;
    Ok(Artifact::from_bytes(rel, &bytes))
}

/// Default output: `<base>/dist/<package>.mpk` (unversioned; the version lives
/// in the manifest). `--output` overrides it verbatim.
fn output_path(args: &BundleArgs, base: &Utf8Path, package: &str) -> Utf8PathBuf {
    args.output
        .clone()
        .unwrap_or_else(|| base.join("dist").join(format!("{package}.mpk")))
}

/// Write the tar.gz `.mpk`: `manifest.json` at the root plus each artifact at the
/// same relative path recorded in the manifest, so the two never disagree.
fn package(output: &Utf8Path, manifest_path: &Utf8Path, staged: &[StagedArtifact]) -> Result<()> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).wrap_err_with(|| format!("failed to create {parent}"))?;
    }
    let file = File::create(output).wrap_err_with(|| format!("failed to create {output}"))?;
    let encoder = GzEncoder::new(file, Compression::default());
    let mut tar = tar::Builder::new(encoder);

    let staging = manifest_path
        .parent()
        .ok_or_else(|| eyre!("manifest path has no parent"))?;

    // First, and not merely by convention: the node's manifest scan runs before
    // any signature check, so it is bounded and stops a few MiB in.
    tar.append_path_with_name(manifest_path, "manifest.json")
        .wrap_err("failed to add manifest.json to the bundle")?;
    for artifact in staged {
        for rel in [&artifact.wasm.path, &artifact.abi.path] {
            tar.append_path_with_name(staging.join(rel), rel)
                .wrap_err_with(|| format!("failed to add {rel} to the bundle"))?;
        }
    }

    tar.into_inner()
        .wrap_err("failed to finalize tar")?
        .finish()
        .wrap_err("failed to finalize gzip")?;
    Ok(())
}
