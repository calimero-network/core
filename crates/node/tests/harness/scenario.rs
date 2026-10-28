//! The scenario runner: scenario files on an in-process cluster
//! (`tests/harness/cluster.rs`), run by `tests/scenarios.rs`.
//!
//! Two formats (both documented in `docs/src/content/docs/contribute/testing.mdx`):
//!
//! - **App-level scenarios**, `apps/<app>/scenarios/<name>.json`: the JSON array of
//!   `calimero_app_sdk::testing::scenario` (`call`/`view`/`tick`/`advance`/`removeMember`,
//!   `as`, `expect`, `save`, `$var`, `@account`, `events`), the same file the app's own
//!   harness test runs. Here it runs on a real cluster: the app is built (a component with
//!   `calimero-app-build`, a wasmer app with `cargo mero build`), the organization of
//!   `<name>.org.json` is created with invites and teams, the app is deployed in it, and each
//!   step goes to node `step % nodes` as its `as` account. [`run_app_scenario`].
//! - **Cluster scenarios**, `apps/<app>/scenarios/cluster/**/*.json` and
//!   `workflows/**/scenarios/**/*.json`: an object `{ name, description, cluster, accounts,
//!   steps }` whose steps are node-level operations (orgs, invites, teams, packages,
//!   upgrades, blobs, raw HTTP) besides calls and views. The format is merobox's cloud-v2
//!   workflow format as JSON (they were converted by `scripts/workflow-to-scenario.py`): the
//!   same step types and fields, `{{var}}` placeholders, `outputs`, `expected_error`,
//!   `expected_failure`, `allow_failure`, and merobox's `assert` / `json_assert` statements
//!   with Python's semantics (`harness/py.rs`). [`run_cluster_scenario`].
//!
//! Limits from the environment: `SCENARIO_WAIT_CAP_SECS` (default 2) caps a `wait` step
//! unless it says `"exact": true` (the node replicates synchronously; merobox's waits were
//! for v1's gossip); `SCENARIO_FUZZY_SECONDS` (default 10) is how long a `fuzzy_test` runs
//! (the soak lengths are the Python suite's); `SCENARIO_REPEAT_CAP` (default none) caps a
//! `repeat` step's count. Node process faults (`kill_node`, `pause_node`, `restart_node`, ...)
//! are not steps here: they run against real processes, in the Python fault suite.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use calimero_client::{Client, Consistency, Error as ClientError, Session};
use calimero_identity::testing::SoftAuthenticator;
use futures_util::StreamExt;
use serde_json::{json, Map, Value};
use sha2::Digest as _;

use crate::cluster::{Cluster, ORIGIN};
use crate::py;

/// The repository root.
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

// ------------------------------------------------------------------ fixtures

/// Where the converted scenarios find what they install: `{{env.CORE_WASM_DIR}}`.
/// `CORE_WASM_DIR` if set, else `target/workflow-wasm` under the cargo target directory.
pub fn wasm_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CORE_WASM_DIR") {
        return PathBuf::from(d);
    }
    let target =
        std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root().join("target"), PathBuf::from);
    target.join("workflow-wasm")
}

/// Builds are serialized: two builds of one crate into one target directory race.
static BUILDING: Mutex<()> = Mutex::new(());

/// `PATH` with the repository's own `cargo-mero` first: `CARGO_MERO_BIN_DIR` (CI's), else
/// the target directory's `debug/` (`scripts/setup-cargo-mero.sh` builds it there).
fn mero_path() -> std::ffi::OsString {
    let target =
        std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root().join("target"), PathBuf::from);
    let mut dirs: Vec<PathBuf> = std::env::var_os("CARGO_MERO_BIN_DIR")
        .map(PathBuf::from)
        .into_iter()
        .chain(std::iter::once(target.join("debug")))
        .filter(|d| d.join("cargo-mero").exists())
        .collect();
    dirs.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    std::env::join_paths(dirs).unwrap_or_default()
}

fn run_cmd(cmd: &mut std::process::Command) -> Result<(), String> {
    let out = cmd
        .env("CARGO_INCREMENTAL", "0")
        .env("PATH", mero_path())
        .output()
        .map_err(|e| format!("{cmd:?}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let tail = |b: &[u8]| {
        let s = String::from_utf8_lossy(b);
        s.chars()
            .rev()
            .take(3000)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>()
    };
    Err(format!(
        "{cmd:?} failed:\n{}\n{}",
        tail(&out.stdout),
        tail(&out.stderr)
    ))
}

/// The crate name of the app at `dir` (its `Cargo.toml` `[package] name`).
fn crate_name(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("Cargo.toml")).ok()?;
    let doc: toml::Table = text.parse().ok()?;
    doc.get("package")?.get("name")?.as_str().map(str::to_owned)
}

/// The app directory under `apps/` (up to three levels down, e.g. `apps/migrations/*`)
/// whose crate is `krate` (`-` and `_` alike).
fn app_dir_of_crate(krate: &str) -> Option<PathBuf> {
    fn crates(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name();
            if !p.is_dir()
                || matches!(
                    name.to_str(),
                    Some("target" | "res" | "node_modules" | "src")
                )
            {
                continue;
            }
            if p.join("Cargo.toml").exists() {
                out.push(p.clone());
            }
            if depth > 1 {
                crates(&p, depth - 1, out);
            }
        }
    }
    let want = krate.replace('-', "_");
    let mut dirs: Vec<PathBuf> = Vec::new();
    crates(&root().join("apps"), 3, &mut dirs);
    dirs.sort();
    dirs.into_iter()
        .find(|d| crate_name(d).is_some_and(|n| n.replace('-', "_") == want))
}

/// Whether the crate at `dir` is a `calimero:app` component (the v2 SDK).
fn is_component(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join("Cargo.toml")).is_ok_and(|t| t.contains("calimero-app-sdk"))
}

/// The app at `dir` (relative to the repository root or absolute) as WASM: a component built
/// with `calimero-app-build` (cached by source hash under `target/app-components`, as
/// `tests/common/mod.rs` does), a wasmer app with `cargo mero build` (its ABI embedded).
pub fn build_app(dir: &Path) -> Result<Vec<u8>, String> {
    let root = root();
    let dir = if dir.is_absolute() {
        dir.to_owned()
    } else {
        root.join(dir)
    };
    let _one = BUILDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if is_component(&dir) {
        let sdk = root.join("crates/app-sdk");
        let macros = root.join("crates/app-sdk-macros");
        let wit = root.join("crates/app-wit");
        let target =
            std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
        return calimero_app_build::build_component_cached(
            &dir,
            &[&sdk, &macros, &wit],
            &target.join("app-components"),
        )
        .map_err(|e| format!("build {}: {e}", dir.display()));
    }
    let krate = crate_name(&dir).ok_or_else(|| format!("{}: no crate", dir.display()))?;
    run_cmd(
        std::process::Command::new("cargo")
            .args(["mero", "build", "--manifest-path"])
            .arg(dir.join("Cargo.toml"))
            .current_dir(&root),
    )?;
    let out = dir
        .join("res")
        .join(format!("{}.wasm", krate.replace('-', "_")));
    std::fs::read(&out).map_err(|e| format!("{}: {e}", out.display()))
}

/// The bytes of a file a scenario names, made first when it is a fixture that is missing:
/// `<CORE_WASM_DIR>/<crate>.wasm` (`cargo mero build` of that app),
/// `<CORE_WASM_DIR>/dist/<package>-<version>.mpk` (`scripts/build-workflow-fixtures.py`, which
/// bundles the signed releases), or a random `static/generated/blob-<N>mib.bin` (what
/// `apps/blobs/scripts/gen-blob-fixtures.sh` makes; random, so no node holds it already).
pub fn fixture(path: &Path) -> Result<Vec<u8>, String> {
    if let Ok(b) = std::fs::read(path) {
        return Ok(b);
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_owned();
    if let Some(mib) = name
        .strip_prefix("blob-")
        .and_then(|n| n.strip_suffix("mib.bin"))
        .and_then(|n| n.parse::<usize>().ok())
    {
        if path
            .parent()
            .is_some_and(|p| p.ends_with("static/generated"))
        {
            let mut rng = Rng::seeded();
            let mut bytes = vec![0u8; mib * 1024 * 1024];
            for chunk in bytes.chunks_mut(8) {
                let r = rng.next_u64().to_le_bytes();
                chunk.copy_from_slice(&r[..chunk.len()]);
            }
            return Ok(bytes);
        }
    }
    let dir = wasm_dir();
    if !path.starts_with(&dir) {
        return Err(format!("file not found: {}", path.display()));
    }
    if name.ends_with(".mpk") {
        build_mpk(&dir, &name)?;
    } else if let Some(krate) = name.strip_suffix(".wasm") {
        let built = match app_dir_of_crate(krate) {
            Some(app) => build_app(&app)?,
            // A migration fixture's WASM comes out of its bundle.
            None => {
                build_mpk_for_crate(&dir, krate)?;
                return std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()));
            }
        };
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(path, &built).map_err(|e| e.to_string())?;
        return Ok(built);
    }
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn fixtures_script(dir: &Path, filters: &[String]) -> Result<(), String> {
    let _one = BUILDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    run_cmd(
        std::process::Command::new("python3")
            .arg(root().join("scripts/build-workflow-fixtures.py"))
            .args(filters)
            .env("CORE_WASM_DIR", dir)
            .current_dir(root()),
    )
}

/// `dist/<package>-<version>.mpk`, by `scripts/build-workflow-fixtures.py`'s filter
/// `<package>@<version>` (a release fixture), else the package's app name (a multi-service app).
fn build_mpk(dir: &Path, name: &str) -> Result<(), String> {
    let stem = name.trim_end_matches(".mpk");
    let (package, version) = stem
        .rsplit_once('-')
        .ok_or_else(|| format!("{name}: not <package>-<version>.mpk"))?;
    fixtures_script(dir, &[format!("{package}@{version}")])?;
    if dir.join("dist").join(name).exists() {
        return Ok(());
    }
    let app = package.strip_prefix("com.calimero.").unwrap_or(package);
    fixtures_script(dir, &[app.to_owned()])
}

/// A migration fixture's `<crate>.wasm`: the script writes it beside the bundle it makes.
fn build_mpk_for_crate(dir: &Path, krate: &str) -> Result<(), String> {
    fixtures_script(dir, &[krate.replace('_', "-")])
}

// ------------------------------------------------------------------ registry

/// The package registry a scenario's cluster installs from, on 127.0.0.1, laid out as the
/// real one is (`artifacts/{package}/{version}/{file}`). `install_package` publishes to it.
#[derive(Clone, Default)]
pub struct Registry {
    files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    pub url: String,
}

impl Registry {
    pub async fn start() -> Self {
        use axum::extract::Path as AxPath;
        use axum::http::StatusCode;
        use axum::response::IntoResponse;
        let files: Arc<Mutex<BTreeMap<String, Vec<u8>>>> = Arc::default();
        let served = Arc::clone(&files);
        let app = axum::Router::new().route(
            "/artifacts/{package}/{version}/{file}",
            axum::routing::get(
                move |AxPath((package, version, _file)): AxPath<(String, String, String)>| {
                    let files = Arc::clone(&served);
                    async move {
                        let got = files
                            .lock()
                            .unwrap()
                            .get(&format!("{package}@{version}"))
                            .cloned();
                        match got {
                            Some(b) => (StatusCode::OK, b).into_response(),
                            None => StatusCode::NOT_FOUND.into_response(),
                        }
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(tokio::spawn(
            async move { axum::serve(listener, app).await },
        ));
        Self {
            files,
            url: format!("http://{addr}/"),
        }
    }

    /// Puts a `.mpk` where the nodes fetch it; answers its `(package, appVersion)`.
    pub fn publish(&self, mpk: Vec<u8>) -> Result<(String, String), String> {
        let (package, version) = mpk_coords(&mpk)?;
        let _prev = self
            .files
            .lock()
            .unwrap()
            .insert(format!("{package}@{version}"), mpk);
        Ok((package, version))
    }
}

/// `(package, appVersion)` from a `.mpk`'s top-level `manifest.json`.
fn mpk_coords(mpk: &[u8]) -> Result<(String, String), String> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(mpk));
    for entry in tar.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        if path.to_string_lossy().trim_start_matches("./") == "manifest.json" {
            let mut text = String::new();
            let _n = entry.read_to_string(&mut text).map_err(|e| e.to_string())?;
            let m: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            let s = |k: &str| match &m[k] {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            return Ok((s("package"), s("appVersion")));
        }
    }
    Err("not a .mpk: no manifest.json".into())
}

// ------------------------------------------------------------------ random

/// xorshift64*, seeded from the clock: `fuzzy_test`'s picks and generated values.
pub struct Rng(u64);

impl Rng {
    pub fn seeded() -> Self {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let t = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        let n = N.fetch_add(0x9E37_79B9_7F4A_7C15, std::sync::atomic::Ordering::Relaxed);
        Self((t ^ n) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// An integer in `lo..=hi`.
    pub fn int(&mut self, lo: i64, hi: i64) -> i64 {
        if hi <= lo {
            return lo;
        }
        let span = (hi - lo) as u64 + 1;
        lo + (self.next_u64() % span) as i64
    }

    fn uuid(&mut self) -> String {
        let a = self.next_u64();
        let b = self.next_u64();
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&a.to_le_bytes());
        bytes[8..].copy_from_slice(&b.to_le_bytes());
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let h = hex::encode(bytes);
        format!(
            "{}-{}-{}-{}-{}",
            &h[0..8],
            &h[8..12],
            &h[12..16],
            &h[16..20],
            &h[20..32]
        )
    }
}

// ------------------------------------------------------------------ placeholders

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.' || c == '-'
}

/// The expression of `{{ expr }}`'s inside, if it is one: `name(.part)*` alternatives
/// joined by `??` (merobox's `_PLACEHOLDER`).
fn placeholder_expr(inner: &str) -> Option<String> {
    let parts: Vec<&str> = inner.split("??").map(str::trim).collect();
    if parts
        .iter()
        .all(|p| !p.is_empty() && p.chars().all(is_name_char))
    {
        Some(parts.join("??"))
    } else {
        None
    }
}

/// Every placeholder in `s`, leftmost first, as `(start, end, expr)`.
fn placeholders(s: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(at) = s[i..].find("{{") {
        let start = i + at;
        let Some(close) = s[start + 2..].find("}}") else {
            break;
        };
        let end = start + 2 + close;
        match placeholder_expr(&s[start + 2..end]) {
            Some(expr) => {
                out.push((start, end + 2, expr));
                i = end + 2;
            }
            None => i = start + 1,
        }
    }
    out
}

fn lookup_path(vars: &Value, path: &str) -> Result<Value, String> {
    let mut cur = vars;
    for part in path.split('.') {
        cur = match cur {
            Value::Object(m) if m.contains_key(part) => &m[part],
            Value::Array(a) if part.chars().all(|c| c.is_ascii_digit()) && !part.is_empty() => {
                match part.parse::<usize>().ok().and_then(|i| a.get(i)) {
                    Some(v) => v,
                    None => return Err(format!("unresolved placeholder {{{{{path}}}}}")),
                }
            }
            _ => return Err(format!("unresolved placeholder {{{{{path}}}}}")),
        };
    }
    Ok(cur.clone())
}

/// `a.b.c`, or `a ?? b`: the first alternative that resolves.
fn lookup(vars: &Value, expr: &str) -> Result<Value, String> {
    let alts: Vec<&str> = expr.split("??").map(str::trim).collect();
    for alt in &alts[..alts.len() - 1] {
        if let Ok(v) = lookup_path(vars, alt) {
            return Ok(v);
        }
    }
    lookup_path(vars, alts[alts.len() - 1])
}

/// merobox's `resolve`: a string that is one placeholder keeps the variable's type; a
/// placeholder inside text is spliced in as `json.dumps` of a non-string.
pub fn resolve(value: &Value, vars: &Value) -> Result<Value, String> {
    Ok(match value {
        Value::String(s) => {
            let t = s.trim();
            let found = placeholders(t);
            if found.len() == 1 && found[0].0 == 0 && found[0].1 == t.len() {
                return lookup(vars, &found[0].2);
            }
            let found = placeholders(s);
            if found.is_empty() {
                return Ok(value.clone());
            }
            let mut out = String::new();
            let mut at = 0;
            for (start, end, expr) in found {
                out.push_str(&s[at..start]);
                let v = lookup(vars, &expr)?;
                out.push_str(&match v {
                    Value::String(x) => x,
                    other => py::dumps(&other),
                });
                at = end;
            }
            out.push_str(&s[at..]);
            Value::String(out)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|v| resolve(v, vars))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| Ok((k.clone(), resolve(v, vars)?)))
                .collect::<Result<_, String>>()?,
        ),
        other => other.clone(),
    })
}

