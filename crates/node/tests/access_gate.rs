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

use calimero_client::{Client, Consistency, Policy};
use calimero_identity::testing::SoftAuthenticator;
use calimero_node::{run, Running};
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

use harness::{cluster_name, free_port, kv_wasm, node_config, ORIGIN};

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
        // `free_port` hands the port back before the node binds it, so another
        // test can take it meanwhile: a start that loses that race tries again
        // on other ports, as `Cluster::start_with` does.
        let mut attempt = 0;
        let (nodes, direct, hold, dirs) = loop {
            attempt += 1;
            let mut peers = BTreeMap::new();
            let mut direct = Vec::new();
            let mut hold = Vec::new();
            let mut proxies = Vec::new();
            for id in 1..=3_u64 {
                let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let proxy_port = proxy_listener.local_addr().unwrap().port();
                let node_port = free_port();
                let (tx, rx) = watch::channel(false);
                proxies.push(tokio::spawn(proxy(
                    proxy_listener,
                    format!("127.0.0.1:{node_port}"),
                    rx,
                )));
                let _prev = peers.insert(id, format!("http://127.0.0.1:{proxy_port}/"));
                direct.push(format!("http://127.0.0.1:{node_port}/"));
                hold.push(tx);
            }
            // Its own name, so its own peer secret: a node of another test that
            // a reused port leads to refuses this cluster's Raft messages.
            let name = cluster_name();
            let mut nodes = Vec::new();
            let mut dirs = Vec::new();
            let mut failed = None;
            for id in 1..=3_u64 {
                let dir = tempfile::tempdir().unwrap();
                let mut config = node_config(id, &peers, dir.path());
                config.cluster.clone_from(&name);
                // Peers name the proxy; the node listens on its own port.
                config.listen = url::Url::parse(&direct[id as usize - 1])
                    .unwrap()
                    .socket_addrs(|| None)
                    .unwrap()[0];
                // A held follower must not start an election while it waits.
                config.raft.election_min_ms = 5_000;
                config.raft.election_max_ms = 6_000;
                match run(config).await {
                    Ok(r) => nodes.push(r),
                    Err(e) => {
                        failed = Some(e);
                        break;
                    }
                }
                dirs.push(dir);
            }
            match failed {
                None => break (nodes, direct, hold, dirs),
                Some(e) if attempt < 3 => {
                    eprintln!("proxied cluster start failed ({e:#}); retrying on other ports");
                    for r in nodes {
                        r.stop().await;
                    }
                    for p in proxies {
                        p.abort();
                    }
                }
                Some(e) => panic!("proxied cluster did not start: {e:#}"),
            }
        };
        let c = Self {
            nodes,
            direct,
            hold,
            _dirs: dirs,
        };
        for _ in 0..200 {
            if c.nodes.iter().all(|r| r.node.sys(|s| s.nodes.len() == 3)) {
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

/// An anonymous `GET /apps/{app}`: once the app stops being public through the
/// leader, a follower that has not applied that must not show the app.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "fails until #4723: app_get does not read fresh state for anonymous callers"]
async fn an_app_made_private_is_hidden_from_anonymous_callers_on_a_follower_behind() {
    let c = ProxiedCluster::start().await;
    let leader = c.system_leader();
    let follower = (0..3).find(|i| *i != leader).unwrap();
    let (alice, _a) = signed_up(&c, leader, "alice").await;
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let created = alice
        .post("apps", json!({ "name": "kv", "bundle": bundle }))
        .await
        .unwrap();
    let app = created["appId"].as_str().unwrap().to_owned();
    let _ok = alice
        .post(
            &format!("apps/{app}/access"),
            json!({ "access": { "kind": "public" } }),
        )
        .await
        .unwrap();
    let follower_node = &c.nodes[follower].node;
    let public_here =
        || follower_node.sys(|s| s.apps.get(&app).is_some_and(|a| s.may_reach(a, None)));
    for _ in 0..100 {
        if public_here() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let anonymous = c.client(follower);
    let shown = anonymous.get(&format!("apps/{app}")).await;
    assert!(shown.is_ok(), "public: {:?}", shown.err());

    // Hold the follower, make the app private through the leader, then ask the
    // follower anonymously.
    c.hold[follower].send_replace(true);
    let _ok = alice
        .post(
            &format!("apps/{app}/access"),
            json!({ "access": { "kind": "owner" } }),
        )
        .await
        .unwrap();
    assert!(
        public_here(),
        "the follower must still hold the app as public"
    );
    let call = tokio::spawn({
        let app = app.clone();
        async move { anonymous.get(&format!("apps/{app}")).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    c.hold[follower].send_replace(false);
    let shown = call.await.unwrap();
    let err = shown.expect_err("an app made private before the request must be hidden");
    assert!(
        matches!(err.code(), Some("NOT_FOUND" | "UNAUTHENTICATED")),
        "{err}"
    );
}

/// A blob deleted through the app's leader is gone for a follower that has not
/// applied the delete yet: a download there must not serve the bytes it still
/// holds (the intermittent `release-v2-parity` CI failure in
/// `blob_delete_through_one_node_is_gone_everywhere`).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_blob_deleted_through_the_leader_is_not_served_by_a_follower_behind() {
    let c = ProxiedCluster::start().await;
    let (alice, mut alice_pk) = signed_up(&c, c.system_leader(), "alice").await;
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let created = alice
        .post("apps", json!({ "name": "kv", "bundle": bundle }))
        .await
        .unwrap();
    let app = created["appId"].as_str().unwrap().to_owned();
    let mut leader = None;
    for _ in 0..200 {
        leader = (0..3).find(|i| {
            c.nodes[*i]
                .node
                .hosted(&app)
                .is_some_and(|g| g.status().is_leader)
        });
        if leader.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let leader = leader.expect("the app's group elects a leader");
    let follower = (0..3).find(|i| *i != leader).unwrap();

    let blob = alice.upload_blob(&app, vec![7_u8; 4096]).await.unwrap();
    let id = blob["id"].as_str().unwrap().to_owned();
    let raw: [u8; 32] = hex::decode(&id).unwrap().try_into().unwrap();
    // The follower holds the mark and the bytes before its appends are held.
    let follower_group = c.nodes[follower].node.hosted(&app).unwrap();
    for _ in 0..100 {
        if follower_group.read_local(|m, _| m.holds_blob(&raw)) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // Alice through the follower, and through the app's leader.
    let at_follower = c.client(follower);
    let _s = at_follower.sign_in(&mut alice_pk).await.unwrap();
    let deleter = c.client(leader);
    let _s = deleter.sign_in(&mut alice_pk).await.unwrap();
    assert_eq!(
        at_follower.download_blob(&app, &id).await.unwrap().len(),
        4096,
        "served before the delete"
    );

    // Hold the follower, delete through the leader: the other follower's ack
    // commits it, and the bytes' deletion on the held follower waits.
    c.hold[follower].send_replace(true);
    let delete = tokio::spawn({
        let (app, id) = (app.clone(), id.clone());
        async move { deleter.delete_blob(&app, &id).await }
    });
    let leader_group = c.nodes[leader].node.hosted(&app).unwrap();
    for _ in 0..200 {
        if !leader_group.read_local(|m, _| m.holds_blob(&raw)) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !leader_group.read_local(|m, _| m.holds_blob(&raw)),
        "the delete committed on the leader"
    );
    assert!(
        follower_group.read_local(|m, _| m.holds_blob(&raw)),
        "the follower must still hold the mark"
    );

    let read = at_follower.download_blob(&app, &id).await.map(|b| b.len());
    c.hold[follower].send_replace(false);
    let _deleted = delete.await.unwrap();
    let err = read.expect_err("a blob deleted before the request must not be served");
    assert_eq!(err.code(), Some("NOT_FOUND"), "{err}");
}

/// A blob deleted through the app's leader leaves no bytes on a follower that
/// had not applied the delete when the leader asked it to drop them: the
/// follower must not keep them because its own replica still marks the blob
/// (the intermittent `blobs_list_and_delete_and_storage_is_inspectable`
/// failure, "the bytes outlived every app").
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_follower_behind_drops_the_bytes_of_a_blob_deleted_through_the_leader() {
    let c = ProxiedCluster::start().await;
    let (alice, mut alice_pk) = signed_up(&c, c.system_leader(), "alice").await;
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let created = alice
        .post("apps", json!({ "name": "kv", "bundle": bundle }))
        .await
        .unwrap();
    let app = created["appId"].as_str().unwrap().to_owned();
    let mut leader = None;
    for _ in 0..200 {
        leader = (0..3).find(|i| {
            c.nodes[*i]
                .node
                .hosted(&app)
                .is_some_and(|g| g.status().is_leader)
        });
        if leader.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let leader = leader.expect("the app's group elects a leader");
    let follower = (0..3).find(|i| *i != leader).unwrap();

    let blob = alice.upload_blob(&app, vec![9_u8; 4096]).await.unwrap();
    let id = blob["id"].as_str().unwrap().to_owned();
    let raw: [u8; 32] = hex::decode(&id).unwrap().try_into().unwrap();
    let follower_group = c.nodes[follower].node.hosted(&app).unwrap();
    let follower_has_bytes = || {
        c.nodes[follower]
            .node
            .engine
            .get_blob(&id)
            .unwrap()
            .is_some()
    };
    for _ in 0..100 {
        if follower_group.read_local(|m, _| m.holds_blob(&raw)) && follower_has_bytes() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(follower_has_bytes(), "the follower holds the bytes first");
    let deleter = c.client(leader);
    let _s = deleter.sign_in(&mut alice_pk).await.unwrap();

    // Hold the follower and delete through the leader: the other follower's
    // ack commits the delete, and the request to drop the bytes waits with
    // the held follower's appends.
    c.hold[follower].send_replace(true);
    let delete = tokio::spawn({
        let (app, id) = (app.clone(), id.clone());
        async move { deleter.delete_blob(&app, &id).await }
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        follower_group.read_local(|m, _| m.holds_blob(&raw)),
        "the follower must still hold the mark"
    );
    c.hold[follower].send_replace(false);
    let _deleted = delete.await.unwrap().unwrap();

    // Once the follower catches up, it holds neither the mark nor the bytes.
    for _ in 0..150 {
        if !follower_group.read_local(|m, _| m.holds_blob(&raw)) && !follower_has_bytes() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !follower_group.read_local(|m, _| m.holds_blob(&raw)),
        "the follower applied the delete"
    );
    assert!(
        !follower_has_bytes(),
        "the bytes outlived the only app that marked them"
    );
}
