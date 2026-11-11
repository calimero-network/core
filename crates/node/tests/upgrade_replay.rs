//! A data directory a release's merod wrote opens on this node, which replays its
//! system log, and answers what that release answered.
//!
//! `fixtures/upgrade/<release>/` holds, for each release the replay is pinned to,
//! the data directory of a one-node cluster as that release's `merod run --dev`
//! left it, and `expected.json`: the API keys, and every request the release was
//! sent before it stopped with the status and body it answered. They are written by
//! `crates/node/tests/fault/upgrade_fixture.py` (once per release; see Testing
//! strategy, "Upgrade path"). The data set stays below the log's snapshot interval,
//! so the directory holds no system snapshot and this node rebuilds the whole
//! system state by applying every entry the old release logged; each app's state is
//! the old release's on-disk state.
//!
//! Each request is sent again, as plain HTTP with the same key: the status must be
//! the same and the body must hold every field the old body held, with the same
//! value (this release may add fields, never drop or change one). Then the
//! replayed state takes a write.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use calimero_node::attest::Mode;
use calimero_node::{run, Config};
use serde_json::{json, Value};

/// The fixture node's cluster, and its development storage key's derivation
/// (`merod run --dev`, `crates/merod/src/main.rs`).
const CLUSTER: &str = "upgrade-replay";
/// The ops token `rv2.py` starts every node with.
const OPS_TOKEN: &str = "opsrv2raft0000000000000000000000";
/// Fields that move on their own and say nothing about the replayed state.
const VOLATILE: &[&str] = &["fresh", "lastActiveAt", "lastActiveDay", "sessions"];

/// Totals of the UTC day the read is made on (`today`, `developerFuelToday`,
/// `developerWritesToday`, ...): a replay on a later day than the release's
/// reads them from 0.
fn of_today(key: &str) -> bool {
    key == "today" || key.ends_with("Today")
}

/// A receipt's signature covers the answer's log position (below), so it is
/// another signature over the same state.
const SIGNATURES: &[&str] = &["signature"];

/// Log positions an answer was served at. The replaying node elects itself
/// leader of each group in a new term, which appends a blank entry, so these
/// only move forward from what the release answered.
const LOG_POSITIONS: &[&str] = &["index"];

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn config(dir: &Path, port: u16, release: &str) -> Config {
    let url = format!("http://127.0.0.1:{port}/");
    Config {
        node_id: 1,
        public_url: url.clone(),
        listen: format!("127.0.0.1:{port}").parse().unwrap(),
        data_dir: dir.to_owned(),
        peers: BTreeMap::from([(1, url)]),
        bootstrap: false,
        cluster: CLUSTER.into(),
        storage_key: calimero_identity::sha256(
            format!("calimero-dev-storage-key|{CLUSTER}").as_bytes(),
        ),
        attestation: Mode::Mock,
        allow_mock_peers: true,
        sealed_required: false,
        rp_id: "localhost".into(),
        origins: vec!["http://localhost:3000".into()],
        release: release.into(),
        ops_token: Some(OPS_TOKEN.into()),
        ops_token_sha256: None,
        metrics_listen: None,
        replicas: 1,
        backup_dir: None,
        raft: calimero_log::Config::default(),
        read_wait: Duration::from_secs(2),
        refresh_rotate_after: calimero_node::api::REFRESH_ROTATE_AFTER,
        tee_release: None,
        tee_profile: None,
        peer_measurements: calimero_client::Measurements::None,
        clock_offset_ms: 0,
        kms_keys: Default::default(),
        rollback: None,
        accept_rollback: false,
        budget: None,
        registry: None,
        allow_unsigned_bundles: true,
        allow_package_uploads: true,
        outbound: None,
        governance: Default::default(),
        apps_domain: None,
        operator_key: None,
    }
}

/// `got` holds every field `want` holds, with the same value; `Err` names the first
/// difference by its path.
fn holds(got: &Value, want: &Value, path: &str) -> Result<(), String> {
    match (got, want) {
        (Value::Object(g), Value::Object(w)) => {
            for (k, v) in w {
                if VOLATILE.contains(&k.as_str()) || of_today(k) {
                    continue;
                }
                let at = format!("{path}.{k}");
                let Some(gv) = g.get(k) else {
                    return Err(format!("{at} is gone"));
                };
                if path.ends_with(".receipt") && SIGNATURES.contains(&k.as_str()) {
                    if !gv.is_string() {
                        return Err(format!("{at} is {gv}, was a signature"));
                    }
                    continue;
                }
                if LOG_POSITIONS.contains(&k.as_str()) {
                    if let (Some(gn), Some(wn)) = (gv.as_u64(), v.as_u64()) {
                        if gn < wn {
                            return Err(format!("{at} is {gn}, behind {wn}"));
                        }
                        continue;
                    }
                }
                holds(gv, v, &at)?;
            }
            Ok(())
        }
        (Value::Array(g), Value::Array(w)) => {
            if g.len() != w.len() {
                return Err(format!("{path} has {} items, was {}", g.len(), w.len()));
            }
            for (i, (gv, wv)) in g.iter().zip(w).enumerate() {
                holds(gv, wv, &format!("{path}[{i}]"))?;
            }
            Ok(())
        }
        _ if got == want => Ok(()),
        _ => Err(format!("{path} is {got}, was {want}")),
    }
}

