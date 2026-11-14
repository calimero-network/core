//! Job nodes next to a cluster: in the directory, never members, replicas or
//! blob targets, and attested by the leader's side before any work reaches them.

mod common;
use common::UploadBytes as _;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::RawQuery;
use axum::routing::get;
use calimero_client::types::OpsRegisterNodeRequestRole;
use calimero_client::Client;
use calimero_node::cluster_key::Request;
use common::{signed_up, typed, wire, Cluster};
use reqwest::Method;
use serde_json::{json, Value};

const JOB: u64 = 4;

async fn ops(c: &Cluster) -> Client {
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    ops
}

async fn create(alice: &Client, name: &str) -> String {
    let bundle = alice
        .packages()
        .bundle_put(common::component("kv"))
        .await
        .map(|b| b.hash)
        .unwrap();
    alice
        .apps()
        .create(typed(json!({ "name": name, "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap()
}

fn replicas(c: &Cluster, app: &str) -> Vec<u64> {
    c.nodes[0]
        .as_ref()
        .unwrap()
        .node
        .sys(|s| s.apps[app].replicas.clone())
}

/// The job node's registration changes nothing about where the cluster's data
/// goes and who holds its keys.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_job_node_is_listed_but_never_a_member_a_replica_or_a_key_holder() {
    let c = Cluster::start(3).await;
    let job = c.start_job_node(JOB, |_| {}).await;
    c.register_job_node(JOB, &job.url).await;
    // Several reconcile rounds (every 500 ms): admission and promotion would show by now.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let leader = c.nodes[0].as_ref().unwrap();
    let status = leader.node.system.status();
    assert!(
        !status.voters.contains(&JOB) && !status.learners.contains(&JOB),
        "{status:?}"
    );

    // Two apps: the second would take the idle node 4 if it were a candidate.
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let first = create(&alice, "one").await;
    let second = create(&alice, "two").await;
    for app in [&first, &second] {
        assert_eq!(replicas(&c, app), vec![1, 2, 3], "{app}");
    }

    let ops = ops(&c).await;
    let e = ops
        .ops()
        .set_replicas(&first, vec![1, 2, JOB], None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("JOB_NODE"), "{e}");

    // Roles are a closed set.
    assert!(ops
        .transport()
        .approved(
            calimero_client::Request::new(Method::POST, "ops/nodes")
                .json(json!({ "id": 9, "addr": "http://x/", "role": "gpu" })),
            None,
            Some(&common::operator_sign)
        )
        .await
        .is_err());

    let placement = ops.ops().placement().await.map(wire).unwrap();
    let nodes = placement["nodes"].as_array().unwrap();
    let listed = nodes.iter().find(|n| n["id"] == JOB).unwrap();
    assert_eq!(listed["role"], "job", "{placement}");
    assert_eq!(listed["apps"], 0, "{placement}");
    assert!(nodes
        .iter()
        .filter(|n| n["id"] != JOB)
        .all(|n| n["role"] == "replica"));
    let status = ops.ops().status().await.map(wire).unwrap();
    let record = status["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == JOB)
        .unwrap();
    assert_eq!(record["role"], "job", "{status}");

    let metrics = ops.ops().metrics().await.unwrap();
    assert!(metrics.contains("calimero_cluster_nodes 3\n"), "{metrics}");
    assert!(
        metrics.contains("calimero_cluster_job_nodes 1\n"),
        "{metrics}"
    );

    // The cluster key goes to no job node.
    let e = alice
        .transport()
        .json(
            calimero_client::Request::new(Method::POST, "internal/cluster-key").json(
                serde_json::to_value(Request {
                    cluster: c.configs[0].cluster.clone(),
                    node: JOB,
                    token: hex::encode([1_u8; 32]),
                })
                .unwrap(),
            ),
        )
        .await
        .unwrap_err();
    assert!(e.to_string().contains("never holds the cluster key"), "{e}");

    // Removing it changes no voter and no app.
    let voters = status_voters(&ops).await;
    let _ = ops
        .ops()
        .remove_node(&JOB.to_string(), Some(&common::operator_sign))
        .await
        .unwrap();
    assert!(leader
        .node
        .sys(|s| s.nodes.len() == 3 && s.removed_nodes.contains(&JOB)));
    assert_eq!(status_voters(&ops).await, voters);
    assert_eq!(replicas(&c, &first), vec![1, 2, 3]);
}

async fn status_voters(ops: &Client) -> Vec<u64> {
    let v: Value = ops.ops().status().await.map(wire).unwrap();
    let mut voters: Vec<u64> = serde_json::from_value(v["system"]["voters"].clone()).unwrap();
    voters.sort_unstable();
    voters
}

/// A bundle needs a majority of the nodes that hold things: a registered job
/// node that is not there does not count against it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_bundles_majority_counts_only_replica_nodes() {
    let mut c = Cluster::start(3).await;
    c.register_job_node(JOB, "http://127.0.0.1:1/").await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    c.stop(2).await;
    // 2 of 3 replicas hold it; counting the job node it would be 2 of 4.
    let bundle = alice
        .packages()
        .bundle_put(common::component("kv"))
        .await
        .map(|b| b.hash)
        .unwrap();
    assert!(!bundle.is_empty());
}

/// The leader attests a job node (keys, approved release) and asks its load over the key it bound.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_leader_attests_a_job_node_and_asks_its_load() {
    let c = Cluster::start_with(3, |cfg| cfg.tee_release = Some("1.0.0".into())).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let app = create(&alice, "kv").await;
    let node = &c.nodes[0].as_ref().unwrap().node;
    let record = node.sys(|s| s.apps[&app].clone());

    let job = c
        .start_job_node(JOB, |cfg| cfg.tee_release = Some("1.0.0".into()))
        .await;
    c.register_job_node(JOB, &job.url).await;
    let peer = node.attest_job(JOB, &record).await.unwrap();
    assert_eq!(peer.release, "1.0.0");
    assert_eq!(peer.job_key, job.running.job_key);
    assert_eq!(node.job_load(&peer).await.unwrap(), (0, 2));

    // A verified answer is kept: it still holds with the job node gone.
    job.running.stop().await;
    assert_eq!(
        node.attest_job(JOB, &record).await.unwrap().job_key,
        peer.job_key
    );

    // A release not approved for the app is refused.
    let other = c
        .start_job_node(JOB + 1, |cfg| cfg.tee_release = Some("2.0.0".into()))
        .await;
    c.register_job_node(JOB + 1, &other.url).await;
    let e = node.attest_job(JOB + 1, &record).await.unwrap_err();
    assert!(
        e.to_string().contains("is not approved for this app"),
        "{e}"
    );

    // A cluster node has no job key to attest.
    let nonce = hex::encode([5_u8; 32]);
    let status = reqwest::get(format!(
        "{}attestation?nonce={nonce}&purpose=jobs",
        c.urls[0]
    ))
    .await
    .unwrap()
    .status();
    assert_eq!(status, 404);

    // A replica is no job node, and neither is an unknown id.
    for id in [1, 9] {
        let e = node.attest_job(id, &record).await.unwrap_err();
        assert!(e.to_string().contains("not a registered job node"), "{e}");
    }
}

/// A node that answers with a job key its quote does not bind is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_job_node_whose_quote_binds_another_job_key_is_refused() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let app = create(&alice, "kv").await;
    let node = &c.nodes[0].as_ref().unwrap().node;
    let record = node.sys(|s| s.apps[&app].clone());
    let job = c.start_job_node(JOB + 1, |_| {}).await;

    // Relays the job node's document with another job key put in.
    let real = job.url.clone();
    let relay = axum::Router::new().route(
        "/attestation",
        get(move |RawQuery(q): RawQuery| {
            let url = format!("{real}attestation?{}", q.unwrap_or_default());
            async move {
                let mut doc: Value = reqwest::get(url).await.unwrap().json().await.unwrap();
                doc["jobKey"] = json!(hex::encode([9_u8; 32]));
                axum::Json(doc)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, relay).await });
    c.register_job_node(JOB + 1, &addr).await;
    let e = node.attest_job(JOB + 1, &record).await.unwrap_err();
    assert!(e.to_string().contains("does not bind"), "{e}");
    server.abort();
}

/// A job node that takes work from another cluster is not attested, whatever its quote says.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_job_node_started_for_another_cluster_is_not_attested() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let app = create(&alice, "kv").await;
    let node = &c.nodes[0].as_ref().unwrap().node;
    let record = node.sys(|s| s.apps[&app].clone());
    let theirs = calimero_node::backup::cluster_identity_key(&[9; 32]);
    let job = c
        .start_job_node(JOB + 3, |cfg| {
            cfg.cluster_identity = theirs.verifying_key().to_bytes();
        })
        .await;
    c.register_job_node(JOB + 3, &job.url).await;
    let e = node.attest_job(JOB + 3, &record).await.unwrap_err();
    assert!(e.to_string().contains("another cluster"), "{e}");
    // One that takes this cluster's work is.
    let ours = c.start_job_node(JOB + 4, |_| {}).await;
    c.register_job_node(JOB + 4, &ours.url).await;
    assert!(node.attest_job(JOB + 4, &record).await.is_ok());
}

/// What a job node answers is read only up to a cap.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_job_nodes_answer_past_the_cap_is_refused() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let app = create(&alice, "kv").await;
    let node = &c.nodes[0].as_ref().unwrap().node;
    let record = node.sys(|s| s.apps[&app].clone());
    let flood = axum::Router::new().route("/attestation", get(|| async { "a".repeat(1 << 20) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, flood).await });
    c.register_job_node(JOB + 2, &addr).await;
    let e = node.attest_job(JOB + 2, &record).await.unwrap_err();
    assert!(e.to_string().contains("larger than"), "{e}");
    server.abort();
}

