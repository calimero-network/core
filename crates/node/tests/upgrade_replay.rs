//! A data directory a release's merod wrote opens on this node, which replays its
//! system log, and answers what that release answered.
//!
//! A fixture is the data directory of a one-node cluster as a release's `merod run
//! --dev` left it, and `expected.json`: the API keys, and every request the release
//! was sent before it stopped with the status and body it answered. They are written
//! by `crates/node/tests/fault/upgrade_fixture.py`: the older releases' are committed
//! under `fixtures/upgrade/`, and the pinned release's is recorded by the `Upgrade
//! path` job (`scripts/upgrade-path.sh`) into `UPGRADE_REPLAY_DIR`, so it is never
//! committed (Testing strategy, "Upgrade path"). The data set stays below the log's
//! snapshot interval, so the directory holds no system snapshot and this node rebuilds
//! the whole system state by applying every entry the old release logged, of every kind
//! (`a_fixture_that_covers_every_kind_holds_an_entry_of_each`); each app's state is
//! the old release's on-disk state.
//!
//! Each request is sent again, as plain HTTP with the same key: the status must be
//! the same and the body must hold every field the old body held, with the same
//! value (this release may add fields, never drop or change one). Then the
//! replayed state takes a write.

#![allow(
    clippy::unwrap_used,
    reason = "a test's panic is its report; clippy.toml exempts #[test] fns, not their helpers"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use calimero_log::store::Db;
use calimero_node::attest::Mode;
use calimero_node::node::SYSTEM_GROUP;
use calimero_node::system::SystemCmd;
use calimero_node::{cluster_key, run, Config};
use serde::de::value::MapDeserializer;
use serde::{de, Deserialize};
use serde_json::{json, Value};

/// The fixture node's cluster, and its development storage key's derivation
/// (`merod run --dev`, `crates/merod/src/main.rs`).
const CLUSTER: &str = "upgrade-replay";
/// The ops token `rv2.py` starts every node with.
const OPS_TOKEN: &str = "opsrv2raft0000000000000000000000";
/// The directory the `Upgrade path` job records the pinned release's fixtures into.
const GENERATED: &str = "UPGRADE_REPLAY_DIR";
/// Fields that move on their own and say nothing about the replayed state.
const VOLATILE: &[&str] = &["fresh", "lastActiveAt", "lastActiveDay", "sessions"];
/// Times a read is sent until the apps it names have opened (100 ms apart).
const OPEN_TRIES: usize = 200;

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

/// Whether a dated grant (an invite, a public link) has passed its `expiresAt`, worked
/// out when it is read (`expiresAt <= now`): a replay a week after the release wrote
/// the fixture finds its invites expired. It must agree with `expiresAt` and the
/// replay's clock rather than the release's answer.
const EXPIRED: &str = "expired";

/// Entry kinds (`SystemCmd`'s `op`) a recorded fixture holds none of, each with why. A new
/// command belongs here until the pinned release writes it, or `upgrade_kinds.py` records one.
const NOT_RECORDED: &[(&str, &str)] = &[
    ("startAccountRecovery", "starting one takes a WebAuthn registration made outside a session, which neither meroctl nor the recording's Python can make"),
    ("cancelAccountRecovery", "it cancels a started recovery (startAccountRecovery)"),
    ("finishAccountRecovery", "it finishes a started recovery (startAccountRecovery)"),
    ("finishRecovery", "an organization's recovery finishes `RECOVERY_DELAY_SECS` (days) after it starts"),
    ("useRefresh", "a refresh token is exchanged only once it is `REFRESH_ROTATE_AFTER` (an hour) old, which a recording cannot wait for"),
    ("refreshReused", "it answers an exchanged token presented again (useRefresh)"),
    ("restoreApp", "a restore needs a backup, whose snapshot of the system group purges the log the fixture keeps whole"),
    ("importHolder", "it needs the recovery disk, which snapshots every group as a backup does"),
    ("requireSessionProofs", "a system leader writes it into a cluster begun before session proofs; a new cluster's `Genesis` carries them"),
    ("setClusterOwner", "only a cluster deployed with `--deployers owner` takes one, and governance is fixed at genesis"),
    ("setReleaseApproval", "only a cluster whose release approvals are the owners' takes one; the recording's are the operator's"),
    ("setReplicas", "it moves an app between nodes; a one-node cluster has nowhere to move it and logs nothing for the same set"),
    ("upgradeFailed", "it notes an upgrade that failed after it was approved, which no release of the suite's packages does"),
];

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn storage_key() -> [u8; 32] {
    calimero_identity::sha256(format!("calimero-dev-storage-key|{CLUSTER}").as_bytes())
}

