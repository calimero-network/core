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
//! Builds the component kv fixture `apps/v2-fixtures/kv` from this checkout
//! (see `harness/cluster.rs`).

// Tracking: these tests call the route methods `calimero-client` wrote by hand,
// deprecated for its generated layer (`client.<group>().<method>()`); they move
// over, and the hand-written methods go, in a follow-up.
#![allow(
    deprecated,
    reason = "the hand-written route methods, until these tests use the generated ones"
)]

use std::collections::BTreeMap;
use std::time::Duration;

use calimero_client::{Client, Consistency, Policy};
use calimero_identity::testing::SoftAuthenticator;
use calimero_node::node::{CallRequest, NOT_REACHED};
use calimero_node::types::Caller;
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
        Self::start_with(|_| {}).await
    }

    async fn start_with(configure: impl Fn(&mut calimero_node::Config)) -> Self {
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
                configure(&mut config);
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

/// A deployment deleted through the system leader is gone for an anonymous
/// reader on a follower that has not applied the delete yet: the public read
/// must not serve the copy that follower still holds (the intermittent
/// `deployments::accounts_and_organizations_own_deployments_their_managers_change`
/// failure).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_deployment_deleted_through_the_leader_is_gone_on_a_follower_behind() {
    let c = ProxiedCluster::start().await;
    let leader = c.system_leader();
    let follower = (0..3).find(|i| *i != leader).unwrap();
    let (alice, mut alice_pk) = signed_up(&c, leader, "alice").await;
    let alice_id = alice.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let created = alice
        .post_confirmed(
            "deployments",
            json!({ "name": "labs", "owner": { "account": alice_id } }),
            &mut alice_pk,
        )
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_owned();
    let follower_node = &c.nodes[follower].node;
    let here = || follower_node.sys(|s| s.deployments.contains_key(&id));
    for _ in 0..100 {
        if here() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let anonymous = c.client(follower);
    let shown = anonymous.get(&format!("deployments/{id}")).await;
    assert!(shown.is_ok(), "{:?}", shown.err());

    // Hold the follower, delete through the leader, then read anonymously
    // on the follower.
    c.hold[follower].send_replace(true);
    let _deleted = alice
        .delete_confirmed(&format!("deployments/{id}"), &mut alice_pk)
        .await
        .unwrap();
    assert!(here(), "the follower must still hold the deployment");
    let read = tokio::spawn({
        let id = id.clone();
        async move { anonymous.get(&format!("deployments/{id}")).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    c.hold[follower].send_replace(false);
    let err = read
        .await
        .unwrap()
        .expect_err("a deployment deleted before the request must be gone");
    assert_eq!(err.code(), Some("NOT_FOUND"), "{err}");
}

/// Read-index rounds every node began on the system group so far
/// (`calimero_system_read_rounds_total`).
fn rounds(c: &ProxiedCluster) -> u64 {
    c.nodes
        .iter()
        .map(|r| r.node.sys_rounds.load(std::sync::atomic::Ordering::SeqCst))
        .sum()
}

/// A client's call costs one read-index round on the system group, whichever
/// node it enters and wherever the app's leader is: the round the gate makes
/// for the request covers the checks the app's leader makes before it runs
/// the call (`Node::sys_fresh_from`). Before, the leader ran a round of its
/// own (two per call, three when the call entered a follower replica first);
/// a strong view was one, and stays one.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_call_costs_one_system_read_round() {
    let c = ProxiedCluster::start().await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
    let _warm = alice
        .call(&app, "set", json!({ "key": "k", "value": "0" }), None)
        .await
        .unwrap();
    for entry in 0..3 {
        // Alice's session on the node the requests enter.
        let client = c.client(entry);
        let _s = client.sign_in(&mut pk).await.unwrap();
        for n in 0..3 {
            let before = rounds(&c);
            let _r = client
                .call(
                    &app,
                    "set",
                    json!({ "key": "k", "value": n.to_string() }),
                    None,
                )
                .await
                .unwrap();
            let call = rounds(&c) - before;
            let before = rounds(&c);
            let _v = client
                .view(&app, "get", json!({ "key": "k" }), Consistency::Strong)
                .await
                .unwrap();
            let view = rounds(&c) - before;
            assert_eq!(
                (call, view),
                (1, 1),
                "system read rounds of one call and one strong view entering node {entry}"
            );
        }
    }
}

/// A revocation committed before a call's round holds on the app's leader,
/// though the leader has not applied it when the call reaches it, and no node
/// runs a round for the call there: the call carries the index its gate's
/// round named (`CallRequest::sys_index`) and the leader waits to apply it
/// (`Node::sys_fresh_from`). Without that wait the leader would run the call
/// on its stale grant and commit it.
///
/// Apps get two replicas, so a later app is led by a node other than the
/// system leader (placement takes the least loaded, then the lowest id); its
/// leader's peer traffic, the system appends included, is held. The call is
/// handed to the leader as `internal/call` hands it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_revocation_committed_before_the_round_holds_on_a_leader_behind() {
    let c = ProxiedCluster::start_with(|cfg| cfg.replicas = 2).await;
    let sys_leader = c.system_leader();
    let (alice, _a) = signed_up(&c, sys_leader, "alice").await;
    let (bob, _b) = signed_up(&c, sys_leader, "bob").await;
    let bob_id = bob.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let mut placed = None;
    for n in 0..4 {
        let app = alice
            .create_app(&format!("kv{n}"), &bundle, json!({}))
            .await
            .unwrap();
        let mut leader = None;
        for _ in 0..100 {
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
        let leader = leader.expect("the app has a leader");
        if leader != sys_leader {
            placed = Some((app, leader));
            break;
        }
    }
    let (app, app_leader) = placed.expect("an app led by a node other than the system leader");
    let _ok = alice
        .post(
            &format!("apps/{app}/access"),
            json!({ "access": { "kind": "accounts", "accounts": [bob_id] } }),
        )
        .await
        .unwrap();
    let leader_node = std::sync::Arc::clone(&c.nodes[app_leader].node);
    let reaches = || {
        leader_node.sys(|s| {
            s.apps
                .get(&app)
                .is_some_and(|a| s.may_reach(a, Some(&bob_id)))
        })
    };
    for _ in 0..100 {
        if reaches() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(reaches(), "the app's leader must hold the grant");
    let bob_call = |value: &str| {
        CallRequest::new(
            app.clone(),
            "set".into(),
            json!({ "key": "k", "value": value }),
            Caller::Account {
                account: bob_id.clone(),
            },
        )
    };
    let granted = leader_node.call_component(&bob_call("0")).await.unwrap();
    assert!(granted.error.is_null(), "granted: {:?}", granted.error);

    // Hold the app's leader, revoke Bob through the system leader, then make
    // the round a request would make now.
    c.hold[app_leader].send_replace(true);
    let _ok = alice
        .post(
            &format!("apps/{app}/access"),
            json!({ "access": { "kind": "owner" } }),
        )
        .await
        .unwrap();
    let (index, _caught_up) = c.nodes[sys_leader].node.sys_fresh_index().await;
    let index = index.expect("the system leader names its index");
    assert!(
        reaches(),
        "the app's leader must not have applied the revocation"
    );

    let before = rounds(&c);
    let call = tokio::spawn({
        let node = std::sync::Arc::clone(&leader_node);
        let req = CallRequest {
            sys_index: Some(index),
            ..bob_call("1")
        };
        async move { node.call_component(&req).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    c.hold[app_leader].send_replace(false);
    let answer = call.await.unwrap().expect("an answer");
    assert_eq!(
        answer.error.as_str(),
        Some(NOT_REACHED),
        "a revocation committed before the round must hold: {:?}",
        answer.result
    );
    assert_eq!(rounds(&c), before, "the call must run no round of its own");
}