fn nonce_query(purpose: Option<&str>) -> String {
    let purpose = purpose.map(|p| format!("&purpose={p}")).unwrap_or_default();
    format!("attestation?nonce={}{purpose}", hex::encode([5_u8; 32]))
}

/// A job node attests only its job key, so no channel that attests a transport
/// key (key delivery, peer posts, Raft, blob pushes) opens to it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_job_node_attests_only_its_job_key_so_no_cluster_channel_opens_to_it() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let app = create(&alice, "kv").await;
    let node = &c.nodes[0].as_ref().unwrap().node;
    let record = node.sys(|s| s.apps[&app].clone());
    let job = c.start_job_node(JOB, |_| {}).await;
    c.register_job_node(JOB, &job.url).await;

    for purpose in [None, Some("receipts"), Some("transport")] {
        let status = reqwest::get(format!("{}{}", job.url, nonce_query(purpose)))
            .await
            .unwrap()
            .status();
        assert_eq!(status, 404, "{purpose:?}");
    }
    let jobs = reqwest::get(format!("{}{}", job.url, nonce_query(Some("jobs"))))
        .await
        .unwrap();
    assert_eq!(jobs.status(), 200);

    let http = reqwest::Client::new();
    let e = calimero_node::cluster_key::deliver(
        &c.configs[0],
        &http,
        &job.url,
        &hex::encode([1_u8; 32]),
        node.cluster_key(),
    )
    .await
    .unwrap_err();
    assert!(format!("{e:#}").contains("404"), "{e:#}");
    // `/ops/placement` asks every node for its release: no secret goes to this one.
    let e = node
        .peer_post(&job.url, "internal/system/index", &json!({}))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("not a node of this cluster"), "{e}");

    // Its load is still asked over the key its jobs attestation bound.
    let peer = node.attest_job(JOB, &record).await.unwrap();
    assert_eq!(node.job_load(&peer).await.unwrap(), (0, 2));
}

