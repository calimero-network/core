//! A three-node cluster in one process, driven only through the client, over
//! sealed channels, with the real kv-store app.
//!
//! Needs `kv_store.wasm`: `cargo build -p kv-store --target wasm32-unknown-unknown --profile app-release`,
//! or set `KV_STORE_WASM`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use calimero_cloud_client::{Client, Consistency, Policy};
use calimero_cloud_node::attest::Mode;
use calimero_cloud_node::{run, Config, Running};
use calimero_identity::testing::SoftAuthenticator;
use futures_util::StreamExt;
use serde_json::{json, Value};

const ORIGIN: &str = "http://localhost:3000";

fn kv_wasm() -> Vec<u8> {
    let path = std::env::var("KV_STORE_WASM")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/wasm32-unknown-unknown/app-release/kv_store.wasm")
        });
    std::fs::read(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; build kv-store for wasm32 first", path.display()))
}

fn wasm(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/wasm32-unknown-unknown/app-release")
        .join(format!("{name}.wasm"));
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!("{}: {e}; build it with cargo build --target wasm32-unknown-unknown --profile app-release", path.display())
    })
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct Cluster {
    nodes: Vec<Option<Running>>,
    urls: Vec<String>,
    _dirs: Vec<tempfile::TempDir>,
}

impl Cluster {
    async fn start(n: u64) -> Self {
        Self::start_with(n, |_| {}).await
    }