async fn send(
    http: &reqwest::Client,
    base: &str,
    bearer: Option<&str>,
    method: &str,
    path: &str,
    body: &Value,
) -> (u16, Value) {
    let url = format!("{base}{path}");
    let mut req = match method {
        "GET" => http.get(&url),
        "POST" => http.post(&url).json(body),
        other => panic!("method {other}"),
    };
    if let Some(b) = bearer {
        req = req.bearer_auth(b);
    }
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

fn fixtures() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upgrade");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("expected.json").exists())
        .collect();
    dirs.sort();
    assert!(
        !dirs.is_empty(),
        "no replay fixture under {}",
        root.display()
    );
    dirs
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_release_data_directory_replays_to_the_answers_that_release_gave() {
    for fixture in fixtures() {
        replay(&fixture).await;
    }
}

async fn replay(fixture: &Path) {
    let name = fixture.file_name().unwrap().to_string_lossy().into_owned();
    let expected: Value =
        serde_json::from_slice(&std::fs::read(fixture.join("expected.json")).unwrap()).unwrap();
    assert_eq!(expected["cluster"], CLUSTER, "{name}");
    // No system snapshot: the system state below is rebuilt from the old log alone.
    assert_eq!(
        expected["snapshotsHeld"],
        json!([]),
        "{name}: the fixture holds snapshots, so its system log is not replayed whole"
    );
    let tmp = tempfile::tempdir().unwrap();
    let archive = std::fs::read(fixture.join("datadir.tar.gz")).unwrap();
    tar::Archive::new(flate2::read::GzDecoder::new(&archive[..]))
        .unpack(tmp.path())
        .unwrap();
    let dir = tmp.path().join("node1");
    // The fixture's port is in its store, but a single node never dials itself.
    let port = free_port();
    let release = expected["nodeRelease"]
        .as_str()
        .unwrap_or("test")
        .to_owned();
    let r = run(config(&dir, port, &release)).await.unwrap();
    let base = format!("http://127.0.0.1:{port}/");
    let http = reqwest::Client::new();
    let keys = expected["keys"].as_object().unwrap();
    let bearer = |who: &str| -> Option<String> {
        match who {
            "ops" => Some(OPS_TOKEN.to_owned()),
            "anon" => None,
            k => Some(keys[k].as_str().unwrap().to_owned()),
        }
    };

    // Every app's group opens on the old state (the replayed system log names them).
    let alice = bearer("alice");
    for app in expected["apps"].as_object().unwrap().values() {
        let path = format!("apps/{}/storage", app.as_str().unwrap());
        let mut last = (0, Value::Null);
        for _ in 0..200 {
            last = send(&http, &base, alice.as_deref(), "GET", &path, &Value::Null).await;
            if last.0 == 200 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(last.0, 200, "{name}: {path} answers {last:?}");
    }

    let mut diffs = Vec::new();
    for read in expected["reads"].as_array().unwrap() {
        let who = read["as"].as_str().unwrap();
        let method = read["method"].as_str().unwrap();
        let path = read["path"].as_str().unwrap();
        let (status, body) = send(
            &http,
            &base,
            bearer(who).as_deref(),
            method,
            path,
            &read["body"],
        )
        .await;
        let what = format!("{method} {path} as {who}");
        if status != read["status"].as_u64().unwrap() as u16 {
            diffs.push(format!(
                "{what}: status {status} ({body}), was {}",
                read["status"]
            ));
            continue;
        }
        if status != 200 {
            if body.get("error") != read["answer"].get("error") {
                diffs.push(format!("{what}: refused {body}, was {}", read["answer"]));
            }
            continue;
        }
        if let Err(e) = holds(&body, &read["answer"], "$") {
            diffs.push(format!("{what}: {e}"));
        }
    }
    assert!(
        diffs.is_empty(),
        "{name}: this release answers the replayed state differently:\n{}",
        diffs.join("\n")
    );

    // The replayed state takes a write and reads it back.
    let app = expected["apps"]["org"].as_str().unwrap();
    let (status, body) = send(
        &http,
        &base,
        alice.as_deref(),
        "POST",
        &format!("apps/{app}/intents"),
        &json!({ "method": "set", "args": { "key": "replayed", "value": name } }),
    )
    .await;
    assert_eq!(status, 200, "{name}: a write after the replay: {body}");
    let (_, body) = send(
        &http,
        &base,
        alice.as_deref(),
        "POST",
        &format!("apps/{app}/views"),
        &json!({ "method": "get", "args": { "key": "replayed" } }),
    )
    .await;
    assert_eq!(body["result"], json!(name), "{name}");
    r.stop().await;
}