// ------------------------------------------------------------------ assertions

/// A `{{placeholder}}` in an operand that names no variable.
struct Unresolved(String);

/// An operand's value (merobox's `resolve_operand`): a whole `{{name}}` keeps the
/// variable's type, an embedded one is spliced in as Python's `str`, a quoted literal
/// loses its quotes. Names are looked up as they are, never as dotted paths.
fn resolve_operand(value: &str, vars: &Map<String, Value>) -> Result<Value, Unresolved> {
    let b = value.as_bytes();
    if b.len() >= 2
        && b[0] == b[b.len() - 1]
        && (b[0] == b'\'' || b[0] == b'"')
        && !value.contains("{{")
        && !value.contains("}}")
    {
        return Ok(Value::String(value[1..value.len() - 1].to_owned()));
    }
    if !value.contains("{{") || !value.contains("}}") {
        return Ok(Value::String(value.to_owned()));
    }
    if value.starts_with("{{") && value.ends_with("}}") && value.matches("{{").count() == 1 {
        let name = value[2..value.len() - 2].trim();
        return vars
            .get(name)
            .cloned()
            .ok_or_else(|| Unresolved(value.to_owned()));
    }
    let mut result = value.to_owned();
    let mut start = 0;
    loop {
        let Some(open) = result[start..].find("{{").map(|i| i + start) else {
            break;
        };
        let Some(close) = result[open..].find("}}").map(|i| i + open) else {
            break;
        };
        let name = result[open + 2..close].trim().to_owned();
        let text = vars
            .get(&name)
            .map(py::str_of)
            .ok_or_else(|| Unresolved(format!("{{{{{name}}}}}")))?;
        result = format!("{}{}{}", &result[..open], text, &result[close + 2..]);
        start = open + text.len();
    }
    Ok(Value::String(result))
}

/// `name(...)`, case-insensitively, as merobox's `_call`.
fn is_call(expr: &str, name: &str) -> bool {
    expr.to_lowercase().starts_with(&format!("{name}(")) && expr.ends_with(')')
}

fn args_of<'e>(expr: &'e str, name: &str) -> &'e str {
    &expr[name.len() + 1..expr.len() - 1]
}

fn is_empty_value(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty() || matches!(s.as_str(), "''" | "\"\"" | "[]" | "{}"),
        Value::Array(a) => a.is_empty(),
        Value::Object(m) => m.is_empty(),
        _ => false,
    }
}

/// One `assert` statement: `(passed, detail)` (merobox's `eval_statement`).
fn eval_statement(
    statement: &str,
    vars: &Map<String, Value>,
) -> Result<(bool, String), Unresolved> {
    let expr = statement.trim();
    let r = |o: &str| resolve_operand(o, vars);
    for (func, negate) in [("contains", false), ("not_contains", true)] {
        if is_call(expr, func) {
            let args: Vec<&str> = args_of(expr, func).split(',').map(str::trim).collect();
            if args.len() != 2 {
                return Ok((false, format!("(invalid {func} arg count)")));
            }
            let (a, b) = (r(args[0])?, r(args[1])?);
            let ok = py::str_of(&a).contains(&py::str_of(&b)) != negate;
            return Ok((
                ok,
                format!("(left={}, right={})", py::repr(&a), py::repr(&b)),
            ));
        }
    }
    if is_call(expr, "regex") {
        let args: Vec<&str> = args_of(expr, "regex").split(',').map(str::trim).collect();
        if args.len() != 2 {
            return Ok((false, "(invalid regex arg count)".into()));
        }
        let (a, pattern) = (r(args[0])?, r(args[1])?);
        let ok = match regex_lite_search(&py::str_of(&pattern), &py::str_of(&a)) {
            Ok(m) => m,
            Err(e) => return Ok((false, format!("(bad pattern: {e})"))),
        };
        return Ok((
            ok,
            format!("(left={}, pattern={})", py::repr(&a), py::repr(&pattern)),
        ));
    }
    if is_call(expr, "is_set") {
        let a = r(args_of(expr, "is_set").trim())?;
        let ok = !matches!(&a, Value::Null) && a != json!("") && a != json!([]) && a != json!({});
        return Ok((ok, format!("(value={})", py::repr(&a))));
    }
    if is_call(expr, "is_empty") {
        let a = r(args_of(expr, "is_empty").trim())?;
        return Ok((is_empty_value(&a), format!("(value={})", py::repr(&a))));
    }
    for (func, negate) in [
        ("equal", false),
        ("equals", false),
        ("not_equal", true),
        ("not_equals", true),
    ] {
        if is_call(expr, func) {
            let body = args_of(expr, func);
            let Some((x, y)) = body.split_once(',') else {
                return Ok((false, "(invalid equality arg count)".into()));
            };
            let (a, b) = (r(x.trim())?, r(y.trim())?);
            let ok = py::eq(&a, &b) != negate;
            return Ok((
                ok,
                format!("(left={}, right={})", py::repr(&a), py::repr(&b)),
            ));
        }
    }
    // `\s(==|!=|>=|<=|>|<)\s`, leftmost.
    for (i, c) in expr.char_indices() {
        if !c.is_whitespace() {
            continue;
        }
        let rest = &expr[i + c.len_utf8()..];
        for op in ["==", "!=", ">=", "<=", ">", "<"] {
            if let Some(after) = rest.strip_prefix(op) {
                if !after.starts_with(char::is_whitespace) {
                    continue;
                }
                let ws = after.chars().next().map_or(1, char::len_utf8);
                let left = r(expr[..i].trim())?;
                let right = r(after[ws..].trim())?;
                let passed = match (py::float(&left), py::float(&right)) {
                    (Some(l), Some(rn)) => match op {
                        "==" => l == rn,
                        "!=" => l != rn,
                        ">" => l > rn,
                        ">=" => l >= rn,
                        "<" => l < rn,
                        _ => l <= rn,
                    },
                    _ => {
                        let (l, rs) = (py::str_of(&left), py::str_of(&right));
                        match op {
                            "==" => l == rs,
                            "!=" => l != rs,
                            ">" => l > rs,
                            ">=" => l >= rs,
                            "<" => l < rs,
                            _ => l <= rs,
                        }
                    }
                };
                return Ok((
                    passed,
                    format!("(left={}, right={})", py::repr(&left), py::repr(&right)),
                ));
            }
        }
    }
    Ok((false, "(unrecognized statement)".into()))
}

/// `re.search(pattern, text)` for the patterns the scenarios use: literal text with `.`,
/// `^`, `$`, `\d`, `\w`, `\s`, `[...]`, `*`, `+`, `?` and alternation-free groups. A
/// pattern beyond that is an error, never a silent pass.
fn regex_lite_search(pattern: &str, text: &str) -> Result<bool, String> {
    #[derive(Clone, Debug)]
    enum Atom {
        Char(char),
        Any,
        Class(Vec<(char, char)>, bool),
        Digit(bool),
        Word(bool),
        Space(bool),
    }
    #[derive(Clone, Debug)]
    struct Piece {
        atom: Atom,
        min: usize,
        max: usize,
    }
    fn matches(a: &Atom, c: char) -> bool {
        match a {
            Atom::Char(x) => *x == c,
            Atom::Any => c != '\n',
            Atom::Class(ranges, neg) => {
                ranges.iter().any(|(lo, hi)| (*lo..=*hi).contains(&c)) != *neg
            }
            Atom::Digit(neg) => c.is_ascii_digit() != *neg,
            Atom::Word(neg) => (c.is_alphanumeric() || c == '_') != *neg,
            Atom::Space(neg) => c.is_whitespace() != *neg,
        }
    }
    let mut chars = pattern.chars().peekable();
    let mut pieces = Vec::new();
    let (mut anchored_start, mut anchored_end) = (false, false);
    if chars.peek() == Some(&'^') {
        anchored_start = true;
        let _ = chars.next();
    }
    while let Some(c) = chars.next() {
        let atom = match c {
            '$' if chars.peek().is_none() => {
                anchored_end = true;
                break;
            }
            '.' => Atom::Any,
            '\\' => match chars.next().ok_or("trailing backslash")? {
                'd' => Atom::Digit(false),
                'D' => Atom::Digit(true),
                'w' => Atom::Word(false),
                'W' => Atom::Word(true),
                's' => Atom::Space(false),
                'S' => Atom::Space(true),
                'n' => Atom::Char('\n'),
                't' => Atom::Char('\t'),
                x if !x.is_alphanumeric() => Atom::Char(x),
                x => return Err(format!("unsupported escape \\{x}")),
            },
            '[' => {
                let mut neg = false;
                if chars.peek() == Some(&'^') {
                    neg = true;
                    let _ = chars.next();
                }
                let mut ranges = Vec::new();
                let mut first = true;
                loop {
                    let x = chars.next().ok_or("unterminated class")?;
                    if x == ']' && !first {
                        break;
                    }
                    first = false;
                    let x = if x == '\\' {
                        chars.next().ok_or("unterminated class")?
                    } else {
                        x
                    };
                    if chars.peek() == Some(&'-') {
                        let _ = chars.next();
                        match chars.peek() {
                            Some(']') | None => {
                                ranges.push((x, x));
                                ranges.push(('-', '-'));
                            }
                            Some(&y) => {
                                let _ = chars.next();
                                ranges.push((x, y));
                            }
                        }
                    } else {
                        ranges.push((x, x));
                    }
                }
                Atom::Class(ranges, neg)
            }
            '(' | ')' | '|' | '{' | '}' => return Err(format!("unsupported regex syntax {c:?}")),
            '*' | '+' | '?' => return Err("nothing to repeat".into()),
            x => Atom::Char(x),
        };
        let (min, max) = match chars.peek() {
            Some('*') => (0, usize::MAX),
            Some('+') => (1, usize::MAX),
            Some('?') => (0, 1),
            _ => (1, 1),
        };
        if (min, max) != (1, 1) {
            let _ = chars.next();
        }
        pieces.push(Piece { atom, min, max });
    }
    fn go(p: &[Piece], t: &[char], end: bool) -> bool {
        let Some((first, rest)) = p.split_first() else {
            return !end || t.is_empty();
        };
        let mut n = 0;
        while n < first.max && n < t.len() && matches(&first.atom, t[n]) {
            n += 1;
        }
        loop {
            if n >= first.min && go(rest, &t[n..], end) {
                return true;
            }
            if n == 0 || n <= first.min {
                return false;
            }
            n -= 1;
        }
    }
    let t: Vec<char> = text.chars().collect();
    let starts = if anchored_start { 0..=0 } else { 0..=t.len() };
    Ok(starts
        .into_iter()
        .any(|s| go(&pieces, &t[s..], anchored_end)))
}

/// Splits at the first comma outside a string and outside `{}`/`[]` (merobox's
/// `_split_operands`).
fn split_operands(body: &str) -> Vec<String> {
    let (mut depth, mut in_string, mut escaped) = (0i32, false, false);
    for (i, c) in body.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
        } else if c == '"' {
            in_string = !in_string;
        } else if in_string {
            continue;
        } else if c == '{' || c == '[' {
            depth += 1;
        } else if c == '}' || c == ']' {
            depth -= 1;
        } else if c == ',' && depth == 0 {
            return vec![body[..i].trim().to_owned(), body[i + 1..].trim().to_owned()];
        }
    }
    vec![body.trim().to_owned()]
}

fn normalize_json(v: Value) -> Value {
    match &v {
        Value::String(s) => py::loads(s).unwrap_or(v),
        _ => v,
    }
}

/// Whether `right` is contained in `left` (dict keys recursively, list items).
fn is_subset(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Object(l), Value::Object(r)) => r
            .iter()
            .all(|(k, v)| l.get(k).is_some_and(|lv| is_subset(lv, v))),
        (Value::Array(l), Value::Array(r)) => {
            r.iter().all(|item| l.iter().any(|x| py::eq(x, item)))
        }
        _ => py::eq(left, right),
    }
}

