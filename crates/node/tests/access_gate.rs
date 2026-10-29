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

// ------------------------------------------------- D81: open streams lose access

type Frame = (String, String, serde_json::Value);

/// The node leading `app`'s group, once it has one.
async fn app_leader(c: &ProxiedCluster, app: &str) -> usize {
    for _ in 0..250 {
        if let Some(i) = (0..3).find(|i| {
            c.nodes[*i]
                .node
                .hosted(app)
                .is_some_and(|g| g.status().is_leader)
        }) {
            return i;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{app} has no leader");
}

/// Wait until every node has applied every system entry the leader has.
async fn system_applied_everywhere(c: &ProxiedCluster) {
    let last = c.nodes[c.system_leader()]
        .node
        .system
        .status()
        .last_log
        .unwrap_or(0);
    for r in &c.nodes {
        assert!(
            r.node
                .system
                .wait_applied(last, Duration::from_secs(5))
                .await,
            "a node did not apply the system log"
        );
    }
}

/// Read `stream` to its end (at most 10 s): its last frame must be
/// `access_lost` with `status` and `code`, and no frame may mention `after`.
async fn ends_with_access_lost(
    stream: &mut (impl futures_util::Stream<Item = Frame> + Unpin),
    name: &str,
    status: u16,
    code: &str,
    after: &str,
) {
    use futures_util::StreamExt;
    let seen = tokio::time::timeout(Duration::from_secs(10), async {
        let mut seen = Vec::new();
        while let Some(f) = stream.next().await {
            seen.push(f);
        }
        seen
    })
    .await
    .unwrap_or_else(|_| panic!("the stream {name} stayed open after its caller lost access"));
    assert!(
        !seen.iter().any(|(_, _, d)| d.to_string().contains(after)),
        "the stream {name} carried what was written after access was lost: {seen:?}"
    );
    let (_, kind, data) = seen
        .last()
        .unwrap_or_else(|| panic!("the stream {name} ended without a frame"));
    assert_eq!(
        kind, "access_lost",
        "the stream {name} must end with access_lost: {seen:?}"
    );
    assert_eq!(data["status"], json!(status), "{name}: {data}");
    assert_eq!(data["error"], json!(code), "{name}: {data}");
}

/// `stream` carries a frame mentioning `what` within 10 s, so it is still open.
async fn carries(stream: &mut (impl futures_util::Stream<Item = Frame> + Unpin), what: &str) {
    use futures_util::StreamExt;
    let got = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some((_, _, d)) = stream.next().await {
            if d.to_string().contains(what) {
                return true;
            }
        }
        false
    })
    .await;
    assert_eq!(
        got,
        Ok(true),
        "a stream that keeps access must stay open and carry {what}"
    );
}

/// A public kv app of alice's (signed up on the system leader), public on
/// every node; returns the app and its leader.
async fn public_app(c: &ProxiedCluster, alice: &Client) -> (String, usize) {
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
    system_applied_everywhere(c).await;
    assert!(c.nodes.iter().all(|r| r
        .node
        .sys(|s| s.apps.get(&app).is_some_and(|a| s.may_reach(a, None)))));
    let leader = app_leader(c, &app).await;
    (app, leader)
}

/// An open event stream closes for a caller who can no longer see the app
/// (D81): opened anonymously while the app is public, on the app's leader and
/// on a follower, both send `access_lost` (401, what `gate` now answers an
/// anonymous caller) and end once the app is made private, and neither carries
/// an event written after that. The owner's own stream stays open.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_open_event_stream_closes_once_the_app_is_made_private() {
    let c = ProxiedCluster::start().await;
    let (alice, mut alice_pk) = signed_up(&c, c.system_leader(), "alice").await;
    let (app, leader) = public_app(&c, &alice).await;
    let follower = (0..3).find(|i| *i != leader).unwrap();

    let mut on_leader = Box::pin(c.client(leader).events(&app, 0).await.unwrap());
    let mut on_follower = Box::pin(c.client(follower).events(&app, 0).await.unwrap());
    let alice_follower = c.client(follower);
    let _s = alice_follower.sign_in(&mut alice_pk).await.unwrap();
    let mut owners = Box::pin(alice_follower.events(&app, 0).await.unwrap());

    let _ok = alice
        .post(
            &format!("apps/{app}/access"),
            json!({ "access": { "kind": "owner" } }),
        )
        .await
        .unwrap();
    system_applied_everywhere(&c).await;
    let _r = alice
        .call(
            &app,
            "set",
            json!({ "key": "after-private", "value": "v" }),
            None,
        )
        .await
        .unwrap();

    ends_with_access_lost(
        &mut on_leader,
        "on the leader",
        401,
        "UNAUTHENTICATED",
        "after-private",
    )
    .await;
    ends_with_access_lost(
        &mut on_follower,
        "on a follower",
        401,
        "UNAUTHENTICATED",
        "after-private",
    )
    .await;
    carries(&mut owners, "after-private").await;
}

