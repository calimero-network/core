//! `meroctl registry` against a stub of the app registry's `GET /api/bundles`
//! (`api/bundles/index.js` in calimero-network/app-registry): list, search,
//! versions, get, and `login`'s saved key reaching private packages,
//! `--mine` and `app publish`. No node.

#![allow(
    clippy::unwrap_used,
    reason = "a test's panic is its report; clippy.toml exempts #[test] fns, not their helpers"
)]

use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::Json;
use serde_json::{json, Value};

const KEY: &str = "reg_key";

fn bundle(package: &str, version: &str, name: &str, description: &str, category: &str) -> Value {
    json!({
        "package": package,
        "appVersion": version,
        "downloads": 3,
        "verified": true,
        "metadata": { "name": name, "description": description, "category": category, "tags": ["crdt"], "icon": "data:image/png;base64,AAAA" },
    })
}

/// The stub's answer for `query`, as the registry's listing answers it.
fn answer(query: &HashMap<String, String>, signed_in: bool) -> (StatusCode, Value) {
    let notes = |v: &str| bundle("com.acme.notes", v, "Notes", "Take notes", "productivity");
    let secret = bundle("com.me.secret", "0.1.0", "Secret", "Mine only", "tools");
    let public = [
        notes("1.9.0"),
        bundle(
            "com.acme.chat",
            "2.0.0",
            "Chat",
            "A chat that keeps notes",
            "social",
        ),
    ];
    let private = query.get("include_private").is_some_and(|v| v == "1") && signed_in;
    if query.contains_key("mine") {
        if !signed_in {
            return (StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" }));
        }
        return (StatusCode::OK, json!([secret]));
    }
    match (
        query.get("package").map(String::as_str),
        query.get("version"),
    ) {
        (None, _) => {
            let mut all = public.to_vec();
            if private {
                all.push(secret);
            }
            (StatusCode::OK, json!(all))
        }
        (Some("com.acme.notes"), None) if query.contains_key("all_versions") => {
            let mut yanked = notes("1.10.0");
            yanked["yanked"] = json!(true);
            (
                StatusCode::OK,
                json!([notes("1.2.0"), yanked, notes("1.9.0")]),
            )
        }
        (Some("com.acme.notes"), None) => (StatusCode::OK, json!([notes("1.9.0")])),
        (Some("com.acme.notes"), Some(v)) if v == "1.2.0" => (StatusCode::OK, json!([notes(v)])),
        (Some("com.me.secret"), None) if private => (StatusCode::OK, json!([secret])),
        (Some(_), Some(_)) => (StatusCode::NOT_FOUND, json!({ "error": "not_found" })),
        (Some(_), None) => (StatusCode::OK, json!([])),
    }
}

type Seen = Arc<Mutex<Vec<(String, Option<String>)>>>;

async fn bundles(
    State(seen): State<Seen>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    let auth = headers
        .get("authorization")
        .map(|a| a.to_str().unwrap().to_owned());
    let signed_in = auth.as_deref() == Some(&format!("Bearer {KEY}"));
    seen.lock().unwrap().push(("GET".into(), auth));
    let (status, body) = answer(&query, signed_in);
    (status, Json(body))
}

async fn push(State(seen): State<Seen>, headers: HeaderMap) -> (StatusCode, Json<Value>) {
    let auth = headers
        .get("authorization")
        .map(|a| a.to_str().unwrap().to_owned());
    seen.lock().unwrap().push(("PUSH".into(), auth));
    (
        StatusCode::CREATED,
        Json(json!({"package": "p", "version": "1.0.0"})),
    )
}

fn meroctl(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_meroctl"))
        .current_dir(cwd)
        .env("CALIMERO_HOME", cwd.join("home"))
        .env_remove("CALIMERO_REGISTRY_URL")
        .env_remove("CALIMERO_REGISTRY_API_KEY")
        .args(args)
        .output()
        .unwrap()
}

fn ok(out: &Output) -> Value {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn err(out: &Output) -> String {
    assert!(
        !out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn packages(v: &Value) -> Vec<&str> {
    v["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["package"].as_str().unwrap())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn registry_lists_searches_and_signs_in() {
    let seen: Seen = Arc::default();
    let app = axum::Router::new()
        .route("/api/bundles", get(bundles))
        .route("/api/bundles/push", post(push))
        .with_state(Arc::clone(&seen));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let _server = tokio::spawn(async move { axum::serve(listener, app).await });

    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().to_owned();
    let run = |args: &[&str]| {
        let d = d.clone();
        let mut all = vec!["registry".to_owned(), "--registry".to_owned(), url.clone()];
        all.extend(args.iter().map(|a| (*a).to_owned()));
        std::thread::spawn(move || meroctl(&d, &all.iter().map(String::as_str).collect::<Vec<_>>()))
            .join()
            .unwrap()
    };

    // Signed out: the public packages, summarised (no icon), no key sent.
    let list = ok(&run(&["list"]));
    assert_eq!(packages(&list), ["com.acme.notes", "com.acme.chat"]);
    assert_eq!(list["packages"][0]["version"], "1.9.0");
    assert_eq!(list["packages"][0]["visibility"], "public");
    assert!(list["packages"][0].get("icon").is_none());
    assert!(seen.lock().unwrap().iter().all(|(_, auth)| auth.is_none()));
    assert!(err(&run(&["list", "--mine"])).contains("registry login"));

    // Search: every word, the package id ranked first, a category filter.
    let found = ok(&run(&["search", "NOTES"]));
    assert_eq!(packages(&found), ["com.acme.notes", "com.acme.chat"]);
    let found = ok(&run(&["search", "notes", "--category", "social"]));
    assert_eq!(packages(&found), ["com.acme.chat"]);
    assert!(packages(&ok(&run(&["search", "absent"]))).is_empty());

    // Versions newest first by semver; the latest skips a yanked one.
    let versions = ok(&run(&["versions", "com.acme.notes"]));
    let listed: Vec<&str> = versions["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["version"].as_str().unwrap())
        .collect();
    assert_eq!(listed, ["1.10.0", "1.9.0", "1.2.0"]);
    assert_eq!(versions["versions"][0]["yanked"], true);
    assert_eq!(versions["latest"], "1.9.0");

    // One release, or the latest; a private package hints at signing in.
    assert_eq!(
        ok(&run(&["get", "com.acme.notes@1.2.0"]))["appVersion"],
        "1.2.0"
    );
    assert_eq!(ok(&run(&["get", "com.acme.notes"]))["appVersion"], "1.9.0");
    assert!(err(&run(&["get", "com.acme.notes@9.9.9"])).contains("release com.acme.notes@9.9.9"));
    assert!(err(&run(&["versions", "com.me.secret"])).contains("registry login"));

    // A key the registry refuses is not saved.
    assert!(err(&run(&["login", "--api-key", "wrong"])).contains("refused the API key"));
    assert!(!d.join("home/meroctl/registry.json").exists());

    // Signed in: the saved key (0600) reaches private packages and `--mine`.
    let login = ok(&run(&["login", "--api-key", KEY]));
    assert_eq!(login["signedIn"], true);
    assert_eq!(login["packages"], 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(d.join("home/meroctl/registry.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    assert_eq!(
        packages(&ok(&run(&["list"]))),
        ["com.acme.notes", "com.acme.chat", "com.me.secret"]
    );
    assert_eq!(packages(&ok(&run(&["list", "--mine"]))), ["com.me.secret"]);
    assert_eq!(ok(&run(&["get", "com.me.secret"]))["appVersion"], "0.1.0");

    // `app publish` takes the saved key when none is given.
    std::fs::write(d.join("app.wasm"), b"\0asm\x0d\0\x01\0app").unwrap();
    let packed = meroctl(
        &d,
        &[
            "app",
            "pack",
            "--wasm",
            "app.wasm",
            "--package",
            "p",
            "--app-version",
            "1.0.0",
            "--out",
            "out.mpk",
        ],
    );
    let _ = ok(&packed);
    let d2 = d.clone();
    let url2 = url.clone();
    let published = std::thread::spawn(move || {
        meroctl(&d2, &["app", "publish", "out.mpk", "--registry", &url2])
    })
    .join()
    .unwrap();
    let _ = ok(&published);
    let auth = seen
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|(kind, _)| kind == "PUSH")
        .and_then(|(_, auth)| auth.clone());
    assert_eq!(auth.as_deref(), Some("Bearer reg_key"));

    // Signed out again: private packages are hidden, the key is forgotten.
    assert_eq!(ok(&run(&["logout"]))["signedIn"], false);
    assert_eq!(
        packages(&ok(&run(&["list"]))),
        ["com.acme.notes", "com.acme.chat"]
    );
}

#[test]
fn a_registry_over_plain_http_is_refused_unless_loopback() {
    let tmp = tempfile::tempdir().unwrap();
    let out = meroctl(
        tmp.path(),
        &["registry", "--registry", "http://registry.example", "list"],
    );
    assert!(err(&out).contains("https://"));
}