/// Nothing carrying the cluster secret goes to an address the directory does not
/// list as a replica, a ceremony's holder included.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_cluster_secret_goes_only_to_replica_addresses() {
    let c = Cluster::start(3).await;
    let job = c.start_job_node(JOB, |_| {}).await;
    c.register_job_node(JOB, &job.url).await;
    let node = &c.nodes[0].as_ref().unwrap().node;
    for addr in [job.url.as_str(), "http://127.0.0.1:1/"] {
        let e = node
            .peer_post(addr, "internal/system/index", &json!({}))
            .await
            .unwrap_err();
        assert!(
            e.to_string().contains("not a node of this cluster"),
            "{addr}: {e}"
        );
    }
    // A replica is reached (a follower may still answer that it does not lead).
    if let Err(e) = node
        .peer_post(&c.urls[1], "internal/system/index", &json!({}))
        .await
    {
        assert!(!e.to_string().contains("not a node of this cluster"), "{e}");
    }

    // A ceremony said to be begun on the job node, or on no node, is unknown.
    for id in [format!("{JOB}.x"), "9.x".to_owned()] {
        let e = c
            .client(0)
            .auth()
            .login_finish(
                &id,
                typed(json!({
                    "credentialId": "a", "authenticatorData": "a",
                    "clientDataJson": "a", "signature": "a" })),
            )
            .await
            .unwrap_err();
        assert_eq!(e.code(), Some("UNKNOWN_CHALLENGE"), "{id}: {e}");
    }
}

