//! Bundle metadata parsing from `[package.metadata.calimero]` /
//! `[workspace.metadata.calimero]` tables.

use camino::{Utf8Path, Utf8PathBuf};
use eyre::{eyre, Result};
use serde::Deserialize;
use serde_json::Value;

/// Resolved bundle metadata, ready for Task 6 (build) / Task 7 (bundle+manifest.json).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleMeta {
    pub package: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub author: Option<String>,
    pub min_runtime_version: String,
    pub frontend: Option<String>,
    pub app_version: String,
    pub services: Vec<ServiceMeta>,
}

/// One entry of a workspace-level `services` table; empty on the single-crate side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceMeta {
    pub name: String,
    pub crate_name: String,
}

/// Raw `[..metadata.calimero]` shape, deserialized before defaults are applied.
/// `deny_unknown_fields` turns a snake_case typo (e.g. `min_runtime_version`)
/// into a loud error instead of a silently-ignored field.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawCalimeroMeta {
    package: Option<String>,
    name: Option<String>,
    description: Option<String>,
    author: Option<String>,
    min_runtime_version: Option<String>,
    frontend: Option<String>,
    #[serde(default)]
    services: Vec<RawServiceMeta>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawServiceMeta {
    name: String,
    #[serde(rename = "crate")]
    crate_name: String,
}

const MISSING_PACKAGE_HELP: &str = r#"add a `[package.metadata.calimero]` table (or `[workspace.metadata.calimero]` for a multi-service workspace) with a reverse-DNS app id, e.g.:

[package.metadata.calimero]
package = "com.example.my-app""#;

/// `load` returns this (a distinct, downcastable type) when no usable
/// `[*.metadata.calimero]` package id is available - either the table is absent
/// entirely or it lacks the `package` key. A misplaced-`services` or malformed
/// table is a *different* error, so callers that tolerate a missing table (e.g.
/// `cargo mero build` for a single-crate app) can downcast to this instead of
/// string-matching, and real misconfigurations still surface.
#[derive(Debug)]
pub struct MissingCalimeroPackage;

impl std::fmt::Display for MissingCalimeroPackage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(MISSING_PACKAGE_HELP)
    }
}

impl std::error::Error for MissingCalimeroPackage {}

/// Canonicalize the directory containing a `--manifest-path` so it can be
/// compared against cargo_metadata's always-absolute, canonical manifest paths.
/// A relative or non-canonical path (`./my-app/Cargo.toml`, `a/../a`) would
/// otherwise never match and silently resolve to the wrong package. Falls back
/// to the un-canonicalized dir when the path can't be resolved (then it just
/// won't match, as before).
pub(crate) fn canonical_dir(manifest_path: &Utf8Path) -> Utf8PathBuf {
    let parent = match manifest_path.parent() {
        Some(p) if !p.as_str().is_empty() => p,
        _ => Utf8Path::new("."),
    };
    parent
        .canonicalize_utf8()
        .unwrap_or_else(|_| parent.to_owned())
}

/// True when `pkg_manifest_path`'s parent dir and `query_dir` name the same
/// directory. cargo_metadata reports paths as cargo resolves them, which keeps
/// symlinked components (macOS temp dirs live in `/var/folders`, a symlink to
/// `/private/var/folders`), so BOTH sides must be canonicalized or a
/// `--manifest-path` through a symlink never matches its own package.
pub(crate) fn same_dir(pkg_manifest_path: &Utf8Path, query_dir: &Utf8Path) -> bool {
    fn canon(dir: &Utf8Path) -> Utf8PathBuf {
        dir.canonicalize_utf8().unwrap_or_else(|_| dir.to_owned())
    }
    pkg_manifest_path
        .parent()
        .is_some_and(|dir| canon(dir) == canon(query_dir))
}

/// Service names become on-disk `services/<name>.wasm` staging paths, so a name
/// must be a simple path-safe identifier. The metadata comes from a possibly
/// untrusted third-party `Cargo.toml`, so a name with path separators or `..`
/// could escape the staging dir - reject anything but `[A-Za-z0-9_-]`.
fn validate_service_name(name: &str) -> Result<()> {
    let safe = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !safe {
        return Err(eyre!(
            "invalid service name `{name}`: service names may contain only ASCII letters, digits, `-`, and `_`"
        ));
    }
    Ok(())
}