fn config(dir: &Path, port: u16, release: &str, node: &Value) -> Config {
    let url = format!("http://127.0.0.1:{port}/");
    Config {
        node_id: 1,
        public_url: url.clone(),
        listen: format!("127.0.0.1:{port}").parse().unwrap(),
        data_dir: dir.to_owned(),
        peers: BTreeMap::from([(1, url)]),
        bootstrap: false,
        cluster: CLUSTER.into(),
        storage_key: storage_key(),
        attestation: Mode::Mock,
        allow_mock_peers: true,
        tls_extra_roots: Vec::new(),
        sealed_required: false,
        rp_id: "localhost".into(),
        origins: vec!["http://localhost:3000".into()],
        release: release.into(),
        ops_token: Some(OPS_TOKEN.into()),
        ops_token_sha256: None,
        metrics_listen: None,
        replicas: 1,
        backup_dir: None,
        recovery_dir: None,
        recover_from: None,
        restore_from: None,
        raft: calimero_log::Config::default(),
        read_wait: Duration::from_secs(2),
        refresh_rotate_after: calimero_node::api::REFRESH_ROTATE_AFTER,
        tee_release: node["teeRelease"].as_str().map(str::to_owned),
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
        app_logs: false,
        outbound: None,
        governance: Default::default(),
        apps_domain: node["appsDomain"].as_str().map(str::to_owned),
        attestation_verifier_url: None,
        operator_key: None,
    }
}

