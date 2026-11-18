//! The apps an app's release `uses`, from its crate: the declarations
//! (`[package.metadata.calimero.uses]` in `Cargo.toml`), the lock that pins each to a release
//! (`calimero.lock`) and that release's vendored ABI (`calimero/abi/<name>.json`), checked
//! against each other, and the typed clients generated from them
//! (`calimero_app_abi::codegen::rust`).
//!
//! An app's `build.rs` is one line, and its code brings the clients in with `app::uses!()`:
//!
//! ```ignore
//! // build.rs
//! fn main() {
//!     calimero_app_build::uses::build();
//! }
//! // src/lib.rs
//! app::uses!();
//! // .. uses::calendar()?.add_event(&title, &at, None)?;
//! ```
//!
//! `meroctl app uses update` writes the lock and the vendored ABI ([`vendor`]), after checking
//! the release is inside the declared range; `meroctl app uses check` and every build check
//! that the three agree ([`check`]), and `meroctl app pack` packs them into the manifest.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use calimero_app_abi::codegen::rust::{uses_module, Use};
use calimero_app_abi::AppAbi;
use eyre::{bail, eyre, Result as EyreResult, WrapErr};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The lock, beside the crate's `Cargo.toml`.
pub const LOCK_FILE: &str = "calimero.lock";
/// Where each reference's ABI is vendored, as `<name>.json`.
pub const ABI_DIR: &str = "calimero/abi";
/// What `build.rs` writes into `OUT_DIR`.
pub const OUT_FILE: &str = "uses.rs";

/// One reference, as `[package.metadata.calimero.uses]` declares it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declared {
    pub package: String,
    /// A semver requirement (`^0.1`).
    pub range: String,
    /// `create` (made with each instance) or `attach` (the owner links one).
    pub mode: String,
    /// The methods the app calls as the person it serves, once they grant it.
    pub methods: Vec<String>,
}

/// One reference, as `calimero.lock` pins it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Locked {
    pub name: String,
    pub package: String,
    /// The release whose ABI is vendored.
    pub version: String,
    /// The range the lock was made for: a changed declaration shows as a stale lock.
    pub range: String,
    pub mode: String,
    /// `sha256:<hex>` of the vendored ABI file.
    pub abi: String,
}

/// A reference with its lock entry and vendored ABI, checked against each other.
#[derive(Clone, Debug)]
pub struct Reference {
    pub name: String,
    pub declared: Declared,
    pub locked: Locked,
    pub abi: AppAbi,
}

/// The `uses` table of the crate at `dir`; empty when it declares none.
pub fn declared(dir: &Path) -> EyreResult<BTreeMap<String, Declared>> {
    let path = dir.join("Cargo.toml");
    let text = fs::read_to_string(&path).wrap_err_with(|| format!("reading {}", path.display()))?;
    let doc: toml::Table = text
        .parse()
        .wrap_err_with(|| format!("parsing {}", path.display()))?;
    let Some(table) = doc
        .get("package")
        .and_then(|p| p.get("metadata"))
        .and_then(|m| m.get("calimero"))
        .and_then(|c| c.get("uses"))
    else {
        return Ok(BTreeMap::new());
    };
    let table = table
        .as_table()
        .ok_or_else(|| eyre!("[package.metadata.calimero.uses] is not a table"))?;
    let mut out = BTreeMap::new();
    for (name, entry) in table {
        if !is_use_name(name) {
            bail!("uses.{name}: a reference's name is a-z, 0-9 and _, starting with a letter");
        }
        let entry = entry
            .as_table()
            .ok_or_else(|| eyre!("uses.{name}: takes {{ package, range, mode, methods }}"))?;
        if let Some(unknown) = entry
            .keys()
            .find(|k| !matches!(k.as_str(), "package" | "range" | "mode" | "methods"))
        {
            bail!("uses.{name}: unknown key {unknown} (package, range, mode, methods)");
        }
        let text = |key: &str| entry.get(key).and_then(toml::Value::as_str);
        let mode = text("mode").unwrap_or("create").to_owned();
        if mode != "create" && mode != "attach" {
            bail!("uses.{name}: mode is create or attach, not {mode}");
        }
        let methods = match entry.get("methods") {
            None => Vec::new(),
            Some(m) => m
                .as_array()
                .and_then(|a| {
                    a.iter()
                        .map(|m| m.as_str().map(str::to_owned))
                        .collect::<Option<Vec<_>>>()
                })
                .ok_or_else(|| eyre!("uses.{name}: methods is a list of method names"))?,
        };
        let _ = out.insert(
            name.clone(),
            Declared {
                package: text("package")
                    .ok_or_else(|| eyre!("uses.{name}: no package"))?
                    .to_owned(),
                range: text("range")
                    .ok_or_else(|| eyre!("uses.{name}: no range"))?
                    .to_owned(),
                mode,
                methods,
            },
        );
    }
    Ok(out)
}