/// Pull the `calimero` subtable out of a cargo `[*.metadata]` value, which cargo
/// delivers nested under the tool key as `{ "calimero": { .. } }`. `None` when
/// the input is absent, not an object, or the subtable is null.
fn calimero_subtable(metadata: Option<&Value>) -> Option<&Value> {
    metadata?.get("calimero").filter(|v| !v.is_null())
}

/// Load bundle metadata for the crate at `manifest_dir`.
///
/// Workspace-level `[workspace.metadata.calimero]` wins over the crate's own
/// `[package.metadata.calimero]` when both are present (multi-service apps
/// configure services at the workspace root).
pub fn load(metadata: &cargo_metadata::Metadata, manifest_dir: &Utf8Path) -> Result<BundleMeta> {
    let package = metadata
        .packages
        .iter()
        .find(|p| same_dir(&p.manifest_path, manifest_dir));

    // The calimero table must come from the package actually resolved: deriving
    // it from the dir match alone meant a failed match plus a root-package
    // fallback reported "missing table" for a manifest that has one.
    let resolved = package.or_else(|| metadata.root_package());
    let package_value = calimero_subtable(resolved.map(|p| &p.metadata));
    let workspace_value = calimero_subtable(Some(&metadata.workspace_metadata));

    match resolved {
        Some(p) => load_from_values(
            workspace_value,
            package_value,
            p.name.as_ref(),
            &p.version.to_string(),
        ),
        // A VIRTUAL workspace root (no root package) is the normal multi-service
        // shape. A missing calimero table stays the downcastable
        // MissingCalimeroPackage so `build` can tolerate it; a present table
        // sources its app_version from [workspace.package].version, left empty
        // when that key is absent - `bundle` enforces a non-empty version once
        // the --app-version override is known.
        None => {
            if workspace_value.is_none() && package_value.is_none() {
                return Err(MissingCalimeroPackage.into());
            }
            let version = workspace_package_version(manifest_dir).unwrap_or_default();
            let name = manifest_dir.file_name().unwrap_or("app");
            load_from_values(workspace_value, package_value, name, &version)
        }
    }
}

/// Read `[workspace.package].version` from the virtual-workspace root manifest.
/// cargo_metadata 0.20 doesn't surface this key, so parse it directly; `None`
/// when the file is unreadable or the key is absent.
fn workspace_package_version(workspace_dir: &Utf8Path) -> Option<String> {
    let text = std::fs::read_to_string(workspace_dir.join("Cargo.toml")).ok()?;
    let doc: toml::Value = toml::from_str(&text).ok()?;
    doc.get("workspace")?
        .get("package")?
        .get("version")?
        .as_str()
        .map(str::to_owned)
}