/// One `json_assert` statement: `(passed, description, left, right)`.
fn eval_json_statement(
    statement: &str,
    vars: &Map<String, Value>,
) -> Result<(bool, String, Value, Value), Unresolved> {
    let expr = statement.trim();
    for (func, desc, mode) in [
        ("json_not_equal", "JSON inequality", "not_equal"),
        ("not_equal", "JSON inequality", "not_equal"),
        ("json_equal", "JSON equality", "equal"),
        ("equal", "JSON equality", "equal"),
        ("json_subset", "JSON subset", "subset"),
        ("subset", "JSON subset", "subset"),
    ] {
        if is_call(expr, func) {
            let parts = split_operands(args_of(expr, func));
            if parts.len() != 2 {
                return Ok((false, desc.into(), Value::Null, Value::Null));
            }
            let left = resolve_operand(&parts[0], vars)?;
            let right = resolve_operand(&parts[1], vars)?;
            let (ln, rn) = (normalize_json(left.clone()), normalize_json(right.clone()));
            let passed = match mode {
                "equal" => py::eq(&ln, &rn),
                "not_equal" => !py::eq(&ln, &rn),
                _ => is_subset(&ln, &rn),
            };
            return Ok((passed, desc.into(), left, right));
        }
    }
    Ok((
        false,
        "Unrecognized JSON assertion".into(),
        Value::Null,
        Value::Null,
    ))
}

/// An `assert` or `json_assert` step: `Ok(())` when it passes (or is `non_blocking`).
fn run_assert(step: &Map<String, Value>, vars: &Map<String, Value>) -> Result<(), String> {
    let label = step
        .get("name")
        .or_else(|| step.get("type"))
        .map(py::str_of)
        .unwrap_or_default();
    let Some(Value::Array(statements)) = step
        .get("statements")
        .filter(|s| s.as_array().is_some_and(|a| !a.is_empty()))
    else {
        return Err(format!("{label}: 'statements' must be a non-empty list"));
    };
    let is_json = step.get("type") == Some(&json!("json_assert"));
    let mut failed = Vec::new();
    for (idx, stmt) in statements.iter().enumerate() {
        let (text, message) = match stmt {
            Value::String(s) => (s.clone(), None),
            Value::Object(m) => match m.get("statement") {
                Some(Value::String(s)) => (s.clone(), m.get("message").map(py::str_of)),
                Some(_) => {
                    failed.push(format!("invalid statement at #{}", idx + 1));
                    continue;
                }
                None => {
                    return Err(format!(
                        "{label}: statement #{} missing 'statement'",
                        idx + 1
                    ))
                }
            },
            _ => {
                return Err(format!(
                    "{label}: statement #{} must be a string or dict with 'statement'",
                    idx + 1
                ))
            }
        };
        if is_json {
            match eval_json_statement(&text, vars) {
                Ok((true, ..)) => {}
                Ok((false, desc, l, r)) => failed.push(format!(
                    "{} failed: {text}\n  left={}\n  right={}",
                    message.unwrap_or(desc),
                    py::repr(&l),
                    py::repr(&r)
                )),
                Err(Unresolved(p)) => failed.push(format!(
                    "{text}: {p:?} contains a placeholder that never resolved"
                )),
            }
        } else {
            match eval_statement(&text, vars) {
                Ok((true, _)) => {}
                Ok((false, detail)) => failed.push(format!(
                    "{} failed {detail}",
                    message.unwrap_or_else(|| format!("Assertion #{}: {text}", idx + 1))
                )),
                Err(Unresolved(p)) => failed.push(format!(
                    "{text}: {p:?} contains a placeholder that never resolved"
                )),
            }
        }
    }
    if failed.is_empty()
        || (!is_json && step.get("non_blocking").and_then(Value::as_bool) == Some(true))
    {
        return Ok(());
    }
    Err(format!("{label}: assertion failed\n{}", failed.join("\n")))
}

// ------------------------------------------------------------------ the cluster runner

/// How a step was refused: the node's answer (`message` is `"{status} {code}: {message}"`,
/// `details` its details as text), or the runner's own refusal shaped the same way.
#[derive(Debug)]
struct Refusal {
    message: String,
    code: String,
    details: String,
}

/// A step's failure: a refusal (which `expected_error` and friends can expect), or the
/// scenario is wrong or a check failed (which nothing expects).
enum Fail {
    Refused(Refusal),
    Workflow(String),
}

impl From<ClientError> for Fail {
    fn from(e: ClientError) -> Self {
        Fail::Refused(match e {
            ClientError::Api {
                status,
                code,
                message,
                details,
            } => Refusal {
                message: format!("{status} {code}: {message}"),
                code,
                details: details.to_string(),
            },
            ClientError::Transport(r) => Refusal {
                message: format!("{r:#}"),
                code: "TRANSPORT".into(),
                details: "null".into(),
            },
        })
    }
}

impl From<String> for Fail {
    fn from(e: String) -> Self {
        Fail::Workflow(e)
    }
}

fn refused(message: &str, code: &str, details: &Value) -> Fail {
    Fail::Refused(Refusal {
        message: message.to_owned(),
        code: code.to_owned(),
        details: py::str_of(details),
    })
}

type StepResult = Result<Value, Fail>;

/// Required fields per step type (merobox's `STEP_TYPES`), for the ones this runner runs.
const STEP_TYPES: &[(&str, &[&str])] = &[
    ("deploy", &[]),
    ("migration_status", &["apps"]),
    ("upload_bundle", &["wasm"]),
    ("create_app", &[]),
    ("install_package", &["mpk"]),
    ("upgrade_app", &["app"]),
    ("uninstall_package", &["package"]),
    ("delete_app", &["app"]),
    ("packages", &[]),
    ("call", &["app", "method"]),
    ("view", &["app", "method"]),
    ("wait_for", &["app", "method"]),
    ("events", &["app"]),
    ("whoami", &[]),
    ("create_api_key", &[]),
    ("create_org", &["org_name"]),
    ("create_team", &["org", "team_name"]),
    ("invite", &["org"]),
    ("redeem_invite", &["token"]),
    ("remove_member", &["org", "member"]),
    ("team_add", &["org", "team", "members"]),
    ("team_remove", &["org", "team", "members"]),
    ("team_update", &["org", "team"]),
    ("team_get", &["org", "team"]),
    ("team_list", &["org"]),
    ("team_join", &["org", "team"]),
    ("team_delete", &["org", "team"]),
    ("capabilities", &["org", "capabilities"]),
    ("member_capabilities", &["org", "member"]),
    ("add_device", &["account", "device"]),
    ("set_role", &["org", "member", "role"]),
    ("http", &["method", "path"]),
    ("upload_blob", &["app", "file_path"]),
    ("download_blob", &["app", "blob_id"]),
    ("list_blobs", &["app"]),
    ("blob_info", &["app", "blob_id"]),
    ("delete_blob", &["app", "blob_id"]),
    ("publish_presence", &["app", "seq"]),
    ("presence", &["app"]),
    ("transfer_app", &["app", "to"]),
    ("approve_release", &["version"]),
    ("approve_outbound", &["package"]),
    ("upgrade_org", &["org", "package", "version"]),
    ("upgrade_run", &["org", "run"]),
    ("retry_upgrade_run", &["org", "run"]),
    ("find_leader", &["app"]),
    ("wait", &["seconds"]),
    ("set", &["values"]),
    ("assert", &["statements"]),
    ("json_assert", &["statements"]),
    ("repeat", &["count", "steps"]),
    ("fuzzy_test", &["operations"]),
];

/// Steps that act as someone: an `account` (session) or an `api_key`.
const NOT_ACTING: &[&str] = &[
    "wait",
    "set",
    "assert",
    "json_assert",
    "repeat",
    "fuzzy_test",
    "find_leader",
];

/// Node process faults: real processes only (the Python fault suite).
const PROCESS_FAULTS: &[&str] = &[
    "kill_node",
    "pause_node",
    "resume_node",
    "start_node",
    "restart_node",
    "ops",
];

fn validate_steps(steps: &Value, accounts: &[String], where_: &str) -> Result<(), String> {
    let Some(steps) = steps.as_array().filter(|s| !s.is_empty()) else {
        return Err(format!("{where_}: 'steps' must be a non-empty list"));
    };
    for (i, step) in steps.iter().enumerate() {
        let label = format!("{where_} step {}", i + 1);
        let Some(kind) = step.get("type").and_then(Value::as_str) else {
            return Err(format!("{label}: each step needs a 'type'"));
        };
        if PROCESS_FAULTS.contains(&kind) {
            return Err(format!(
                "{label}: '{kind}' is a node process fault; it runs against real processes \
                 (the Python fault suite), not in a cluster scenario"
            ));
        }
        let Some((_, required)) = STEP_TYPES.iter().find(|(t, _)| *t == kind) else {
            return Err(format!("{label}: unknown type '{kind}'"));
        };
        let missing: Vec<&str> = required
            .iter()
            .filter(|f| step.get(**f).is_none())
            .copied()
            .collect();
        if !missing.is_empty() {
            return Err(format!("{label} ({kind}): missing {}", missing.join(", ")));
        }
        if !NOT_ACTING.contains(&kind) {
            if step.get("account").is_none() && step.get("api_key").is_none() {
                return Err(format!("{label} ({kind}): needs 'account' or 'api_key'"));
            }
            if let Some(acct) = step.get("account").and_then(Value::as_str) {
                if !acct.contains("{{") && !accounts.iter().any(|a| a == acct) {
                    return Err(format!("{label}: account '{acct}' is not in 'accounts'"));
                }
            }
        }
        if kind == "deploy" && step.get("wasm").is_some() == step.get("package").is_some() {
            return Err(format!("{label} (deploy): name a 'wasm' or a 'package'"));
        }
        if kind == "repeat" {
            validate_steps(&step["steps"], accounts, &format!("{label} (repeat)"))?;
        }
        if step.get("outputs").is_some_and(|o| !o.is_object()) {
            return Err(format!(
                "{label}: 'outputs' must map variable -> result path"
            ));
        }
    }
    Ok(())
}

/// The id an app sees as `env::context_id()`; the node accepts it for the app.
fn context_id(app: &str) -> String {
    hex::encode(sha2::Sha256::digest(app.as_bytes()))
}

/// An account's public key as apps see it (`env::executor_id`): its id's 32 bytes in hex.
fn account_key(account: &str) -> Option<String> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(account.trim_end_matches('='))
        .ok()
        .map(hex::encode)
}

/// Member rows (`{"account": id, ...}`) with each account's public key as `publicKey`.
fn with_member_keys(rows: &Value) -> Value {
    let Some(rows) = rows.as_array() else {
        return rows.clone();
    };
    Value::Array(
        rows.iter()
            .map(|r| {
                let mut r = r.clone();
                if let Some(key) = r
                    .get("account")
                    .and_then(Value::as_str)
                    .and_then(account_key)
                {
                    if let Some(m) = r.as_object_mut() {
                        let _prev = m.insert("publicKey".into(), Value::String(key));
                    }
                }
                r
            })
            .collect(),
    )
}

/// `result` at `path` (an output's): `""`/`"."` is the whole of it.
fn pick(result: &Value, path: &str) -> Result<Value, String> {
    if path.is_empty() || path == "." {
        return Ok(result.clone());
    }
    let base = if result.is_object() {
        result.clone()
    } else {
        json!({ "value": result })
    };
    lookup(&base, path).map_err(|_| {
        let shown: String = result.to_string().chars().take(300).collect();
        format!("output path '{path}' is not in the step's result: {shown}")
    })
}

fn s(step: &Map<String, Value>, k: &str) -> String {
    step.get(k).map(py::str_of).unwrap_or_default()
}

fn opt_s(step: &Map<String, Value>, k: &str) -> Option<String> {
    step.get(k)
        .filter(|v| !v.is_null() && *v != &json!("") && *v != &json!(false))
        .map(py::str_of)
}

fn as_int(v: &Value) -> Result<i64, String> {
    match v {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .ok_or_else(|| format!("not an integer: {v}")),
        Value::String(t) => t
            .trim()
            .parse()
            .map_err(|_| format!("not an integer: {t:?}")),
        Value::Bool(b) => Ok(i64::from(*b)),
        _ => Err(format!("not an integer: {v}")),
    }
}

fn as_float(v: &Value) -> Result<f64, String> {
    py::float(v).ok_or_else(|| format!("not a number: {v}"))
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(m)) => !m.is_empty(),
    }
}

/// Semver-ish order: numeric parts compare as numbers.
fn version_key(v: &str) -> Vec<(u8, u64, String)> {
    v.split(['.', '+', '-'])
        .map(|p| match p.parse::<u64>() {
            Ok(n) if p.chars().all(|c| c.is_ascii_digit()) => (0, n, String::new()),
            _ => (1, 0, p.to_owned()),
        })
        .collect()
}

/// A parsed cluster scenario file.
pub struct ClusterScenario {
    pub name: String,
    pub nodes: u64,
    /// `cluster.settings`: the TOML every node starts with (its `[outbound]` table).
    pub settings: Option<String>,
    pub accounts: Vec<String>,
    pub variables: Value,
    pub steps: Value,
    pub dir: PathBuf,
}

impl ClusterScenario {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let wf: Value =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let Some(o) = wf.as_object() else {
            return Err("a cluster scenario is a JSON object".into());
        };
        let accounts: Vec<String> = match o.get("accounts") {
            None => Vec::new(),
            Some(Value::Array(a)) if a.iter().all(Value::is_string) => a
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
            Some(_) => return Err("'accounts' must be a list of names".into()),
        };
        let cluster = o.get("cluster").cloned().unwrap_or_else(|| json!({}));
        let dir = path.parent().unwrap_or(Path::new(".")).to_owned();
        let settings = match cluster.get("settings") {
            None | Some(Value::Null) => None,
            Some(Value::String(t)) if t.contains('\n') => Some(t.clone()),
            Some(Value::String(p)) => Some(
                std::fs::read_to_string(dir.join(p))
                    .map_err(|e| format!("cluster.settings: cannot read {p}: {e}"))?,
            ),
            Some(_) => return Err("'cluster.settings' must be a TOML path or inline TOML".into()),
        };
        let steps = o.get("steps").cloned().unwrap_or(Value::Null);
        validate_steps(&steps, &accounts, "scenario")?;
        Ok(Self {
            name: o
                .get("name")
                .map_or_else(|| path.display().to_string(), py::str_of),
            nodes: cluster.get("nodes").and_then(Value::as_u64).unwrap_or(3),
            settings,
            accounts,
            variables: o.get("variables").cloned().unwrap_or_else(|| json!({})),
            steps,
            dir,
        })
    }
}

