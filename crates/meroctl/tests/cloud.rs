//! `meroctl cloud` against a stand-in for the Cloud's manager API: the session
//! is checked at `login`, kept owner-only, refreshed from the manager's
//! header, and `use` saves a cluster's node as the default node.

use std::path::Path;
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Json;
use serde_json::{json, Value};

fn meroctl(home: &Path, args: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_meroctl"))
        .env("CALIMERO_HOME", home)
        .env_remove("CALIMERO_CLOUD_TOKEN")
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
    serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
}

type Tokens = Arc<Mutex<Vec<String>>>;

fn bearer(headers: &HeaderMap) -> String {
    headers["authorization"]
        .to_str()
        .unwrap()
        .trim_start_matches("Bearer ")
        .to_owned()
}

async fn orgs(State(seen): State<Tokens>, headers: HeaderMap) -> impl IntoResponse {
    let token = bearer(&headers);
    seen.lock().unwrap().push(token.clone());
    if token == "bad" {
        return (
            StatusCode::UNAUTHORIZED,
            HeaderMap::new(),
            Json(json!({"detail": "invalid session"})),
        );
    }
    // The manager rolls the session: the next request uses the new one.
    let mut h = HeaderMap::new();
    let _ = h.insert(
        "x-mdma-session-refresh",
        format!("{token}+").parse().unwrap(),
    );
    (
        StatusCode::OK,
        h,
        Json(json!([{ "id": "o1", "name": "Personal" }])),
    )
}

async fn clusters(State(seen): State<Tokens>, headers: HeaderMap) -> Json<Value> {
    seen.lock().unwrap().push(bearer(&headers));
    Json(json!([{
        "id": "c-1",
        "name": "prod eu",
        "nodes": [{ "node_id": "n1", "public_url": "https://n1.prod.example/" }],
    }]))
}

#[tokio::test(flavor = "multi_thread")]
async fn cloud_signs_in_lists_and_uses_a_cluster() {
    signs_in_lists_and_uses_a_cluster("/api").await;
}

/// A manager from before its unversioned routes serves only `/api/v2/...`:
/// each request falls back to it on the `404`.
#[tokio::test(flavor = "multi_thread")]
async fn cloud_falls_back_to_an_older_managers_v2_routes() {
    signs_in_lists_and_uses_a_cluster("/api/v2").await;
}

/// The manager serving its routes under `prefix`.
async fn signs_in_lists_and_uses_a_cluster(prefix: &str) {
    let seen: Tokens = Arc::default();
    let app = axum::Router::new()
        .route(&format!("{prefix}/orgs"), get(orgs))
        .route(&format!("{prefix}/clusters"), get(clusters))
        .with_state(Arc::clone(&seen));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let _server = tokio::spawn(async move { axum::serve(listener, app).await });

    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_owned();
    let run = |args: &[&str]| {
        let home = home.clone();
        let args: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
        tokio::task::spawn_blocking(move || meroctl(&home, &args))
    };

    // Before login: a hint, not a request.
    let out = run(&["cloud", "orgs"]).await.unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("meroctl cloud login"));

    // A refused session is not saved.
    let out = run(&["cloud", "login", "--token", "bad", "--url", &url])
        .await
        .unwrap();
    assert!(!out.status.success());
    assert!(!home.join("meroctl/cloud.json").exists());

    let login = ok(&run(&["cloud", "login", "--token", "t", "--url", &url])
        .await
        .unwrap());
    assert_eq!(login["orgs"][0]["name"], "Personal");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(home.join("meroctl/cloud.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "the session is the owner's only");
    }

    // The refreshed session from login is the one sent next.
    let _ = ok(&run(&["cloud", "orgs"]).await.unwrap());
    let used = ok(&run(&["cloud", "use", "prod eu"]).await.unwrap());
    assert_eq!(used["node"], "prod-eu");
    assert_eq!(used["url"], "https://n1.prod.example/");
    assert_eq!(*seen.lock().unwrap(), ["bad", "t", "t+", "t++"]);

    // `use` saved it as the default node, attested (no mock).
    let nodes = ok(&run(&["node", "list"]).await.unwrap());
    assert_eq!(nodes["default"], "prod-eu");
    assert_eq!(nodes["nodes"]["prod-eu"]["allowMock"], false);

    let out = run(&["cloud", "use", "missing"]).await.unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("no cluster missing"));

    let _ = run(&["cloud", "logout"]).await.unwrap();
    assert!(!home.join("meroctl/cloud.json").exists());
}