    async fn start_with(n: u64, configure: impl Fn(&mut Config)) -> Self {
        let ports: Vec<u16> = (0..n).map(|_| free_port()).collect();
        let urls: Vec<String> = ports
            .iter()
            .map(|p| format!("http://127.0.0.1:{p}/"))
            .collect();
        let peers: BTreeMap<u64, String> = urls
            .iter()
            .enumerate()
            .map(|(i, u)| (i as u64 + 1, u.clone()))
            .collect();
        let mut nodes = Vec::new();
        let mut dirs = Vec::new();
        for (i, port) in ports.iter().enumerate() {
            let dir = tempfile::tempdir().unwrap();
            let mut config = Config {
                node_id: i as u64 + 1,
                public_url: urls[i].clone(),
                listen: format!("127.0.0.1:{port}").parse().unwrap(),
                data_dir: dir.path().to_owned(),
                peers: peers.clone(),
                bootstrap: i == 0,
                cluster: "test".into(),
                storage_key: [42; 32],
                attestation: Mode::Mock,
                allow_mock_peers: true,
                sealed_required: true,
                rp_id: "localhost".into(),
                origins: vec![ORIGIN.into()],
                release: "test".into(),
                ops_token: Some("ops-secret".into()),
                replicas: 3,
                backup_dir: Some(dir.path().join("backups")),
                raft: calimero_log::Config {
                    heartbeat_ms: 100,
                    election_min_ms: 500,
                    election_max_ms: 1000,
                    snapshot_every: 200,
                },
                session_key_rotation_secs: 6 * 3600,
                tee_release: None,
                tee_profile: None,
                peer_measurements: calimero_cloud_client::Measurements::None,
            };
            configure(&mut config);
            nodes.push(Some(run(config).await.unwrap()));
            dirs.push(dir);
        }
        let c = Self {
            nodes,
            urls,
            _dirs: dirs,
        };
        // Wait for the system group to elect, register nodes and mint a session key.
        for _ in 0..100 {
            let ready = c.nodes[0]
                .as_ref()
                .unwrap()
                .node
                .sys(|s| s.nodes.len() == n as usize && !s.session_keys.is_empty());
            if ready {
                return c;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("cluster did not form");
    }

    fn client(&self, i: usize) -> Client {
        Client::connect(
            &self.urls[i],
            Policy {
                allow_mock: true,
                ..Policy::default()
            },
        )
        .unwrap()
    }

    async fn stop(&mut self, i: usize) {
        if let Some(r) = self.nodes[i].take() {
            r.stop().await;
        }
    }

    fn app_leader(&self, app: &str) -> Option<usize> {
        self.nodes.iter().enumerate().find_map(|(i, r)| {
            let r = r.as_ref()?;
            r.node
                .hosted(app)
                .filter(|g| g.status().is_leader)
                .map(|_| i)
        })
    }
}

async fn signed_up(c: &Cluster, i: usize, name: &str) -> (Client, SoftAuthenticator) {
    let client = c.client(i);
    let mut pk = SoftAuthenticator::new("localhost", ORIGIN);
    let _s = client.sign_up(&mut pk, name).await.unwrap();
    (client, pk)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn end_to_end() {
    let mut c = Cluster::start(3).await;

    // Sign up with a passkey; sign in again from another node.
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let alice_id = alice.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let alice_on_2 = c.client(1);
    let s = alice_on_2.sign_in(&mut alice_pk).await.unwrap();
    assert_eq!(s.account_id, alice_id);

    // Deploy kv-store and call it.
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
    let r = alice
        .call(&app, "set", json!({ "key": "a", "value": "1" }), None)
        .await
        .unwrap();
    assert!(r.index > 0);
    assert_eq!(r.events.len(), 1, "{:?}", r.events);

    // Strong read anywhere; stale read with read-your-writes on another node.
    let v = alice
        .view(&app, "get", json!({ "key": "a" }), Consistency::Strong)
        .await
        .unwrap();
    assert_eq!(v.result, json!("1"));
    let v = alice_on_2
        .view(
            &app,
            "get",
            json!({ "key": "a" }),
            Consistency::Stale { min_index: r.index },
        )
        .await
        .unwrap();
    assert_eq!(v.result, json!("1"));

    // An idempotency key makes a retry answer once.
    let r1 = alice
        .call(
            &app,
            "set",
            json!({ "key": "b", "value": "x" }),
            Some("op-1"),
        )
        .await
        .unwrap();
    let r2 = alice
        .call(
            &app,
            "set",
            json!({ "key": "b", "value": "y" }),
            Some("op-1"),
        )
        .await
        .unwrap();
    assert_eq!(r1.index, r2.index);
    let v = alice
        .view(&app, "get", json!({ "key": "b" }), Consistency::Strong)
        .await
        .unwrap();
    assert_eq!(v.result, json!("x"));

    // The app's own error comes back as APP_ERROR.
    let e = alice
        .call(&app, "get_result", json!({ "key": "missing" }), None)
        .await
        .unwrap_err();
    assert!(matches!(e.code(), Some("APP_ERROR" | "TRAP")), "{e}");

    // Events: replay from the start of the log.
    let mut events = alice.events(&app, 0).await.unwrap();
    let (_id, kind, _data) = tokio::time::timeout(Duration::from_secs(5), events.next())
        .await
        .unwrap()
        .unwrap();
    assert!(!kind.is_empty());

    // Another account cannot reach an owner-only app.
    let (bob, _bob_pk) = signed_up(&c, 2, "bob").await;
    let e = bob
        .call(&app, "set", json!({ "key": "z", "value": "1" }), None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NOT_FOUND"));

    // Leader loss: stop the app's leader; calls keep working through the others.
    let leader = c.app_leader(&app).unwrap();
    c.stop(leader).await;
    let other = (0..3).find(|i| *i != leader).unwrap();
    let alice_other = c.client(other);
    let _s = alice_other.sign_in(&mut alice_pk).await.unwrap();
    let mut ok = None;
    for _ in 0..40 {
        match alice_other
            .call(
                &app,
                "set",
                json!({ "key": "c", "value": "after" }),
                Some("op-failover"),
            )
            .await
        {
            Ok(r) => {
                ok = Some(r);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(250)).await,
        }
    }
    assert!(ok.is_some(), "no write after leader loss");
    let v = alice_other
        .view(&app, "get", json!({ "key": "a" }), Consistency::Strong)
        .await
        .unwrap();
    assert_eq!(v.result, json!("1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn confirm_methods_need_a_fresh_passkey() {
    let c = Cluster::start(3).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice
        .create_app("kv", &bundle, json!({ "confirmMethods": ["clear"] }))
        .await
        .unwrap();
    let _r = alice
        .call(&app, "set", json!({ "key": "a", "value": "1" }), None)
        .await
        .unwrap();
    let e = alice
        .call(&app, "clear", json!({}), None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"));
    let _r = alice
        .call_confirmed(&app, "clear", json!({}), &mut pk)
        .await
        .unwrap();
    let v = alice
        .view(&app, "len", json!({}), Consistency::Strong)
        .await
        .unwrap();
    assert_eq!(v.result, json!(0));

    // A passkey of another account cannot approve.
    let (_bob, mut bob_pk) = signed_up(&c, 1, "bob").await;
    let e = alice
        .call_confirmed(&app, "clear", json!({}), &mut bob_pk)
        .await
        .unwrap_err();
    assert!(
        matches!(e.code(), Some("UNAUTHENTICATED" | "FORBIDDEN")),
        "{e}"
    );

    // An API key without allowConfirm cannot call it at all.
    let (_id, key) = alice
        .create_api_key(json!({ "label": "agent" }))
        .await
        .unwrap();
    let agent = c.client(2);
    agent.use_api_key(&key).await;
    let _r = agent
        .call(&app, "set", json!({ "key": "k", "value": "v" }), None)
        .await
        .unwrap();
    let e = agent
        .call(&app, "clear", json!({}), None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn organizations_gate_apps_and_api_keys_are_scoped() {
    let c = Cluster::start(3).await;
    let (alice, _apk) = signed_up(&c, 0, "alice").await;
    let (bob, _bpk) = signed_up(&c, 1, "bob").await;
    let (carol, _cpk) = signed_up(&c, 2, "carol").await;
    let bob_id = bob.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();

    let org = alice.post("orgs", json!({ "name": "Acme" })).await.unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let team = alice
        .post(
            &format!("orgs/{org}/teams"),
            json!({ "name": "eng", "join": "invite" }),
        )
        .await
        .unwrap()["teamId"]
        .as_str()
        .unwrap()
        .to_owned();
    let invite = alice
        .post(
            &format!("orgs/{org}/invites"),
            json!({ "role": "member", "teams": [team] }),
        )
        .await
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = bob
        .post(&format!("invites/{invite}/redeem"), json!({}))
        .await
        .unwrap();
    // The invite was single-use.
    let e = carol
        .post(&format!("invites/{invite}/redeem"), json!({}))
        .await
        .unwrap_err();
    assert!(e.code().is_some());

    let o: Value = bob.get(&format!("orgs/{org}")).await.unwrap();
    assert_eq!(o["members"][&bob_id], json!("member"));
    assert_eq!(o["teams"][&team]["members"][&bob_id], json!(false));

    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice
        .create_app("team-kv", &bundle, json!({ "org": org }))
        .await
        .unwrap();
    let _r = bob
        .call(&app, "set", json!({ "key": "from", "value": "bob" }), None)
        .await
        .unwrap();
    let e = carol
        .call(
            &app,
            "set",
            json!({ "key": "from", "value": "carol" }),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NOT_FOUND"));

    // Removing bob from the org cuts him off at once.
    let _r = alice
        .delete(&format!("orgs/{org}/members/{bob_id}"))
        .await
        .unwrap();
    for _ in 0..20 {
        if bob
            .call(&app, "set", json!({ "key": "x", "value": "y" }), None)
            .await
            .is_err()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let e = bob
        .call(&app, "set", json!({ "key": "x", "value": "y" }), None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NOT_FOUND"));

    // A read-only API key reads but does not write.
    let (_id, key) = alice
        .create_api_key(json!({ "readOnly": true, "apps": [app] }))
        .await
        .unwrap();
    let agent = c.client(2);
    agent.use_api_key(&key).await;
    let v = agent
        .view(&app, "get", json!({ "key": "from" }), Consistency::Strong)
        .await
        .unwrap();
    assert_eq!(v.result, json!("bob"));
    let e = agent
        .call(&app, "set", json!({ "key": "a", "value": "b" }), None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clients_refuse_unattested_nodes_and_plain_requests() {
    let c = Cluster::start(3).await;
    // A client that trusts only real TEEs refuses a mock quote before sending anything.
    let strict = Client::connect(
        &c.urls[0],
        Policy {
            allow_mock: false,
            ..Policy::default()
        },
    )
    .unwrap();
    let e = strict
        .post("auth/passkey/login/begin", json!({}))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("mock quote refused"), "{e}");

    // A plain (unsealed) request is refused; attestation and health are served.
    let plain = reqwest::Client::new();
    let r = plain
        .post(format!("{}auth/passkey/login/begin", c.urls[0]))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let r = plain
        .get(format!("{}health", c.urls[0]))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // A peer route without the cluster secret is refused, even sealed.
    let client = c.client(0);
    let e = client
        .post(
            "internal/system",
            json!({ "op": "registerNode", "node": { "id": 9, "addr": "x" } }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"));

    // The Ops API needs the operator credential and returns counts only.
    let e = client.get("ops/status").await.unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backups_are_encrypted_and_restorable() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
    let _r = alice
        .call(
            &app,
            "set",
            json!({ "key": "secret", "value": "plaintext-value" }),
            None,
        )
        .await
        .unwrap();
    let leader = c.app_leader(&app).unwrap();
    let node = &c.nodes[leader].as_ref().unwrap().node;
    let written = node.backup().await.unwrap();
    assert_eq!(written, 1);
    let dir = node.config.backup_dir.clone().unwrap();
    let file = std::fs::read_dir(dir.join(&app))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let raw = std::fs::read(&file).unwrap();
    assert!(!String::from_utf8_lossy(&raw).contains("plaintext-value"));
    let (_index, data) = calimero_cloud_node::backup::latest(&dir, &node.config.storage_key, &app)
        .unwrap()
        .unwrap();
    let state: Value = serde_json::from_slice(&data).unwrap();
    assert!(state["kv"].as_object().is_some_and(|m| !m.is_empty()));
}

/// Regression: Raft entries carrying WASM bundles exceeded the HTTP body
/// limit (2 MiB), so followers were refused (413) forever and fell behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn large_entries_reach_every_replica() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let mut wasm = kv_wasm();
    // A 2.5 MiB custom section: valid WASM, one entry well over 2 MiB.
    let payload = vec![0_u8; 2_500_000];
    let mut section = vec![3, b'p', b'a', b'd'];
    section.extend_from_slice(&payload);
    let mut size = section.len();
    let mut leb = Vec::new();
    loop {
        let byte = (size & 0x7f) as u8;
        size >>= 7;
        if size == 0 {
            leb.push(byte);
            break;
        }
        leb.push(byte | 0x80);
    }
    wasm.push(0);
    wasm.extend_from_slice(&leb);
    wasm.extend_from_slice(&section);
    let hash = alice.upload_bundle(wasm).await.unwrap();
    let leader_applied = c
        .nodes
        .iter()
        .flatten()
        .filter_map(|r| r.node.system.status().last_log)
        .max()
        .unwrap_or(0);
    for r in c.nodes.iter().flatten() {
        assert!(
            r.node
                .system
                .wait_applied(leader_applied, Duration::from_secs(10))
                .await,
            "node {} fell behind",
            r.node.config.node_id
        );
        assert_eq!(r.node.sys(|s| s.bundles.len()), 1);
        assert!(r.node.engine.get_blob(&hash).unwrap().is_some());
    }
    // The log keeps moving after the large bundle.
    let app = alice.create_app("big", &hash, json!({})).await.unwrap();
    let _out = alice
        .call(&app, "set", json!({ "key": "k", "value": "v" }), None)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn ops_moves_an_app_and_points_followers_at_the_leader() {
    let c = Cluster::start(4).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let created = alice
        .post("apps", json!({ "name": "kv", "bundle": bundle }))
        .await
        .unwrap();
    let app = created["appId"].as_str().unwrap().to_owned();
    let replicas: Vec<u64> = serde_json::from_value(created["replicas"].clone()).unwrap();
    let _ = alice
        .call(&app, "set", json!({ "key": "a", "value": "1" }), None)
        .await
        .unwrap();
    let spare = (1..=4).find(|n| !replicas.contains(n)).unwrap();
    let leader = c.app_leader(&app).unwrap() as u64 + 1;
    let follower = *replicas.iter().find(|r| **r != leader).unwrap();
    // Move the app off its current leader, the hardest case.
    let new: Vec<u64> = replicas
        .iter()
        .copied()
        .filter(|r| *r != leader)
        .chain([spare])
        .collect();

    let ops = |i: u64| {
        let client = c.client(i as usize - 1);
        async move {
            client.use_api_key("ops-secret").await;
            client
        }
    };
    // A follower names the leader instead of failing opaquely.
    let err = ops(follower)
        .await
        .post(
            &format!("ops/apps/{app}/replicas"),
            json!({ "replicas": new }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("NOT_LEADER"), "{err}");
    let url = err.details().unwrap()["leaderUrl"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(url, c.urls[leader as usize - 1]);

    let moved = ops(leader)
        .await
        .post(
            &format!("ops/apps/{app}/replicas"),
            json!({ "replicas": new }),
        )
        .await
        .unwrap();
    let mut voters: Vec<u64> = serde_json::from_value(moved["status"]["voters"].clone()).unwrap();
    voters.sort_unstable();
    let mut want = new.clone();
    want.sort_unstable();
    assert_eq!(voters, want);
    let v = alice
        .view(&app, "get", json!({ "key": "a" }), Consistency::Strong)
        .await
        .unwrap();
    assert_eq!(v.result, json!("1"));

    // The node the app moved off still serves it, by forwarding.
    let off = c.client(leader as usize - 1);
    off.use_session(alice.session().await.unwrap()).await;
    let v = off
        .view(&app, "get", json!({ "key": "a" }), Consistency::Strong)
        .await
        .unwrap();
    assert_eq!(v.result, json!("1"));
    let _ = off
        .call(&app, "set", json!({ "key": "b", "value": "2" }), None)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_failed_init_is_reported_not_hidden() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    // `(module (memory (export "memory") 1))`: it compiles, but has no `init`.
    let empty = b"\0asm\x01\0\0\0\x05\x03\x01\x00\x01\x07\x0a\x01\x06memory\x02\x00".to_vec();
    let bundle = alice.upload_bundle(empty).await.unwrap();
    let err = alice
        .create_app("broken", &bundle, json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("INIT_FAILED"), "{err}");
    assert!(err.details().unwrap()["appId"].is_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn quotas_are_enforced_by_the_log() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let ops = c.client(1);
    ops.use_api_key("ops-secret").await;
    let mut quotas = ops.get("ops/quotas").await.unwrap();
    assert_eq!(quotas["maxAppsPerAccount"], json!(100));
    quotas["maxAppsPerAccount"] = json!(1);
    quotas["maxApiKeysPerAccount"] = json!(1);
    let _ok = ops.post("ops/quotas", quotas).await.unwrap();

    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let _first = alice.create_app("one", &bundle, json!({})).await.unwrap();
    let err = alice
        .create_app("two", &bundle, json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("QUOTA_EXCEEDED"), "{err}");

    let _key = alice.create_api_key(json!({})).await.unwrap();
    let err = alice.create_api_key(json!({})).await.unwrap_err();
    assert_eq!(err.code(), Some("QUOTA_EXCEEDED"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn session_keys_rotate_and_old_sessions_stay_valid() {
    let c = Cluster::start_with(3, |cfg| cfg.session_key_rotation_secs = 3).await;
    let (alice, pk) = signed_up(&c, 0, "alice").await;
    let version = || {
        c.nodes[0]
            .as_ref()
            .unwrap()
            .node
            .sys(|s| s.session_keys.last().map(|k| k.version))
            .unwrap()
    };
    let before = version();
    // Wait for exactly one rotation (keys are kept: current and previous).
    let mut after = before;
    for _ in 0..80 {
        after = version();
        if after > before {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(after > before, "key did not rotate ({before} -> {after})");
    // The session minted under the previous key still works...
    let _me = alice.me().await.unwrap();
    // ...and a new sign-in is signed with the current key.
    let mut pk = pk;
    let fresh = c.client(2);
    let s = fresh.sign_in(&mut pk).await.unwrap();
    let claims = calimero_identity::peek_session(&s.session_token).unwrap();
    assert_eq!(claims.key_version, after);
}

async fn strong_view(cl: &Client, app: &str, method: &str, args: Value) -> Value {
    cl.view(app, method, args, Consistency::Strong)
        .await
        .unwrap()
        .result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn apps_see_the_callers_organization_role_and_teams() {
    let c = Cluster::start(3).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let str_of = |v: Value, k: &str| v[k].as_str().unwrap().to_owned();
    let org = str_of(
        alice.post("orgs", json!({ "name": "Acme" })).await.unwrap(),
        "orgId",
    );
    let team = str_of(
        alice
            .post(
                &format!("orgs/{org}/teams"),
                json!({ "name": "eng", "join": "invite" }),
            )
            .await
            .unwrap(),
        "teamId",
    );
    let invite = str_of(
        alice
            .post(
                &format!("orgs/{org}/invites"),
                json!({ "role": "member", "teams": [team] }),
            )
            .await
            .unwrap(),
        "token",
    );
    let _r = bob
        .post(&format!("invites/{invite}/redeem"), json!({}))
        .await
        .unwrap();

    let bundle = alice.upload_bundle(wasm("org_gated")).await.unwrap();
    let app = alice
        .create_app("notes", &bundle, json!({ "org": org }))
        .await
        .unwrap();

    // The app reads each caller's standing from the node.
    let me = strong_view(&alice, &app, "whoami", json!({})).await;
    assert_eq!(me["role"], "owner");
    assert_eq!(me["org"], json!(org));
    let me = strong_view(&bob, &app, "whoami", json!({})).await;
    assert_eq!(me["role"], "member");
    assert_eq!(me["teams"], json!([team]));

    // Admins write the pinned note; members read it but cannot write it.
    let _ok = alice
        .call(&app, "set_pinned", json!({ "text": "hello" }), None)
        .await
        .unwrap();
    assert_eq!(strong_view(&bob, &app, "pinned", json!({})).await, "hello");
    let err = bob
        .call(&app, "set_pinned", json!({ "text": "mine" }), None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("APP_ERROR"), "{err}");

    // Team members write their team's note; alice (owner, not in the team) cannot.
    let _ok = bob
        .call(
            &app,
            "set_team_note",
            json!({ "team": team, "text": "ship it" }),
            None,
        )
        .await
        .unwrap();
    let err = alice
        .call(
            &app,
            "set_team_note",
            json!({ "team": team, "text": "x" }),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("APP_ERROR"), "{err}");

    // Outside an organization the app sees no standing.
    let solo = alice.create_app("solo", &bundle, json!({})).await.unwrap();
    let me = alice
        .view(&solo, "whoami", json!({}), Consistency::Strong)
        .await
        .unwrap()
        .result;
    assert_eq!(me, Value::Null);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn browsers_on_allowed_origins_may_call_the_node() {
    let c = Cluster::start(3).await;
    let http = reqwest::Client::new();
    let preflight = |origin: &'static str| {
        http.request(reqwest::Method::OPTIONS, format!("{}sealed/v2", c.urls[0]))
            .header("origin", origin)
            .header("access-control-request-method", "POST")
            .header("access-control-request-headers", "content-type")
            .send()
    };
    let ok = preflight(ORIGIN).await.unwrap();
    assert_eq!(
        ok.headers().get("access-control-allow-origin").unwrap(),
        ORIGIN
    );
    let other = preflight("https://evil.example").await.unwrap();
    assert!(other.headers().get("access-control-allow-origin").is_none());
    // A simple GET carries the header too.
    let health = http
        .get(format!("{}health", c.urls[0]))
        .header("origin", ORIGIN)
        .send()
        .await
        .unwrap();
    assert_eq!(
        health.headers().get("access-control-allow-origin").unwrap(),
        ORIGIN
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_answers_to_its_context_id_too() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let created = alice
        .post("apps", json!({ "name": "kv", "bundle": bundle }))
        .await
        .unwrap();
    let app = created["appId"].as_str().unwrap();
    let ctx = created["contextId"].as_str().unwrap();
    assert_eq!(ctx, hex::encode(calimero_identity::sha256(app.as_bytes())));
    let _ = alice
        .call(ctx, "set", json!({ "key": "a", "value": "1" }), None)
        .await
        .unwrap();
    let v = alice
        .view(app, "get", json!({ "key": "a" }), Consistency::Strong)
        .await
        .unwrap();
    assert_eq!(v.result, json!("1"));
}

/// A core fixture app built with `cargo mero build` (so its ABI is embedded:
/// handlers, timers and xcall entry points come from it).
fn mero_wasm(app: &str, name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps")
        .join(app)
        .join("res")
        .join(format!("{name}.wasm"));
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}; build it with cargo mero build --manifest-path apps/{app}/Cargo.toml",
            path.display()
        )
    })
}

/// Deploy `wasm` as a new app owned by `cl`; answers (app id, context id).
async fn deploy(cl: &Client, name: &str, wasm: Vec<u8>, options: Value) -> (String, String) {
    let bundle = cl.upload_bundle(wasm).await.unwrap();
    let mut body = json!({ "name": name, "bundle": bundle });
    if let (Some(o), Some(b)) = (options.as_object(), body.as_object_mut()) {
        b.extend(o.clone());
    }
    let created = cl.post("apps", body).await.unwrap();
    (
        created["appId"].as_str().unwrap().to_owned(),
        created["contextId"].as_str().unwrap().to_owned(),
    )
}

/// Poll a strong view until `done` holds (follow-ups run after their entry).
async fn eventually(
    cl: &Client,
    app: &str,
    method: &str,
    args: Value,
    done: impl Fn(&Value) -> bool,
) -> Value {
    let mut last = Value::Null;
    for _ in 0..100 {
        if let Ok(v) = cl
            .view(app, method, args.clone(), Consistency::Strong)
            .await
        {
            if done(&v.result) {
                return v.result;
            }
            last = v.result;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{method} never got there; last answer {last}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn blobs_are_the_apps_own_and_reach_every_node() {
    let c = Cluster::start(3).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let (files, _) = deploy(&alice, "files", mero_wasm("blobs", "blobs"), json!({})).await;
    let (other, _) = deploy(&alice, "other", mero_wasm("blobs", "blobs"), json!({})).await;

    let pdf = b"%PDF-1.7 an agreement".repeat(1000);
    let up = alice.upload_blob(&files, pdf.clone()).await.unwrap();
    let blob_id = up["blobId"].as_str().unwrap().to_owned();
    assert_eq!(up["size"], json!(pdf.len()));

    // The app records it and announces it (a no-op here, but it must not fail).
    let file = alice
        .call(
            &files,
            "upload_file",
            json!({ "name": "a.pdf", "blob_id": blob_id, "size": pdf.len(), "mime_type": "application/pdf" }),
            None,
        )
        .await
        .unwrap();
    let file_id = file.result.as_str().unwrap().to_owned();
    let record = strong_view(&alice, &files, "get_file", json!({ "file_id": file_id })).await;
    assert_eq!(record["blob_id"], json!(blob_id));

    // Any node serves it, by either id form.
    for i in 0..3 {
        let cl = c.client(i);
        cl.use_session(alice.session().await.unwrap()).await;
        assert_eq!(cl.download_blob(&files, &blob_id).await.unwrap(), pdf);
    }
    let hex_id = up["id"].as_str().unwrap();
    assert_eq!(alice.download_blob(&files, hex_id).await.unwrap(), pdf);

    // Another app does not hold it, whatever it knows of the id.
    let err = alice.download_blob(&other, &blob_id).await.unwrap_err();
    assert_eq!(err.code(), Some("NOT_FOUND"));
    let err = alice
        .call(
            &other,
            "upload_file",
            json!({ "name": "a.pdf", "blob_id": blob_id, "size": 1, "mime_type": "x" }),
            None,
        )
        .await;
    // The blobs app only logs a failed announce; the record lands, the bytes don't.
    assert!(err.is_ok());
    assert!(alice.download_blob(&other, &blob_id).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn event_handlers_run_once_after_the_entry_that_emitted_them() {
    let c = Cluster::start(3).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let (kv, _) = deploy(
        &alice,
        "kv",
        mero_wasm("kv-store-with-handlers", "kv_store_with_handlers"),
        json!({}),
    )
    .await;
    for (k, v) in [("a", "1"), ("a", "2"), ("b", "3")] {
        let _ = alice
            .call(&kv, "set", json!({ "key": k, "value": v }), None)
            .await
            .unwrap();
    }
    let _ = eventually(&alice, &kv, "get_handler_execution_count", json!({}), |v| {
        v == &json!(3)
    })
    .await;
    // Once each, not once per replica.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        strong_view(&alice, &kv, "get_handler_execution_count", json!({})).await,
        json!(3)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn xcalls_reach_only_declared_entry_points_in_the_same_organization() {
    let c = Cluster::start(3).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let xcall = || mero_wasm("xcall-example", "xcall_example");
    let (ping, _) = deploy(&alice, "ping", xcall(), json!({})).await;
    let (pong, pong_ctx) = deploy(&alice, "pong", xcall(), json!({})).await;
    let (stranger, stranger_ctx) = deploy(&bob, "stranger", xcall(), json!({})).await;

    let _ = alice
        .call(&ping, "ping", json!({ "target_context": pong_ctx }), None)
        .await
        .unwrap();
    let _ = eventually(&alice, &pong, "get_counter", json!({}), |v| v == &json!(1)).await;

    // `secret` is not an #[app::xcall] entry point.
    let _ = alice
        .call(
            &ping,
            "ping_secret",
            json!({ "target_context": pong_ctx }),
            None,
        )
        .await
        .unwrap();
    // Bob's app is another owner's, outside any shared organization.
    let _ = alice
        .call(
            &ping,
            "ping",
            json!({ "target_context": stranger_ctx }),
            None,
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        strong_view(&alice, &pong, "get_secret_counter", json!({})).await,
        json!(0)
    );
    assert_eq!(
        strong_view(&bob, &stranger, "get_counter", json!({})).await,
        json!(0)
    );
    assert_eq!(
        strong_view(&alice, &pong, "get_counter", json!({})).await,
        json!(1)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn tee_handlers_and_timers_run_as_the_apps_tee() {
    let c = Cluster::start(3).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let (dice, _) = deploy(&alice, "dice", mero_wasm("tee-dice", "tee_dice"), json!({})).await;
    let _ = alice
        .call(&dice, "roll", json!({ "roll_id": "r1", "sides": 6 }), None)
        .await
        .unwrap();
    let face = eventually(&alice, &dice, "get_roll", json!({ "roll_id": "r1" }), |v| {
        !v.is_null()
    })
    .await;
    let face = face.as_u64().unwrap();
    assert!((1..=6).contains(&face), "{face}");
    // A member cannot resolve a roll itself.
    let err = alice
        .call(
            &dice,
            "resolve_roll",
            json!({ "roll_id": "r2", "sides": 6 }),
            None,
        )
        .await;
    assert!(err.is_err(), "{err:?}");

    // `reshuffle` is `#[app::tee(every = "1m")]`: the current tick fires at once.
    let (cards, _) = deploy(
        &alice,
        "cards",
        mero_wasm("tee-cards", "tee_cards"),
        json!({}),
    )
    .await;
    let _ = eventually(&alice, &cards, "cards_left", json!({}), |v| !v.is_null()).await;

    // A card the TEE deals is sealed to its player: Alice reads hers, nobody
    // else's run can open it.
    let _ = alice.call(&cards, "sit", json!({}), None).await.unwrap();
    let _ = alice.call(&cards, "draw", json!({}), None).await.unwrap();
    let hand = eventually(&alice, &cards, "my_hand", json!({}), |v| {
        v.as_array().is_some_and(|h| h.len() == 1)
    })
    .await;
    let card = hand[0].as_u64().unwrap();
    assert!(card < 52, "{card}");
    let left = strong_view(&alice, &cards, "cards_left", json!({})).await;
    assert_eq!(left, json!(51));
    let _ = alice
        .post(
            &format!("apps/{cards}/access"),
            json!({ "access": "public" }),
        )
        .await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    if let Ok(v) = bob
        .view(&cards, "my_hand", json!({}), Consistency::Strong)
        .await
    {
        assert_eq!(v.result, json!([]));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn views_search_the_apps_own_index_as_it_changes() {
    let c = Cluster::start(3).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let (chat, _) = deploy(
        &alice,
        "chat",
        mero_wasm("search-chat", "search_chat"),
        json!({}),
    )
    .await;
    for (id, text) in [
        ("m1", "the quarterly report is late"),
        ("m2", "lunch at noon"),
    ] {
        let _ = alice
            .call(
                &chat,
                "post",
                json!({ "id": id, "sender": "alice", "text": text, "ts": 1 }),
                None,
            )
            .await
            .unwrap();
    }
    let hits = |v: &Value| v["hits"].as_array().map_or(0, Vec::len);
    let page = strong_view(&alice, &chat, "search", json!({ "query": "report" })).await;
    assert_eq!(hits(&page), 1, "{page}");
    // An edit is searched as it is now, on any node.
    let _ = alice
        .call(
            &chat,
            "edit",
            json!({ "id": "m2", "text": "the report again" }),
            None,
        )
        .await
        .unwrap();
    for i in 0..3 {
        let cl = c.client(i);
        cl.use_session(alice.session().await.unwrap()).await;
        let page = strong_view(&cl, &chat, "search", json!({ "query": "report" })).await;
        assert_eq!(hits(&page), 2, "node {i}: {page}");
    }
    let page = strong_view(&alice, &chat, "search", json!({ "query": "lunch" })).await;
    assert_eq!(hits(&page), 0, "{page}");
    // A write may not search (v1 rule: search is for views).
    assert!(alice
        .call(
            &chat,
            "search_in_a_write",
            json!({ "query": "report" }),
            None
        )
        .await
        .is_err());
}