/// `[outbound]` of a settings TOML, as merod's `--settings` reads it.
fn outbound_of(settings: &str) -> Result<Option<Arc<calimero_outbound::OutboundService>>, String> {
    #[derive(serde::Deserialize, Default)]
    #[serde(default, deny_unknown_fields)]
    struct Settings {
        outbound: calimero_outbound::OutboundSettings,
    }
    let parsed: Settings = toml::from_str(settings).map_err(|e| format!("settings: {e}"))?;
    calimero_outbound::OutboundService::from_settings(&parsed.outbound)
        .map_err(|e| format!("settings: {e:#}"))
}

/// Starts a cluster for `sc` (with its own package registry and settings) and runs it.
pub async fn run_cluster_scenario(path: &Path) -> Result<(), String> {
    let sc = ClusterScenario::load(path)?;
    let registry = Registry::start().await;
    let outbound = match &sc.settings {
        Some(t) => outbound_of(t)?,
        None => None,
    };
    let url = registry.url.clone();
    let c = Cluster::start_with(sc.nodes, move |cfg| {
        cfg.registry = Some(url.clone());
        cfg.outbound.clone_from(&outbound);
        // What `merod` reports as its release (the scenarios read it back).
        cfg.release = env!("CARGO_PKG_VERSION").to_owned();
    })
    .await;
    let mut runner = Runner::new(&c, registry, &sc);
    let out = runner.run(&sc).await;
    drop(runner);
    let mut c = c;
    for i in 0..c.nodes.len() {
        c.stop(i).await;
    }
    out
}

struct Runner<'a> {
    c: &'a Cluster,
    registry: Registry,
    base_dir: PathBuf,
    accounts: Vec<String>,
    vars: Map<String, Value>,
    passkeys: HashMap<String, SoftAuthenticator>,
    sessions: HashMap<String, Session>,
    /// An account's public key (hex) -> its name, for steps that pick their caller by key
    /// (`as_key`).
    by_key: HashMap<String, String>,
    ids: HashMap<String, String>,
    clients: HashMap<(String, usize), Arc<Client>>,
    rng: Rng,
    wait_cap: f64,
}

type Fut<'b, T> = Pin<Box<dyn Future<Output = T> + 'b>>;

impl<'a> Runner<'a> {
    fn new(c: &'a Cluster, registry: Registry, sc: &ClusterScenario) -> Self {
        let mut env: Map<String, Value> = std::env::vars()
            .map(|(k, v)| (k, Value::String(v)))
            .collect();
        let _prev = env.insert(
            "CORE_WASM_DIR".into(),
            Value::String(wasm_dir().display().to_string()),
        );
        let mut vars = Map::new();
        let _prev = vars.insert("env".into(), Value::Object(env));
        Self {
            c,
            registry,
            base_dir: sc.dir.clone(),
            accounts: sc.accounts.clone(),
            vars,
            passkeys: HashMap::new(),
            sessions: HashMap::new(),
            by_key: HashMap::new(),
            ids: HashMap::new(),
            clients: HashMap::new(),
            rng: Rng::seeded(),
            wait_cap: env_f64("SCENARIO_WAIT_CAP_SECS", 2.0),
        }
    }

    fn vars_value(&self) -> Value {
        Value::Object(self.vars.clone())
    }

    fn set_var(&mut self, k: &str, v: Value) {
        let _prev = self.vars.insert(k.to_owned(), v);
    }

    async fn run(&mut self, sc: &ClusterScenario) -> Result<(), String> {
        let initial = resolve(&sc.variables, &self.vars_value())?;
        if let Value::Object(m) = initial {
            self.vars.extend(m);
        }
        self.sign_up().await?;
        let steps = sc.steps.as_array().cloned().unwrap_or_default();
        self.run_steps(&steps, 0).await
    }

    async fn sign_up(&mut self) -> Result<(), String> {
        let mut accounts = Map::new();
        let names = self.accounts.clone();
        for name in names {
            let mut pk = SoftAuthenticator::new("localhost", ORIGIN);
            let client = self.c.client(0);
            let session = client
                .sign_up(&mut pk, &name)
                .await
                .map_err(|e| format!("sign up {name}: {e}"))?;
            let key = account_key(&session.account_id).unwrap_or_default();
            let _p = self.by_key.insert(key.clone(), name.clone());
            let _p = self.ids.insert(name.clone(), session.account_id.clone());
            let _p = accounts.insert(
                name.clone(),
                json!({ "account_id": session.account_id, "public_key": key }),
            );
            self.set_var(&format!("public_key_{name}"), Value::String(key));
            let _p = self.passkeys.insert(name.clone(), pk);
            let _p = self.sessions.insert(name, session);
        }
        self.set_var("accounts", Value::Object(accounts));
        self.set_var("ops_token", json!("ops-secret"));
        Ok(())
    }

    /// The account a step acts as: `as_key`'s account, else `account`.
    fn actor(&self, step: &Map<String, Value>) -> Option<String> {
        let key = s(step, "as_key");
        self.by_key
            .get(&key)
            .cloned()
            .or_else(|| step.get("account").filter(|v| !v.is_null()).map(py::str_of))
    }

    /// The node a step goes to: `node` (1-based) if it names one, else the actor's own:
    /// each account starts at its own node, as each v1 member talked to its own node.
    fn node_for(&self, step: &Map<String, Value>, actor: Option<&str>) -> Result<usize, String> {
        let n = self.c.nodes.len();
        if let Some(node) = step.get("node").filter(|v| !v.is_null()) {
            let i = as_int(node)?;
            if i < 1 || i as usize > n {
                return Err(format!("node {i} is not in the cluster (1..={n})"));
            }
            return Ok(i as usize - 1);
        }
        Ok(actor
            .and_then(|a| self.accounts.iter().position(|x| x == a))
            .map_or(0, |i| i % n))
    }

    /// The client of the step's actor (or API key) on its node.
    async fn client(&mut self, step: &Map<String, Value>) -> Result<Arc<Client>, Fail> {
        let actor = self.actor(step);
        let node = self.node_for(step, actor.as_deref())?;
        let api_key = opt_s(step, "api_key");
        let who = match (&api_key, &actor) {
            (Some(k), _) => format!("k:{k}"),
            (None, Some(a)) => format!("s:{a}"),
            (None, None) => String::new(),
        };
        if let Some(c) = self.clients.get(&(who.clone(), node)) {
            return Ok(Arc::clone(c));
        }
        let cl = self.c.client(node);
        if let Some(k) = &api_key {
            cl.use_api_key(k).await;
        } else if let Some(a) = &actor {
            let session = self
                .sessions
                .get(a)
                .cloned()
                .ok_or_else(|| format!("no session for {a}"))?;
            cl.use_session(session).await;
        }
        let cl = Arc::new(cl);
        let _prev = self.clients.insert((who, node), Arc::clone(&cl));
        Ok(cl)
    }

    fn passkey_name(&self, step: &Map<String, Value>) -> Result<String, Fail> {
        let a = self.actor(step).unwrap_or_default();
        if self.passkeys.contains_key(&a) {
            Ok(a)
        } else {
            Err(Fail::Workflow(format!(
                "{}: needs an 'account' (or 'as_key') whose passkey approves it",
                s(step, "type")
            )))
        }
    }

    /// An account name, an account's public key, or an account id -> the id.
    fn account_id(&self, who: &str) -> String {
        if let Some(id) = self.ids.get(who) {
            return id.clone();
        }
        if let Some(name) = self.by_key.get(who) {
            return self.ids[name].clone();
        }
        who.to_owned()
    }

    fn file(&self, path: &str) -> Result<Vec<u8>, String> {
        let p = Path::new(path);
        let root = root();
        let mut candidates = vec![if p.is_absolute() {
            p.to_owned()
        } else {
            root.join(p)
        }];
        candidates.extend(self.base_dir.ancestors().map(|d| d.join(p)));
        for c in &candidates {
            if c.exists() {
                return fixture(c);
            }
        }
        // A generated fixture is made where the scenario's app keeps it.
        for c in &candidates {
            if c.parent().is_some_and(|d| d.ends_with("static/generated")) {
                if let Some(app) = c.parent().and_then(Path::parent).and_then(Path::parent) {
                    if app.join("Cargo.toml").exists() {
                        return fixture(c);
                    }
                }
            }
        }
        fixture(&candidates[0])
    }

    fn wasm(&self, step: &Map<String, Value>) -> Result<Vec<u8>, String> {
        let p = PathBuf::from(s(step, "wasm"));
        fixture(&if p.is_absolute() {
            p
        } else {
            self.base_dir.join(p)
        })
    }

    fn app_options(&self, step: &Map<String, Value>) -> Value {
        let mut opts = Map::new();
        let mut access = step.get("access").cloned();
        if let Some(Value::Object(a)) = &mut access {
            if let Some(Value::Array(list)) = a.get("accounts").cloned() {
                let ids = list
                    .iter()
                    .map(|v| Value::String(self.account_id(&py::str_of(v))))
                    .collect();
                let _p = a.insert("accounts".into(), Value::Array(ids));
            }
        }
        let mut init = step.get("init_args").cloned();
        if let Some(Value::String(t)) = &init {
            if let Some(v) = py::loads(t) {
                init = Some(v);
            }
        }
        for (key, field, v) in [
            ("init_args", "initArgs", init),
            ("org", "org", step.get("org").cloned()),
            ("access", "access", access),
            (
                "confirm_methods",
                "confirmMethods",
                step.get("confirm_methods").cloned(),
            ),
        ] {
            if step.contains_key(key) {
                let _p = opts.insert(field.into(), v.unwrap_or(Value::Null));
            }
        }
        Value::Object(opts)
    }

    fn release_body(&self, step: &Map<String, Value>) -> Map<String, Value> {
        let mut body = Map::new();
        let _p = body.insert(
            "name".into(),
            json!(opt_s(step, "app_name").unwrap_or_else(|| "app".into())),
        );
        let _p = body.insert("package".into(), step["package"].clone());
        for k in ["version", "service"] {
            if truthy(step.get(k)) {
                let _p = body.insert(k.into(), step[k].clone());
            }
        }
        if let Value::Object(o) = self.app_options(step) {
            body.extend(o);
        }
        body
    }

    async fn post_approved(
        &mut self,
        step: &Map<String, Value>,
        path: &str,
        body: Value,
    ) -> StepResult {
        let cl = self.client(step).await?;
        match cl.post(path, body.clone()).await {
            Err(e) if e.code() == Some("CONFIRM_REQUIRED") => {
                let who = self.actor(step).unwrap_or_default();
                let Some(pk) = self.passkeys.get_mut(&who) else {
                    return Err(e.into());
                };
                Ok(cl.post_confirmed(path, body, pk).await?)
            }
            other => Ok(other?),
        }
    }

    async fn delete_approved(&mut self, step: &Map<String, Value>, path: &str) -> StepResult {
        let cl = self.client(step).await?;
        match cl.delete(path).await {
            Err(e) if e.code() == Some("CONFIRM_REQUIRED") => {
                let who = self.actor(step).unwrap_or_default();
                let Some(pk) = self.passkeys.get_mut(&who) else {
                    return Err(e.into());
                };
                Ok(cl.delete_confirmed(path, pk).await?)
            }
            other => Ok(other?),
        }
    }