/// What `attest_job` kept for a node is dropped when the node is removed and
/// registered again, and when its channel fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_cached_attestation_does_not_outlive_the_registration_or_the_channel() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let app = create(&alice, "kv").await;
    let node = &c.nodes[0].as_ref().unwrap().node;
    let record = node.sys(|s| s.apps[&app].clone());
    let job = c.start_job_node(JOB, |_| {}).await;
    c.register_job_node(JOB, &job.url).await;
    let first = node.attest_job(JOB, &record).await.unwrap();

    // Removed, restarted at the same address with new keys, registered again.
    let leader = (0..3)
        .find(|i| c.nodes[*i].as_ref().unwrap().node.system.status().is_leader)
        .unwrap();
    let lead = c.client(leader);
    lead.use_api_key("ops-secret").await;
    let _ = lead
        .ops()
        .remove_node(&JOB.to_string(), Some(&common::operator_sign))
        .await
        .unwrap();
    let port = job
        .url
        .trim_end_matches('/')
        .rsplit(':')
        .next()
        .unwrap()
        .to_owned();
    job.running.stop().await;
    let again = c
        .start_job_node(JOB, |cfg| {
            cfg.listen = format!("127.0.0.1:{port}").parse().unwrap()
        })
        .await;
    assert_eq!(again.url, job.url);
    c.register_job_node(JOB, &again.url).await;
    let second = node.attest_job(JOB, &record).await.unwrap();
    assert_eq!(second.job_key, again.running.job_key);
    assert_ne!(second.job_key, first.job_key);

    // A failed channel drops it, so the next call attests afresh and fails too.
    again.running.stop().await;
    assert!(node.job_load(&second).await.is_err());
    assert!(node.attest_job(JOB, &record).await.is_err());
}

/// A node's role is fixed when it is registered: a voter cannot be made a job
/// node, and a job node cannot be made a replica, by registering it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_registered_nodes_role_cannot_be_changed_by_registering_it_again() {
    let c = Cluster::start(3).await;
    let ops = ops(&c).await;
    let e = ops
        .ops()
        .register_node(
            2,
            &c.urls[1],
            None,
            None,
            Some(OpsRegisterNodeRequestRole::Job),
            Some(&common::operator_sign),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NODE_HOLDS_CLUSTER"), "{e}");
    let job = c.start_job_node(JOB, |_| {}).await;
    c.register_job_node(JOB, &job.url).await;
    let e = ops
        .ops()
        .register_node(
            JOB,
            &job.url,
            None,
            None,
            None,
            Some(&common::operator_sign),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("REJECTED"), "{e}");
    let leader = c.nodes[0].as_ref().unwrap();
    assert!(leader.node.sys(|s| s.job_nodes().any(|n| n.id == JOB)));
    assert_eq!(leader.node.system.status().voters.len(), 3);
}

/// Respelling a replica's address leaves the cluster working: Raft goes by id,
/// to the address the directory holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_replicas_address_may_be_respelled_without_stopping_the_cluster() {
    let c = Cluster::start(3).await;
    let ops = ops(&c).await;
    let respelled = c.urls[1]
        .trim_end_matches('/')
        .to_uppercase()
        .replace("HTTP", "http");
    let _ = ops
        .ops()
        .register_node(
            2,
            &respelled,
            None,
            None,
            None,
            Some(&common::operator_sign),
        )
        .await
        .unwrap();
    let node = &c.nodes[0].as_ref().unwrap().node;
    for _ in 0..100 {
        if node.sys(|s| s.nodes[&2].addr == respelled) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let app = create(&alice, "kv").await;
    for i in 0..3 {
        let _ = alice
            .apps()
            .call(
                &app,
                "set",
                Some(json!({ "key": "k", "value": i.to_string() })),
                None,
                None,
            )
            .await
            .unwrap();
    }
    // The node is reached by id, and by any spelling of what the directory holds.
    node.peer_post(&c.urls[1], "internal/system/index", &json!({}))
        .await
        .map(drop)
        .or_else(|e| {
            (!e.to_string().contains("not a node of this cluster"))
                .then_some(())
                .ok_or(e)
        })
        .unwrap();
}