/// A member's open event stream closes once they are removed from the app's
/// organization (D81), on the app's leader and on a follower: `access_lost`
/// (404, what `gate` now answers them), then the end. Another member's stream
/// stays open.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_removed_members_open_event_stream_closes() {
    let c = ProxiedCluster::start().await;
    let (alice, mut alice_pk) = signed_up(&c, c.system_leader(), "alice").await;
    let (bob, mut bob_pk) = signed_up(&c, c.system_leader(), "bob").await;
    let bob_id = bob.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let org = alice.post("orgs", json!({ "name": "Acme" })).await.unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let invite = alice
        .post(&format!("orgs/{org}/invites"), json!({ "role": "member" }))
        .await
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = bob
        .post(&format!("invites/{invite}/redeem"), json!({}))
        .await
        .unwrap();
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice
        .create_app("org-kv", &bundle, json!({ "org": org }))
        .await
        .unwrap();
    system_applied_everywhere(&c).await;
    let leader = app_leader(&c, &app).await;
    let follower = (0..3).find(|i| *i != leader).unwrap();

    let mut bobs = Vec::new();
    for (name, i) in [("on the leader", leader), ("on a follower", follower)] {
        let client = c.client(i);
        let _s = client.sign_in(&mut bob_pk).await.unwrap();
        bobs.push((name, Box::pin(client.events(&app, 0).await.unwrap())));
    }
    let alice_follower = c.client(follower);
    let _s = alice_follower.sign_in(&mut alice_pk).await.unwrap();
    let mut alices = Box::pin(alice_follower.events(&app, 0).await.unwrap());

    let _r = alice
        .delete(&format!("orgs/{org}/members/{bob_id}"))
        .await
        .unwrap();
    system_applied_everywhere(&c).await;
    let _r = alice
        .call(
            &app,
            "set",
            json!({ "key": "after-removal", "value": "v" }),
            None,
        )
        .await
        .unwrap();

    for (name, stream) in &mut bobs {
        ends_with_access_lost(stream, name, 404, "NOT_FOUND", "after-removal").await;
    }
    carries(&mut alices, "after-removal").await;
}

/// The presence stream closes the same way (D81): opened anonymously while
/// the app is public, on the app's leader and on a follower, both send
/// `access_lost` and end once the app is made private, before presence
/// published after that.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_open_presence_stream_closes_once_the_app_is_made_private() {
    let c = ProxiedCluster::start().await;
    let (alice, _alice_pk) = signed_up(&c, c.system_leader(), "alice").await;
    let (app, leader) = public_app(&c, &alice).await;
    let follower = (0..3).find(|i| *i != leader).unwrap();

    let mut on_leader = Box::pin(c.client(leader).presence_stream(&app).await.unwrap());
    let mut on_follower = Box::pin(c.client(follower).presence_stream(&app).await.unwrap());

    let _ok = alice
        .post(
            &format!("apps/{app}/access"),
            json!({ "access": { "kind": "owner" } }),
        )
        .await
        .unwrap();
    system_applied_everywhere(&c).await;
    let state = b"after-private";
    let _r = alice.publish_presence(&app, 1, Some(state)).await.unwrap();
    let after = hex::encode(state);

    ends_with_access_lost(
        &mut on_leader,
        "on the leader",
        401,
        "UNAUTHENTICATED",
        &after,
    )
    .await;
    ends_with_access_lost(
        &mut on_follower,
        "on a follower",
        401,
        "UNAUTHENTICATED",
        &after,
    )
    .await;
}

