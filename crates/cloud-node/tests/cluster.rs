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
                registry: None,
                allow_unsigned_bundles: true,
                outbound: None,
                upgrade_policy: calimero_cloud_node::system::UpgradePolicy::Owner,
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
    // The app's rows are in it, opened with the storage key.
    assert!(data
        .iter()
        .any(|(_k, v)| String::from_utf8_lossy(v).contains("plaintext-value")));
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

    // Team members write their team's note; alice (owner, who left the team
    // she created and led) cannot.
    let alice_id = alice.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ok = alice
        .delete(&format!("orgs/{org}/teams/{team}/members/{alice_id}"))
        .await
        .unwrap();
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
    // A declared view called as a call runs as a view, so it may search.
    let called = alice
        .call(&chat, "search", json!({ "query": "report" }), None)
        .await
        .unwrap();
    assert_eq!(hits(&called.result), 2, "{}", called.result);
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

async fn set(cl: &Client, app: &str, v: &str) -> Result<(), Box<calimero_cloud_client::Error>> {
    cl.call(app, "set", json!({ "key": "k", "value": v }), None)
        .await
        .map(drop)
        .map_err(Box::new)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn teams_work_like_v1_subgroups() {
    let c = Cluster::start(3).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let s = |v: Value, k: &str| v[k].as_str().unwrap().to_owned();
    let org = s(
        alice.post("orgs", json!({ "name": "Docs" })).await.unwrap(),
        "orgId",
    );
    let token = s(
        alice
            .post(&format!("orgs/{org}/invites"), json!({ "role": "member" }))
            .await
            .unwrap(),
        "token",
    );
    let _r = bob
        .post(&format!("invites/{token}/redeem"), json!({}))
        .await
        .unwrap();
    let bob_id = s(bob.me().await.unwrap(), "accountId");
    let team = |name: &str, join: &str, parent: Option<&str>| {
        let body = json!({ "name": name, "join": join, "parent": parent });
        let (alice, org) = (&alice, &org);
        async move {
            s(
                alice
                    .post(&format!("orgs/{org}/teams"), body)
                    .await
                    .unwrap(),
                "teamId",
            )
        }
    };
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app_in = |team: String| {
        let (alice, org, bundle) = (&alice, &org, &bundle);
        async move {
            alice
                .create_app(
                    "folder",
                    bundle,
                    json!({ "org": org, "access": { "kind": "team", "team": team } }),
                )
                .await
                .unwrap()
        }
    };

    // An open folder: Bob reaches it through the organization and writes.
    let open = team("Open", "everyone", None).await;
    let open_app = app_in(open.clone()).await;
    set(&bob, &open_app, "bob").await.unwrap();

    // Removed from an open folder, Bob does not come back by inheritance;
    // added again, he is back.
    let _ok = alice
        .delete(&format!("orgs/{org}/teams/{open}/members/{bob_id}"))
        .await
        .unwrap();
    let err = set(&bob, &open_app, "x").await.unwrap_err();
    assert_eq!(err.code(), Some("NOT_FOUND"), "{err}");
    let _ok = alice
        .post(
            &format!("orgs/{org}/teams/{open}/members"),
            json!({ "account": bob_id }),
        )
        .await
        .unwrap();
    set(&bob, &open_app, "back").await.unwrap();

    // A restricted folder is a wall, invisible to Bob until he is added.
    let secret = team("Secret", "invite", None).await;
    let secret_app = app_in(secret.clone()).await;
    let err = set(&bob, &secret_app, "x").await.unwrap_err();
    assert_eq!(err.code(), Some("NOT_FOUND"), "{err}");
    assert!(bob
        .get(&format!("orgs/{org}/teams/{secret}"))
        .await
        .is_err());
    let listed = bob.get(&format!("orgs/{org}")).await.unwrap();
    assert!(listed["teams"].get(&secret).is_none(), "{listed}");
    assert!(listed["teams"].get(&open).is_some(), "{listed}");
    let _ok = alice
        .post(
            &format!("orgs/{org}/teams/{secret}/members"),
            json!({ "account": bob_id, "role": "member" }),
        )
        .await
        .unwrap();
    set(&bob, &secret_app, "in").await.unwrap();

    // Nested open teams inherit through their parent; a direct read-only row
    // refuses writes there and nowhere else.
    let child = team("Child", "everyone", Some(&secret)).await;
    let child_app = app_in(child.clone()).await;
    set(&bob, &child_app, "inherited").await.unwrap();
    let _ok = alice
        .post(
            &format!("orgs/{org}/teams/{child}/members"),
            json!({ "account": bob_id, "role": "readOnly" }),
        )
        .await
        .unwrap();
    let err = set(&bob, &child_app, "ro").await.unwrap_err();
    assert_eq!(err.code(), Some("FORBIDDEN"), "{err}");
    // Leaving does not shed the role: he does not come back by inheritance.
    let _ok = bob
        .delete(&format!("orgs/{org}/teams/{child}/members/{bob_id}"))
        .await
        .unwrap();
    assert!(bob
        .view(
            &child_app,
            "get",
            json!({ "key": "k" }),
            Consistency::Strong
        )
        .await
        .is_err());
    let _ok = alice
        .post(
            &format!("orgs/{org}/teams/{child}/members"),
            json!({ "account": bob_id, "role": "readOnly" }),
        )
        .await
        .unwrap();
    // A call that writes nothing still runs for him.
    let read = bob.call(&child_app, "len", json!({}), None).await.unwrap();
    assert_eq!(read.result, json!(1));
    set(&bob, &secret_app, "still writes the parent")
        .await
        .unwrap();
    let v = bob
        .view(
            &child_app,
            "get",
            json!({ "key": "k" }),
            Consistency::Strong,
        )
        .await;
    assert_eq!(v.unwrap().result, json!("inherited"));

    // Metadata and visibility belong to the team; moving it under a team Bob
    // is not in hides it from him.
    let _ok = alice
        .post(
            &format!("orgs/{org}/teams/{child}"),
            json!({ "metadata": { "name": "Child", "color": "#f00" } }),
        )
        .await
        .unwrap();
    let seen = bob.get(&format!("orgs/{org}/teams/{child}")).await.unwrap();
    assert_eq!(seen["metadata"]["color"], "#f00");
    assert_eq!(seen["myRole"], "readOnly");
    let board = team("Board", "invite", None).await;
    let _ok = alice
        .post(
            &format!("orgs/{org}/teams/{child}"),
            json!({ "parent": board, "join": "invite" }),
        )
        .await
        .unwrap();
    // Still a direct member of Child, Bob sees it but not Board above it.
    assert!(bob.get(&format!("orgs/{org}/teams/{child}")).await.is_ok());
    assert!(bob.get(&format!("orgs/{org}/teams/{board}")).await.is_err());
    let err = alice
        .post(
            &format!("orgs/{org}/teams/{board}"),
            json!({ "parent": child }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("REJECTED"), "{err}");

    // A member creates top-level teams only with the capability.
    let err = bob
        .post(
            &format!("orgs/{org}/teams"),
            json!({ "name": "Mine", "join": "everyone" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("REJECTED"), "{err}");
    let _ok = alice
        .post(
            &format!("orgs/{org}/capabilities"),
            json!({ "capabilities": 37 }),
        )
        .await
        .unwrap();
    let mine = s(
        bob.post(
            &format!("orgs/{org}/teams"),
            json!({ "name": "Mine", "join": "everyone" }),
        )
        .await
        .unwrap(),
        "teamId",
    );
    assert_eq!(
        bob.get(&format!("orgs/{org}/teams/{mine}")).await.unwrap()["myRole"],
        "lead"
    );

    // Deleting a team deletes what is under it, apps included.
    let gone = alice
        .delete(&format!("orgs/{org}/teams/{board}"))
        .await
        .unwrap();
    assert_eq!(gone["teams"].as_array().unwrap().len(), 2, "{gone}");
    assert_eq!(gone["apps"], json!([child_app]));
    assert!(alice.get(&format!("apps/{child_app}")).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_passkey_is_another_device_of_the_same_account() {
    let c = Cluster::start(3).await;
    let (laptop, _pk) = signed_up(&c, 0, "alice").await;
    let mut phone_key = SoftAuthenticator::new("localhost", ORIGIN);
    let phone_cred = laptop.add_passkey(&mut phone_key).await.unwrap();
    let phone = c.client(1);
    let _s = phone.sign_in(&mut phone_key).await.unwrap();
    let (a, b) = (laptop.me().await.unwrap(), phone.me().await.unwrap());
    assert_eq!(a["accountId"], b["accountId"]);
    assert_eq!(b["passkeyId"], json!(phone_cred));
    assert_ne!(a["passkeyId"], b["passkeyId"]);
    assert_eq!(b["passkeys"].as_array().unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn upgrades_plan_and_run_the_apps_migration_and_its_check() {
    let c = Cluster::start(3).await;
    let (alice, mut alice_key) = signed_up(&c, 0, "alice").await;
    // An upgrade is approved with the owner's passkey; a refused one is a 422
    // whose details carry the run's status.
    async fn upgrade(
        alice: &Client,
        key: &mut SoftAuthenticator,
        app: String,
        bundle: String,
    ) -> Value {
        match alice
            .upgrade_app(&app, json!({ "bundle": bundle }), key)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                assert_eq!(e.code(), Some("UPGRADE_FAILED"), "{e}");
                e.details().cloned().unwrap()
            }
        }
    }

    // v1 -> v2 adds a field: the node finds the migration in v2's ABI, runs
    // it, and v1's data reads under v2 beside the new field's default.
    let v1 = alice
        .upload_bundle(mero_wasm(
            "migrations/migration-suite-v1",
            "migration_suite_v1",
        ))
        .await
        .unwrap();
    let app = alice.create_app("suite", &v1, json!({})).await.unwrap();
    let _ok = alice
        .call(
            &app,
            "set_item",
            json!({ "key": "a", "value": "kept" }),
            None,
        )
        .await
        .unwrap();
    let v2 = alice
        .upload_bundle(mero_wasm(
            "migrations/migration-suite-v2-add-field",
            "migration_suite_v2_add_field",
        ))
        .await
        .unwrap();
    let up = upgrade(&alice, &mut alice_key, app.clone(), v2.clone()).await;
    assert_eq!(up["status"], "ok", "{up}");
    assert_eq!(
        strong_view(&alice, &app, "get_item", json!({ "key": "a" })).await,
        json!("kept")
    );
    assert_eq!(
        strong_view(&alice, &app, "get_notes", json!({})).await,
        json!("added in v2")
    );
    // Down again is refused, and the app stays on v2.
    let down = upgrade(&alice, &mut alice_key, app.clone(), v1).await;
    assert_eq!(down["status"], "appError", "{down}");
    assert_eq!(
        strong_view(&alice, &app, "get_notes", json!({})).await,
        json!("added in v2")
    );

    // A migration its own check refuses leaves the app on v1, data intact.
    let f1 = alice
        .upload_bundle(mero_wasm(
            "migrations/scenario-migration-check-fail-v1",
            "scenario_migration_check_fail_v1",
        ))
        .await
        .unwrap();
    let app = alice.create_app("checked", &f1, json!({})).await.unwrap();
    for k in ["x", "y"] {
        let _ok = alice
            .call(&app, "set_item", json!({ "key": k, "value": k }), None)
            .await
            .unwrap();
    }
    let f2 = alice
        .upload_bundle(mero_wasm(
            "migrations/scenario-migration-check-fail-v2",
            "scenario_migration_check_fail_v2",
        ))
        .await
        .unwrap();
    let refused = upgrade(&alice, &mut alice_key, app.clone(), f2.clone()).await;
    assert_eq!(refused["status"], "appError", "{refused}");
    assert_eq!(
        strong_view(&alice, &app, "item_count", json!({})).await,
        json!(2)
    );
    // The directory still names the bundle the app runs.
    assert_eq!(
        alice.get(&format!("apps/{app}")).await.unwrap()["bundle"],
        json!(f1)
    );
}

// ---------------------------------------------------------------- packages

/// A signed single-service `.mpk`, the shape `cargo mero bundle` produces.
fn mpk(package: &str, version: &str, wasm: &[u8], key: &ed25519_dalek::SigningKey) -> Vec<u8> {
    mpk_declaring(package, version, wasm, key, &[])
}

/// [`mpk`] whose manifest declares `outbound` URL prefixes.
fn mpk_declaring(
    package: &str,
    version: &str,
    wasm: &[u8],
    key: &ed25519_dalek::SigningKey,
    outbound: &[&str],
) -> Vec<u8> {
    use sha2::Digest;
    let manifest = calimero_bundle::BundleManifest {
        version: "1.0".to_owned(),
        package: package.to_owned(),
        app_version: version.to_owned(),
        signer_id: Some(calimero_bundle::derive_signer_id_did_key(
            key.verifying_key().as_bytes(),
        )),
        min_runtime_version: "0.1.0".to_owned(),
        metadata: None,
        handlers: None,
        build_info: None,
        interfaces: None,
        wasm: Some(calimero_bundle::BundleArtifact {
            path: "app.wasm".to_owned(),
            hash: hex::encode(sha2::Sha256::digest(wasm)),
            size: wasm.len() as u64,
        }),
        abi: None,
        links: None,
        frontend: None,
        frontend_aliases: Vec::new(),
        outbound: outbound.iter().map(|u| (*u).to_owned()).collect(),
        services: None,
        signature: None,
    };
    let mut manifest = serde_json::to_value(&manifest).unwrap();
    calimero_bundle::sign_manifest_json(&mut manifest, key).unwrap();
    pack(&[
        ("manifest.json", serde_json::to_vec(&manifest).unwrap()),
        ("app.wasm", wasm.to_vec()),
    ])
}

fn pack(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::default(),
    ));
    for (name, bytes) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_path(name).unwrap();
        header.set_size(bytes.len() as u64);
        header.set_cksum();
        tar.append(&header, bytes.as_slice()).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

/// `wasm` with a custom section named `tag`: another module that runs the same.
fn variant(wasm: &[u8], tag: &str) -> Vec<u8> {
    let mut out = wasm.to_vec();
    let body_len = 1 + tag.len();
    out.push(0);
    out.push(u8::try_from(body_len).unwrap());
    out.push(u8::try_from(tag.len()).unwrap());
    out.extend_from_slice(tag.as_bytes());
    out
}

/// A registry serving `{base}/artifacts/{package}/{version}/{package}-{version}.mpk`,
/// counting what it is asked for.
#[derive(Clone, Default)]
struct FakeRegistry {
    files: std::sync::Arc<std::sync::Mutex<BTreeMap<String, Vec<u8>>>>,
    hits: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl FakeRegistry {
    fn publish(&self, package: &str, version: &str, mpk: Vec<u8>) {
        let _prev = self
            .files
            .lock()
            .unwrap()
            .insert(format!("{package}@{version}"), mpk);
    }

    fn hits(&self, release: &str) -> usize {
        self.hits
            .lock()
            .unwrap()
            .iter()
            .filter(|h| *h == release)
            .count()
    }

    async fn serve(&self) -> String {
        use axum::extract::Path;
        use axum::http::StatusCode;
        use axum::response::IntoResponse;
        let reg = self.clone();
        let app = axum::Router::new().route(
            "/artifacts/{package}/{version}/{file}",
            axum::routing::get(
                move |Path((package, version, _file)): Path<(String, String, String)>| {
                    let reg = reg.clone();
                    async move {
                        let key = format!("{package}@{version}");
                        reg.hits.lock().unwrap().push(key.clone());
                        match reg.files.lock().unwrap().get(&key) {
                            Some(b) => (StatusCode::OK, b.clone()).into_response(),
                            None => StatusCode::NOT_FOUND.into_response(),
                        }
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _serving = tokio::spawn(async move { axum::serve(listener, app).await });
        format!("http://{addr}/")
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn packages_install_from_the_registry_and_every_node_verifies_its_own_copy() {
    let registry = FakeRegistry::default();
    let url = registry.serve().await;
    let c = Cluster::start_with(3, |cfg| {
        cfg.registry = Some(url.clone());
        cfg.allow_unsigned_bundles = false;
    })
    .await;
    let (alice, mut alice_key) = signed_up(&c, 0, "alice").await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let kv = kv_wasm();
    registry.publish(
        "com.test.kv",
        "1.0.0",
        mpk("com.test.kv", "1.0.0", &kv, &publisher),
    );

    // Production nodes take no raw WASM: it carries no publisher signature.
    let e = alice.upload_bundle(kv.clone()).await.unwrap_err();
    assert!(e.to_string().starts_with("403"), "{e}");

    // Install by name; the node that installs it verifies it first.
    let installed = alice.install_package("com.test.kv", "1.0.0").await.unwrap();
    assert_eq!(installed["version"], json!("1.0.0"));
    assert_eq!(
        installed["signerId"],
        json!(calimero_bundle::derive_signer_id_did_key(
            publisher.verifying_key().as_bytes()
        ))
    );
    let created = alice
        .post(
            "apps",
            json!({ "name": "kv", "package": "com.test.kv", "version": "1.0.0" }),
        )
        .await
        .unwrap();
    let app = created["appId"].as_str().unwrap().to_owned();
    let _ = alice
        .call(&app, "set", json!({ "key": "k", "value": "v1" }), None)
        .await
        .unwrap();

    // Each node runs the app from its own copy, fetched from the registry
    // (never pushed by a peer) and verified again: one fetch per node.
    let session = alice.session().await.unwrap();
    for i in 0..3 {
        let cl = c.client(i);
        cl.use_session(session.clone()).await;
        let v = cl
            .view(
                &app,
                "get",
                json!({ "key": "k" }),
                Consistency::Stale { min_index: 0 },
            )
            .await
            .unwrap();
        assert_eq!(v.result, json!("v1"), "node {i}");
    }
    assert_eq!(registry.hits("com.test.kv@1.0.0"), 3);

    // The package is pinned to its publisher: another key cannot release it.
    let intruder = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
    registry.publish(
        "com.test.kv",
        "1.1.0",
        mpk("com.test.kv", "1.1.0", &kv, &intruder),
    );
    let e = alice
        .install_package("com.test.kv", "1.1.0")
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("403"), "{e}");

    // Bytes that do not match the signed manifest are refused.
    let mut tampered = mpk("com.test.kv", "1.2.0", &kv, &publisher);
    let manifest_and_bad_wasm = {
        let good = mpk("com.test.kv", "1.2.0", &kv, &publisher);
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(&good[..]));
        let mut entries = Vec::new();
        for e in archive.entries().unwrap() {
            let mut e = e.unwrap();
            let name = e.path().unwrap().to_string_lossy().into_owned();
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut e, &mut bytes).unwrap();
            if name == "app.wasm" {
                bytes = variant(&bytes, "evil");
            }
            entries.push((name, bytes));
        }
        entries
    };
    tampered.clear();
    tampered.extend(pack(
        &manifest_and_bad_wasm
            .iter()
            .map(|(n, b)| (n.as_str(), b.clone()))
            .collect::<Vec<_>>(),
    ));
    registry.publish("com.test.kv", "1.2.0", tampered);
    let e = alice
        .install_package("com.test.kv", "1.2.0")
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("BAD_PACKAGE"), "{e}");

    // Upgrade by version, within the package.
    let v101 = variant(&kv, "v101");
    registry.publish(
        "com.test.kv",
        "1.0.1",
        mpk("com.test.kv", "1.0.1", &v101, &publisher),
    );
    let up = alice
        .upgrade_app(&app, json!({ "version": "1.0.1" }), &mut alice_key)
        .await
        .unwrap();
    assert_eq!(up["status"], json!("ok"), "{up}");
    let listed = alice.get("apps?package=com.test.kv").await.unwrap();
    assert_eq!(listed["apps"][0]["version"], json!("1.0.1"), "{listed}");
    let v = alice
        .view(&app, "get", json!({ "key": "k" }), Consistency::Strong)
        .await
        .unwrap();
    assert_eq!(v.result, json!("v1"), "state survives the upgrade");

    // A release some node fetches later must be the one the fleet installed:
    // a registry that swaps the bytes is refused, never run.
    let v2 = variant(&kv, "two");
    registry.publish(
        "com.test.kv",
        "2.0.0",
        mpk("com.test.kv", "2.0.0", &v2, &publisher),
    );
    let _ = alice.install_package("com.test.kv", "2.0.0").await.unwrap();
    let created = alice
        .post(
            "apps",
            json!({ "name": "kv2", "package": "com.test.kv", "version": "2.0.0" }),
        )
        .await
        .unwrap();
    let app2 = created["appId"].as_str().unwrap().to_owned();
    let swapped = variant(&kv, "swap");
    registry.publish(
        "com.test.kv",
        "2.0.0",
        mpk("com.test.kv", "2.0.0", &swapped, &publisher),
    );
    let mut refused = 0;
    for i in 0..3 {
        let cl = c.client(i);
        cl.use_session(session.clone()).await;
        match cl
            .view(
                &app2,
                "get",
                json!({ "key": "k" }),
                Consistency::Stale { min_index: 0 },
            )
            .await
        {
            Ok(_) => {}
            Err(e) => {
                assert!(e.to_string().contains("differs"), "node {i}: {e}");
                refused += 1;
            }
        }
    }
    assert!(
        refused >= 1,
        "a node without a copy must refuse the swapped release"
    );

    // Uninstall: refused while an app runs the release, done once none does.
    let e = alice
        .uninstall_package("com.test.kv", Some("1.0.1"))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("still runs"), "{e}");
    let _ = alice
        .uninstall_package("com.test.kv", Some("1.0.0"))
        .await
        .unwrap();
    let _ = alice.delete_app(&app).await.unwrap();
    let _ = alice.delete_app(&app2).await.unwrap();
    let e = alice
        .call(&app, "set", json!({ "key": "k", "value": "x" }), None)
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("404"), "{e}");
    let _ = alice.uninstall_package("com.test.kv", None).await.unwrap();
    assert_eq!(alice.packages().await.unwrap(), json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_uploaded_package_is_verified_and_reaches_the_other_nodes() {
    let c = Cluster::start_with(3, |cfg| cfg.allow_unsigned_bundles = false).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let kv = kv_wasm();

    // An unsigned archive is refused.
    let unsigned = pack(&[("app.wasm", kv.clone())]);
    let e = alice.upload_package(unsigned).await.unwrap_err();
    assert_eq!(e.code(), Some("BAD_PACKAGE"), "{e}");

    let installed = alice
        .upload_package(mpk("com.test.up", "0.1.0", &kv, &publisher))
        .await
        .unwrap();
    assert_eq!(installed["package"], json!("com.test.up"));
    let created = alice
        .post("apps", json!({ "name": "up", "package": "com.test.up" }))
        .await
        .unwrap();
    let app = created["appId"].as_str().unwrap().to_owned();
    let _ = alice
        .call(&app, "set", json!({ "key": "a", "value": "b" }), None)
        .await
        .unwrap();
    let session = alice.session().await.unwrap();
    for i in 0..3 {
        let cl = c.client(i);
        cl.use_session(session.clone()).await;
        let v = cl
            .view(
                &app,
                "get",
                json!({ "key": "a" }),
                Consistency::Stale { min_index: 0 },
            )
            .await
            .unwrap();
        assert_eq!(v.result, json!("b"), "node {i}");
    }
    let p = alice.package("com.test.up").await.unwrap();
    assert_eq!(p["latest"], json!("0.1.0"));
    assert_eq!(p["versions"][0]["source"], json!("upload"));
}

/// Xil's review, issues 5, 6 and 7, each reproduced as the review ran it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn sessions_confirmations_and_key_scopes_hold() {
    let c = Cluster::start(3).await;
    let (laptop, _pk) = signed_up(&c, 0, "alice").await;

    // 5. Removing a passkey ends the sessions opened with it.
    let mut phone_key = SoftAuthenticator::new("localhost", ORIGIN);
    let phone_cred = laptop.add_passkey(&mut phone_key).await.unwrap();
    let phone = c.client(1);
    let _s = phone.sign_in(&mut phone_key).await.unwrap();
    assert!(phone.me().await.is_ok());
    let _ok = laptop
        .delete(&format!("auth/passkeys/{phone_cred}"))
        .await
        .unwrap();
    let e = phone.me().await.unwrap_err();
    assert!(e.to_string().starts_with("401"), "{e}");
    let e = phone.refresh().await.unwrap_err();
    assert!(e.to_string().starts_with("401"), "{e}");
    assert!(
        laptop.me().await.is_ok(),
        "the other device stays signed in"
    );

    // 5. A refresh token used twice is a stolen copy: the session ends.
    let before = laptop.session().await.unwrap();
    let _after = laptop.refresh().await.unwrap();
    let thief = c.client(2);
    thief.use_session(before).await;
    let e = thief.refresh().await.unwrap_err();
    assert!(e.to_string().contains("already used"), "{e}");
    let e = laptop.me().await.unwrap_err();
    assert!(e.to_string().starts_with("401"), "{e}");

    // 6. A confirm method does not answer through /views.
    let (bob, _pk) = signed_up(&c, 0, "bob").await;
    let (app, _ctx) = deploy(&bob, "kv", kv_wasm(), json!({ "confirmMethods": ["get"] })).await;
    let e = bob
        .view(&app, "get", json!({ "key": "k" }), Consistency::Strong)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");

    // 7. A read-only key installs nothing; an app-scoped key starts no app.
    let (_id, read_only) = bob
        .create_api_key(json!({ "readOnly": true }))
        .await
        .unwrap();
    let svc = c.client(0);
    svc.use_api_key(&read_only).await;
    let e = svc.upload_bundle(kv_wasm()).await.unwrap_err();
    assert!(e.to_string().starts_with("403"), "{e}");
    let (_id, scoped) = bob
        .create_api_key(json!({ "apps": [app], "methods": ["set"] }))
        .await
        .unwrap();
    let svc = c.client(0);
    svc.use_api_key(&scoped).await;
    let bundle = bob.upload_bundle(kv_wasm()).await.unwrap();
    let e = svc
        .post("apps", json!({ "name": "new", "bundle": bundle }))
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("403"), "{e}");
}

/// The owner, not the operator, decides which mero-tee releases hold an app:
/// a node on a release the owner has not approved is placed nothing, cannot be
/// moved onto, and is sent no entries; approving lets it in, withdrawing stops
/// it again. Upgrades need the owner's passkey over the exact upgrade.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn only_releases_the_owner_approved_hold_an_apps_data() {
    let c = Cluster::start_with(3, |cfg| {
        // The operator rolled node 3 onto a new release.
        cfg.tee_release = Some(if cfg.node_id == 3 { "2.0.0" } else { "1.0.0" }.into());
    })
    .await;
    let (alice, mut key) = signed_up(&c, 0, "alice").await;
    assert_eq!(
        alice.me().await.unwrap()["approvedReleases"],
        json!(["1.0.0"])
    );
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let created = alice
        .post("apps", json!({ "name": "kv", "bundle": bundle }))
        .await
        .unwrap();
    let app = created["appId"].as_str().unwrap().to_owned();
    let mut replicas: Vec<u64> = serde_json::from_value(created["replicas"].clone()).unwrap();
    replicas.sort_unstable();
    assert_eq!(
        replicas,
        vec![1, 2],
        "the unapproved node is placed nothing"
    );
    let _ = alice
        .call(&app, "set", json!({ "key": "k", "value": "v1" }), None)
        .await
        .unwrap();

    // The operator cannot move the app onto it.
    let ops = |i: usize| {
        let client = c.client(i);
        async move {
            client.use_api_key("ops-secret").await;
            client
        }
    };
    let e = ops(0)
        .await
        .post(
            &format!("ops/apps/{app}/replicas"),
            json!({ "replicas": [1, 2, 3] }),
        )
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("403"), "{e}");

    // An upgrade needs the owner's passkey over this upgrade.
    let e = alice
        .post(&format!("apps/{app}/upgrade"), json!({ "bundle": bundle }))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    let (bob, _bob_key) = signed_up(&c, 1, "bob").await;
    let e = bob
        .post(&format!("apps/{app}/upgrade"), json!({ "bundle": bundle }))
        .await
        .unwrap_err();
    assert!(
        e.to_string().starts_with("403") || e.to_string().starts_with("404"),
        "{e}"
    );

    // The owner approves 2.0.0: now the app may move there, and node 3 serves it.
    let _ok = alice
        .approve_release(None, "2.0.0", &mut key)
        .await
        .unwrap();
    let leader = c.app_leader(&app).unwrap();
    let _moved = ops(leader)
        .await
        .post(
            &format!("ops/apps/{app}/replicas"),
            json!({ "replicas": [1, 2, 3] }),
        )
        .await
        .unwrap();
    let session = alice.session().await.unwrap();
    let on3 = c.client(2);
    on3.use_session(session).await;
    let mut seen = None;
    for _ in 0..100 {
        if let Ok(v) = on3
            .view(
                &app,
                "get",
                json!({ "key": "k" }),
                Consistency::Stale { min_index: 0 },
            )
            .await
        {
            seen = Some(v.result);
            if seen == Some(json!("v1")) {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(seen, Some(json!("v1")), "node 3 caught up once approved");

    // Withdrawn again: node 3 is sent nothing new.
    let _ok = alice
        .post_confirmed(
            "auth/releases",
            json!({ "version": "2.0.0", "approved": false }),
            &mut key,
        )
        .await
        .unwrap();
    let _ = alice
        .call(&app, "set", json!({ "key": "k", "value": "v2" }), None)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let v = on3
        .view(
            &app,
            "get",
            json!({ "key": "k" }),
            Consistency::Stale { min_index: 0 },
        )
        .await
        .unwrap();
    assert_eq!(v.result, json!("v1"), "the new write never reached node 3");
}

/// A user can check that an answer came from a genuine TEE cluster: every
/// replica of the app attests its receipt key, and each answer carries a
/// receipt signed by one of those keys over that exact result.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn answers_carry_receipts_from_attested_replicas() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let (app, _ctx) = deploy(&alice, "kv", kv_wasm(), json!({})).await;
    let cluster = alice.verify_app_cluster(&app).await.unwrap();
    let keys = &cluster.keys;
    assert_eq!(keys.len(), 3, "every replica attests");

    let call = alice
        .call(&app, "set", json!({ "key": "k", "value": "v" }), None)
        .await
        .unwrap();
    let receipt = call.receipt.clone().unwrap();
    assert!(receipt.verify(&keys[&receipt.node], &call.result));

    let view = alice
        .view(&app, "get", json!({ "key": "k" }), Consistency::Strong)
        .await
        .unwrap();
    let receipt = view.receipt.clone().unwrap();
    assert_eq!(receipt.index, view.index);
    assert!(receipt.verify(&keys[&receipt.node], &view.result));
    assert!(!receipt.verify(&keys[&receipt.node], &json!("forged")));
}

// ---------------------------------------------------------------- outbound

async fn probe(cl: &Client, app: &str) -> Value {
    cl.call(
        app,
        "probe",
        json!({ "path": "check", "body": "a=1" }),
        None,
    )
    .await
    .unwrap()
    .result
}

/// v1's outbound gate, with the owner's approval in place of the namespace
/// admin's: a run reaches a declared prefix only once the app's owner
/// approved it for the publisher, never on the operator's say; a view and
/// another account's approval reach nothing; a replace revokes.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn outbound_requests_need_the_owners_approval() {
    const PREFIX: &str = "https://echo.outbound.test/v1/";
    const PACKAGE: &str = "com.calimero.outbound-probe";
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
    let signer = calimero_bundle::derive_signer_id_did_key(publisher.verifying_key().as_bytes());
    let settings = calimero_outbound::OutboundSettings {
        enabled: true,
        hosts: vec!["echo.outbound.test".to_owned()],
        testing: Some(calimero_outbound::OutboundTestSettings {
            echo: true,
            secrets: [(
                "ECHO_KEY".to_owned(),
                calimero_outbound::TestSecretSettings {
                    value: "s3cret-value".to_owned(),
                    packages: vec![PACKAGE.to_owned()],
                    signers: vec![signer.clone()],
                    hosts: vec!["echo.outbound.test".to_owned()],
                },
            )]
            .into(),
        }),
        ..calimero_outbound::OutboundSettings::default()
    };
    let service = calimero_outbound::OutboundService::from_settings(&settings).unwrap();
    let c = Cluster::start_with(3, |cfg| cfg.outbound.clone_from(&service)).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let (bob, mut bob_pk) = signed_up(&c, 1, "bob").await;
    let _ = alice
        .upload_package(mpk_declaring(
            PACKAGE,
            "0.1.0",
            &wasm("outbound_probe"),
            &publisher,
            &[PREFIX],
        ))
        .await
        .unwrap();
    let app = alice
        .post("apps", json!({ "name": "probe", "package": PACKAGE }))
        .await
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    // The operator allows the host and the manifest declares the prefix, but
    // the owner was never asked.
    let before = probe(&alice, &app).await;
    assert!(
        before.as_str().unwrap().contains("[NotApproved]"),
        "{before}"
    );

    // Bob approving for his own apps reaches nothing of Alice's.
    let _ = bob
        .approve_outbound(None, PACKAGE, None, false, &mut bob_pk)
        .await
        .unwrap();
    let still = probe(&alice, &app).await;
    assert!(still.as_str().unwrap().contains("[NotApproved]"), "{still}");

    // The owner approves what the release declares, with a passkey.
    let approved = alice
        .approve_outbound(None, PACKAGE, None, false, &mut alice_pk)
        .await
        .unwrap();
    assert_eq!(approved["signer"], json!(signer));
    assert_eq!(approved["approval"]["prefixes"], json!([PREFIX]));
    let sent = probe(&alice, &app).await;
    let sent = sent.as_str().unwrap();
    assert!(sent.starts_with("200 "), "{sent}");
    assert!(sent.contains("/v1/check"), "{sent}");
    // The secret went out, and its echo came back redacted.
    assert!(!sent.contains("s3cret-value"), "{sent}");
    assert!(sent.contains("[redacted]"), "{sent}");

    // A view gets no client, approval or not.
    let view = alice
        .view(&app, "probe_in_view", json!({}), Consistency::Strong)
        .await;
    assert!(
        !format!("{view:?}").contains("200 "),
        "a view called out: {view:?}"
    );

    // Replacing with nothing revokes: what was reviewed reads as not approved.
    let _ = alice
        .approve_outbound(None, PACKAGE, Some(&[]), true, &mut alice_pk)
        .await
        .unwrap();
    let revoked = probe(&alice, &app).await;
    assert!(
        revoked.as_str().unwrap().contains("[NotApproved]"),
        "{revoked}"
    );
}

// ---------------------------------------------------------------- governance

/// Who moves apps to new code is chosen when the cluster is deployed, and a
/// user proves which: every replica binds the cluster's governance into its
/// receipt-key quote. Owner-managed, the operator can neither upgrade an app
/// nor approve a release; operator-managed, it does both for every app, and
/// owners approve no releases (they still upgrade their own apps).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_deployment_decides_who_upgrades_and_users_can_prove_it() {
    use calimero_cloud_node::system::UpgradePolicy;
    for policy in [UpgradePolicy::Owner, UpgradePolicy::Operator] {
        let operator = policy == UpgradePolicy::Operator;
        let c = Cluster::start_with(3, |cfg| cfg.upgrade_policy = policy).await;
        let (alice, mut pk) = signed_up(&c, 0, "alice").await;
        let kv = kv_wasm();
        let (app, _ctx) = deploy(&alice, "kv", kv.clone(), json!({})).await;
        let next = alice.upload_bundle(variant(&kv, "next")).await.unwrap();

        let proven = alice.verify_app_cluster(&app).await.unwrap();
        assert_eq!(proven.keys.len(), 3, "{policy:?}");
        assert_eq!(proven.governance.upgrades, policy, "{policy:?}");
        // A replica claiming other rules than its quote binds is refused.
        let nonce = [5u8; 32];
        let docs = alice
            .get(&format!(
                "apps/{app}/attestation?nonce={}",
                hex::encode(nonce)
            ))
            .await
            .unwrap();
        let mut doc: calimero_cloud_client::attest::AttestationDoc =
            serde_json::from_value(docs["replicas"][0]["attestation"].clone()).unwrap();
        let policy_mock = Policy {
            allow_mock: true,
            ..Policy::default()
        };
        assert!(
            calimero_cloud_client::attest::verify_receipt_key(&doc, &nonce, &policy_mock)
                .await
                .is_ok()
        );
        doc.governance = Some(calimero_cloud_client::attest::Governance {
            upgrades: if operator {
                UpgradePolicy::Owner
            } else {
                UpgradePolicy::Operator
            },
        });
        assert!(
            calimero_cloud_client::attest::verify_receipt_key(&doc, &nonce, &policy_mock)
                .await
                .is_err(),
            "{policy:?}: a forged governance verified"
        );

        let ops = c.client(1);
        ops.use_api_key("ops-secret").await;
        let status = ops.get("ops/status").await.unwrap();
        assert_eq!(
            status["governance"]["upgrades"],
            json!(policy),
            "{policy:?}"
        );

        let upgraded = ops
            .post(
                &format!("ops/apps/{app}/upgrade"),
                json!({ "bundle": next }),
            )
            .await;
        assert_eq!(upgraded.is_ok(), operator, "{policy:?}: {upgraded:?}");
        if operator {
            let a = alice.get(&format!("apps/{app}")).await.unwrap();
            assert_eq!(a["bundle"], json!(next), "the directory records it");
            // The app still answers, on the new bundle.
            let _ = alice
                .call(&app, "set", json!({ "key": "k", "value": "v" }), None)
                .await
                .unwrap();
        }

        let ops_release = ops
            .post("ops/releases", json!({ "version": "9.0.0" }))
            .await;
        assert_eq!(ops_release.is_ok(), operator, "{policy:?}: {ops_release:?}");
        let owner_release = alice.approve_release(None, "9.0.0", &mut pk).await;
        assert_eq!(
            owner_release.is_ok(),
            !operator,
            "{policy:?}: {owner_release:?}"
        );
    }
}
