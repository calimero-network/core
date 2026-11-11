// The release a bundle's `minRuntimeVersion` is checked against (`packages.rs`).
// Prefer workspace metadata when building in-repo, but fall back to package version
// when building from a published crate tarball.

use std::path::{Path, PathBuf};

fn main() {
    let version = read_workspace_version().unwrap_or_else(|| {
        let pkg_version = std::env::var("CARGO_PKG_VERSION")
            .expect("CARGO_PKG_VERSION is not set by Cargo for build script");
        println!(
            "cargo:warning=unable to resolve workspace metadata version; \
             using CARGO_PKG_VERSION={pkg_version}"
        );
        pkg_version
    });

    println!("cargo:rustc-env=CALIMERO_RELEASE_VERSION={version}");
    deprecated_routes();
}

/// `api-deprecations.toml`'s deprecated routes, as `(method, route template, Unix seconds of
/// its `date`)` in `$OUT_DIR/deprecated_routes.rs`, for the `Deprecation` header
/// (`api::deprecation`). `scripts/check-api-compat.py` checks the file's shape; a file this
/// cannot read fails the build, since a deprecation must never be silently dropped.
fn deprecated_routes() {
    let dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR"));
    let file = dir.join("api-deprecations.toml");
    println!("cargo:rerun-if-changed={}", file.display());
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let doc: toml::Table = toml::from_str(&text).expect("api-deprecations.toml is not TOML");
    let mut out = String::from("&[\n");
    let entries = doc.get("deprecated").and_then(toml::Value::as_array);
    for entry in entries.into_iter().flatten() {
        let Some(route) = entry.get("route").and_then(toml::Value::as_str) else {
            continue;
        };
        let (method, path) = route
            .split_once(' ')
            .expect("api-deprecations.toml: a route is `METHOD /path`");
        let Some(toml::Value::Datetime(toml::value::Datetime {
            date: Some(date), ..
        })) = entry.get("date")
        else {
            panic!("api-deprecations.toml: {route} needs `date`, a TOML date");
        };
        let days = days_from_civil(date.year.into(), date.month.into(), date.day.into());
        out.push_str(&format!("    ({method:?}, {path:?}, {}),\n", days * 86_400));
    }
    out.push_str("]\n");
    let target = PathBuf::from(std::env::var("OUT_DIR").expect("Cargo sets OUT_DIR"));
    std::fs::write(target.join("deprecated_routes.rs"), out).expect("write deprecated_routes.rs");
}

/// Days from 1970-01-01 to a proleptic Gregorian date (H. Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn read_workspace_version() -> Option<String> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let (version, version_file) = read_workspace_version_for_dir(Path::new(&manifest_dir))?;

    // Re-run build script when workspace version changes.
    println!("cargo:rerun-if-changed={}", version_file.display());
    Some(version)
}

fn read_workspace_version_for_dir(manifest_dir: &Path) -> Option<(String, PathBuf)> {
    let mut dir = manifest_dir.to_path_buf();

    loop {
        let cargo_toml = dir.join("Cargo.toml");

        if cargo_toml.exists() {
            if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
                if let Some(version) = parse_workspace_metadata_version(&content) {
                    return Some((version, cargo_toml));
                }
            }
        }

        if !dir.pop() {
            break;
        }
    }

    None
}

fn parse_workspace_metadata_version(content: &str) -> Option<String> {
    let value: toml::Value = toml::from_str(content).ok()?;
    let version = value
        .get("workspace")?
        .get("metadata")?
        .get("workspaces")?
        .get("version")?
        .as_str()?
        .trim();

    (!version.is_empty()).then(|| version.to_string())
}