/// An open event stream opened with an API key closes once the key is revoked
/// (D81): `access_lost` (401 "unknown API key", what `principal` now answers
/// the key), then the end, on the app's leader and on a follower.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_open_event_stream_closes_once_its_api_key_is_revoked() {
    let c = ProxiedCluster::start().await;
    let (alice, _alice_pk) = signed_up(&c, c.system_leader(), "alice").await;
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let created = alice
        .post("apps", json!({ "name": "kv", "bundle": bundle }))
        .await
        .unwrap();
    let app = created["appId"].as_str().unwrap().to_owned();
    let (key_id, key) = alice
        .create_api_key(json!({ "label": "agent" }))
        .await
        .unwrap();
    system_applied_everywhere(&c).await;
    let leader = app_leader(&c, &app).await;
    let follower = (0..3).find(|i| *i != leader).unwrap();

    let mut agents = Vec::new();
    for (name, i) in [("on the leader", leader), ("on a follower", follower)] {
        let client = c.client(i);
        client.use_api_key(&key).await;
        agents.push((name, Box::pin(client.events(&app, 0).await.unwrap())));
    }

    let _r = alice
        .delete(&format!("auth/apikeys/{key_id}"))
        .await
        .unwrap();
    system_applied_everywhere(&c).await;
    let _r = alice
        .call(
            &app,
            "set",
            json!({ "key": "after-revoke", "value": "v" }),
            None,
        )
        .await
        .unwrap();

    for (name, stream) in &mut agents {
        ends_with_access_lost(stream, name, 401, "UNAUTHENTICATED", "after-revoke").await;
    }
}

/// An owner-only kv app of `alice`'s, created on every node; returns it and
/// its leader.
async fn owners_app(c: &ProxiedCluster, alice: &Client) -> (String, usize) {
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let created = alice
        .post("apps", json!({ "name": "kv", "bundle": bundle }))
        .await
        .unwrap();
    let app = created["appId"].as_str().unwrap().to_owned();
    system_applied_everywhere(c).await;
    let leader = app_leader(c, &app).await;
    (app, leader)
}

/// An open event stream opened with an API key closes when the key expires
/// (D81), with no system entry to wake it: one timer per stream, set to the
/// expiry. `access_lost` (401 "API key expired"), then the end.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_open_event_stream_closes_when_its_api_key_expires() {
    let c = ProxiedCluster::start().await;
    let (alice, _alice_pk) = signed_up(&c, c.system_leader(), "alice").await;
    let (app, leader) = owners_app(&c, &alice).await;
    let follower = (0..3).find(|i| *i != leader).unwrap();
    let (_key_id, key) = alice
        .create_api_key(json!({ "label": "short", "ttlSecs": 3 }))
        .await
        .unwrap();
    system_applied_everywhere(&c).await;
    let mut agents = Vec::new();
    for (name, i) in [("on the leader", leader), ("on a follower", follower)] {
        let client = c.client(i);
        client.use_api_key(&key).await;
        agents.push((name, Box::pin(client.events(&app, 0).await.unwrap())));
    }
    for (name, stream) in &mut agents {
        ends_with_access_lost(stream, name, 401, "UNAUTHENTICATED", "never-written").await;
    }
}

/// An open event stream closes once its session is revoked (D81): the
/// account signs out everywhere; its streams on the app's leader and on a
/// follower send `access_lost` (401 "session revoked") and end.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_open_event_stream_closes_once_its_session_is_revoked() {
    let c = ProxiedCluster::start().await;
    let (alice, mut alice_pk) = signed_up(&c, c.system_leader(), "alice").await;
    let (app, leader) = owners_app(&c, &alice).await;
    let follower = (0..3).find(|i| *i != leader).unwrap();
    let mut streams = Vec::new();
    for (name, i) in [("on the leader", leader), ("on a follower", follower)] {
        let client = c.client(i);
        let _s = client.sign_in(&mut alice_pk).await.unwrap();
        streams.push((name, Box::pin(client.events(&app, 0).await.unwrap())));
    }
    let _r = alice.sign_out_everywhere().await.unwrap();
    system_applied_everywhere(&c).await;
    for (name, stream) in &mut streams {
        ends_with_access_lost(stream, name, 401, "UNAUTHENTICATED", "never-written").await;
    }
}