/// Pure core of `load`: no cargo_metadata dependency, so tests can build the
/// `serde_json::Value` tables directly.
fn load_from_values(
    workspace_value: Option<&Value>,
    package_value: Option<&Value>,
    crate_name: &str,
    crate_version: &str,
) -> Result<BundleMeta> {
    let (value, from_workspace) = match workspace_value {
        Some(v) => (v, true),
        None => match package_value {
            Some(v) => (v, false),
            None => return Err(MissingCalimeroPackage.into()),
        },
    };

    let raw: RawCalimeroMeta = serde_json::from_value(value.clone())
        .map_err(|e| eyre!("invalid [..metadata.calimero] table: {e}"))?;

    if !from_workspace && !raw.services.is_empty() {
        return Err(eyre!(
            "`services` belongs under [workspace.metadata.calimero], not [package.metadata.calimero]"
        ));
    }

    // The workspace table wins for actual metadata, but a stray `services` array
    // under the package table would otherwise go unparsed and unchecked.
    if from_workspace {
        if let Some(pv) = package_value {
            let raw_package: RawCalimeroMeta = serde_json::from_value(pv.clone())
                .map_err(|e| eyre!("invalid [..metadata.calimero] table: {e}"))?;
            if !raw_package.services.is_empty() {
                return Err(eyre!(
                    "`services` belongs under [workspace.metadata.calimero], not [package.metadata.calimero]"
                ));
            }
        }
    }

    let package = raw.package.ok_or(MissingCalimeroPackage)?;

    let services = raw
        .services
        .into_iter()
        .map(|s| {
            validate_service_name(&s.name)?;
            Ok(ServiceMeta {
                name: s.name,
                crate_name: s.crate_name,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    // Duplicate names collide in bundle::stage()'s services/<name>.wasm staging
    // path, silently overwriting one service's files; duplicate crates are fine.
    let mut seen_names = std::collections::HashSet::new();
    for s in &services {
        if !seen_names.insert(&s.name) {
            return Err(eyre!(
                "duplicate service name `{}`: service names must be unique",
                s.name
            ));
        }
    }

    Ok(BundleMeta {
        package,
        name: raw.name.or_else(|| Some(crate_name.to_string())),
        description: raw.description,
        author: raw.author,
        min_runtime_version: raw
            .min_runtime_version
            .unwrap_or_else(|| "0.1.0".to_string()),
        frontend: raw.frontend,
        app_version: crate_version.to_string(),
        services,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    // Guards the wrapper bug this replaced: `load` fed the whole `[package.metadata]`
    // table (nested under `calimero`) into the parser, so it never saw the fields.
    // `load` extracts via this fn, so covering it fails a plain `cargo test` if the
    // subtable extraction regresses.
    #[test]
    fn calimero_subtable_unwraps_the_tool_key() {
        let nested = json!({ "calimero": { "package": "com.example.app" } });
        assert_eq!(
            calimero_subtable(Some(&nested)),
            Some(&json!({ "package": "com.example.app" }))
        );
        assert!(calimero_subtable(Some(&json!({ "other": 1 }))).is_none());
        assert!(calimero_subtable(None).is_none());
        assert!(calimero_subtable(Some(&Value::Null)).is_none());
    }

    #[test]
    fn missing_table_is_typed_but_misplaced_services_is_not() {
        // No calimero table at all -> the downcastable missing-table error.
        let absent = load_from_values(None, None, "app", "0.1.0").unwrap_err();
        assert!(absent.downcast_ref::<MissingCalimeroPackage>().is_some());

        // A present-but-misconfigured table (services under the package table) is
        // a real error, NOT missing-table, so `build`'s fallback won't swallow it.
        let misplaced = json!({
            "package": "com.example.app",
            "services": [{ "name": "api", "crate": "api-service" }],
        });
        let err = load_from_values(None, Some(&misplaced), "app", "0.1.0").unwrap_err();
        assert!(err.downcast_ref::<MissingCalimeroPackage>().is_none());
        assert!(err.to_string().contains("[workspace.metadata.calimero]"));
    }

    #[test]
    fn single_crate_package_metadata() {
        let package_value = json!({ "package": "com.example.my-app" });

        let meta = load_from_values(None, Some(&package_value), "my-app", "1.2.3").unwrap();

        assert_eq!(meta.package, "com.example.my-app");
        assert_eq!(meta.name.as_deref(), Some("my-app"));
        assert_eq!(meta.app_version, "1.2.3");
        assert_eq!(meta.min_runtime_version, "0.1.0");
        assert!(meta.services.is_empty());
    }

    #[test]
    fn workspace_metadata_with_services() {
        let workspace_value = json!({
            "package": "com.example.suite",
            "services": [
                { "name": "api", "crate": "api-service" },
                { "name": "worker", "crate": "worker-service" },
            ],
        });

        let meta = load_from_values(Some(&workspace_value), None, "suite", "0.5.0").unwrap();

        assert_eq!(meta.package, "com.example.suite");
        assert_eq!(meta.services.len(), 2);
        assert_eq!(meta.services[0].name, "api");
        assert_eq!(meta.services[0].crate_name, "api-service");
        assert_eq!(meta.services[1].name, "worker");
        assert_eq!(meta.services[1].crate_name, "worker-service");
    }

    #[test]
    fn missing_package_field_is_actionable() {
        let err = load_from_values(None, None, "my-app", "0.1.0")
            .unwrap_err()
            .to_string();

        assert!(err.contains("[package.metadata.calimero]"));
        assert!(err.contains(r#"package = "com.example.my-app""#));
    }

    #[test]
    fn services_in_package_table_rejected() {
        let package_value = json!({
            "package": "com.example.my-app",
            "services": [{ "name": "api", "crate": "api-service" }],
        });

        let err = load_from_values(None, Some(&package_value), "my-app", "1.0.0")
            .unwrap_err()
            .to_string();

        assert!(err.contains("[workspace.metadata.calimero]"));
    }

    #[test]
    fn missing_package_key_in_present_table_is_actionable() {
        let package_value = json!({ "name": "my-app" });

        let err = load_from_values(None, Some(&package_value), "my-app", "1.0.0")
            .unwrap_err()
            .to_string();

        assert!(err.contains("[package.metadata.calimero]"));
        assert!(err.contains(r#"package = "com.example.my-app""#));
    }

    #[test]
    fn reads_workspace_package_version_from_root_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"2.3.4\"\n",
        )
        .unwrap();
        assert_eq!(workspace_package_version(dir).as_deref(), Some("2.3.4"));

        std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        assert_eq!(workspace_package_version(dir), None);
    }

    #[test]
    fn rejects_path_traversal_service_name() {
        for bad in ["../../../../etc/evil", "a/b", "a\\b", "..", "", "svc.wasm"] {
            let workspace_value = json!({
                "package": "com.example.suite",
                "services": [{ "name": bad, "crate": "api-service" }],
            });
            let err = load_from_values(Some(&workspace_value), None, "suite", "0.5.0")
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("invalid service name"),
                "`{bad}` should be rejected, got: {err}"
            );
        }
    }

    #[test]
    fn canonical_dir_resolves_noncanonical_manifest_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();
        std::fs::create_dir(dir.join("app")).unwrap();
        std::fs::write(dir.join("app/Cargo.toml"), "").unwrap();

        // A non-canonical --manifest-path (extra `.`/`..` components, exactly
        // what a relative invocation from a workspace root produces) must
        // resolve to the same canonical dir cargo_metadata reports, so the
        // package-by-dir lookup in `load`/`resolve_targets` matches.
        let messy = dir.join("app/../app/./Cargo.toml");
        assert_eq!(
            canonical_dir(&messy),
            dir.join("app").canonicalize_utf8().unwrap()
        );
        assert!(canonical_dir(&messy).is_absolute());
    }

    #[cfg(unix)]
    #[test]
    fn same_dir_matches_through_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();
        std::fs::create_dir(dir.join("real")).unwrap();
        std::fs::write(dir.join("real/Cargo.toml"), "").unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("link").as_std_path()).unwrap();

        // cargo_metadata may report the package via one spelling while the
        // query dir arrives via the other (macOS temp dirs: /var/folders is a
        // symlink to /private/var/folders). Both directions must match, or
        // `bundle --manifest-path` in a temp dir resolves the wrong package.
        assert!(same_dir(&dir.join("link/Cargo.toml"), &dir.join("real")));
        assert!(same_dir(&dir.join("real/Cargo.toml"), &dir.join("link")));
        assert!(!same_dir(&dir.join("real/Cargo.toml"), dir));
    }

    #[test]
    fn duplicate_service_names_rejected() {
        let workspace_value = json!({
            "package": "com.example.suite",
            "services": [
                { "name": "api", "crate": "api-service" },
                { "name": "api", "crate": "other-service" },
            ],
        });

        let err = load_from_values(Some(&workspace_value), None, "suite", "0.5.0")
            .unwrap_err()
            .to_string();
        assert!(err.contains("duplicate service name"));
        assert!(err.contains("api"));

        // Distinct names deliberately pointing at the same crate are fine.
        let same_crate = json!({
            "package": "com.example.suite",
            "services": [
                { "name": "api", "crate": "shared-service" },
                { "name": "worker", "crate": "shared-service" },
            ],
        });
        let meta = load_from_values(Some(&same_crate), None, "suite", "0.5.0").unwrap();
        assert_eq!(meta.services.len(), 2);
    }

    #[test]
    fn misplaced_services_rejected_even_when_workspace_table_wins() {
        let workspace_value = json!({ "package": "com.example.workspace-wins" });
        let package_value = json!({
            "package": "com.example.package-loses",
            "services": [{ "name": "api", "crate": "api-service" }],
        });

        let err = load_from_values(
            Some(&workspace_value),
            Some(&package_value),
            "my-app",
            "1.0.0",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("[workspace.metadata.calimero]"));
    }

    #[test]
    fn workspace_table_wins_over_package_table() {
        let workspace_value = json!({ "package": "com.example.workspace-wins" });
        let package_value = json!({ "package": "com.example.package-loses" });

        let meta = load_from_values(
            Some(&workspace_value),
            Some(&package_value),
            "my-app",
            "1.0.0",
        )
        .unwrap();

        assert_eq!(meta.package, "com.example.workspace-wins");
    }
}