/// An address belongs to one node in any spelling, and is an http(s) URL.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_address_belongs_to_one_node_in_any_spelling() {
    let c = Cluster::start(3).await;
    let ops = ops(&c).await;
    let bare = c.urls[1].trim_end_matches('/').to_owned();
    for (role, addr) in [
        ("job", c.urls[1].clone()),
        ("job", bare.clone()),
        ("replica", bare),
    ] {
        let e = ops
            .ops()
            .register_node(
                9,
                &addr,
                None,
                None,
                Some(typed(json!(role))),
                Some(&common::operator_sign),
            )
            .await
            .unwrap_err();
        assert_eq!(e.code(), Some("ADDRESS_TAKEN"), "{role} {addr}: {e}");
    }

    for addr in ["ftp://host/", "host:2528", "not a url"] {
        let e = ops
            .ops()
            .register_node(9, addr, None, None, None, Some(&common::operator_sign))
            .await
            .unwrap_err();
        assert_eq!(e.code(), Some("BAD_ADDRESS"), "{addr}: {e}");
    }

    let job = c.start_job_node(JOB, |_| {}).await;
    c.register_job_node(JOB, &job.url).await;
    let node = &c.nodes[0].as_ref().unwrap().node;
    for spelling in [
        job.url.trim_end_matches('/').to_owned(),
        job.url.to_uppercase(),
    ] {
        let e = node
            .peer_post(&spelling, "internal/system/index", &json!({}))
            .await
            .unwrap_err();
        assert!(
            e.to_string().contains("not a node of this cluster"),
            "{spelling}: {e}"
        );
    }
}

/// No job record exists while any replica node, voter or not, runs an older release.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_job_node_is_registered_only_once_every_replica_node_runs_this_release() {
    let c = Cluster::start_with(3, |cfg| {
        cfg.tee_release = Some(if cfg.node_id == 3 { "1.0.0" } else { "2.0.0" }.into());
    })
    .await;
    let ops = ops(&c).await;
    let e = ops
        .ops()
        .register_node(
            JOB,
            "http://127.0.0.1:1/",
            None,
            None,
            Some(OpsRegisterNodeRequestRole::Job),
            Some(&common::operator_sign),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CLUSTER_NOT_UPGRADED"), "{e}");
    assert!(c.nodes[0]
        .as_ref()
        .unwrap()
        .node
        .sys(|s| s.job_nodes().next().is_none()));
}

/// An older replica that is not a voter blocks registration too.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_older_replica_that_is_not_a_voter_blocks_job_registration() {
    let c = Cluster::start(3).await;
    // A directory entry that attests as node 1 but names an older release.
    let real = c.urls[0].clone();
    let relay = axum::Router::new().route(
        "/attestation",
        get(move |RawQuery(q): RawQuery| {
            let url = format!("{real}attestation?{}", q.unwrap_or_default());
            async move {
                let mut doc: Value = reqwest::get(url).await.unwrap().json().await.unwrap();
                doc["release"]["version"] = json!("1.0.0");
                axum::Json(doc)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, relay).await });
    let ops = ops(&c).await;
    let _ = ops
        .ops()
        .register_node(9, &addr, None, None, None, Some(&common::operator_sign))
        .await
        .unwrap();
    let st = c.nodes[0].as_ref().unwrap().node.system.status();
    assert!(!st.voters.contains(&9), "{st:?}");
    let e = ops
        .ops()
        .register_node(
            JOB,
            "http://127.0.0.1:1/",
            None,
            None,
            Some(OpsRegisterNodeRequestRole::Job),
            Some(&common::operator_sign),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CLUSTER_NOT_UPGRADED"), "{e}");
    assert!(e.to_string().contains("node 9"), "{e}");
    server.abort();
}

/// A job node registered as a replica is never admitted: it does not attest
/// as a cluster node, so it never becomes a learner.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_job_node_registered_as_a_replica_is_never_admitted() {
    let c = Cluster::start(3).await;
    let job = c.start_job_node(JOB, |_| {}).await;
    let ops = ops(&c).await;
    let _ = ops
        .ops()
        .register_node(
            JOB,
            &job.url,
            None,
            None,
            None,
            Some(&common::operator_sign),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;
    let status = c.nodes[0].as_ref().unwrap().node.system.status();
    assert!(
        !status.voters.contains(&JOB) && !status.learners.contains(&JOB),
        "{status:?}"
    );
}

/// A cached attestation is not answered from a directory that could not be
/// brought up to date.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_job_node_is_not_vouched_for_by_a_stale_directory() {
    let mut c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let app = create(&alice, "kv").await;
    let job = c.start_job_node(JOB, |_| {}).await;
    c.register_job_node(JOB, &job.url).await;
    let node = c.nodes[0].as_ref().unwrap().node.clone();
    let record = node.sys(|s| s.apps[&app].clone());
    let _cached = node.attest_job(JOB, &record).await.unwrap();
    c.stop(1).await;
    c.stop(2).await;
    assert!(node.attest_job(JOB, &record).await.is_err());
}

