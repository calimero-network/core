//! `cargo mero test`: node-free native test runner. Runs the TestHost unit
//! tests + `tests/converge.rs` via a plain `cargo test`; no node, no Docker.

use std::process::Command;

use camino::Utf8Path;
use cargo_metadata::{DependencyKind, Metadata, Package};
use eyre::{Context, Result};

use crate::{workspace, TestArgs};

/// The dev-dep line `cargo mero new` writes, quoted back at the user.
fn example_dev_dep() -> String {
    crate::templates::testing_dev_dep(crate::DEFAULT_SDK_VERSION).unwrap_or_default()
}

pub fn run(args: &TestArgs) -> Result<()> {
    preflight(args);

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let mut cmd = Command::new(cargo);
    cmd.args(assemble_cargo_test_args(
        args.manifest_path.as_deref(),
        &args.args,
    ));

    let status = cmd.status().wrap_err("failed to spawn `cargo test`")?;
    std::process::exit(status.code().unwrap_or(1));
}

/// `trailing_var_arg` strips the user's `--`, so re-insert it: without it cargo
/// parses forwarded flags like `--nocapture` as its own and rejects them.
fn assemble_cargo_test_args(manifest_path: Option<&Utf8Path>, user_args: &[String]) -> Vec<String> {
    let mut out = vec!["test".to_string()];
    if let Some(mp) = manifest_path {
        out.push("--manifest-path".to_string());
        out.push(mp.to_string());
    }
    if !user_args.is_empty() {
        out.push("--".to_string());
        out.extend(user_args.iter().cloned());
    }
    out
}

/// Best-effort warning, never blocks: a missing `testing` feature means the
/// TestHost tests won't build, but `cargo test` will say so loudly on its own.
fn preflight(args: &TestArgs) {
    if let Ok(metadata) = workspace::metadata_for(args.manifest_path.as_deref()) {
        if let Some(hint) = missing_testing_feature(&metadata, args.manifest_path.as_deref()) {
            eprintln!("warning: {hint}");
        }
    }
}

/// Dependency facts projected out of `cargo_metadata::Dependency`, which tests
/// cannot construct (non-`Default` fields like `VersionReq`).
struct DepFacts<'a> {
    name: &'a str,
    is_dev: bool,
    has_testing_feature: bool,
}

/// Hint when the target package's dev-deps lack `calimero-storage`'s `testing`
/// feature. A virtual workspace root resolves to no package, so check every
/// member instead of silently passing.
fn missing_testing_feature(
    metadata: &Metadata,
    manifest_path: Option<&Utf8Path>,
) -> Option<String> {
    if let Some(package) = resolve_package(metadata, manifest_path) {
        return missing_testing_feature_for_package(package);
    }

    let offenders: Vec<&str> = metadata
        .workspace_packages()
        .into_iter()
        .filter(|p| missing_testing_feature_for_package(p).is_some())
        .map(|p| p.name.as_str())
        .collect();
    if offenders.is_empty() {
        return None;
    }
    Some(format!(
        "packages missing the `testing` dev-dependency on `calimero-storage`: {}. Add \
         this to each one's Cargo.toml:\n\n[dev-dependencies]\n{}",
        offenders.join(", "),
        example_dev_dep()
    ))
}

/// Same resolution order as `build`: the package at `--manifest-path`, else the
/// root package. `None` on a virtual workspace root with no `--manifest-path`.
fn resolve_package<'a>(
    metadata: &'a Metadata,
    manifest_path: Option<&Utf8Path>,
) -> Option<&'a Package> {
    manifest_path
        .map(workspace::manifest_dir)
        .and_then(|dir| workspace::package_in_dir(metadata, &dir))
        .or_else(|| metadata.root_package())
}

fn missing_testing_feature_for_package(package: &Package) -> Option<String> {
    let facts: Vec<DepFacts<'_>> = package
        .dependencies
        .iter()
        .map(|d| DepFacts {
            name: d.name.as_str(),
            is_dev: d.kind == DependencyKind::Development,
            has_testing_feature: d.features.iter().any(|f| f == "testing"),
        })
        .collect();
    missing_testing_feature_core(&facts)
}

fn missing_testing_feature_core(deps: &[DepFacts<'_>]) -> Option<String> {
    let present = deps
        .iter()
        .any(|d| d.is_dev && d.name == "calimero-storage" && d.has_testing_feature);
    if present {
        return None;
    }
    Some(format!(
        "no dev-dependency on `calimero-storage` with the `testing` feature was found; \
         the scaffolded TestHost tests and tests/converge.rs need it to build. Add this to \
         your Cargo.toml:\n\n[dev-dependencies]\n{}",
        example_dev_dep()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn present_when_dev_dep_has_testing_feature() {
        let deps = [DepFacts {
            name: "calimero-storage",
            is_dev: true,
            has_testing_feature: true,
        }];
        assert!(missing_testing_feature_core(&deps).is_none());
    }

    #[test]
    fn absent_gives_actionable_hint_with_the_exact_toml_line() {
        // Regular (non-dev) dependency doesn't count, nor does a dev-dep missing
        // the feature.
        let deps = [
            DepFacts {
                name: "calimero-storage",
                is_dev: false,
                has_testing_feature: true,
            },
            DepFacts {
                name: "calimero-storage",
                is_dev: true,
                has_testing_feature: false,
            },
        ];
        let hint = missing_testing_feature_core(&deps).unwrap();
        assert!(hint.contains(&example_dev_dep()));
    }

    #[test]
    fn empty_deps_is_missing() {
        assert!(missing_testing_feature_core(&[]).is_some());
    }

    #[test]
    fn assembles_bare_name_filter_after_separator() {
        let args = assemble_cargo_test_args(None, &["my_test".to_string()]);
        assert_eq!(args, vec!["test", "--", "my_test"]);
    }

    #[test]
    fn assembles_hyphenated_libtest_flag_after_separator() {
        let manifest = Utf8Path::new("app/Cargo.toml");
        let args = assemble_cargo_test_args(Some(manifest), &["--nocapture".to_string()]);
        assert_eq!(
            args,
            vec![
                "test",
                "--manifest-path",
                "app/Cargo.toml",
                "--",
                "--nocapture"
            ]
        );
    }

    #[test]
    fn no_separator_when_no_passthrough_args() {
        assert_eq!(assemble_cargo_test_args(None, &[]), vec!["test"]);
    }
}