/// An open event stream of a session opened with a passkey closes once that
/// passkey is removed from the account (D81): `access_lost` (401), then the
/// end, on the app's leader and on a follower. The session opened with the
/// account's other passkey keeps its stream.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_open_event_stream_closes_once_its_passkey_is_removed() {
    let c = ProxiedCluster::start().await;
    let (laptop, mut laptop_pk) = signed_up(&c, c.system_leader(), "alice").await;
    let mut phone_pk = SoftAuthenticator::new("localhost", ORIGIN);
    let phone_cred = laptop.add_passkey(&mut phone_pk).await.unwrap();
    let (app, leader) = owners_app(&c, &laptop).await;
    let follower = (0..3).find(|i| *i != leader).unwrap();
    let mut phones = Vec::new();
    for (name, i) in [("on the leader", leader), ("on a follower", follower)] {
        let client = c.client(i);
        let _s = client.sign_in(&mut phone_pk).await.unwrap();
        phones.push((name, Box::pin(client.events(&app, 0).await.unwrap())));
    }
    let laptop_follower = c.client(follower);
    let _s = laptop_follower.sign_in(&mut laptop_pk).await.unwrap();
    let mut laptops = Box::pin(laptop_follower.events(&app, 0).await.unwrap());

    let _r = laptop
        .delete(&format!("auth/passkeys/{phone_cred}"))
        .await
        .unwrap();
    system_applied_everywhere(&c).await;
    let _r = laptop
        .call(
            &app,
            "set",
            json!({ "key": "after-removal", "value": "v" }),
            None,
        )
        .await
        .unwrap();
    for (name, stream) in &mut phones {
        ends_with_access_lost(stream, name, 401, "UNAUTHENTICATED", "after-removal").await;
    }
    carries(&mut laptops, "after-removal").await;
}

/// A member's open event stream on a team app closes once they are taken off
/// the team, though they stay in the organization (D81): `access_lost` (404),
/// then the end, on the app's leader and on a follower. The owner's stream
/// stays open.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_open_event_stream_closes_once_its_member_leaves_the_team() {
    let c = ProxiedCluster::start().await;
    let (alice, mut alice_pk) = signed_up(&c, c.system_leader(), "alice").await;
    let (bob, mut bob_pk) = signed_up(&c, c.system_leader(), "bob").await;
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
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice
        .create_app(
            "team-kv",
            &bundle,
            json!({ "org": org, "access": { "kind": "team", "team": team } }),
        )
        .await
        .unwrap();
    system_applied_everywhere(&c).await;
    let leader = app_leader(&c, &app).await;
    let follower = (0..3).find(|i| *i != leader).unwrap();

    let mut bobs = Vec::new();
    for (name, i) in [("on the leader", leader), ("on a follower", follower)] {
        let client = c.client(i);
        let _s = client.sign_in(&mut bob_pk).await.unwrap();
        bobs.push((name, Box::pin(client.events(&app, 0).await.unwrap())));
    }
    let alice_follower = c.client(follower);
    let _s = alice_follower.sign_in(&mut alice_pk).await.unwrap();
    let mut alices = Box::pin(alice_follower.events(&app, 0).await.unwrap());

    let _r = alice
        .delete(&format!("orgs/{org}/teams/{team}/members/{bob_id}"))
        .await
        .unwrap();
    system_applied_everywhere(&c).await;
    assert!(
        c.nodes[follower]
            .node
            .sys(|s| s.orgs.get(&org).is_some_and(|o| o.role(&bob_id).is_some())),
        "bob stays in the organization"
    );
    let _r = alice
        .call(
            &app,
            "set",
            json!({ "key": "after-team-change", "value": "v" }),
            None,
        )
        .await
        .unwrap();
    for (name, stream) in &mut bobs {
        ends_with_access_lost(stream, name, 404, "NOT_FOUND", "after-team-change").await;
    }
    carries(&mut alices, "after-team-change").await;
}