/// A job node's attestation is for its own id and cluster: another id
/// registered at its address is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_job_node_attested_under_another_id_is_refused() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let app = create(&alice, "kv").await;
    let node = &c.nodes[0].as_ref().unwrap().node;
    let record = node.sys(|s| s.apps[&app].clone());
    let job = c.start_job_node(JOB, |_| {}).await;
    c.register_job_node(JOB + 1, &job.url).await;
    let e = node.attest_job(JOB + 1, &record).await.unwrap_err();
    assert!(e.to_string().contains("attests as node 4"), "{e}");
    let other = c
        .start_job_node(JOB + 2, |cfg| cfg.cluster = "elsewhere".into())
        .await;
    c.register_job_node(JOB + 2, &other.url).await;
    let e = node.attest_job(JOB + 2, &record).await.unwrap_err();
    assert!(e.to_string().contains("cluster elsewhere"), "{e}");
}

/// An address that counts every request it gets, registered as a job node.
async fn spy() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&hits);
    let app = axum::Router::new().fallback(move || {
        let _ = seen.fetch_add(1, Ordering::SeqCst);
        async { axum::http::StatusCode::NOT_FOUND }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (addr, hits, server)
}

/// A sealed file, uploaded or written by the app, is pushed to the app's
/// replicas only: a job node's address gets no request.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_sealed_file_is_never_sent_to_a_job_node() {
    let c = Cluster::start_with(3, |cfg| {
        cfg.budget = Some(calimero_app_host::Budget {
            fuel: u64::MAX,
            wall_clock: Duration::from_secs(30),
            ..calimero_app_host::Budget::call()
        });
    })
    .await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice
        .packages()
        .bundle_put(common::component("files"))
        .await
        .map(|b| b.hash)
        .unwrap();
    let app = alice
        .apps()
        .create(typed(json!({ "name": "files", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let (addr, hits, server) = spy().await;
    c.register_job_node(JOB, &addr).await;

    let bytes: Vec<u8> = (0..300_000_u32).map(|i| (i % 251) as u8).collect();
    let up = alice.upload_bytes(&app, bytes).await.unwrap();
    assert!(up["id"].is_string(), "{up}");
    let put = alice
        .apps()
        .call(
            &app,
            "putFile",
            Some(json!({ "len": "200000", "seed": "7" })),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(put.result["id"].is_string(), "{:?}", put.result);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(hits.load(Ordering::SeqCst), 0, "the job node was contacted");
    server.abort();
}

/// A job node on a release past an operator's deadline takes no work, as a replica takes no new app.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_job_node_on_a_retired_release_is_not_attested() {
    let c = Cluster::start_with(3, |cfg| cfg.tee_release = Some("1.0.0".into())).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let app = create(&alice, "kv").await;
    let mine = alice.personal_org().await.unwrap();
    let _ok = alice
        .orgs()
        .release_approval(&mine, "2.0.0", None, Some(&mut pk))
        .await
        .unwrap();
    let node = &c.nodes[0].as_ref().unwrap().node;
    let record = node.sys(|s| s.apps[&app].clone());
    let old = c
        .start_job_node(JOB, |cfg| cfg.tee_release = Some("1.0.0".into()))
        .await;
    let new = c
        .start_job_node(JOB + 1, |cfg| cfg.tee_release = Some("2.0.0".into()))
        .await;
    c.register_job_node(JOB, &old.url).await;
    c.register_job_node(JOB + 1, &new.url).await;
    assert!(node.attest_job(JOB, &record).await.is_ok());

    let _ok = common::ops_signed(
        &ops(&c).await,
        "ops/releases/2.0.0/deadline",
        json!({ "after": "2001-01-01T00:00:00Z" }),
    )
    .await
    .unwrap();
    let e = node.attest_job(JOB, &record).await.unwrap_err();
    assert!(e.to_string().contains("retired"), "{e}");
    assert!(node.attest_job(JOB + 1, &record).await.is_ok());
}

/// A replica attests the cluster identity it holds with its transport key; a job node
/// refuses the purpose, so a quote naming this cluster never comes from one.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_replica_attests_as_the_orchestrator_and_a_job_node_refuses() {
    let c = Cluster::start(3).await;
    let node = &c.nodes[0].as_ref().unwrap().node;
    let identity = node.cluster_identity();
    let policy = calimero_client::attest::Policy {
        allow_mock: true,
        ..Default::default()
    };
    let nonce = [5_u8; 32];
    let ask = |url: String, purpose: &'static str| async move {
        reqwest::get(format!(
            "{url}attestation?nonce={}&purpose={purpose}",
            hex::encode(nonce)
        ))
        .await
        .unwrap()
    };

    let doc: calimero_client::attest::AttestationDoc = ask(c.urls[0].clone(), "orchestrator")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(doc.cluster_identity, Some(hex::encode(identity)));
    let key = calimero_client::attest::verify_orchestrator(&doc, &nonce, &identity, &policy)
        .await
        .unwrap();
    assert_eq!(hex::encode(key), doc.transport_key);
    // Another cluster's identity, and another nonce, refuse it.
    let theirs = calimero_node::backup::cluster_identity_key(&[9; 32]).verifying_key();
    let wrong =
        calimero_client::attest::verify_orchestrator(&doc, &nonce, &theirs.to_bytes(), &policy)
            .await;
    assert!(wrong.is_err());
    assert!(
        calimero_client::attest::verify_orchestrator(&doc, &[6; 32], &identity, &policy)
            .await
            .is_err()
    );
    // The plain transport attestation of the same node is not an orchestrator one.
    let plain: calimero_client::attest::AttestationDoc = reqwest::get(format!(
        "{}attestation?nonce={}",
        c.urls[0],
        hex::encode(nonce)
    ))
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert!(
        calimero_client::attest::verify_orchestrator(&plain, &nonce, &identity, &policy)
            .await
            .is_err()
    );

    let job = c.start_job_node(JOB, |_| {}).await;
    c.register_job_node(JOB, &job.url).await;
    let refused = ask(job.url.clone(), "orchestrator").await;
    assert_eq!(refused.status(), 404);
    let body: Value = refused.json().await.unwrap();
    assert_eq!(body["error"], "NOT_FOUND");
}

async fn verdict(client: &Client, id: u64) -> Result<Value, Box<calimero_client::Error>> {
    let answer = client
        .ops()
        .job_node_attestation(&id.to_string())
        .await
        .map_err(Box::new)?;
    Ok(wire(answer))
}

/// The operator reads whether the cluster accepts a job node: admitted, refused with the
/// reason, or unreachable, and a node that is no job node is a 404.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_ops_route_tells_admitted_from_refused_from_unreachable() {
    let c = Cluster::start_with(3, |cfg| cfg.tee_release = Some("1.0.0".into())).await;
    let ops = ops(&c).await;

    let ours = c
        .start_job_node(JOB, |cfg| cfg.tee_release = Some("1.0.0".into()))
        .await;
    c.register_job_node(JOB, &ours.url).await;
    assert_eq!(
        verdict(&ops, JOB).await.unwrap(),
        json!({ "state": "admitted", "node": JOB, "release": "1.0.0" })
    );

    // Answers, but takes work from another cluster.
    let theirs = calimero_node::backup::cluster_identity_key(&[9; 32]);
    let other = c
        .start_job_node(JOB + 1, |cfg| {
            cfg.tee_release = Some("1.0.0".into());
            cfg.cluster_identity = theirs.verifying_key().to_bytes();
        })
        .await;
    c.register_job_node(JOB + 1, &other.url).await;
    let refused = verdict(&ops, JOB + 1).await.unwrap();
    assert_eq!(refused["state"], "refused", "{refused}");
    assert_eq!(refused["node"], JOB + 1);
    assert!(
        refused["reason"]
            .as_str()
            .unwrap()
            .contains("another cluster"),
        "{refused}"
    );

    // Nothing listening.
    c.register_job_node(JOB + 2, "http://127.0.0.1:1/").await;
    let down = verdict(&ops, JOB + 2).await.unwrap();
    assert_eq!(down["state"], "unreachable", "{down}");
    assert_eq!(down["node"], JOB + 2);
    assert!(!down["reason"].as_str().unwrap().is_empty(), "{down}");

    // Answers with something that is not an attestation.
    let junk = axum::Router::new().route("/attestation", get(|| async { "not json" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, junk).await });
    c.register_job_node(JOB + 3, &addr).await;
    let unreadable = verdict(&ops, JOB + 3).await.unwrap();
    assert_eq!(unreadable["state"], "unreachable", "{unreadable}");
    server.abort();

    // A replica and an unknown id are no job nodes.
    for id in [1, 99] {
        let e = verdict(&ops, id).await.unwrap_err();
        assert_eq!(e.code(), Some("NOT_FOUND"), "{id}: {e}");
    }
}

/// A job node whose real quote the peer policy trusts no image for is refused: a final
/// verdict, not a transient one.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_ops_route_refuses_a_job_node_the_peer_policy_does_not_trust() {
    let c = Cluster::start_with(3, |cfg| cfg.tee_release = Some("1.0.0".into())).await;
    let ops = ops(&c).await;
    let job = c
        .start_job_node(JOB, |cfg| cfg.tee_release = Some("1.0.0".into()))
        .await;

    // Relays the job node's document with its quote swapped for a real-looking one.
    let real = job.url.clone();
    let relay = axum::Router::new().route(
        "/attestation",
        get(move |RawQuery(q): RawQuery| {
            let url = format!("{real}attestation?{}", q.unwrap_or_default());
            async move {
                let mut doc: Value = reqwest::get(url).await.unwrap().json().await.unwrap();
                doc["quote"] = json!("cmVhbA==");
                doc["mock"] = json!(false);
                axum::Json(doc)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, relay).await });
    c.register_job_node(JOB, &addr).await;
    let refused = verdict(&ops, JOB).await.unwrap();
    assert_eq!(refused["state"], "refused", "{refused}");
    assert!(
        refused["reason"]
            .as_str()
            .unwrap()
            .contains("trusts no TD image"),
        "{refused}"
    );
    server.abort();
}

/// A job node on a release the cluster has retired is refused, naming the release.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_ops_route_refuses_a_job_node_on_a_retired_release() {
    let c = Cluster::start_with(3, |cfg| cfg.tee_release = Some("1.0.0".into())).await;
    let ops = ops(&c).await;
    let job = c
        .start_job_node(JOB, |cfg| cfg.tee_release = Some("1.0.0".into()))
        .await;
    c.register_job_node(JOB, &job.url).await;
    assert_eq!(verdict(&ops, JOB).await.unwrap()["state"], "admitted");

    let _ok = common::ops_signed(
        &ops,
        "ops/releases/2.0.0/deadline",
        json!({ "after": "2001-01-01T00:00:00Z" }),
    )
    .await
    .unwrap();
    let refused = verdict(&ops, JOB).await.unwrap();
    assert_eq!(refused["state"], "refused", "{refused}");
    assert!(
        refused["reason"].as_str().unwrap().contains("retired"),
        "{refused}"
    );
}

/// Like every ops route, the verdict needs the ops token.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_ops_route_needs_the_ops_token() {
    let c = Cluster::start(3).await;
    let job = c.start_job_node(JOB, |_| {}).await;
    c.register_job_node(JOB, &job.url).await;
    let e = verdict(&c.client(0), JOB).await.unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"), "{e}");
}