/// The name rule `calimero-bundle` enforces at install (`is_use_name`).
fn is_use_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// The entries of the crate's `calimero.lock`; empty when it has none.
pub fn read_lock(dir: &Path) -> EyreResult<Vec<Locked>> {
    let path = dir.join(LOCK_FILE);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(eyre!("reading {}: {e}", path.display())),
    };
    let doc: toml::Table = text
        .parse()
        .wrap_err_with(|| format!("parsing {}", path.display()))?;
    let Some(entries) = doc.get("uses") else {
        return Ok(Vec::new());
    };
    entries
        .as_array()
        .ok_or_else(|| eyre!("{}: uses is not a list", path.display()))?
        .iter()
        .map(|e| {
            let field = |key: &str| {
                e.get(key)
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| eyre!("{}: an entry has no {key}", path.display()))
            };
            Ok(Locked {
                name: field("name")?,
                package: field("package")?,
                version: field("version")?,
                // A lock written before ranges were recorded reads as stale.
                range: field("range").unwrap_or_default(),
                mode: field("mode")?,
                abi: field("abi")?,
            })
        })
        .collect()
}

fn write_lock(dir: &Path, mut entries: Vec<Locked>) -> EyreResult<()> {
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    let mut text = String::from("# Written by `meroctl app uses update`; do not edit by hand.\n");
    for l in &entries {
        let _ = write!(text, "\n[[uses]]\n");
        for (key, value) in [
            ("name", &l.name),
            ("package", &l.package),
            ("version", &l.version),
            ("range", &l.range),
            ("mode", &l.mode),
            ("abi", &l.abi),
        ] {
            let _ = writeln!(text, "{key} = {}", toml::Value::String(value.clone()));
        }
    }
    let path = dir.join(LOCK_FILE);
    fs::write(&path, text).wrap_err_with(|| format!("writing {}", path.display()))
}

/// Where the ABI of reference `name` is vendored.
#[must_use]
pub fn abi_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(ABI_DIR).join(format!("{name}.json"))
}