/// The apps a report names with a last write: their groups have opened, which a
/// slow runner may not have done when the first read is sent.
fn written(report: &Value) -> BTreeSet<&str> {
    let apps = report["apps"].as_array().into_iter().flatten();
    apps.filter(|a| !a["lastWriteAt"].is_null())
        .filter_map(|a| a["app"].as_str())
        .collect()
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
                if k == EXPIRED && w.contains_key("expiresAt") {
                    let want = g
                        .get("expiresAt")
                        .and_then(Value::as_u64)
                        .is_some_and(|at| at <= now_secs());
                    if gv.as_bool() != Some(want) {
                        return Err(format!("{at} is {gv}, expiresAt says {want}"));
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

/// The fixture folders under `root`.
fn folders(root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("expected.json").exists())
        .collect();
    dirs.sort();
    dirs
}

/// What the `Upgrade path` job recorded just now, none where `GENERATED` is unset.
fn generated() -> Vec<PathBuf> {
    let Some(root) = std::env::var_os(GENERATED) else {
        return Vec::new();
    };
    let dirs = folders(Path::new(&root));
    assert!(!dirs.is_empty(), "{GENERATED} holds no fixture");
    dirs
}

fn fixtures() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upgrade");
    let mut dirs = folders(&root);
    dirs.extend(generated());
    // None right after a hard cutover, which drops the releases before it
    // (`fixtures/upgrade/README.md`), until the next release records one.
    if dirs.is_empty() {
        eprintln!(
            "no replay fixture under {} since the last hard cutover",
            root.display()
        );
    }
    dirs
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_release_data_directory_replays_to_the_answers_that_release_gave() {
    for fixture in fixtures() {
        replay(&fixture).await;
    }
}

/// A fixture's data directory: `datadir.tar.zst`, or `datadir.tar.gz` for the older ones.
fn unpack(fixture: &Path, into: &Path) {
    let zst = fixture.join("datadir.tar.zst");
    if zst.exists() {
        let archive = std::fs::File::open(zst).unwrap();
        tar::Archive::new(zstd::stream::read::Decoder::new(archive).unwrap())
            .unpack(into)
            .unwrap();
        return;
    }
    let archive = std::fs::read(fixture.join("datadir.tar.gz")).unwrap();
    tar::Archive::new(flate2::read::GzDecoder::new(&archive[..]))
        .unpack(into)
        .unwrap();
}

async fn replay(fixture: &Path) {
    let name = fixture.file_name().unwrap().to_string_lossy().into_owned();
    let expected: Value =
        serde_json::from_slice(&std::fs::read(fixture.join("expected.json")).unwrap()).unwrap();
    assert_eq!(expected["cluster"], CLUSTER, "{name}");
    // No system snapshot: the system state below is rebuilt from the old log alone (an
    // app's snapshot, a restore point's, leaves the system log whole).
    let system_snapshot = format!("g{SYSTEM_GROUP}-");
    assert!(
        expected["snapshotsHeld"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| !s.as_str().unwrap().starts_with(&system_snapshot)),
        "{name}: the fixture holds a system snapshot, so its system log is not replayed whole: {}",
        expected["snapshotsHeld"]
    );
    let tmp = tempfile::tempdir().unwrap();
    unpack(fixture, tmp.path());
    let dir = tmp.path().join("node1");
    // The fixture's port is in its store, but a single node never dials itself.
    let port = free_port();
    let release = expected["nodeRelease"]
        .as_str()
        .unwrap_or("test")
        .to_owned();
    let r = run(config(&dir, port, &release, &expected["node"]))
        .await
        .unwrap();
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
        let mut got = (0, Value::Null);
        for _ in 0..OPEN_TRIES {
            got = send(
                &http,
                &base,
                bearer(who).as_deref(),
                method,
                path,
                &read["body"],
            )
            .await;
            if written(&read["answer"]).is_subset(&written(&got.1)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let (status, body) = got;
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

/// A `serde` error that keeps the variants a derived enum names when its tag is unknown.
#[derive(Debug)]
struct Variants(&'static [&'static str]);

impl fmt::Display for Variants {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

impl std::error::Error for Variants {}

impl de::Error for Variants {
    fn custom<T: fmt::Display>(_: T) -> Self {
        Variants(&[])
    }

    fn unknown_variant(_: &str, expected: &'static [&'static str]) -> Self {
        Variants(expected)
    }
}

/// Every kind of system command this tree reads, as the derive names them.
fn command_kinds() -> BTreeSet<&'static str> {
    let unknown_op = MapDeserializer::<_, Variants>::new([("op", "")].into_iter());
    match SystemCmd::deserialize(unknown_op) {
        Err(Variants(kinds)) if !kinds.is_empty() => kinds.iter().copied().collect(),
        other => panic!("an unknown op should name the kinds: {other:?}"),
    }
}

/// The kinds the system log of the data directory `dir` holds an entry of.
fn recorded_kinds(dir: &Path) -> BTreeSet<String> {
    let k_r = storage_key();
    let key = cluster_key::load(dir, &k_r).unwrap().unwrap_or(k_r);
    let db = Db::open(dir, &key).unwrap();
    db.commands(SYSTEM_GROUP)
        .unwrap()
        .iter()
        .map(|cmd| {
            let entry: Value = serde_json::from_slice(cmd).unwrap();
            entry["op"]
                .as_str()
                .expect("a command names its op")
                .to_owned()
        })
        .collect()
}

/// What is wrong with `recorded` as the kinds of a fixture that covers `known`, but for `allowed`.
fn coverage_gaps(
    known: &BTreeSet<&str>,
    recorded: &BTreeSet<String>,
    allowed: &[(&str, &str)],
) -> Vec<String> {
    let mut gaps = Vec::new();
    for kind in known {
        let listed = allowed.iter().any(|(k, _)| k == kind);
        match (recorded.contains(*kind), listed) {
            (false, false) => gaps.push(format!("{kind}: no entry of it in the fixture")),
            (true, true) => gaps.push(format!(
                "{kind}: is recorded, so leave it out of NOT_RECORDED"
            )),
            _ => {}
        }
    }
    for kind in recorded.iter().filter(|k| !known.contains(k.as_str())) {
        gaps.push(format!(
            "{kind}: the fixture holds it, this tree reads no such command"
        ));
    }
    for (kind, why) in allowed {
        if !known.contains(kind) {
            gaps.push(format!("{kind}: in NOT_RECORDED but no command"));
        }
        if why.is_empty() {
            gaps.push(format!("{kind}: in NOT_RECORDED with no reason"));
        }
    }
    gaps
}

/// The release recorded now (its fixtures: one cluster holds under 1000 entries) holds an entry of
/// every kind. Without `GENERATED` there is nothing recorded, and the test has nothing to read.
#[test]
fn a_fixture_that_covers_every_kind_holds_an_entry_of_each() {
    if std::env::var_os(GENERATED).is_none() {
        eprintln!("{GENERATED} is unset: the Upgrade path job records the fixture and runs this");
        return;
    }
    let mut by_release: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for fixture in generated() {
        let expected: Value =
            serde_json::from_slice(&std::fs::read(fixture.join("expected.json")).unwrap()).unwrap();
        if expected["everyKind"] != json!(true) {
            continue;
        }
        let tmp = tempfile::tempdir().unwrap();
        unpack(&fixture, tmp.path());
        by_release
            .entry(expected["release"].as_str().unwrap().to_owned())
            .or_default()
            .extend(recorded_kinds(&tmp.path().join("node1")));
    }
    assert!(
        !by_release.is_empty(),
        "no fixture records every kind of entry"
    );
    for (release, recorded) in by_release {
        let gaps = coverage_gaps(&command_kinds(), &recorded, NOT_RECORDED);
        assert!(
            gaps.is_empty(),
            "{release}: the system logs of its fixtures do not cover every kind of entry:\n{}\nwrite one in \
             crates/node/tests/fault/upgrade_kinds.py or list the kind in NOT_RECORDED",
            gaps.join("\n")
        );
    }
}

#[test]
fn a_kind_the_fixture_lacks_is_a_gap_unless_it_is_listed_with_a_reason() {
    let known = BTreeSet::from(["createOrg", "setReplicas", "genesis"]);
    let recorded = BTreeSet::from(["createOrg".to_owned(), "genesis".to_owned()]);
    assert_eq!(
        coverage_gaps(&known, &recorded, &[]),
        ["setReplicas: no entry of it in the fixture"]
    );
    let listed = [("setReplicas", "one node has nowhere to move an app")];
    assert!(coverage_gaps(&known, &recorded, &listed).is_empty());
    assert_eq!(
        coverage_gaps(&known, &recorded, &[("genesis", "x"), ("setReplicas", "")]),
        [
            "genesis: is recorded, so leave it out of NOT_RECORDED",
            "setReplicas: in NOT_RECORDED with no reason"
        ]
    );
    let stale = BTreeSet::from([
        "createOrg".to_owned(),
        "genesis".to_owned(),
        "gone".to_owned(),
    ]);
    assert_eq!(
        coverage_gaps(&known, &stale, &listed),
        ["gone: the fixture holds it, this tree reads no such command"]
    );
}

#[test]
fn the_kinds_of_system_command_are_read_from_the_enum() {
    let kinds = command_kinds();
    assert!(
        kinds.contains("createOrg") && kinds.contains("genesis"),
        "{kinds:?}"
    );
    assert!(kinds.len() > 100, "{kinds:?}");
}

#[test]
fn an_invite_the_release_answered_open_reads_expired_once_its_time_passes() {
    // The release answered `expired: false` before its `expiresAt` (1); a replay
    // after it must read `true`, and nothing else.
    let want = json!({ "expiresAt": 1, "expired": false });
    holds(&json!({ "expiresAt": 1, "expired": true }), &want, "invite").unwrap();
    assert!(holds(
        &json!({ "expiresAt": 1, "expired": false }),
        &want,
        "invite"
    )
    .is_err());
    let later = json!({ "expiresAt": u64::MAX, "expired": false });
    holds(&later, &later, "invite").unwrap();
    assert!(holds(
        &json!({ "expiresAt": 2, "expired": false }),
        &want,
        "invite"
    )
    .is_err());
}

#[test]
fn an_app_is_open_once_the_report_names_its_last_write() {
    let report = json!({ "apps": [
        { "app": "a", "lastWriteAt": 5 },
        { "app": "b", "lastWriteAt": null },
        { "app": "c" },
    ] });
    assert_eq!(written(&report), BTreeSet::from(["a"]));
    assert!(written(&json!({ "error": "x" })).is_empty());
    assert!(written(&report).is_subset(&written(&report)));
    assert!(!written(&report).is_subset(&written(
        &json!({ "apps": [{ "app": "a", "lastWriteAt": null }] })
    )));
}