    async fn team_step(&mut self, kind: &str, step: &Map<String, Value>) -> StepResult {
        let org = s(step, "org");
        let team = opt_s(step, "team");
        let base = format!("orgs/{org}/teams/{}", team.clone().unwrap_or_default());
        let cl = self.client(step).await?;
        match kind {
            "team_remove" => {
                for m in step["members"].as_array().cloned().unwrap_or_default() {
                    let id = self.account_id(&py::str_of(&m));
                    let _r = cl.delete(&format!("{base}/members/{id}")).await?;
                }
                return Ok(json!({}));
            }
            "team_update" => {
                let mut body = Map::new();
                for k in ["join", "metadata"] {
                    if let Some(v) = step.get(k) {
                        let _p = body.insert(k.into(), v.clone());
                    }
                }
                if let Some(v) = step.get("team_name") {
                    let _p = body.insert("name".into(), v.clone());
                }
                if let Some(v) = step.get("parent") {
                    let _p = body.insert("parent".into(), v.clone());
                }
                return Ok(cl.post(&base, Value::Object(body)).await?);
            }
            "team_delete" => return Ok(cl.delete(&base).await?),
            "capabilities" => {
                let mut body = Map::new();
                let _p = body.insert("capabilities".into(), json!(as_int(&step["capabilities"])?));
                if let Some(m) = opt_s(step, "member") {
                    let _p = body.insert("account".into(), json!(self.account_id(&m)));
                }
                if let Some(t) = &team {
                    let _p = body.insert("team".into(), json!(t));
                }
                return Ok(cl
                    .post(&format!("orgs/{org}/capabilities"), Value::Object(body))
                    .await?);
            }
            "team_list" => {
                let o = cl.get(&format!("orgs/{org}")).await?;
                let teams = o
                    .get("teams")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                let kids: Vec<Value> = teams
                    .iter()
                    .filter(|(_, r)| match &team {
                        Some(t) => r.get("parent").and_then(Value::as_str) == Some(t.as_str()),
                        None => !r
                            .get("parent")
                            .and_then(Value::as_str)
                            .is_some_and(|p| teams.contains_key(p)),
                    })
                    .map(|(t, _)| json!(t))
                    .collect();
                let members: Vec<Value> = o
                    .get("members")
                    .and_then(Value::as_object)
                    .map(|m| {
                        m.iter()
                            .map(|(a, r)| json!({ "account": a, "role": r }))
                            .collect()
                    })
                    .unwrap_or_default();
                return Ok(json!({
                    "subgroups": kids,
                    "teams": kids,
                    "members": with_member_keys(&Value::Array(members)),
                }));
            }
            _ => {}
        }
        let t = cl.get(&base).await?;
        let meta = t.get("metadata").cloned().unwrap_or(Value::Null);
        let mut out = t.as_object().cloned().unwrap_or_default();
        let name = match &meta {
            Value::Object(m) => m
                .get("name")
                .cloned()
                .unwrap_or_else(|| t.get("name").cloned().unwrap_or(Value::Null)),
            _ => t.get("name").cloned().unwrap_or(Value::Null),
        };
        let contexts: Vec<Value> = t
            .get("apps")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|x| x.get("contextId").cloned().unwrap_or(Value::Null))
                    .collect()
            })
            .unwrap_or_default();
        let extra = json!({
            "members": with_member_keys(t.get("members").unwrap_or(&Value::Null)),
            "groupId": t.get("teamId").cloned().unwrap_or(Value::Null),
            "memberCount": t.get("members").and_then(Value::as_array).map_or(0, Vec::len),
            "subgroups": t.get("children").filter(|c| truthy(Some(c))).cloned().unwrap_or_else(|| json!([])),
            "name": name,
            "contexts": contexts,
            "subgroupVisibility": if t.get("join") == Some(&json!("everyone")) { "open" } else { "restricted" },
            "data": match &meta { Value::Object(m) => m.get("data").cloned().unwrap_or(Value::Null), _ => Value::Null },
        });
        if let Value::Object(e) = extra {
            out.extend(e);
        }
        if kind == "team_join" {
            if !truthy(t.get("member")) {
                return Err(refused(
                    &format!("not a member of {}", team.unwrap_or_default()),
                    "NOT_FOUND",
                    &Value::Null,
                ));
            }
            let me = cl.me().await?;
            let me_id = me["accountId"].as_str().unwrap_or_default().to_owned();
            let inherited = t.get("myRole").is_none_or(Value::is_null);
            if inherited {
                let _r = cl
                    .post(
                        &format!("{base}/members"),
                        json!({ "account": me_id, "lead": false }),
                    )
                    .await?;
            }
            let _p = out.insert("wasInherited".into(), json!(inherited));
            let _p = out.insert(
                "memberPublicKey".into(),
                json!(account_key(&me_id).unwrap_or_default()),
            );
        }
        Ok(Value::Object(out))
    }

    async fn call_or_view(&mut self, kind: &str, step: &Map<String, Value>) -> StepResult {
        let cl = self.client(step).await?;
        let (app, method) = (s(step, "app"), s(step, "method"));
        let args = match step.get("args") {
            None | Some(Value::Null) => json!({}),
            Some(a) => a.clone(),
        };
        if kind == "call" {
            if truthy(step.get("confirm")) {
                let who = s(step, "account");
                let pk = self
                    .passkeys
                    .get_mut(&who)
                    .ok_or_else(|| format!("no passkey for {who}"))?;
                let r = cl.call_confirmed(&app, &method, args, pk).await?;
                return Ok(serde_json::to_value(r).map_err(|e| e.to_string())?);
            }
            let key = opt_s(step, "idempotency_key");
            let r = cl.call(&app, &method, args, key.as_deref()).await?;
            return Ok(serde_json::to_value(r).map_err(|e| e.to_string())?);
        }
        Ok(serde_json::to_value(self.view_once(&cl, step, args).await?)
            .map_err(|e| e.to_string())?)
    }

    async fn view_once(
        &self,
        cl: &Client,
        step: &Map<String, Value>,
        args: Value,
    ) -> Result<calimero_client::ViewResult, ClientError> {
        let consistency = if truthy(step.get("stale")) {
            Consistency::Stale {
                min_index: step
                    .get("min_index")
                    .and_then(|v| as_int(v).ok())
                    .unwrap_or(0) as u64,
            }
        } else {
            Consistency::Strong
        };
        cl.view(&s(step, "app"), &s(step, "method"), args, consistency)
            .await
    }

    async fn wait_for(&mut self, step: &Map<String, Value>) -> StepResult {
        let timeout = step.get("timeout").map_or(Ok(20.0), as_float)?;
        let interval = step.get("interval").map_or(Ok(0.25), as_float)?;
        let deadline = Instant::now() + Duration::from_secs_f64(timeout);
        let cl = self.client(step).await?;
        let args = step
            .get("args")
            .filter(|a| !a.is_null())
            .cloned()
            .unwrap_or_else(|| json!({}));
        let mut last: Value;
        loop {
            match self.view_once(&cl, step, args.clone()).await {
                Ok(out) => {
                    last = out.result.clone();
                    let v = serde_json::to_value(&out).map_err(|e| e.to_string())?;
                    if let Some(want) = step.get("equals") {
                        if py::eq(&last, want) {
                            return Ok(v);
                        }
                    }
                    if let Some(want) = step.get("contains") {
                        let hay = if last.is_null() {
                            String::new()
                        } else {
                            py::str_of(&last)
                        };
                        if hay.contains(&py::str_of(want)) {
                            return Ok(v);
                        }
                    }
                    if step.get("equals").is_none()
                        && step.get("contains").is_none()
                        && !last.is_null()
                    {
                        return Ok(v);
                    }
                }
                Err(e) => last = Value::String(e.to_string()),
            }
            if Instant::now() > deadline {
                return Err(Fail::Workflow(format!(
                    "wait_for {}: still {} after {timeout}s",
                    s(step, "method"),
                    py::repr(&last)
                )));
            }
            tokio::time::sleep(Duration::from_secs_f64(interval)).await;
        }
    }

    /// Waits (up to 10 s) until every replica of `app` applied the same index and the app
    /// has no follow-up left to run (event handlers, xcalls): merobox's steps were slow
    /// enough for the leader to run them before the next step read their effects, and
    /// the converted scenarios read them right after the call.
    async fn settle(&self, app: &str) {
        let Some(first) = self.c.nodes.iter().flatten().next() else {
            return;
        };
        let app = first.node.canonical_app_id(app);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let mut applied = Vec::new();
            let mut idle = true;
            for r in self.c.nodes.iter().flatten() {
                if let Some(g) = r.node.hosted(&app) {
                    applied.push(g.applied());
                    idle &= g.read_local(|m, _| m.state.pending.is_empty());
                }
            }
            if idle && applied.windows(2).all(|w| w[0] == w[1]) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        if std::env::var_os("SCENARIO_DEBUG").is_some() {
            let state: Vec<(u64, usize)> = self
                .c
                .nodes
                .iter()
                .flatten()
                .filter_map(|r| r.node.hosted(&app))
                .map(|g| (g.applied(), g.read_local(|m, _| m.state.pending.len())))
                .collect();
            eprintln!("    settle {app}: timed out, (applied, pending) per replica {state:?}");
        }
    }

    async fn run_one(&mut self, step: &Map<String, Value>) -> StepResult {
        let kind = s(step, "type");
        match kind.as_str() {
            "deploy" => {
                let cl = self.client(step).await?;
                let (app, bundle) = if step.contains_key("wasm") {
                    let wasm = self.wasm(step)?;
                    let bundle = cl.upload_bundle(wasm).await?;
                    let app = cl
                        .create_app(
                            &opt_s(step, "app_name").unwrap_or_else(|| "app".into()),
                            &bundle,
                            self.app_options(step),
                        )
                        .await?;
                    (app, json!(bundle))
                } else {
                    let created = cl
                        .post("apps", Value::Object(self.release_body(step)))
                        .await?;
                    (
                        s(created.as_object().unwrap_or(&Map::new()), "appId"),
                        Value::Null,
                    )
                };
                let key = self
                    .actor(step)
                    .and_then(|a| self.ids.get(&a).cloned())
                    .and_then(|id| account_key(&id));
                Ok(json!({
                    "appId": app,
                    "contextId": context_id(&app),
                    "bundle": bundle,
                    "memberPublicKey": key,
                }))
            }
            "upload_bundle" => {
                let wasm = self.wasm(step)?;
                let cl = self.client(step).await?;
                Ok(json!({ "hash": cl.upload_bundle(wasm).await? }))
            }
            "create_app" => {
                let cl = self.client(step).await?;
                let app = if step.contains_key("package") {
                    let created = cl
                        .post("apps", Value::Object(self.release_body(step)))
                        .await?;
                    created["appId"].as_str().unwrap_or_default().to_owned()
                } else {
                    cl.create_app(
                        &opt_s(step, "app_name").unwrap_or_else(|| "app".into()),
                        &s(step, "bundle"),
                        self.app_options(step),
                    )
                    .await?
                };
                Ok(json!({ "appId": app, "contextId": context_id(&app) }))
            }
            "install_package" => {
                let mpk = self.file(&s(step, "mpk"))?;
                let (package, version) = self.registry.publish(mpk)?;
                let cl = self.client(step).await?;
                let out = cl
                    .post(
                        "packages/install",
                        json!({ "package": package, "version": version }),
                    )
                    .await?;
                let mut o = out.as_object().cloned().unwrap_or_default();
                let _p = o.insert("package".into(), json!(package));
                let _p = o.insert("version".into(), json!(version));
                Ok(Value::Object(o))
            }
            "upgrade_app" => {
                let mut body = Map::new();
                for k in ["version", "package", "service", "bundle", "migrate"] {
                    if truthy(step.get(k)) {
                        let _p = body.insert(k.into(), step[k].clone());
                    }
                }
                let path = format!("apps/{}/upgrade", s(step, "app"));
                self.post_approved(step, &path, Value::Object(body)).await
            }
            "uninstall_package" => {
                let mut path = format!("packages/{}", s(step, "package"));
                if let Some(v) = opt_s(step, "version") {
                    path = format!("{path}/{v}");
                }
                let cl = self.client(step).await?;
                Ok(cl.delete(&path).await?)
            }
            "delete_app" => {
                let cl = self.client(step).await?;
                Ok(cl.delete(&format!("apps/{}", s(step, "app"))).await?)
            }
            "migration_status" => {
                let apps: Vec<String> = step["apps"]
                    .as_array()
                    .map(|a| a.iter().map(py::str_of).collect())
                    .unwrap_or_default();
                let cl = self.client(step).await?;
                let listed = cl.get("apps").await?;
                let mut versions: HashMap<String, Value> = HashMap::new();
                for a in listed["apps"].as_array().cloned().unwrap_or_default() {
                    let id = a["appId"].as_str().unwrap_or_default().to_owned();
                    let v = a.get("version").cloned().unwrap_or(Value::Null);
                    let _p = versions.insert(context_id(&id), v.clone());
                    let _p = versions.insert(id, v);
                }
                if let Some(unknown) = apps.iter().find(|a| !versions.contains_key(*a)) {
                    return Err(refused(
                        &format!("migration_status: no such app {unknown}"),
                        "NOT_FOUND",
                        &Value::Null,
                    ));
                }
                let mine: Vec<Option<String>> = apps
                    .iter()
                    .map(|a| versions.get(a).filter(|v| truthy(Some(v))).map(py::str_of))
                    .collect();
                let target = mine
                    .iter()
                    .flatten()
                    .max_by(|a, b| version_key(a).cmp(&version_key(b)))
                    .cloned();
                let migrated = mine.iter().filter(|v| v.is_some() && *v == &target).count();
                Ok(json!({
                    "target_version": target,
                    "total": apps.len(),
                    "migrated": migrated,
                    "failed": apps.len() - migrated,
                    "residue_total": 0,
                }))
            }
            "packages" => {
                let cl = self.client(step).await?;
                Ok(cl.get("packages").await?)
            }
            "call" | "view" => {
                let out = self.call_or_view(&kind, step).await?;
                if kind == "call" {
                    self.settle(&s(step, "app")).await;
                }
                let mut o = out.as_object().cloned().unwrap_or_default();
                let result = o.get("result").cloned().unwrap_or(Value::Null);
                // v1 workflows read a call's JSON-RPC envelope: {"output": value}.
                let _p = o.insert("rpc".into(), json!({ "output": result }));
                Ok(Value::Object(o))
            }
            "upload_blob" => {
                let data = self.file(&s(step, "file_path"))?;
                let cl = self.client(step).await?;
                let up = cl.upload_blob(&s(step, "app"), data).await?;
                let mut o = up.as_object().cloned().unwrap_or_default();
                let id = o.get("blobId").cloned().unwrap_or(Value::Null);
                let _p = o.insert("blob_id".into(), id);
                Ok(Value::Object(o))
            }
            "download_blob" => {
                let cl = self.client(step).await?;
                let data = cl
                    .download_blob(&s(step, "app"), &s(step, "blob_id"))
                    .await?;
                let label = opt_s(step, "name").unwrap_or_else(|| "download_blob".into());
                if let Some(want) = step.get("expected_size").filter(|v| !v.is_null()) {
                    let want = as_int(want)?;
                    if data.len() as i64 != want {
                        return Err(Fail::Workflow(format!(
                            "{label}: expected {want} bytes, got {}",
                            data.len()
                        )));
                    }
                }
                let digest = hex::encode(sha2::Sha256::digest(&data));
                if let Some(want) = opt_s(step, "expected_sha256") {
                    if want.to_lowercase() != digest {
                        return Err(Fail::Workflow(format!(
                            "{label}: expected sha256 {want}, got {digest}"
                        )));
                    }
                }
                Ok(json!({
                    "size": data.len(),
                    "sha256": digest,
                    "data": String::from_utf8_lossy(&data),
                }))
            }
            "list_blobs" => {
                let cl = self.client(step).await?;
                Ok(cl.list_blobs(&s(step, "app")).await?)
            }
            "blob_info" => {
                let cl = self.client(step).await?;
                Ok(cl.blob_info(&s(step, "app"), &s(step, "blob_id")).await?)
            }
            "delete_blob" => {
                let cl = self.client(step).await?;
                Ok(cl.delete_blob(&s(step, "app"), &s(step, "blob_id")).await?)
            }
            "publish_presence" => {
                let cl = self.client(step).await?;
                let state = step.get("state").filter(|v| !v.is_null()).map(py::str_of);
                let seq = as_int(&step["seq"])? as u64;
                Ok(cl
                    .publish_presence(&s(step, "app"), seq, state.as_deref().map(str::as_bytes))
                    .await?)
            }
            "presence" => {
                let cl = self.client(step).await?;
                Ok(json!({ "presence": cl.presence(&s(step, "app")).await? }))
            }
            "transfer_app" | "approve_release" | "approve_outbound" | "upgrade_org"
            | "retry_upgrade_run" => {
                let who = self.passkey_name(step)?;
                let cl = self.client(step).await?;
                let to = self.account_id(&s(step, "to"));
                let pk = self.passkeys.get_mut(&who).expect("checked");
                let org = opt_s(step, "org");
                Ok(match kind.as_str() {
                    "transfer_app" => cl.transfer_app(&s(step, "app"), &to, pk).await?,
                    "approve_release" => {
                        cl.approve_release(org.as_deref(), &s(step, "version"), pk)
                            .await?
                    }
                    "approve_outbound" => {
                        let prefixes: Option<Vec<String>> = step
                            .get("prefixes")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().map(py::str_of).collect());
                        cl.approve_outbound(
                            org.as_deref(),
                            &s(step, "package"),
                            prefixes.as_deref(),
                            truthy(step.get("replace")),
                            pk,
                        )
                        .await?
                    }
                    "upgrade_org" => {
                        let body = json!({
                            "package": step["package"],
                            "version": s(step, "version"),
                            "team": step.get("team").cloned().unwrap_or(Value::Null),
                            "cascade": truthy(step.get("cascade")),
                        });
                        cl.upgrade_org(&s(step, "org"), body, pk).await?
                    }
                    _ => {
                        cl.retry_upgrade_run(&s(step, "org"), &s(step, "run"), pk)
                            .await?
                    }
                })
            }
            "upgrade_run" => {
                let cl = self.client(step).await?;
                Ok(cl.upgrade_run(&s(step, "org"), &s(step, "run")).await?)
            }
            "whoami" => {
                let cl = self.client(step).await?;
                let me = cl.me().await?;
                let mut o = me.as_object().cloned().unwrap_or_default();
                let key = account_key(me["accountId"].as_str().unwrap_or_default());
                let _p = o.insert("publicKey".into(), json!(key));
                Ok(Value::Object(o))
            }
            "team_add" => {
                let cl = self.client(step).await?;
                let mut out = Value::Null;
                for m in step["members"].as_array().cloned().unwrap_or_default() {
                    let mut body = Map::new();
                    let _p = body.insert("account".into(), json!(self.account_id(&py::str_of(&m))));
                    let _p = body.insert("lead".into(), json!(truthy(step.get("lead"))));
                    if truthy(step.get("role")) {
                        let _p = body.insert("role".into(), step["role"].clone());
                    }
                    if let Some(c) = step.get("capabilities").filter(|v| !v.is_null()) {
                        let _p = body.insert("capabilities".into(), json!(as_int(c)?));
                    }
                    out = cl
                        .post(
                            &format!("orgs/{}/teams/{}/members", s(step, "org"), s(step, "team")),
                            Value::Object(body),
                        )
                        .await?;
                }
                Ok(out)
            }
            "team_remove" | "team_update" | "team_get" | "team_list" | "team_join"
            | "team_delete" | "capabilities" => self.team_step(&kind, step).await,
            "add_device" => {
                // Another passkey on `account`'s account; `device` signs in with it and acts as
                // that account from here on (v1: a paired device).
                let mut pk = SoftAuthenticator::new("localhost", ORIGIN);
                let cl = self.client(step).await?;
                let cred = cl.add_passkey(&mut pk).await?;
                let c = self.c.client(0);
                let session = c.sign_in(&mut pk).await?;
                let device = s(step, "device");
                let id = session.account_id.clone();
                let _p = self.passkeys.insert(device.clone(), pk);
                let _p = self.sessions.insert(device.clone(), session);
                let _p = self.ids.insert(device.clone(), id.clone());
                self.clients
                    .retain(|(who, _), _| who != &format!("s:{device}"));
                Ok(json!({ "passkeyId": cred, "accountId": id, "keyDelivered": true }))
            }
            "member_capabilities" => {
                let who = self.account_id(&s(step, "member"));
                let cl = self.client(step).await?;
                let org = s(step, "org");
                if let Some(team) = opt_s(step, "team") {
                    let t = cl.get(&format!("orgs/{org}/teams/{team}")).await?;
                    let row = t["members"]
                        .as_array()
                        .and_then(|m| m.iter().find(|r| r["account"] == json!(who)).cloned());
                    let Some(row) = row else {
                        return Err(refused("not a member of the team", "NOT_FOUND", &t));
                    };
                    return Ok(json!({ "capabilities": row["capabilities"] }));
                }
                let o = cl.get(&format!("orgs/{org}")).await?;
                if !o["members"]
                    .as_object()
                    .is_some_and(|m| m.contains_key(&who))
                {
                    return Err(refused("not a member of the organization", "NOT_FOUND", &o));
                }
                let default = o
                    .get("defaultCapabilities")
                    .or_else(|| o.get("default_capabilities"))
                    .cloned()
                    .unwrap_or(Value::Null);
                let caps = o
                    .get("capabilities")
                    .and_then(|c| c.get(&who))
                    .cloned()
                    .unwrap_or(default);
                Ok(json!({ "capabilities": caps }))
            }
            "set_role" => {
                let cl = self.client(step).await?;
                Ok(cl
                    .post(
                        &format!("orgs/{}/members", s(step, "org")),
                        json!({ "account": self.account_id(&s(step, "member")), "role": step["role"] }),
                    )
                    .await?)
            }
            "wait_for" => self.wait_for(step).await,
            "events" => {
                let cl = self.client(step).await?;
                let from = step.get("from").map_or(Ok(0), as_int)? as u64;
                let count = step.get("count").map_or(Ok(100), as_int)? as usize;
                let timeout = step.get("timeout").map_or(Ok(5.0), as_float)?;
                let stream = cl.events(&s(step, "app"), from).await?;
                let mut stream = Box::pin(stream.take(count));
                let mut out = Vec::new();
                let _elapsed = tokio::time::timeout(Duration::from_secs_f64(timeout), async {
                    while let Some((id, kind, data)) = stream.next().await {
                        out.push(json!({ "id": id, "kind": kind, "data": data }));
                    }
                })
                .await;
                Ok(json!({ "events": out }))
            }
            "create_api_key" => {
                let mut opts = json!({
                    "label": opt_s(step, "label").unwrap_or_else(|| "merobox".into()),
                    "readOnly": truthy(step.get("read_only")),
                });
                if let Some(apps) = step.get("apps") {
                    opts["apps"] = apps.clone();
                }
                let cl = self.client(step).await?;
                let (key_id, key) = cl.create_api_key(opts).await?;
                Ok(json!({ "keyId": key_id, "key": key }))
            }
            "create_org" => {
                let cl = self.client(step).await?;
                Ok(cl.post("orgs", json!({ "name": step["org_name"] })).await?)
            }
            "create_team" => {
                let cl = self.client(step).await?;
                let mut body = json!({
                    "name": step["team_name"],
                    "join": step.get("join").cloned().unwrap_or_else(|| json!("invite")),
                });
                if truthy(step.get("parent")) {
                    body["parent"] = step["parent"].clone();
                }
                Ok(cl
                    .post(&format!("orgs/{}/teams", s(step, "org")), body)
                    .await?)
            }
            "invite" => {
                let cl = self.client(step).await?;
                Ok(cl
                    .post(
                        &format!("orgs/{}/invites", s(step, "org")),
                        json!({
                            "role": step.get("role").cloned().unwrap_or_else(|| json!("member")),
                            "teams": step.get("teams").cloned().unwrap_or_else(|| json!([])),
                        }),
                    )
                    .await?)
            }
            "redeem_invite" => {
                let cl = self.client(step).await?;
                Ok(cl
                    .post(&format!("invites/{}/redeem", s(step, "token")), json!({}))
                    .await?)
            }
            "remove_member" => {
                let cl = self.client(step).await?;
                Ok(cl
                    .delete(&format!(
                        "orgs/{}/members/{}",
                        s(step, "org"),
                        self.account_id(&s(step, "member"))
                    ))
                    .await?)
            }
            "http" => {
                let method = s(step, "method").to_lowercase();
                let path = s(step, "path").trim_start_matches('/').to_owned();
                let body = step
                    .get("body")
                    .filter(|b| !b.is_null())
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                match method.as_str() {
                    "get" => {
                        let cl = self.client(step).await?;
                        Ok(cl.get(&path).await?)
                    }
                    "delete" => self.delete_approved(step, &path).await,
                    "put" => {
                        let cl = self.client(step).await?;
                        Ok(cl.put(&path, body).await?)
                    }
                    "post" => self.post_approved(step, &path, body).await,
                    other => Err(Fail::Workflow(format!(
                        "http: unsupported method '{other}'"
                    ))),
                }
            }
            "find_leader" => {
                let app = s(step, "app");
                let leader = self
                    .c
                    .app_leader(&app)
                    .ok_or_else(|| format!("no node reports a leader for {app}"))?;
                let follower = (0..self.c.nodes.len()).find(|i| *i != leader);
                Ok(json!({ "leader": leader + 1, "follower": follower.map(|f| f + 1) }))
            }
            "wait" => {
                let secs = as_float(&step["seconds"])?;
                let secs = if truthy(step.get("exact")) {
                    secs
                } else {
                    secs.min(self.wait_cap)
                };
                tokio::time::sleep(Duration::from_secs_f64(secs.max(0.0))).await;
                Ok(Value::Null)
            }
            "set" => {
                if let Some(Value::Object(m)) = step.get("values") {
                    self.vars.extend(m.clone());
                }
                Ok(Value::Null)
            }
            other => Err(Fail::Workflow(format!("unhandled step type {other}"))),
        }
    }

    /// Runs one resolved step and checks what it promised (merobox's `_step`).
    async fn step(&mut self, step: &Map<String, Value>, label: &str) -> Result<(), String> {
        let expected = opt_s(step, "expected_error");
        let any_failure = truthy(step.get("expected_failure"));
        let tolerated = truthy(step.get("allow_failure"));
        let outputs = step
            .get("outputs")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let result = match self.run_one(step).await {
            Ok(r) => r,
            Err(Fail::Workflow(e)) => return Err(format!("{label}: {e}")),
            Err(Fail::Refused(e)) => {
                if expected.as_deref() == Some(e.code.as_str()) || any_failure || tolerated {
                    let error = format!("{} {}", e.message, e.details);
                    for (var, path) in &outputs {
                        if py::str_of(path).starts_with("error") {
                            self.set_var(var, Value::String(error.clone()));
                        }
                    }
                    return Ok(());
                }
                return Err(format!("{label}: {} ({})", e.message, e.details));
            }
        };
        if expected.is_some() || any_failure {
            return Err(format!("{label}: expected a refusal, but it succeeded"));
        }
        if let Some(want) = step.get("expect") {
            let got = match &result {
                Value::Object(m) => m.get("result").cloned().unwrap_or(Value::Null),
                other => other.clone(),
            };
            if !py::eq(&got, want) {
                return Err(format!(
                    "{label}: expected {}, got {}",
                    py::repr(want),
                    py::repr(&got)
                ));
            }
        }
        for (var, path) in &outputs {
            let path = py::str_of(path);
            if path == "error" {
                self.set_var(var, Value::Null);
                continue;
            }
            let v = pick(&result, &path).map_err(|e| format!("{label}: {e}"))?;
            self.set_var(var, v);
        }
        Ok(())
    }

    fn run_steps<'b>(
        &'b mut self,
        steps: &'b [Value],
        depth: usize,
    ) -> Fut<'b, Result<(), String>> {
        Box::pin(async move {
            for (i, raw) in steps.iter().enumerate() {
                let raw = raw.as_object().ok_or("a step is an object")?;
                let kind = s(raw, "type");
                let label = opt_s(raw, "name").unwrap_or_else(|| kind.clone());
                let at = || format!("step {} ({label})", i + 1);
                match kind.as_str() {
                    "assert" | "json_assert" => {
                        run_assert(raw, &self.vars).map_err(|e| format!("{}: {e}", at()))?;
                        continue;
                    }
                    "repeat" => {
                        let count = as_int(&resolve(&raw["count"], &self.vars_value())?)?;
                        let cap = std::env::var("SCENARIO_REPEAT_CAP")
                            .ok()
                            .and_then(|v| v.parse::<i64>().ok())
                            .unwrap_or(i64::MAX);
                        let inner = raw["steps"].as_array().cloned().unwrap_or_default();
                        for n in 0..count.min(cap) {
                            self.set_var("iteration", json!(n));
                            self.set_var("iteration_node", json!(n + 1));
                            if let Some(Value::Object(outs)) = raw.get("outputs") {
                                for (var, path) in outs {
                                    let p = py::str_of(path);
                                    if p == "iteration" || p == "iteration_node" {
                                        let v = self.vars[&p].clone();
                                        self.set_var(var, v);
                                    }
                                }
                            }
                            self.run_steps(&inner, depth + 1)
                                .await
                                .map_err(|e| format!("{} iteration {n}: {e}", at()))?;
                        }
                        continue;
                    }
                    "fuzzy_test" => {
                        self.fuzzy(raw, &label)
                            .await
                            .map_err(|e| format!("{}: {e}", at()))?;
                        continue;
                    }
                    _ => {}
                }
                let without_steps: Map<String, Value> = raw
                    .iter()
                    .filter(|(k, _)| k.as_str() != "steps")
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let step = resolve(&Value::Object(without_steps), &self.vars_value())
                    .map_err(|e| format!("{}: {e}", at()))?;
                let step = step.as_object().cloned().unwrap_or_default();
                // `retry_for: N` keeps trying for N seconds.
                let started = Instant::now();
                let retry = step.get("retry_for").map_or(Ok(0.0), as_float)?;
                let deadline = started + Duration::from_secs_f64(retry.max(0.0));
                loop {
                    match self.step(&step, &label).await {
                        Ok(()) => break,
                        Err(e) if Instant::now() >= deadline => {
                            return Err(format!("{}: {e}", at()));
                        }
                        Err(_) => tokio::time::sleep(Duration::from_millis(250)).await,
                    }
                }
                if let Some(budget) = step.get("max_seconds") {
                    let budget = as_float(budget)?;
                    let took = started.elapsed().as_secs_f64();
                    if took > budget {
                        return Err(format!(
                            "{}: took {took:.1}s, over its {budget}s budget",
                            at()
                        ));
                    }
                }
            }
            Ok(())
        })
    }

    /// `{{random_int(a, b)}}`, `{{uuid}}`, `{{timestamp}}` in a fuzzy operation's step.
    fn fill(&mut self, v: &Value) -> Value {
        match v {
            Value::String(t) => {
                let mut out = String::new();
                let mut rest = t.as_str();
                while let Some(open) = rest.find("{{") {
                    let Some(close) = rest[open..].find("}}").map(|c| c + open) else {
                        break;
                    };
                    let inner = rest[open + 2..close].trim();
                    let made = if inner == "uuid" {
                        Some(self.rng.uuid())
                    } else if inner == "timestamp" {
                        Some(
                            SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs()
                                .to_string(),
                        )
                    } else if let Some(body) = inner
                        .strip_prefix("random_int(")
                        .and_then(|b| b.strip_suffix(')'))
                    {
                        let parts: Vec<&str> = body.split(',').map(str::trim).collect();
                        match parts.as_slice() {
                            [a, b] => match (a.parse::<i64>(), b.parse::<i64>()) {
                                (Ok(a), Ok(b)) => Some(self.rng.int(a, b).to_string()),
                                _ => None,
                            },
                            _ => None,
                        }
                    } else {
                        None
                    };
                    match made {
                        Some(m) => {
                            out.push_str(&rest[..open]);
                            out.push_str(&m);
                        }
                        None => out.push_str(&rest[..close + 2]),
                    }
                    rest = &rest[close + 2..];
                }
                out.push_str(rest);
                Value::String(out)
            }
            Value::Array(a) => Value::Array(a.iter().map(|x| self.fill(x)).collect()),
            Value::Object(m) => {
                Value::Object(m.iter().map(|(k, x)| (k.clone(), self.fill(x))).collect())
            }
            other => other.clone(),
        }
    }

    /// v1's soak test: weighted operations, picked at random, for `SCENARIO_FUZZY_SECONDS`
    /// (default 10); the run passes at `success_threshold` percent.
    async fn fuzzy(&mut self, raw: &Map<String, Value>, label: &str) -> Result<(), String> {
        let seconds = env_f64("SCENARIO_FUZZY_SECONDS", 10.0);
        let accounts: Vec<Value> = match raw.get("accounts").and_then(Value::as_array) {
            Some(a) if !a.is_empty() => a.clone(),
            _ => self.accounts.iter().map(|a| json!(a)).collect(),
        };
        let ops = raw["operations"].as_array().cloned().unwrap_or_default();
        if ops.is_empty() || accounts.is_empty() {
            return Err("fuzzy_test needs operations and accounts".into());
        }
        let weights: Vec<f64> = ops
            .iter()
            .map(|o| o.get("weight").map_or(1.0, |w| py::float(w).unwrap_or(1.0)))
            .collect();
        let total_w: f64 = weights.iter().sum();
        let delay = raw
            .get("operation_delay_ms")
            .and_then(py::float)
            .unwrap_or(0.0)
            / 1000.0;
        let threshold = raw
            .get("success_threshold")
            .and_then(py::float)
            .unwrap_or(95.0);
        let (mut passed, mut failed) = (0u64, 0u64);
        let mut failures: BTreeMap<String, u64> = BTreeMap::new();
        let deadline = Instant::now() + Duration::from_secs_f64(seconds);
        while Instant::now() < deadline {
            let mut pickw = (self.rng.next_u64() as f64 / u64::MAX as f64) * total_w;
            let mut op = &ops[ops.len() - 1];
            for (o, w) in ops.iter().zip(&weights) {
                if pickw < *w {
                    op = o;
                    break;
                }
                pickw -= w;
            }
            let name = op.get("name").map_or_else(|| "fuzzy".into(), py::str_of);
            let who = &accounts[(self.rng.next_u64() % accounts.len() as u64) as usize];
            let (acct, key) = match who {
                Value::Object(m) => (
                    m.get("account").cloned().unwrap_or(Value::Null),
                    m.get("as_key").cloned().unwrap_or(Value::Null),
                ),
                other => (other.clone(), Value::Null),
            };
            self.set_var("random_account", acct);
            self.set_var("random_key", key);
            let steps = op["steps"].as_array().cloned().unwrap_or_default();
            let mut outcome: Result<(), String> = Ok(());
            for st in &steps {
                let st = self.fill(st);
                let Some(st) = st.as_object() else {
                    outcome = Err("a step is an object".into());
                    break;
                };
                let kind = s(st, "type");
                if kind == "assert" || kind == "json_assert" {
                    if let Err(e) = run_assert(st, &self.vars) {
                        outcome = Err(e);
                        break;
                    }
                    continue;
                }
                if kind == "wait" {
                    let secs = st.get("seconds").and_then(py::float).unwrap_or(0.0);
                    tokio::time::sleep(Duration::from_secs_f64(secs.clamp(0.0, 1.0))).await;
                    continue;
                }
                let step = match resolve(&Value::Object(st.clone()), &self.vars_value()) {
                    Ok(Value::Object(m)) => m,
                    Ok(_) => Map::new(),
                    Err(e) => {
                        outcome = Err(e);
                        break;
                    }
                };
                if kind == "call" {
                    if let Some(Value::Object(args)) = step.get("args") {
                        for (k, v) in args {
                            self.set_var(&format!("fuzzy_{k}"), v.clone());
                        }
                    }
                }
                if let Err(e) = self.step(&step, &name).await {
                    outcome = Err(e);
                    break;
                }
            }
            match outcome {
                Ok(()) => passed += 1,
                Err(e) => {
                    failed += 1;
                    *failures
                        .entry(format!(
                            "{name}: {}",
                            e.chars().take(120).collect::<String>()
                        ))
                        .or_default() += 1;
                }
            }
            if delay > 0.0 {
                tokio::time::sleep(Duration::from_secs_f64(delay)).await;
            }
        }
        let total = passed + failed;
        let rate = if total == 0 {
            0.0
        } else {
            100.0 * passed as f64 / total as f64
        };
        eprintln!("    {label}: {passed}/{total} operations passed ({rate:.1}%)");
        if rate < threshold {
            let mut top: Vec<_> = failures.into_iter().collect();
            top.sort_by(|a, b| b.1.cmp(&a.1));
            let shown: Vec<String> = top
                .iter()
                .take(5)
                .map(|(w, n)| format!("{n}x {w}"))
                .collect();
            return Err(format!(
                "{label}: {rate:.1}% passed, under {threshold}%\n{}",
                shown.join("\n")
            ));
        }
        Ok(())
    }
}