/// `sha256:<hex>` of `bytes`, as the lock and the signed manifest name a vendored ABI.
#[must_use]
pub fn digest(bytes: &[u8]) -> String {
    let mut out = String::from("sha256:");
    for b in Sha256::digest(bytes) {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Pins reference `name` to release `version` of its package, whose ABI is `abi`: writes the
/// ABI to `calimero/abi/<name>.json` and the reference's entry in `calimero.lock`. The caller
/// has checked that `version` is a release of the declared package inside the declared range
/// (`meroctl app uses update` does).
pub fn vendor(dir: &Path, name: &str, version: &str, abi: &Value) -> EyreResult<Locked> {
    let decls = declared(dir)?;
    let d = decls
        .get(name)
        .ok_or_else(|| eyre!("{name} is not in [package.metadata.calimero.uses]"))?;
    let parsed = AppAbi::from_json(abi).wrap_err_with(|| format!("{name}: the ABI"))?;
    if let Some(missing) = d.methods.iter().find(|m| !parsed.methods.contains_key(*m)) {
        bail!(
            "uses.{name} lists method {missing}, which {} {version} does not have",
            d.package
        );
    }
    let mut bytes = serde_json::to_vec_pretty(abi)?;
    bytes.push(b'\n');
    let path = abi_path(dir, name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).wrap_err_with(|| format!("creating {}", parent.display()))?;
    }
    fs::write(&path, &bytes).wrap_err_with(|| format!("writing {}", path.display()))?;
    let locked = Locked {
        name: name.to_owned(),
        package: d.package.clone(),
        version: version.to_owned(),
        range: d.range.clone(),
        mode: d.mode.clone(),
        abi: digest(&bytes),
    };
    let mut entries: Vec<Locked> = read_lock(dir)?
        .into_iter()
        .filter(|l| l.name != name)
        .collect();
    entries.push(locked.clone());
    write_lock(dir, entries)?;
    Ok(locked)
}

/// Every declared reference with its lock entry and vendored ABI: package, range and mode as
/// declared, the ABI as the lock hashed it, every listed method in it, and nothing locked
/// that is not declared. Whether the version is inside the range is `meroctl`'s check
/// (`app uses update|check`, `app pack`), which knows semver.
pub fn check(dir: &Path) -> EyreResult<Vec<Reference>> {
    let decls = declared(dir)?;
    let lock = read_lock(dir)?;
    if let Some(stray) = lock.iter().find(|l| !decls.contains_key(&l.name)) {
        bail!(
            "{LOCK_FILE} pins {}, which [package.metadata.calimero.uses] does not declare: run `meroctl app uses update`",
            stray.name
        );
    }
    let mut out = Vec::new();
    for (name, d) in decls {
        let l = lock
            .iter()
            .find(|l| l.name == name)
            .ok_or_else(|| {
                eyre!("uses.{name} is declared but not locked: run `meroctl app uses update`")
            })?
            .clone();
        if l.package != d.package || l.range != d.range || l.mode != d.mode {
            bail!(
                "{LOCK_FILE} pins {name} as {} {} ({}) but Cargo.toml declares {} {} ({}): run `meroctl app uses update`",
                l.package, l.range, l.mode, d.package, d.range, d.mode
            );
        }
        let path = abi_path(dir, &name);
        let bytes = fs::read(&path).wrap_err_with(|| format!("reading {}", path.display()))?;
        if digest(&bytes) != l.abi {
            bail!(
                "{} is not the ABI {LOCK_FILE} pins for {name} ({}): run `meroctl app uses update`",
                path.display(),
                l.abi
            );
        }
        let json: Value = serde_json::from_slice(&bytes)
            .wrap_err_with(|| format!("reading {}", path.display()))?;
        let abi =
            AppAbi::from_json(&json).wrap_err_with(|| format!("reading {}", path.display()))?;
        if let Some(missing) = d.methods.iter().find(|m| !abi.methods.contains_key(*m)) {
            bail!(
                "uses.{name} lists method {missing}, which {} {} does not have",
                d.package,
                l.version
            );
        }
        out.push(Reference {
            name,
            declared: d,
            locked: l,
            abi,
        });
    }
    Ok(out)
}

/// The `uses` module of the crate at `dir`, after [`check`].
pub fn module(dir: &Path) -> EyreResult<String> {
    let refs = check(dir)?;
    let uses: Vec<(Use<'_>, &AppAbi)> = refs
        .iter()
        .map(|r| {
            (
                Use {
                    name: &r.name,
                    package: &r.locked.package,
                    version: &r.locked.version,
                    range: &r.locked.range,
                    mode: &r.locked.mode,
                    methods: &r.declared.methods,
                },
                &r.abi,
            )
        })
        .collect();
    uses_module(&uses)
}

/// An app's whole `build.rs`: [`generate`], failing the build with what to fix (a build
/// script reports an error by panicking).
pub fn build() {
    if let Err(e) = generate() {
        panic!("{e:#}");
    }
}

/// For an app's `build.rs`: checks the crate's references (`CARGO_MANIFEST_DIR`) and writes
/// the `uses` module to `$OUT_DIR/uses.rs`, rebuilt when `Cargo.toml`, the lock or a vendored
/// ABI changes.
pub fn generate() -> EyreResult<()> {
    let dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .ok_or_else(|| eyre!("CARGO_MANIFEST_DIR is not set: call this from build.rs"))?,
    );
    let out = PathBuf::from(
        std::env::var_os("OUT_DIR")
            .ok_or_else(|| eyre!("OUT_DIR is not set: call this from build.rs"))?,
    );
    println!("cargo::rerun-if-changed=Cargo.toml");
    println!("cargo::rerun-if-changed={LOCK_FILE}");
    println!("cargo::rerun-if-changed={ABI_DIR}");
    let code = module(&dir)?;
    let path = out.join(OUT_FILE);
    fs::write(&path, code).wrap_err_with(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const CARGO: &str = r#"
[package]
name = "x"
version = "0.1.0"
[package.metadata.calimero.uses]
kv = { package = "com.example.kv", range = "^0.1", mode = "attach", methods = ["get"] }
"#;

    fn abi() -> Value {
        json!({
            "abiVersion": 1,
            "methods": {
                "get": { "kind": "view", "params": [["key", "string"]], "returns": "option<string>", "allowApps": ["*"] },
                "set": { "kind": "call", "params": [["key", "string"]], "allowApps": ["*"] },
            },
            "errors": { "Empty": {} },
        })
    }

    fn crate_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), CARGO).unwrap();
        dir
    }

    #[test]
    fn vendoring_writes_a_lock_that_checks_and_generates() {
        let dir = crate_dir();
        let locked = vendor(dir.path(), "kv", "0.1.2", &abi()).unwrap();
        assert_eq!(locked.mode, "attach");
        let lock = fs::read_to_string(dir.path().join(LOCK_FILE)).unwrap();
        assert!(lock.contains("version = \"0.1.2\""), "{lock}");
        assert!(lock.contains("range = \"^0.1\""), "{lock}");
        assert_eq!(read_lock(dir.path()).unwrap(), [locked]);
        let refs = check(dir.path()).unwrap();
        assert_eq!(refs.len(), 1);
        let code = module(dir.path()).unwrap();
        // `methods` limits the client to those.
        assert!(code.contains("pub fn get("), "{code}");
        assert!(!code.contains("pub fn set("), "{code}");
    }

    #[test]
    fn a_drifted_abi_a_stray_lock_and_a_missing_one_are_refused() {
        let dir = crate_dir();
        assert!(check(dir.path())
            .unwrap_err()
            .to_string()
            .contains("not locked"));
        let _ = vendor(dir.path(), "kv", "0.1.0", &abi()).unwrap();
        fs::write(abi_path(dir.path(), "kv"), b"{}").unwrap();
        assert!(check(dir.path())
            .unwrap_err()
            .to_string()
            .contains("is not the ABI"));
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        assert!(check(dir.path())
            .unwrap_err()
            .to_string()
            .contains("does not declare"));
    }

    #[test]
    fn a_changed_declaration_makes_the_lock_stale() {
        let dir = crate_dir();
        let _ = vendor(dir.path(), "kv", "0.1.0", &abi()).unwrap();
        fs::write(dir.path().join("Cargo.toml"), CARGO.replace("^0.1", "^0.2")).unwrap();
        assert!(check(dir.path())
            .unwrap_err()
            .to_string()
            .contains("run `meroctl app uses update`"));
    }

    #[test]
    fn a_listed_method_the_release_lacks_and_a_bad_declaration_are_refused() {
        let dir = crate_dir();
        fs::write(
            dir.path().join("Cargo.toml"),
            CARGO.replace("[\"get\"]", "[\"nope\"]"),
        )
        .unwrap();
        assert!(vendor(dir.path(), "kv", "0.1.0", &abi())
            .unwrap_err()
            .to_string()
            .contains("does not have"));
        fs::write(
            dir.path().join("Cargo.toml"),
            CARGO.replace("attach", "borrow"),
        )
        .unwrap();
        assert!(declared(dir.path())
            .unwrap_err()
            .to_string()
            .contains("create or attach"));
        fs::write(dir.path().join("Cargo.toml"), CARGO.replace("kv =", "Kv =")).unwrap();
        assert!(declared(dir.path())
            .unwrap_err()
            .to_string()
            .contains("a-z"));
    }
}
