//! Access to an app is judged on the cluster's latest system state, never on
//! the record a lagging replica held when the request arrived.
//!
//! A follower applies a system entry once the leader's next append reaches
//! it. A request that reaches a follower before that holds the app's record
//! without a grant the leader already committed. `gate` waits for the fresh
//! state, so it must judge on the record that state holds, or the account just
//! granted gets `404 no such app` (the intermittent `release-v2-parity` CI
//! failure in `shared_storage.rs`).
//!
//! The lag is made deterministic: every node's peers reach it through a TCP
//! proxy the test can hold, so the leader's appends to the chosen follower wait
//! while the request is judged there. The follower's own requests (its read
//! index, from the leader) are not held, and clients reach it directly.
//!
//! Needs `kv_store.wasm`: `cargo build -p kv-store --target wasm32-unknown-unknown --profile app-release`,
//! or set `KV_STORE_WASM`.

use std::collections::BTreeMap;
use std::time::Duration;

use calimero_cloud_client::{Client, Consistency, Policy};
use calimero_cloud_node::{run, Running};
use calimero_identity::testing::SoftAuthenticator;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{free_port, kv_wasm, node_config, ORIGIN};

/// Forward `listen` to `upstream`; while `held` is true, bytes towards
/// `upstream` wait. Answers flow back freely.
async fn proxy(listen: TcpListener, upstream: String, held: watch::Receiver<bool>) {
    loop {
        let Ok((inbound, _)) = listen.accept().await else {
            return;
        };
        let upstream = upstream.clone();
        let mut held = held.clone();
        drop(tokio::spawn(async move {
            let Ok(outbound) = TcpStream::connect(&upstream).await else {
                return;
            };
            let (mut in_read, mut in_write) = inbound.into_split();
            let (mut out_read, mut out_write) = outbound.into_split();
            let towards = async move {
                let mut buf = vec![0_u8; 64 * 1024];
                loop {
                    let n = match in_read.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    if held.wait_for(|h| !*h).await.is_err() {
                        break;
                    }
                    if out_write.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            };
            let back = async move {
                let _ = tokio::io::copy(&mut out_read, &mut in_write).await;
            };
            let _ = tokio::join!(towards, back);
        }));
    }
}

struct ProxiedCluster {
    nodes: Vec<Running>,
    /// Each node's own address: what clients connect to.
    direct: Vec<String>,
    /// Holds peers' traffic towards each node.
    hold: Vec<watch::Sender<bool>>,
    _dirs: Vec<tempfile::TempDir>,
}

impl ProxiedCluster {
    async fn start() -> Self {
        let mut peers = BTreeMap::new();
        let mut direct = Vec::new();
        let mut hold = Vec::new();
        for id in 1..=3_u64 {
            let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let proxy_port = proxy_listener.local_addr().unwrap().port();
            let node_port = free_port();
            let (tx, rx) = watch::channel(false);
            drop(tokio::spawn(proxy(
                proxy_listener,
                format!("127.0.0.1:{node_port}"),
                rx,
            )));
            let _prev = peers.insert(id, format!("http://127.0.0.1:{proxy_port}/"));
            direct.push(format!("http://127.0.0.1:{node_port}/"));
            hold.push(tx);
        }
        let mut nodes = Vec::new();
        let mut dirs = Vec::new();
        for id in 1..=3_u64 {
            let dir = tempfile::tempdir().unwrap();
            let mut config = node_config(id, &peers, dir.path());
            // Peers name the proxy; the node listens on its own port.
            config.listen = url::Url::parse(&direct[id as usize - 1])
                .unwrap()
                .socket_addrs(|| None)
                .unwrap()[0];
            // A held follower must not start an election while it waits.
            config.raft.election_min_ms = 5_000;
            config.raft.election_max_ms = 6_000;
            nodes.push(run(config).await.unwrap());
            dirs.push(dir);
        }
        let c = Self {
            nodes,
            direct,
            hold,
            _dirs: dirs,
        };
        for _ in 0..200 {
            if c.nodes.iter().all(|r| {
                r.node
                    .sys(|s| s.nodes.len() == 3 && !s.session_keys.is_empty())
            }) {
                return c;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("cluster did not form");
    }

    fn client(&self, i: usize) -> Client {
        Client::connect(
            &self.direct[i],
            Policy {
                allow_mock: true,
                ..Policy::default()
            },
        )
        .unwrap()
    }

    fn system_leader(&self) -> usize {
        (0..3)
            .find(|i| self.nodes[*i].node.system.status().is_leader)
            .expect("a system leader")
    }
}

async fn signed_up(c: &ProxiedCluster, i: usize, name: &str) -> (Client, SoftAuthenticator) {
    let client = c.client(i);
    let mut pk = SoftAuthenticator::new("localhost", ORIGIN);
    let _s = client.sign_up(&mut pk, name).await.unwrap();
    (client, pk)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_grant_is_honoured_by_a_follower_that_has_not_applied_it_yet() {
    let c = ProxiedCluster::start().await;
    let leader = c.system_leader();
    let follower = (0..3).find(|i| *i != leader).unwrap();
    let (alice, _a) = signed_up(&c, leader, "alice").await;
    let (bob, _b) = signed_up(&c, follower, "bob").await;
    let bob_id = bob.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let created = alice
        .post("apps", json!({ "name": "kv", "bundle": bundle }))
        .await
        .unwrap();
    let app = created["appId"].as_str().unwrap().to_owned();
    // The follower holds the app's record before its appends are held.
    let follower_node = &c.nodes[follower].node;
    let _record = follower_node.app(&app).await.unwrap();

    // Hold the leader's appends to the follower, then grant Bob through the
    // leader: the other follower's ack commits it.
    c.hold[follower].send_replace(true);
    let _ok = alice
        .post(
            &format!("apps/{app}/access"),
            json!({ "access": { "kind": "accounts", "accounts": [bob_id] } }),
        )
        .await
        .unwrap();
    assert!(
        follower_node.sys(|s| s
            .apps
            .get(&app)
            .is_some_and(|a| !s.may_reach(a, Some(&bob_id)))),
        "the follower must still hold the record without the grant"
    );

    // Bob calls through the follower. The node reads the stale record at once,
    // then waits for the fresh state; the appends are let through meanwhile.
    let call = tokio::spawn({
        let app = app.clone();
        async move {
            bob.view(&app, "get", json!({ "key": "k" }), Consistency::Strong)
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    c.hold[follower].send_replace(false);
    let read = call.await.unwrap();
    assert!(
        read.is_ok(),
        "a grant committed before the request must be honoured: {:?}",
        read.err()
    );
}

/// The other direction: a revocation committed through the leader holds on a
/// follower that has not applied it yet.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_revocation_holds_on_a_follower_that_has_not_applied_it_yet() {
    let c = ProxiedCluster::start().await;
    let leader = c.system_leader();
    let follower = (0..3).find(|i| *i != leader).unwrap();
    let (alice, _a) = signed_up(&c, leader, "alice").await;
    let (bob, _b) = signed_up(&c, follower, "bob").await;
    let bob_id = bob.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let created = alice
        .post("apps", json!({ "name": "kv", "bundle": bundle }))
        .await
        .unwrap();
    let app = created["appId"].as_str().unwrap().to_owned();
    let _ok = alice
        .post(
            &format!("apps/{app}/access"),
            json!({ "access": { "kind": "accounts", "accounts": [bob_id] } }),
        )
        .await
        .unwrap();
    // Bob reaches the app through the follower once it holds the grant.
    let follower_node = &c.nodes[follower].node;
    for _ in 0..100 {
        if follower_node.sys(|s| {
            s.apps
                .get(&app)
                .is_some_and(|a| s.may_reach(a, Some(&bob_id)))
        }) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let read = bob
        .view(&app, "get", json!({ "key": "k" }), Consistency::Strong)
        .await;
    assert!(read.is_ok(), "granted: {:?}", read.err());

    // Hold the follower, revoke Bob through the leader, call through the follower.
    c.hold[follower].send_replace(true);
    let _ok = alice
        .post(
            &format!("apps/{app}/access"),
            json!({ "access": { "kind": "owner" } }),
        )
        .await
        .unwrap();
    assert!(
        follower_node.sys(|s| s
            .apps
            .get(&app)
            .is_some_and(|a| s.may_reach(a, Some(&bob_id)))),
        "the follower must still hold the record with the grant"
    );
    let call = tokio::spawn({
        let app = app.clone();
        async move {
            bob.view(&app, "get", json!({ "key": "k" }), Consistency::Strong)
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    c.hold[follower].send_replace(false);
    let read = call.await.unwrap();
    let err = read.expect_err("a revocation committed before the request must hold");
    assert_eq!(err.code(), Some("NOT_FOUND"), "{err}");
}