// ------------------------------------------------------------------ app-level scenarios

/// The scenario's time zero: a slot boundary (a multiple of 15 minutes), the value the app
/// harness tests give `TestApp::new` (`apps/rooms/tests/scenarios.rs`).
pub const T0: u64 = 1_700_000_100_000;

/// The organization an app-level scenario needs: `<name>.org.json` beside it.
#[derive(Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OrgSpec {
    pub admins: Vec<String>,
    pub members: Vec<String>,
    pub guests: Vec<String>,
    pub teams: BTreeMap<String, Vec<String>>,
}

impl OrgSpec {
    pub fn beside(scenario: &Path) -> Result<Self, String> {
        let stem = scenario
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let path = scenario.with_file_name(format!("{stem}.org.json"));
        match std::fs::read_to_string(&path) {
            Ok(t) => serde_json::from_str(&t).map_err(|e| format!("{}: {e}", path.display())),
            Err(_) => Ok(Self::default()),
        }
    }

    /// Every account and its org role, the owner first.
    fn roles(&self) -> Vec<(String, Option<&'static str>)> {
        let mut out = vec![("owner".to_owned(), None)];
        for (list, role) in [
            (&self.admins, "admin"),
            (&self.members, "member"),
            (&self.guests, "guest"),
        ] {
            out.extend(list.iter().map(|a| (a.clone(), Some(role))));
        }
        for accounts in self.teams.values() {
            for a in accounts {
                if !out.iter().any(|(x, _)| x == a) {
                    // A team needs no org role in the harness; on the cluster a team's member is
                    // a member of the org.
                    out.push((a.clone(), Some("member")));
                }
            }
        }
        out
    }
}

