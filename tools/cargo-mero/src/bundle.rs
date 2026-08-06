//! `cargo mero bundle`: build every service, stage the wasm/abi files, render
//! and sign `manifest.json`, and package everything into a `.mpk` (tar.gz). This
//! replaces the hand-written `build-bundle.sh` heredocs each app used to carry.

use std::fs;
use std::fs::File;
use std::path::{Path, PathBuf};

use calimero_bundle::BundleArtifact;
use calimero_primitives::application::ApplicationId;
use camino::{Utf8Path, Utf8PathBuf};
use ed25519_dalek::SigningKey;
use eyre::{bail, eyre, Context, Result};
use flate2::write::GzEncoder;
use flate2::Compression;

use crate::build::{self, BuiltWasm};
use crate::manifest::{self, StagedArtifact};
use crate::meta::{self, BundleMeta};
use crate::{icon, logo, registry, workspace};
use crate::{BuildArgs, BundleArgs};

/// Environment variable naming a signing-key JSON file, used when none of
/// `--key`/`--dev` is passed.
const SIGN_KEY_ENV: &str = "MERO_SIGN_KEY";

pub fn run(args: &BundleArgs) -> Result<PathBuf> {
    // Resolve signing first: a missing or bad key must fail before a long build.
    let signing_key = resolve_signing_key(args.key.as_deref(), args.dev)?;

    let metadata = workspace::metadata_for(args.manifest_path.as_deref(), &args.features)?;
    let base = base_dir(&metadata, args);
    let mut bundle_meta = resolve_meta(&metadata, &base, args)?;
    // Resolved before the build so a missing icon fails fast, same as signing.
    bundle_meta.icon = icon::resolve(&bundle_meta, args.no_icon)?;
    if args.no_abi {
        eprintln!("warning: bundling without an ABI; this app cannot be migrated");
    }

    // Always build every declared service: `stage` writes one entry per service
    // in the manifest, so a filtered build would silently drop one.
    let built = build::run_all(&BuildArgs {
        profiling: args.profiling,
        package: None,
        manifest_path: args.manifest_path.clone(),
        no_abi: args.no_abi,
        features: args.features.clone(),
    })?;

    let staging = prepare_staging(&base)?;
    println!("• staging bundle files in {staging}");
    let staged = stage(&bundle_meta, &built, &staging, args.no_abi)?;

    println!("• writing manifest.json");
    let manifest_path = write_manifest(&staging, &bundle_meta, &staged)?;
    let signer_id = sign(&manifest_path, &signing_key)?;
    let application_id = ApplicationId::for_bundle(&bundle_meta.package, &signer_id)?;

    let output = output_path(args, &base, &bundle_meta.package, &bundle_meta.app_version);
    println!("• packaging {output}");
    package(&output, &manifest_path, &staged)?;
    fs::remove_dir_all(&staging).wrap_err_with(|| format!("failed to remove {staging}"))?;

    print_summary(
        &output,
        &bundle_meta,
        &application_id,
        &signer_id,
        args.no_logo,
    )?;
    if args.print_output_path {
        println!("{output}");
    }

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
    } else if let Some(bump) = args.bump {
        resolved.app_version =
            registry::next_version(&registry::base_url(), &resolved.package, bump.into())?;
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

/// Resolve `--key`/`--dev`/`MERO_SIGN_KEY`, in that order; shared by `bundle`
/// and `sign` so both fail on a bad key before doing any real work.
pub fn resolve_signing_key(key: Option<&Path>, dev: bool) -> Result<SigningKey> {
    if let Some(path) = key {
        return mero_sign::load_signing_key(path);
    }
    if dev {
        return Ok(mero_sign::dev_signing_key());
    }
    if let Some(path) = std::env::var_os(SIGN_KEY_ENV) {
        return mero_sign::load_signing_key(PathBuf::from(path).as_path());
    }
    bail!(
        "no signing method given. Pass one of:\n  \
         --key <FILE>   sign with a production key (cargo mero key generate)\n  \
         --dev          sign with the well-known dev key (not publishable)\n\
         or set {SIGN_KEY_ENV} to a key-file path."
    );
}

/// Sign `manifest.json` in place; returns the signerId.
fn sign(manifest_path: &Utf8Path, key: &SigningKey) -> Result<String> {
    if mero_sign::is_dev_key(key) {
        print_dev_warning();
        println!("• signing manifest.json with the DEV key");
    } else {
        println!("• signing manifest.json");
    }
    mero_sign::sign_manifest(manifest_path.as_std_path(), key)?;
    Ok(signer_id_of(key))
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

/// Copy built artifacts into staging and describe them for the manifest: single
/// service at `app.wasm`/`abi.json`, multi-service under `services/<name>.*`.
fn stage(
    meta: &BundleMeta,
    built: &[BuiltWasm],
    staging: &Utf8Path,
    no_abi: bool,
) -> Result<Vec<StagedArtifact>> {
    if meta.services.is_empty() {
        let [one] = built else {
            bail!(
                "expected exactly one built artifact for a single-service app, got {}",
                built.len()
            );
        };
        let wasm = stage_file(&one.wasm, staging, "app.wasm")?;
        let abi = stage_abi(one, staging, "abi.json", no_abi)?;
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
        let abi = stage_abi(
            build,
            staging,
            &format!("services/{}-abi.json", service.name),
            no_abi,
        )?;
        staged.push(StagedArtifact {
            service_name: Some(service.name.clone()),
            wasm,
            abi,
        });
    }
    Ok(staged)
}

/// Stage a build's abi.json, or `None` when `--no-abi` deliberately skipped it.
/// Without `--no-abi`, a missing ABI is still the "SDK generates no ABI" error.
fn stage_abi(
    build: &BuiltWasm,
    staging: &Utf8Path,
    rel: &str,
    no_abi: bool,
) -> Result<Option<BundleArtifact>> {
    if no_abi {
        return Ok(None);
    }
    stage_file(abi_of(build)?, staging, rel).map(Some)
}

/// Every entry in `manifest.json` names an `abi.json`, so an app whose SDK
/// generates no ABI entry point has nothing to bundle.
fn abi_of(build: &BuiltWasm) -> Result<&std::path::Path> {
    build.abi_json.as_deref().ok_or_else(|| {
        eyre!(
            "`{}` was built without an ABI, so it cannot be bundled; \
             its SDK generates no `__calimero_abi` entry point",
            build.crate_name
        )
    })
}

/// Copy one source file to `staging/<rel>` and describe it (path, size, hash).
fn stage_file(src: &std::path::Path, staging: &Utf8Path, rel: &str) -> Result<BundleArtifact> {
    let dest = staging.join(rel);
    let bytes = fs::read(src).wrap_err_with(|| format!("failed to read {}", src.display()))?;
    fs::write(&dest, &bytes).wrap_err_with(|| format!("failed to write {dest}"))?;
    Ok(manifest::artifact_from_bytes(rel, &bytes))
}

/// Default output: `<dist_dir>/<package>-<version>.mpk`. `--output` overrides
/// it verbatim.
fn default_output_path(dist_dir: Utf8PathBuf, package: &str, version: &str) -> Utf8PathBuf {
    dist_dir.join(format!("{package}-{version}.mpk"))
}

fn output_path(args: &BundleArgs, base: &Utf8Path, package: &str, version: &str) -> Utf8PathBuf {
    args.output
        .clone()
        .unwrap_or_else(|| default_output_path(base.join("dist"), package, version))
}

/// Post-bundle summary. The Application line matters most: it decides whether
/// this release lands as an update or a brand-new app.
fn print_summary(
    output: &Utf8Path,
    meta: &BundleMeta,
    application_id: &ApplicationId,
    signer_id: &str,
    no_logo: bool,
) -> Result<()> {
    let size = fs::metadata(output)
        .wrap_err_with(|| format!("failed to stat {output}"))?
        .len();

    // Identifiers are printed in full: an application id gets pasted into
    // commands and compared between releases, and an elision serves neither.
    let details = [
        format!(
            "Name         {}",
            meta.name.as_deref().unwrap_or(&meta.package)
        ),
        format!("Package      {}", meta.package),
        format!("Version      {}", meta.app_version),
        format!("Application  {application_id}"),
        format!("Signer       {signer_id}"),
    ];

    let logo = if no_logo {
        None
    } else {
        meta.icon.as_deref().and_then(logo::render)
    };

    // The path goes on its own full-width line: beside the art it would wrap at
    // the terminal edge and the continuation would run back through the logo.
    println!();
    println!("\u{2713} {}  ({})", short_path(output), human_size(size));
    println!();
    match logo {
        Some(logo) => {
            // Vertically centre the text against a logo that is usually taller.
            let pad = logo.lines.len().saturating_sub(details.len()) / 2;
            let blank = " ".repeat(logo.cols);
            for i in 0..logo.lines.len().max(details.len() + pad) {
                let art = logo.lines.get(i).map_or(blank.as_str(), String::as_str);
                let text = i
                    .checked_sub(pad)
                    .and_then(|d| details.get(d))
                    .map_or("", String::as_str);
                println!("{art}   {text}");
            }
        }
        None => details.iter().for_each(|line| println!("  {line}")),
    }
    Ok(())
}

/// The output path relative to the working directory when it sits underneath it,
/// so a deep absolute path does not dominate the summary.
fn short_path(output: &Utf8Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| Utf8PathBuf::from_path_buf(cwd).ok())
        .and_then(|cwd| output.strip_prefix(&cwd).ok())
        .map_or_else(|| output.to_string(), Utf8Path::to_string)
}

/// Binary-scale file size (1.2 MiB), one decimal place above the byte unit.
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
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
        let paths =
            std::iter::once(&artifact.wasm.path).chain(artifact.abi.as_ref().map(|a| &a.path));
        for rel in paths {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::FeatureArgs;

    fn bundle_args() -> BundleArgs {
        BundleArgs {
            key: None,
            dev: false,
            app_version: None,
            package: None,
            output: None,
            profiling: false,
            manifest_path: None,
            no_abi: false,
            no_icon: false,
            no_logo: false,
            print_output_path: false,
            bump: None,
            features: FeatureArgs::default(),
        }
    }

    #[test]
    fn default_output_path_carries_the_version() {
        assert_eq!(
            default_output_path("dist".into(), "com.example.demo", "1.2.3"),
            Utf8PathBuf::from("dist/com.example.demo-1.2.3.mpk"),
        );
    }

    #[test]
    fn output_flag_still_wins_over_the_versioned_default() {
        let mut args = bundle_args();
        args.output = Some(Utf8PathBuf::from("custom/name.mpk"));
        assert_eq!(
            output_path(&args, Utf8Path::new("base"), "com.example.demo", "1.2.3"),
            Utf8PathBuf::from("custom/name.mpk"),
        );
    }

    #[test]
    fn human_size_picks_the_largest_unit_under_the_value() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1_536), "1.5 KiB");
        assert_eq!(human_size(1_258_291), "1.2 MiB");
    }
}
