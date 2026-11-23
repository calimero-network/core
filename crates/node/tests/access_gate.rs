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
//! Builds the component kv fixture `apps/fixture-kv` from this checkout
//! (see `harness/cluster.rs`).

use std::collections::BTreeMap;
use std::time::Duration;

use calimero_client::types::ViewsRequestConsistency;
use calimero_client::{Client, Policy};
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

use harness::{cluster_name, free_port, kv_wasm, node_config, typed, wire, ORIGIN};

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

/// An organization `owner` makes with `who` as a member: an `accounts` list reaches the
/// organization's members (a sign-up makes no organization any more).
async fn shared_org(owner: &Client, pk: &mut SoftAuthenticator, who: &Client) -> String {
    let org = owner
        .orgs()
        .create("Acme", None, None)
        .await
        .map(wire)
        .unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let token = owner
        .orgs()
        .invite_create(&org, typed(json!({ "role": "member" })), Some(pk))
        .await
        .map(wire)
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = who.orgs().invite_redeem(&token).await.unwrap();
    org
}

/// `k` as `cl` reads it, through a strong view, or the refusal as text.
async fn read_k(cl: &Client, app: &str) -> Result<serde_json::Value, String> {
    cl.apps()
        .view(
            app,
            "get",
            Some(json!({ "key": "k" })),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .map(|v| v.result)
        .map_err(|e| e.to_string())
}

/// An `anyone` grant admits every signed-in account, a member of the instance's organization
/// or not, and never a caller without an account (`401`).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn anyone_means_any_signed_in_account() {
    let c = ProxiedCluster::start().await;
    let leader = c.system_leader();
    let (alice, _a) = signed_up(&c, leader, "alice").await;
    let (bob, _b) = signed_up(&c, (leader + 1) % 3, "bob").await;
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let make = |name: &'static str, access: serde_json::Value| {
        let alice = &alice;
        let bundle = bundle.clone();
        async move {
            let mut body = json!({ "name": name, "bundle": bundle });
            body.as_object_mut().unwrap().extend(
                access
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone())),
            );
            let app = alice.apps().create(typed(body)).await.map(wire).unwrap()["appId"]
                .as_str()
                .unwrap()
                .to_owned();
            let _set = alice
                .apps()
                .call(
                    &app,
                    "set",
                    Some(json!({ "key": "k", "value": "v" })),
                    None,
                    None,
                )
                .await
                .unwrap();
            app
        }
    };
    let app = make("granted", json!({ "grants": [{ "kind": "anyone" }] })).await;
    // Bob is a member of none of alice's organizations: signed in, he is admitted.
    assert_eq!(read_k(&bob, &app).await.unwrap(), json!("v"));
    for i in 0..3 {
        // Refused by the platform's gate, before the app runs.
        let e = read_k(&c.client(i), &app).await.unwrap_err();
        assert!(
            e.starts_with("401") && e.contains("sign in to use this app"),
            "node {i}: {e}"
        );
        let e = c.client(i).apps().get(&app).await.unwrap_err();
        assert!(e.to_string().starts_with("404"), "node {i}: {e}");
    }
    let record = alice.apps().get(&app).await.map(wire).unwrap();
    assert_eq!(record["grants"], json!([{ "kind": "anyone" }]), "{record}");
    assert!(record.get("access").is_none(), "{record}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_grant_is_honoured_by_a_follower_that_has_not_applied_it_yet() {
    let c = ProxiedCluster::start().await;
    let leader = c.system_leader();
    let follower = (0..3).find(|i| *i != leader).unwrap();
    let (alice, mut a) = signed_up(&c, leader, "alice").await;
    let (bob, _b) = signed_up(&c, follower, "bob").await;
    let bob_id = bob.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let org = shared_org(&alice, &mut a, &bob).await;
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let created = alice
        .apps()
        .create(typed(json!({
            "name": "kv", "bundle": bundle, "org": org, "grants": [],
        })))
        .await
        .map(wire)
        .unwrap();
    let app = created["appId"].as_str().unwrap().to_owned();
    // The follower holds the app's record before its appends are held.
    let follower_node = &c.nodes[follower].node;
    let _record = follower_node.app(&app).await.unwrap();

    // Hold the leader's appends to the follower, then grant Bob through the
    // leader: the other follower's ack commits it.
    c.hold[follower].send_replace(true);
    let _ok = alice
        .apps()
        .access(
            &app,
            vec![typed(json!({ "kind": "account", "account": bob_id }))],
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
            bob.apps()
                .view(
                    &app,
                    "get",
                    Some(json!({ "key": "k" })),
                    Some(ViewsRequestConsistency::Strong),
                    None,
                )
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
    let (alice, mut a) = signed_up(&c, leader, "alice").await;
    let (bob, _b) = signed_up(&c, follower, "bob").await;
    let bob_id = bob.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let org = shared_org(&alice, &mut a, &bob).await;
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let created = alice
        .apps()
        .create(typed(json!({ "name": "kv", "bundle": bundle, "org": org })))
        .await
        .map(wire)
        .unwrap();
    let app = created["appId"].as_str().unwrap().to_owned();
    let _ok = alice
        .apps()
        .access(
            &app,
            vec![typed(json!({ "kind": "account", "account": bob_id }))],
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
        .apps()
        .view(
            &app,
            "get",
            Some(json!({ "key": "k" })),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await;
    assert!(read.is_ok(), "granted: {:?}", read.err());

    // Hold the follower, revoke Bob through the leader, call through the follower.
    c.hold[follower].send_replace(true);
    let _ok = alice.apps().access(&app, vec![]).await.unwrap();
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
            bob.apps()
                .view(
                    &app,
                    "get",
                    Some(json!({ "key": "k" })),
                    Some(ViewsRequestConsistency::Strong),
                    None,
                )
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    c.hold[follower].send_replace(false);
    let read = call.await.unwrap();
    let err = read.expect_err("a revocation committed before the request must hold");
    assert_eq!(err.code(), Some("NOT_FOUND"), "{err}");
}

/// Review finding A3: `GET /attestation?app=` is in the clear and takes no
/// credential, yet it bound any app's bundle hash, so anyone could learn that a
/// private app exists and which release it runs (`404` against `200`). It now
/// answers as an anonymous `GET /apps/{app}` does: an app reads exactly as an id
/// nobody holds, as no grant admits a caller without an account.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn attestation_names_no_app_an_anonymous_caller_cannot_reach() {
    let c = ProxiedCluster::start().await;
    let leader = c.system_leader();
    let (alice, _a) = signed_up(&c, leader, "alice").await;
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let created = alice
        .apps()
        .create(typed(json!({ "name": "kv", "bundle": bundle })))
        .await
        .map(wire)
        .unwrap();
    let app = created["appId"].as_str().unwrap().to_owned();
    let _ok = alice.apps().access(&app, vec![]).await.unwrap();
    let plain = reqwest::Client::new();
    let attest = |i: usize, app: String| {
        let url = format!(
            "{}attestation?nonce={}&app={app}",
            c.direct[i],
            hex::encode([7_u8; 32])
        );
        let plain = plain.clone();
        async move {
            let r = plain.get(url).send().await.unwrap();
            let status = r.status().as_u16();
            let body: serde_json::Value = r.json().await.unwrap();
            (status, body)
        }
    };
    for i in 0..3 {
        let private = attest(i, app.clone()).await;
        let unknown = attest(i, "no-such-app".to_owned()).await;
        assert_eq!(private.0, 404, "node {i}: {private:?}");
        assert_eq!(private, unknown, "node {i}: a private app reads as no app");
    }
}

/// A blob deleted through the app's leader is gone for a follower that has not
/// applied the delete yet: a download there must not serve the bytes it still
/// holds (the intermittent `release-v2-parity` CI failure in
/// `blob_delete_through_one_node_is_gone_everywhere`).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_blob_deleted_through_the_leader_is_not_served_by_a_follower_behind() {
    let c = ProxiedCluster::start().await;
    let (alice, mut alice_pk) = signed_up(&c, c.system_leader(), "alice").await;
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let created = alice
        .apps()
        .create(typed(json!({ "name": "kv", "bundle": bundle })))
        .await
        .map(wire)
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

    let blob = alice
        .apps()
        .blob_upload(&app, vec![7_u8; 4096], None)
        .await
        .map(wire)
        .unwrap();
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
        at_follower
            .apps()
            .blob_download(&app, &id, None)
            .await
            .unwrap()
            .len(),
        4096,
        "served before the delete"
    );

    // Hold the follower, delete through the leader: the other follower's ack
    // commits it, and the bytes' deletion on the held follower waits.
    c.hold[follower].send_replace(true);
    let delete = tokio::spawn({
        let (app, id) = (app.clone(), id.clone());
        async move { deleter.apps().blob_delete(&app, &id).await.map(wire) }
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

    let read = at_follower
        .apps()
        .blob_download(&app, &id, None)
        .await
        .map(|b| b.len());
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
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let created = alice
        .apps()
        .create(typed(json!({ "name": "kv", "bundle": bundle })))
        .await
        .map(wire)
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

    let blob = alice
        .apps()
        .blob_upload(&app, vec![9_u8; 4096], None)
        .await
        .map(wire)
        .unwrap();
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
        async move { deleter.apps().blob_delete(&app, &id).await.map(wire) }
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        follower_group.read_local(|m, _| m.holds_blob(&raw)),
        "the follower must still hold the mark"
    );
    c.hold[follower].send_replace(false);
    let _deleted = delete.await.unwrap().unwrap();

    // Once the follower catches up, it holds neither the mark nor the bytes.
    harness::poll::eventually(
        harness::poll::EVENTUALLY,
        "the follower applies the delete",
        || async {
            let mark = follower_group.read_local(|m, _| m.holds_blob(&raw));
            let bytes = follower_has_bytes();
            if mark || bytes {
                Err(format!("mark {mark}, bytes {bytes}"))
            } else {
                Ok(())
            }
        },
    )
    .await;
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
    let alice_id = alice.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let created = alice
        .deployments()
        .create(
            "labs",
            typed(json!({ "account": alice_id })),
            Some(&mut alice_pk),
        )
        .await
        .map(wire)
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
    let shown = anonymous.deployments().get(&id).await.map(wire);
    assert!(shown.is_ok(), "{:?}", shown.err());

    // Hold the follower, delete through the leader, then read anonymously
    // on the follower.
    c.hold[follower].send_replace(true);
    let _deleted = alice
        .deployments()
        .delete(&id, Some(&mut alice_pk))
        .await
        .unwrap();
    assert!(here(), "the follower must still hold the deployment");
    let read = tokio::spawn({
        let id = id.clone();
        async move { anonymous.deployments().get(&id).await.map(wire) }
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
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let app = alice
        .apps()
        .create(typed(json!({ "name": "kv", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let _warm = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "k", "value": "0" })),
            None,
            None,
        )
        .await
        .unwrap();
    for entry in 0..3 {
        // Alice's session on the node the requests enter.
        let client = c.client(entry);
        let _s = client.sign_in(&mut pk).await.unwrap();
        for n in 0..3 {
            let before = rounds(&c);
            let _r = client
                .apps()
                .call(
                    &app,
                    "set",
                    Some(json!({ "key": "k", "value": n.to_string() })),
                    None,
                    None,
                )
                .await
                .unwrap();
            let call = rounds(&c) - before;
            let before = rounds(&c);
            let _v = client
                .apps()
                .view(
                    &app,
                    "get",
                    Some(json!({ "key": "k" })),
                    Some(ViewsRequestConsistency::Strong),
                    None,
                )
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
    let (alice, mut a) = signed_up(&c, sys_leader, "alice").await;
    let (bob, _b) = signed_up(&c, sys_leader, "bob").await;
    let bob_id = bob.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let org = shared_org(&alice, &mut a, &bob).await;
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let mut placed = None;
    for n in 0..4 {
        let app = alice
            .apps()
            .create(typed(
                json!({ "name": &format!("kv{n}"), "bundle": &bundle, "org": &org }),
            ))
            .await
            .map(|a| a.app_id)
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
        .apps()
        .access(
            &app,
            vec![typed(json!({ "kind": "account", "account": bob_id }))],
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
    let _ok = alice.apps().access(&app, vec![]).await.unwrap();
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

/// A signed single-service `.mpk` of `com.test.undo` at `version` running `wasm`.
fn undo_release(version: &str, wasm: &[u8]) -> Vec<u8> {
    let key = ed25519_dalek::SigningKey::from_bytes(&[46; 32]);
    let sha = |b: &[u8]| hex::encode(calimero_identity::sha256(b));
    let manifest = calimero_bundle::BundleManifest {
        version: "1.0".to_owned(),
        package: "com.test.undo".to_owned(),
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
            hash: sha(wasm),
            size: wasm.len() as u64,
        }),
        abi: None,
        compute: None,
        links: None,
        frontend: None,
        frontend_aliases: Vec::new(),
        outbound: Vec::new(),
        secrets: None,
        uses: None,
        upgrades: None,
        instance_org: None,
        guests: None,
        services: None,
        pipelines: Vec::new(),
        signature: None,
    };
    let mut manifest = serde_json::to_value(&manifest).unwrap();
    calimero_bundle::sign_manifest_json(&mut manifest, &key).unwrap();
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::default(),
    ));
    for (name, bytes) in [
        ("manifest.json", serde_json::to_vec(&manifest).unwrap()),
        ("app.wasm", wasm.to_vec()),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_path(name).unwrap();
        header.set_size(bytes.len() as u64);
        header.set_cksum();
        tar.append(&header, bytes.as_slice()).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

/// Bugbot on #5210 ("lagging replica undercounts"): the node that took an upgrade's
/// restore point may still host the app and be behind its group. What an undo would lose
/// is then read from a majority of the app's replicas, the furthest counting, as where
/// that node no longer hosts it, never from its own applied index alone: a write it has
/// not applied yet is still a write the undo takes back.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_undo_counts_lost_writes_from_a_majority_where_the_restore_points_node_lags() {
    let c = ProxiedCluster::start().await;
    let (alice, mut pk) = signed_up(&c, c.system_leader(), "alice").await;
    let org = alice
        .orgs()
        .create("Acme", None, None)
        .await
        .map(wire)
        .unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    // The second release runs the same module under another custom section.
    let mut v11 = kv_wasm();
    v11.extend_from_slice(&[0, 4, 3, b'v', b'1', b'1']);
    for (version, wasm) in [("1.0.0", kv_wasm()), ("1.1.0", v11)] {
        let _ = alice
            .packages()
            .upload(undo_release(version, &wasm))
            .await
            .unwrap();
    }
    let app = alice
        .apps()
        .create(typed(json!({
            "name": "notes", "package": "com.test.undo", "version": "1.0.0", "org": org,
        })))
        .await
        .map(wire)
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
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
    let holder = (0..3).find(|i| *i != leader).unwrap();

    // Upgraded through a follower, which takes the restore point itself.
    let at_holder = c.client(holder);
    let _s = at_holder.sign_in(&mut pk).await.unwrap();
    let at_leader = c.client(leader);
    let _s = at_leader.sign_in(&mut pk).await.unwrap();
    let _up = at_holder
        .apps()
        .upgrade(&app, typed(json!({ "version": "1.1.0" })), Some(&mut pk))
        .await
        .unwrap();
    let holder_node = &c.nodes[holder].node;
    let mut point = None;
    for _ in 0..200 {
        point = holder_node.sys(|s| {
            s.apps
                .get(&app)
                .and_then(|a| a.restore_point.clone())
                .and_then(|p| p.file)
        });
        if point.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let point = point.expect("the upgrade logged a restore point");
    assert_eq!(point.node, holder as u64 + 1, "taken on the holder");
    // The holder applied the upgrade, and the system log has settled, before it is held.
    let holder_group = holder_node.hosted(&app).unwrap();
    let leader_group = c.nodes[leader].node.hosted(&app).unwrap();
    for _ in 0..200 {
        let leaders = leader_group.read_local(|m, _| m.state.last_index);
        if holder_group.read_local(|m, _| m.state.last_index) == leaders {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Held, the holder misses a write the other two commit.
    c.hold[holder].send_replace(true);
    let _r = at_leader
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "k", "value": "after" })),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(
        holder_group.read_local(|m, _| m.last_write_ms()).is_none(),
        "the holder must not have applied the write"
    );

    let plan = at_holder
        .transport()
        .json(calimero_client::Request::new(
            reqwest::Method::GET,
            format!("apps/{app}/undo").as_str(),
        ))
        .await;
    c.hold[holder].send_replace(false);
    let plan: serde_json::Value = plan.unwrap();
    assert_eq!(plan["available"], true, "{plan}");
    let lost = &plan["apps"][0]["lostWrites"];
    assert_eq!(
        lost["any"], true,
        "the write the holder has not applied is lost all the same: {plan}"
    );
    assert!(
        lost["entriesSince"].as_u64().is_some_and(|n| n >= 2),
        "the upgrade and the write since the restore point: {plan}"
    );
}