/// How an app-level step ended, in the harness's terms (`calimero_app_sdk::testing::Failure`).
enum AppFailure {
    App {
        code: String,
        fields: Value,
    },
    Trap(String),
    ConfirmRequired,
    /// The platform refused before the app ran: the HTTP status and API code.
    Refused {
        status: u16,
        code: String,
    },
    Other(String),
}

impl std::fmt::Display for AppFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::App { code, fields } => write!(f, "app error {code} {fields}"),
            Self::Trap(kind) => write!(f, "trap {kind}"),
            Self::ConfirmRequired => write!(f, "confirm required"),
            Self::Refused { status, code } => write!(f, "refused {status} {code}"),
            Self::Other(e) => write!(f, "{e}"),
        }
    }
}

fn show(r: &Result<Value, AppFailure>) -> String {
    match r {
        Ok(v) => format!("ok {v}"),
        Err(f) => f.to_string(),
    }
}

fn app_failure(e: &ClientError) -> AppFailure {
    let details = e.details().cloned().unwrap_or(Value::Null);
    match e {
        ClientError::Api { code, .. } if code == "APP_ERROR" => AppFailure::App {
            code: details["code"].as_str().unwrap_or_default().to_owned(),
            fields: details["fields"].clone(),
        },
        ClientError::Api { code, .. } if code == "TRAP" => {
            AppFailure::Trap(details["kind"].as_str().unwrap_or_default().to_owned())
        }
        ClientError::Api { code, .. } if code == "CONFIRM_REQUIRED" => AppFailure::ConfirmRequired,
        ClientError::Api { status, code, .. } => AppFailure::Refused {
            status: *status,
            code: code.clone(),
        },
        ClientError::Transport(e) => AppFailure::Other(e.to_string()),
    }
}

fn check_expect(expect: Option<&Value>, result: &Result<Value, AppFailure>) -> Result<(), String> {
    let Some(expect) = expect else {
        return match result {
            Ok(_) => Ok(()),
            Err(f) => Err(format!("expected success, got {f}")),
        };
    };
    if let Some(want) = expect.get("ok") {
        return match result {
            Ok(got) if got == want => Ok(()),
            other => Err(format!("expected ok {want}, got {}", show(other))),
        };
    }
    if expect.get("confirmRequired").and_then(Value::as_bool) == Some(true) {
        return match result {
            Err(AppFailure::ConfirmRequired) => Ok(()),
            other => Err(format!("expected confirmRequired, got {}", show(other))),
        };
    }
    if let Some(code) = expect.get("error").and_then(Value::as_str) {
        return match result {
            Err(AppFailure::App { code: got, fields }) if got == code => match expect.get("fields")
            {
                Some(want) if want != fields => Err(format!(
                    "expected error {code} with fields {want}, got {fields}"
                )),
                _ => Ok(()),
            },
            other => Err(format!("expected error {code}, got {}", show(other))),
        };
    }
    if let Some(code) = expect.get("refused").and_then(Value::as_str) {
        return match result {
            Err(AppFailure::Refused { code: got, .. }) if got == code => Ok(()),
            other => Err(format!("expected refused {code}, got {}", show(other))),
        };
    }
    if let Some(kind) = expect.get("trap").and_then(Value::as_str) {
        return match result {
            Err(AppFailure::Trap(got)) if got == kind => Ok(()),
            other => Err(format!("expected trap {kind}, got {}", show(other))),
        };
    }
    Err("expect needs ok, error, trap, refused or confirmRequired".into())
}

/// `[A-Za-z_][A-Za-z0-9_]*`.
fn is_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A person of the org: their account id, passkey and one signed-in client per node.
struct Person {
    id: String,
    pk: SoftAuthenticator,
    on: Vec<Client>,
}

type Received = Arc<Mutex<Vec<String>>>;

struct AppRunner<'a> {
    c: &'a Cluster,
    org: String,
    app: String,
    people: BTreeMap<String, Person>,
    anonymous: Vec<Client>,
    /// Team name (the scenario's) -> the team's id on the cluster.
    teams: BTreeMap<String, String>,
    vars: HashMap<String, Value>,
    received: BTreeMap<String, Received>,
    home: BTreeMap<String, usize>,
}

/// Signs `name` up on node 0 and in on the others.
async fn person(c: &Cluster, name: &str) -> Result<Person, String> {
    let first = c.client(0);
    let mut pk = SoftAuthenticator::new("localhost", ORIGIN);
    let session = first
        .sign_up(&mut pk, name)
        .await
        .map_err(|e| format!("sign up {name}: {e}"))?;
    let mut on = vec![first];
    for i in 1..c.nodes.len() {
        let client = c.client(i);
        let _s = client
            .sign_in(&mut pk)
            .await
            .map_err(|e| format!("sign in {name}: {e}"))?;
        on.push(client);
    }
    Ok(Person {
        id: session.account_id,
        pk,
        on,
    })
}

fn str_field(v: &Value, k: &str) -> Result<String, String> {
    v[k].as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{k} in {v}"))
}

/// Runs the app-level scenario `path` (`apps/<app>/scenarios/<name>.json`) on a three-node
/// cluster whose app clock starts at [`T0`].
pub async fn run_app_scenario(path: &Path) -> Result<(), String> {
    let app_dir = path
        .parent()
        .and_then(Path::parent)
        .ok_or("an app-level scenario lives in apps/<app>/scenarios/")?;
    let wasm = build_app(app_dir)?;
    let spec = OrgSpec::beside(path)?;
    let now = i64::try_from(calimero_node::types::now_ms()).unwrap_or(0);
    let offset = i64::try_from(T0).unwrap_or(0) - now;
    let c = Cluster::start_with(3, move |cfg| cfg.clock_offset_ms = offset).await;
    let out = async {
        let mut runner = AppRunner::setup(&c, &spec, wasm).await?;
        runner.open_streams().await;
        runner.run(path).await
    }
    .await;
    let mut c = c;
    for i in 0..c.nodes.len() {
        c.stop(i).await;
    }
    out
}

impl<'a> AppRunner<'a> {
    async fn setup(c: &'a Cluster, spec: &OrgSpec, wasm: Vec<u8>) -> Result<Self, String> {
        let owner = person(c, "owner").await?;
        let o = &owner.on[0];
        let org = str_field(
            &o.post("orgs", json!({ "name": "Scenario" }))
                .await
                .map_err(|e| format!("create org: {e}"))?,
            "orgId",
        )?;
        let mut people = BTreeMap::new();
        for (name, role) in spec.roles() {
            let Some(role) = role else { continue };
            let p = person(c, &name).await?;
            let token = str_field(
                &o.post(&format!("orgs/{org}/invites"), json!({ "role": role }))
                    .await
                    .map_err(|e| format!("invite {name}: {e}"))?,
                "token",
            )?;
            let _r = p.on[0]
                .post(&format!("invites/{token}/redeem"), json!({}))
                .await
                .map_err(|e| format!("redeem {name}: {e}"))?;
            let _prev = people.insert(name, p);
        }
        let mut teams = BTreeMap::new();
        for (team, members) in &spec.teams {
            let made = o
                .post(
                    &format!("orgs/{org}/teams"),
                    json!({ "name": team, "join": "invite" }),
                )
                .await
                .map_err(|e| format!("create team {team}: {e}"))?;
            let id = str_field(&made, "teamId")?;
            for m in members {
                let account = &people
                    .get(m)
                    .map_or_else(|| owner.id.clone(), |p: &Person| p.id.clone());
                let _r = o
                    .post(
                        &format!("orgs/{org}/teams/{id}/members"),
                        json!({ "account": account, "lead": false }),
                    )
                    .await
                    .map_err(|e| format!("add {m} to {team}: {e}"))?;
            }
            let _prev = teams.insert(team.clone(), id);
        }
        // The team's creator (the owner) leads it: drop that row unless the org file names
        // the owner in the team, so `in_team(owner)` answers as the harness does.
        for (team, id) in &teams {
            if !spec.teams[team].iter().any(|m| m == "owner") {
                let _r = o
                    .delete(&format!("orgs/{org}/teams/{id}/members/{}", owner.id))
                    .await;
            }
        }
        let bundle = o
            .upload_bundle(wasm)
            .await
            .map_err(|e| format!("upload bundle: {e}"))?;
        let app = o
            .create_app("scenario", &bundle, json!({ "org": org }))
            .await
            .map_err(|e| format!("create app: {e}"))?;
        let _prev = people.insert("owner".to_owned(), owner);
        Ok(Self {
            c,
            org,
            app,
            people,
            anonymous: (0..c.nodes.len()).map(|i| c.client(i)).collect(),
            teams,
            vars: HashMap::new(),
            received: BTreeMap::new(),
            home: BTreeMap::new(),
        })
    }

    /// One live event stream per account (on a node of its own), from the log's start.
    async fn open_streams(&mut self) {
        let n = self.c.nodes.len();
        for (k, (name, p)) in self.people.iter().enumerate() {
            let node = k % n;
            let got: Received = Arc::default();
            if let Ok(mut stream) = p.on[node].events(&self.app, 0).await {
                let sink = Arc::clone(&got);
                drop(tokio::spawn(async move {
                    while let Some((_id, kind, _data)) = stream.next().await {
                        sink.lock().unwrap().push(kind);
                    }
                }));
            }
            let _prev = self.received.insert(name.clone(), got);
            let _prev = self.home.insert(name.clone(), node);
        }
    }

    async fn run(&mut self, path: &Path) -> Result<(), String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let steps: Vec<Value> =
            serde_json::from_str(&text).map_err(|e| format!("not a JSON array: {e}"))?;
        for (n, step) in steps.iter().enumerate() {
            self.step(n, step)
                .await
                .map_err(|e| format!("step {n}: {step}\n{e}"))?;
        }
        Ok(())
    }

    async fn step(&mut self, n: usize, step: &Value) -> Result<(), String> {
        let node = n % self.c.nodes.len();
        let op = step["op"].as_str().ok_or("step needs an op")?;
        if step.get("save").is_some() && !matches!(op, "call" | "view") {
            return Err(format!("`save` only applies to a call or view, not {op:?}"));
        }
        let args = self.substitute(step.get("args").unwrap_or(&Value::Null), true)?;
        let result = match op {
            "call" | "view" => self.invoke(step, node, op, args).await?,
            "tick" => {
                if step.get("as").is_some() {
                    return Err("a tick always runs as the system; drop `as`".into());
                }
                self.tick(step["method"].as_str().ok_or("tick needs a method")?)
                    .await?
            }
            "advance" => {
                let ms = args.as_u64().ok_or("advance needs ms in args")?;
                for r in self.c.nodes.iter().flatten() {
                    r.node.advance_clock(i64::try_from(ms).unwrap_or(i64::MAX));
                }
                Ok(Value::Null)
            }
            "removeMember" => {
                self.remove_member(args.as_str().ok_or("removeMember needs an account")?)
                    .await?;
                Ok(Value::Null)
            }
            other => return Err(format!("unknown op {other:?}")),
        };
        let result = result.map(|v| self.team_names(v));
        let expect = step
            .get("expect")
            .map(|e| self.substitute(e, false))
            .transpose()?;
        check_expect(expect.as_ref(), &result)?;
        if let (Some(name), Ok(saved)) = (step.get("save").and_then(Value::as_str), &result) {
            let _prev = self.vars.insert(name.to_owned(), saved.clone());
        }
        if let Some(Value::Object(per_account)) = step.get("events") {
            for (account, names) in per_account {
                let want: Vec<String> = names
                    .as_array()
                    .ok_or_else(|| format!("events.{account} is not a list"))?
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect();
                self.events_reach(account, &want).await?;
            }
        }
        Ok(())
    }

    async fn invoke(
        &mut self,
        step: &Value,
        node: usize,
        op: &str,
        args: Value,
    ) -> Result<Result<Value, AppFailure>, String> {
        let method = step["method"].as_str().ok_or("step needs a method")?;
        let who = step["as"].as_str().unwrap_or("owner");
        let confirmed = step["confirmed"].as_bool() == Some(true);
        let app = self.app.clone();
        let answer = match who {
            "@system" => {
                return Err(
                    "`@system` calls are the harness's; no client runs as the system".into(),
                )
            }
            "@anonymous" => {
                let cl = &self.anonymous[node];
                if op == "call" {
                    cl.call(&app, method, args, None).await.map(|r| r.result)
                } else {
                    cl.view(&app, method, args, Consistency::Strong)
                        .await
                        .map(|r| r.result)
                }
            }
            account => {
                let p = self
                    .people
                    .get_mut(account)
                    .ok_or_else(|| format!("no account {account}"))?;
                let cl = &p.on[node];
                match (op, confirmed) {
                    ("call", true) => cl
                        .call_confirmed(&app, method, args, &mut p.pk)
                        .await
                        .map(|r| r.result),
                    ("call", false) => cl.call(&app, method, args, None).await.map(|r| r.result),
                    _ => cl
                        .view(&app, method, args, Consistency::Strong)
                        .await
                        .map(|r| r.result),
                }
            }
        };
        Ok(answer.map_err(|e| app_failure(&e)))
    }

    /// Waits until the timer's latest due slot is applied on every node, and answers with
    /// the logged tick's outcome for that slot.
    async fn tick(&self, timer: &str) -> Result<Result<Value, AppFailure>, String> {
        use calimero_node::app::Status as AppStatus;
        let node = &self.c.nodes[0].as_ref().ok_or("node 0 is down")?.node;
        let g = node.hosted(&self.app).ok_or("app not hosted")?;
        let (bundle, install) =
            g.read_local(|m, _| (m.state.bundle.clone(), m.state.component.head.install_ts_ms));
        let prepared = node
            .prepared_for(&bundle.ok_or("no bundle")?)
            .await
            .map_err(|e| format!("{e:?}"))?;
        let every = prepared
            .abi
            .methods
            .get(timer)
            .and_then(|m| m.every)
            .ok_or_else(|| format!("{timer} is not a timer"))?;
        let slot = calimero_node::node::due_slot(install, every, node.app_clock().now_ms())
            .ok_or_else(|| format!("no slot of {timer} is due"))?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let applied = self.c.nodes.iter().flatten().all(|r| {
                r.node
                    .hosted(&self.app)
                    .is_some_and(|g| g.read_local(|m, _| m.tick_applied(timer, slot)))
            });
            if applied {
                let logged = g.read_local(|m, _| {
                    m.ticks
                        .iter()
                        .find(|t| t.timer == timer && t.scheduled_ms == slot)
                        .map(|t| t.applied.clone())
                });
                let a = logged.ok_or_else(|| format!("no logged tick for slot {slot}"))?;
                return Ok(match a.status {
                    AppStatus::Ok => Ok(a.result),
                    AppStatus::AppError => Err(AppFailure::App {
                        code: a.error["code"].as_str().unwrap_or_default().to_owned(),
                        fields: a.error["fields"].clone(),
                    }),
                    AppStatus::Trap => Err(AppFailure::Trap(
                        a.error["kind"].as_str().unwrap_or_default().to_owned(),
                    )),
                });
            }
            if Instant::now() > deadline {
                return Err(format!("slot {slot} of {timer} never applied everywhere"));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn applied_on(&self, i: usize) -> u64 {
        self.c.nodes[i]
            .as_ref()
            .and_then(|r| r.node.hosted(&self.app))
            .map_or(0, |g| g.applied())
    }

    /// Removes `name` from the org once every stream has delivered what the account may see
    /// now, then waits until every node applied the removal.
    async fn remove_member(&mut self, name: &str) -> Result<(), String> {
        self.flush_events().await?;
        let id = self
            .people
            .get(name)
            .ok_or_else(|| format!("no account {name}"))?
            .id
            .clone();
        let _r = self.people["owner"].on[0]
            .delete(&format!("orgs/{}/members/{id}", self.org))
            .await
            .map_err(|e| e.to_string())?;
        for r in self.c.nodes.iter().flatten() {
            let applied = r.node.system.status().last_log.unwrap_or(0);
            if !r
                .node
                .system
                .wait_applied(applied, Duration::from_secs(5))
                .await
            {
                return Err("a node did not apply the removal".into());
            }
        }
        Ok(())
    }

    /// Waits until each live stream holds as many events as a fresh replay on its node.
    async fn flush_events(&self) -> Result<(), String> {
        let last = self.applied_on(0);
        for (name, got) in &self.received {
            let node = self.home[name];
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.applied_on(node) < last {
                if Instant::now() > deadline {
                    return Err(format!("node {node} never applied {last}"));
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let replayed = self.replay_count(name, node).await;
            while got.lock().unwrap().len() < replayed {
                if Instant::now() > deadline {
                    return Err(format!("{name}'s stream never caught up"));
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        Ok(())
    }

    async fn replay_count(&self, name: &str, node: usize) -> usize {
        let Ok(mut stream) = self.people[name].on[node].events(&self.app, 0).await else {
            return 0;
        };
        let mut n = 0;
        while let Ok(Some(_)) =
            tokio::time::timeout(Duration::from_millis(300), stream.next()).await
        {
            n += 1;
        }
        n
    }

    async fn events_reach(&self, account: &str, want: &[String]) -> Result<(), String> {
        let got = self
            .received
            .get(account)
            .ok_or_else(|| format!("no account {account}"))?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let names = got.lock().unwrap().clone();
            if names == want {
                return Ok(());
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "events for {account}: expected {want:?}, got {names:?}"
                ));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// `$name` -> its saved value, `@name` -> the account's id; in `args`, a string that is
    /// exactly a team's name in the org file -> that team's id on the cluster (the harness
    /// names teams; the node mints their ids).
    fn substitute(&self, value: &Value, teams: bool) -> Result<Value, String> {
        Ok(match value {
            Value::String(s) => {
                if let Some(name) = s.strip_prefix('$').filter(|n| is_var_name(n)) {
                    self.vars
                        .get(name)
                        .cloned()
                        .ok_or_else(|| format!("unknown variable ${name}"))?
                } else if let Some(account) = s.strip_prefix('@').filter(|n| is_var_name(n)) {
                    let p = self
                        .people
                        .get(account)
                        .ok_or_else(|| format!("unknown account @{account}"))?;
                    Value::String(p.id.clone())
                } else if let Some(id) = self.teams.get(s).filter(|_| teams) {
                    Value::String(id.clone())
                } else {
                    value.clone()
                }
            }
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|v| self.substitute(v, teams))
                    .collect::<Result<_, _>>()?,
            ),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| Ok((k.clone(), self.substitute(v, teams)?)))
                    .collect::<Result<_, String>>()?,
            ),
            other => other.clone(),
        })
    }

    /// A result with each team id the cluster minted named as the scenario names it.
    fn team_names(&self, v: Value) -> Value {
        match v {
            Value::String(s) => match self.teams.iter().find(|(_, id)| **id == s) {
                Some((name, _)) => Value::String(name.clone()),
                None => Value::String(s),
            },
            Value::Array(a) => Value::Array(a.into_iter().map(|x| self.team_names(x)).collect()),
            Value::Object(m) => Value::Object(
                m.into_iter()
                    .map(|(k, x)| (k, self.team_names(x)))
                    .collect(),
            ),
            other => other,
        }
    }
}

/// Checks of the placeholder and statement semantics against Python's, run by
/// `tests/scenarios.rs` before any scenario (a custom test main has no `#[test]`s).
pub fn self_check() {
    crate::py::self_check();
    placeholders_resolve_as_merobox_does();
    statements_evaluate_as_python_does();
}

use checks::{placeholders_resolve_as_merobox_does, statements_evaluate_as_python_does};

mod checks {
    use super::*;

    fn vars(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    pub fn placeholders_resolve_as_merobox_does() {
        let v = json!({ "a": { "b": [1, { "c": "x" }] }, "n": 3, "s": "t" });
        assert_eq!(resolve(&json!("{{ a.b.1.c }}"), &v).unwrap(), json!("x"));
        assert_eq!(resolve(&json!("{{n}}"), &v).unwrap(), json!(3));
        assert_eq!(
            resolve(&json!("k_{{n}}_{{a}}"), &v).unwrap(),
            json!(r#"k_3_{"b": [1, {"c": "x"}]}"#)
        );
        assert_eq!(resolve(&json!("{{missing ?? s}}"), &v).unwrap(), json!("t"));
        assert!(resolve(&json!("{{missing}}"), &v).is_err());
        assert_eq!(
            resolve(&json!("{{random_int(1, 2)}}"), &v).unwrap(),
            json!("{{random_int(1, 2)}}")
        );
    }

    pub fn statements_evaluate_as_python_does() {
        let v = vars(json!({ "r": { "output": "refused: [NotApproved]" }, "n": 3, "e": [] }));
        let ok = |s: &str| eval_statement(s, &v).ok().unwrap().0;
        assert!(ok("contains({{r}}, [NotApproved])"));
        assert!(ok("contains({{r}}, 'output': )"));
        assert!(ok("{{n}} >= 2"));
        assert!(!ok("equal({{n}}, 3)"));
        assert!(ok("{{n}} == 3"));
        assert!(ok("is_empty({{e}})"));
        assert!(ok("is_set({{r}})"));
        assert!(ok("regex({{r}}, ^\\{'output': 'refused: \\[Not)"));
        let j = |s: &str| eval_json_statement(s, &v).ok().unwrap().0;
        assert!(j(
            r#"json_subset({{r}}, {"output": "refused: [NotApproved]"})"#
        ));
        assert!(j("json_equal({{n}}, 3)"));
        assert!(!j("equal(\"{{n}}\", \"3\")"));
        assert!(j("equal(\"{{n}}\", \"{{n}}\")"));
    }
}
