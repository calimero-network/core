//! A three-node cluster in one process, driven only through the client, over
//! sealed channels, with the real v2 fixture components (`apps/v2-fixtures/*`,
//! built on first use by `common::component`).

mod common;

use std::sync::Arc;
use std::time::Duration;

use calimero_client::types::{AppCreateRequest, OrgRole, TeamJoin, ViewsRequestConsistency};
use calimero_client::{Client, Policy, Request};
use calimero_identity::testing::SoftAuthenticator;
use common::{signed_up, typed, wire, Cluster, ORIGIN};
use futures_util::StreamExt;
use reqwest::Method;
use serde_json::{json, Value};

fn kv_wasm() -> Vec<u8> {
    common::component("kv")
}

fn wasm(name: &str) -> Vec<u8> {
    common::component(name)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn end_to_end() {
    let mut c = Cluster::start(3).await;

    // Sign up with a passkey; sign in again from another node.
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let alice_id = alice.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let alice_on_2 = c.client(1);
    let s = alice_on_2.sign_in(&mut alice_pk).await.unwrap();
    assert_eq!(s.account_id, alice_id);

    // Deploy kv-store and call it.
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
    let r = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "a", "value": "1" })),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(r.index > 0);
    assert_eq!(r.events.len(), 1, "{:?}", r.events);

    // Strong read anywhere; stale read with read-your-writes on another node.
    let v = alice
        .apps()
        .view(
            &app,
            "get",
            Some(json!({ "key": "a" })),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap();
    assert_eq!(v.result, json!("1"));
    let v = alice_on_2
        .apps()
        .view(
            &app,
            "get",
            Some(json!({ "key": "a" })),
            Some(ViewsRequestConsistency::Stale),
            Some(r.index),
        )
        .await
        .unwrap();
    assert_eq!(v.result, json!("1"));

    // An idempotency key makes a retry answer once.
    let r1 = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "b", "value": "x" })),
            Some("op-1"),
            None,
        )
        .await
        .unwrap();
    let r2 = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "b", "value": "y" })),
            Some("op-1"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(r1.index, r2.index);
    let v = alice
        .apps()
        .view(
            &app,
            "get",
            Some(json!({ "key": "b" })),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap();
    assert_eq!(v.result, json!("x"));

    // The app's own error comes back as APP_ERROR.
    let e = alice
        .apps()
        .call(
            &app,
            "getResult",
            Some(json!({ "key": "missing" })),
            None,
            None,
        )
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
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "z", "value": "1" })),
            None,
            None,
        )
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
            .apps()
            .call(
                &app,
                "set",
                Some(json!({ "key": "c", "value": "after" })),
                Some("op-failover"),
                None,
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
        .apps()
        .view(
            &app,
            "get",
            Some(json!({ "key": "a" })),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap();
    assert_eq!(v.result, json!("1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn confirm_methods_need_a_fresh_passkey() {
    let c = Cluster::start(3).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let app = alice
        .transport()
        .json(
            Request::new(Method::POST, "apps")
                .json(json!({ "name": "kv", "bundle": &bundle, "confirmMethods": ["clear"] })),
        )
        .await
        .map(|v| v["appId"].as_str().unwrap_or_default().to_owned())
        .unwrap();
    let _r = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "a", "value": "1" })),
            None,
            None,
        )
        .await
        .unwrap();
    let e = alice
        .apps()
        .call(&app, "clear", Some(json!({})), None, None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"));
    let _r = alice
        .apps()
        .call(&app, "clear", Some(json!({})), None, Some(&mut pk))
        .await
        .unwrap();
    let v = alice
        .apps()
        .view(
            &app,
            "len",
            Some(json!({})),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap();
    assert_eq!(v.result, json!(0));

    // A passkey of another account cannot approve.
    let (_bob, mut bob_pk) = signed_up(&c, 1, "bob").await;
    let e = alice
        .apps()
        .call(&app, "clear", Some(json!({})), None, Some(&mut bob_pk))
        .await
        .unwrap_err();
    assert!(
        matches!(e.code(), Some("UNAUTHENTICATED" | "FORBIDDEN")),
        "{e}"
    );

    // An API key without allowConfirm cannot call it at all.
    let (_id, key) = alice
        .auth()
        .apikey_create(typed(json!({ "label": "agent" })), Some(&mut pk))
        .await
        .map(|k| (k.key_id, k.key))
        .unwrap();
    let agent = c.client(2);
    agent.use_api_key(&key).await;
    let _r = agent
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
    let e = agent
        .apps()
        .call(&app, "clear", Some(json!({})), None, None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn organizations_gate_apps_and_api_keys_are_scoped() {
    let c = Cluster::start(3).await;
    let (alice, mut apk) = signed_up(&c, 0, "alice").await;
    let (bob, _bpk) = signed_up(&c, 1, "bob").await;
    let (carol, _cpk) = signed_up(&c, 2, "carol").await;
    let bob_id = bob.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();

    let org = alice.orgs().create("Acme", None).await.map(wire).unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let team = alice
        .orgs()
        .team_create(&org, "eng", TeamJoin::Invite, None)
        .await
        .map(wire)
        .unwrap()["teamId"]
        .as_str()
        .unwrap()
        .to_owned();
    let invite = alice
        .orgs()
        .invite_create(
            &org,
            typed(json!({ "role": "member", "teams": [team] })),
            Some(&mut apk),
        )
        .await
        .map(wire)
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = bob.orgs().invite_redeem(&invite).await.unwrap();
    // The invite was single-use.
    let e = carol.orgs().invite_redeem(&invite).await.unwrap_err();
    assert!(e.code().is_some());

    let o: Value = bob.orgs().get(&org).await.map(wire).unwrap();
    assert_eq!(o["members"][&bob_id], json!("member"));
    assert_eq!(o["teams"][&team]["members"][&bob_id], json!(false));

    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let app = alice
        .apps()
        .create(typed(
            json!({ "name": "team-kv", "bundle": &bundle, "org": org }),
        ))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let _r = bob
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "from", "value": "bob" })),
            None,
            None,
        )
        .await
        .unwrap();
    let e = carol
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "from", "value": "carol" })),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NOT_FOUND"));

    // Removing bob from the org cuts him off at once.
    let _r = alice
        .orgs()
        .remove_member(&org, &bob_id, Some(&mut apk))
        .await
        .map(wire)
        .unwrap();
    for _ in 0..20 {
        if bob
            .apps()
            .call(
                &app,
                "set",
                Some(json!({ "key": "x", "value": "y" })),
                None,
                None,
            )
            .await
            .is_err()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let e = bob
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "x", "value": "y" })),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NOT_FOUND"));

    // A read-only API key reads but does not write.
    let (_id, key) = alice
        .auth()
        .apikey_create(
            typed(json!({ "readOnly": true, "apps": [app] })),
            Some(&mut apk),
        )
        .await
        .map(|k| (k.key_id, k.key))
        .unwrap();
    let agent = c.client(2);
    agent.use_api_key(&key).await;
    let v = agent
        .apps()
        .view(
            &app,
            "get",
            Some(json!({ "key": "from" })),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap();
    assert_eq!(v.result, json!("bob"));
    let e = agent
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "a", "value": "b" })),
            None,
            None,
        )
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
    let e = strict.auth().login_begin(None).await.unwrap_err();
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
        .transport()
        .json(
            Request::new(Method::POST, "internal/system")
                .json(json!({ "op": "registerNode", "node": { "id": 9, "addr": "x" } })),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"));

    // The Ops API needs the operator credential and returns counts only.
    let e = client.ops().status().await.unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backups_are_encrypted_and_restorable() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
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
    let _r = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "secret", "value": "plaintext-value" })),
            None,
            None,
        )
        .await
        .unwrap();
    let leader = c.app_leader(&app).unwrap();
    let node = &c.nodes[leader].as_ref().unwrap().node;
    // The app's snapshot, and the system group's when this node leads it too.
    let written = node.backup().await.unwrap();
    assert!(written >= 1, "{written}");
    let dir = node.config.backup_dir.clone().unwrap();
    let file = std::fs::read_dir(dir.join(&app))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let raw = std::fs::read(&file).unwrap();
    assert!(!String::from_utf8_lossy(&raw).contains("plaintext-value"));
    let (_index, data) = calimero_node::backup::latest(&dir, node.cluster_key(), &app)
        .unwrap()
        .unwrap();
    // The app's rows are in it, opened with the storage key (the log's sealed
    // snapshot file).
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
    let hash = alice
        .packages()
        .bundle_put(wasm)
        .await
        .map(|b| b.hash)
        .unwrap();
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
    let app = alice
        .apps()
        .create(typed(json!({ "name": "big", "bundle": &hash })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let _out = alice
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn ops_moves_an_app_and_points_followers_at_the_leader() {
    let c = Cluster::start(4).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
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
    let replicas: Vec<u64> = serde_json::from_value(created["replicas"].clone()).unwrap();
    let _ = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "a", "value": "1" })),
            None,
            None,
        )
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
        .ops()
        .set_replicas(&app, typed(json!(new)), Some(&common::operator_sign))
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
        .ops()
        .set_replicas(&app, typed(json!(new)), Some(&common::operator_sign))
        .await
        .map(wire)
        .unwrap();
    let mut voters: Vec<u64> = serde_json::from_value(moved["status"]["voters"].clone()).unwrap();
    voters.sort_unstable();
    let mut want = new.clone();
    want.sort_unstable();
    assert_eq!(voters, want);
    let v = alice
        .apps()
        .view(
            &app,
            "get",
            Some(json!({ "key": "a" })),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap();
    assert_eq!(v.result, json!("1"));

    // The node the app moved off still serves it, by forwarding.
    let off = c.client(leader as usize - 1);
    off.use_session(alice.session().await.unwrap()).await;
    let v = off
        .apps()
        .view(
            &app,
            "get",
            Some(json!({ "key": "a" })),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap();
    assert_eq!(v.result, json!("1"));
    let _ = off
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "b", "value": "2" })),
            None,
            None,
        )
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_failed_init_is_reported_not_hidden() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    // The kv fixture's `init` takes no arguments: a string for them fails it.
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let err = alice
        .apps()
        .create(typed(
            json!({ "name": "broken", "bundle": &bundle, "initArgs": "not arguments" }),
        ))
        .await
        .map(|a| a.app_id)
        .unwrap_err();
    assert_eq!(err.code(), Some("INIT_FAILED"), "{err}");
    assert!(err.details().unwrap()["appId"].is_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn quotas_are_enforced_by_the_log() {
    let c = Cluster::start(3).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let ops = c.client(1);
    ops.use_api_key("ops-secret").await;
    let mut quotas = ops.ops().quotas_get().await.map(wire).unwrap();
    assert_eq!(quotas["maxAppsPerAccount"], json!(100));
    quotas["maxAppsPerAccount"] = json!(1);
    quotas["maxApiKeysPerAccount"] = json!(1);
    let _ok = ops
        .ops()
        .quotas_set(typed(quotas), Some(&common::operator_sign))
        .await
        .unwrap();

    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let _first = alice
        .apps()
        .create(typed(json!({ "name": "one", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let err = alice
        .apps()
        .create(typed(json!({ "name": "two", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap_err();
    assert_eq!(err.code(), Some("QUOTA_EXCEEDED"), "{err}");

    let _key = alice
        .auth()
        .apikey_create(typed(json!({})), Some(&mut pk))
        .await
        .map(|k| (k.key_id, k.key))
        .unwrap();
    let err = alice
        .auth()
        .apikey_create(typed(json!({})), Some(&mut pk))
        .await
        .map(|k| (k.key_id, k.key))
        .unwrap_err();
    assert_eq!(err.code(), Some("QUOTA_EXCEEDED"), "{err}");
}

async fn strong_view(cl: &Client, app: &str, method: &str, args: Value) -> Value {
    cl.apps()
        .view(
            app,
            method,
            Some(args),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap()
        .result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn apps_see_the_callers_organization_role_and_teams() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let str_of = |v: Value, k: &str| v[k].as_str().unwrap().to_owned();
    let org = str_of(
        alice.orgs().create("Acme", None).await.map(wire).unwrap(),
        "orgId",
    );
    let team = str_of(
        alice
            .orgs()
            .team_create(&org, "eng", TeamJoin::Invite, None)
            .await
            .map(wire)
            .unwrap(),
        "teamId",
    );
    let invite = str_of(
        alice
            .orgs()
            .invite_create(
                &org,
                typed(json!({ "role": "member", "teams": [team] })),
                Some(&mut a),
            )
            .await
            .map(wire)
            .unwrap(),
        "token",
    );
    let _r = bob.orgs().invite_redeem(&invite).await.unwrap();

    let bundle = alice
        .packages()
        .bundle_put(wasm("org-gated"))
        .await
        .map(|b| b.hash)
        .unwrap();
    let app = alice
        .apps()
        .create(typed(
            json!({ "name": "notes", "bundle": &bundle, "org": org }),
        ))
        .await
        .map(|a| a.app_id)
        .unwrap();

    // The app reads each caller's standing from the node.
    let me = strong_view(&alice, &app, "whoami", json!({})).await;
    assert_eq!(me["role"], "owner");
    assert_eq!(me["org"], json!(org));
    let me = strong_view(&bob, &app, "whoami", json!({ "candidates": [team] })).await;
    assert_eq!(me["role"], "member");
    assert_eq!(me["teams"], json!([team]));

    // Admins write the pinned note; members read it but cannot write it.
    let _ok = alice
        .apps()
        .call(
            &app,
            "setPinned",
            Some(json!({ "text": "hello" })),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(strong_view(&bob, &app, "pinned", json!({})).await, "hello");
    let err = bob
        .apps()
        .call(
            &app,
            "setPinned",
            Some(json!({ "text": "mine" })),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("APP_ERROR"), "{err}");

    // Team members write their team's note; alice (owner, who left the team
    // she created and led) cannot.
    let alice_id = alice.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ok = alice
        .orgs()
        .team_remove_member(&org, &team, &alice_id, Some(&mut a))
        .await
        .unwrap();
    let _ok = bob
        .apps()
        .call(
            &app,
            "setTeamNote",
            Some(json!({ "team": team, "text": "ship it" })),
            None,
            None,
        )
        .await
        .unwrap();
    let err = alice
        .apps()
        .call(
            &app,
            "setTeamNote",
            Some(json!({ "team": team, "text": "x" })),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("APP_ERROR"), "{err}");

    // Made without naming an organization, the app is in her personal one,
    // and sees her standing there alone.
    let solo = alice
        .apps()
        .create(typed(json!({ "name": "solo", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let home = json!(alice.personal_org().await.unwrap());
    assert_ne!(home, json!(org));
    let me = alice
        .apps()
        .view(
            &solo,
            "whoami",
            Some(json!({})),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap()
        .result;
    assert_eq!((&me["role"], &me["org"]), (&json!("owner"), &home), "{me}");
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
    let app = created["appId"].as_str().unwrap();
    let ctx = created["contextId"].as_str().unwrap();
    assert_eq!(ctx, hex::encode(calimero_identity::sha256(app.as_bytes())));
    let _ = alice
        .apps()
        .call(
            ctx,
            "set",
            Some(json!({ "key": "a", "value": "1" })),
            None,
            None,
        )
        .await
        .unwrap();
    let v = alice
        .apps()
        .view(
            app,
            "get",
            Some(json!({ "key": "a" })),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap();
    assert_eq!(v.result, json!("1"));
}

async fn set(cl: &Client, app: &str, v: &str) -> Result<(), Box<calimero_client::Error>> {
    cl.apps()
        .call(
            app,
            "set",
            Some(json!({ "key": "k", "value": v })),
            None,
            None,
        )
        .await
        .map(drop)
        .map_err(Box::new)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn teams_work_like_v1_subgroups() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, mut b) = signed_up(&c, 1, "bob").await;
    let s = |v: Value, k: &str| v[k].as_str().unwrap().to_owned();
    let org = s(
        alice.orgs().create("Docs", None).await.map(wire).unwrap(),
        "orgId",
    );
    let token = s(
        alice
            .orgs()
            .invite_create(&org, typed(json!({ "role": "member" })), Some(&mut a))
            .await
            .map(wire)
            .unwrap(),
        "token",
    );
    let _r = bob.orgs().invite_redeem(&token).await.unwrap();
    let bob_id = s(bob.auth().me().await.map(wire).unwrap(), "accountId");
    let team = |name: &str, join: &str, parent: Option<&str>| {
        let (name, join, parent) = (name.to_owned(), join.to_owned(), parent.map(str::to_owned));
        let (alice, org) = (&alice, &org);
        async move {
            s(
                alice
                    .orgs()
                    .team_create(org, &name, typed(json!(join)), parent.as_deref())
                    .await
                    .map(wire)
                    .unwrap(),
                "teamId",
            )
        }
    };
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let app_in = |team: String| {
        let (alice, org, bundle) = (&alice, &org, &bundle);
        async move {
            alice
                .apps().create(typed(json!({ "name": "folder", "bundle": bundle, "org": org, "access": { "kind": "team", "team": team } })))
                .await.map(|a| a.app_id)
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
        .orgs()
        .team_remove_member(&org, &open, &bob_id, Some(&mut a))
        .await
        .map(wire)
        .unwrap();
    let err = set(&bob, &open_app, "x").await.unwrap_err();
    assert_eq!(err.code(), Some("NOT_FOUND"), "{err}");
    let _ok = alice
        .orgs()
        .team_set_member(
            &org,
            &open,
            typed(json!({ "account": bob_id })),
            Some(&mut a),
        )
        .await
        .unwrap();
    set(&bob, &open_app, "back").await.unwrap();

    // A restricted folder is a wall, invisible to Bob until he is added.
    let secret = team("Secret", "invite", None).await;
    let secret_app = app_in(secret.clone()).await;
    let err = set(&bob, &secret_app, "x").await.unwrap_err();
    assert_eq!(err.code(), Some("NOT_FOUND"), "{err}");
    assert!(bob.orgs().team_get(&org, &secret).await.is_err());
    let listed = bob.orgs().get(&org).await.map(wire).unwrap();
    assert!(listed["teams"].get(&secret).is_none(), "{listed}");
    assert!(listed["teams"].get(&open).is_some(), "{listed}");
    let _ok = alice
        .orgs()
        .team_set_member(
            &org,
            &secret,
            typed(json!({ "account": bob_id, "role": "member" })),
            Some(&mut a),
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
        .orgs()
        .team_set_member(
            &org,
            &child,
            typed(json!({ "account": bob_id, "role": "readOnly" })),
            Some(&mut a),
        )
        .await
        .unwrap();
    let err = set(&bob, &child_app, "ro").await.unwrap_err();
    assert_eq!(err.code(), Some("FORBIDDEN"), "{err}");
    // Leaving does not shed the role: he does not come back by inheritance.
    let _ok = bob
        .orgs()
        .team_remove_member(&org, &child, &bob_id, Some(&mut b))
        .await
        .map(wire)
        .unwrap();
    assert!(bob
        .apps()
        .view(
            &child_app,
            "get",
            Some(json!({ "key": "k" })),
            Some(ViewsRequestConsistency::Strong),
            None
        )
        .await
        .is_err());
    let _ok = alice
        .orgs()
        .team_set_member(
            &org,
            &child,
            typed(json!({ "account": bob_id, "role": "readOnly" })),
            Some(&mut a),
        )
        .await
        .unwrap();
    // A call that writes nothing still runs for him.
    let read = bob
        .apps()
        .call(&child_app, "len", Some(json!({})), None, None)
        .await
        .unwrap();
    assert_eq!(read.result, json!(1));
    set(&bob, &secret_app, "still writes the parent")
        .await
        .unwrap();
    let v = bob
        .apps()
        .view(
            &child_app,
            "get",
            Some(json!({ "key": "k" })),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await;
    assert_eq!(v.unwrap().result, json!("inherited"));

    // Metadata and visibility belong to the team; moving it under a team Bob
    // is not in hides it from him.
    let _ok = alice
        .orgs()
        .team_update(
            &org,
            &child,
            None,
            None,
            None,
            Some(json!({ "name": "Child", "color": "#f00" })),
        )
        .await
        .map(wire)
        .unwrap();
    let seen = bob.orgs().team_get(&org, &child).await.map(wire).unwrap();
    assert_eq!(seen["metadata"]["color"], "#f00");
    assert_eq!(seen["myRole"], "readOnly");
    let board = team("Board", "invite", None).await;
    let _ok = alice
        .orgs()
        .team_update(
            &org,
            &child,
            None,
            Some(TeamJoin::Invite),
            Some(Some(&board)),
            None,
        )
        .await
        .unwrap();
    // Still a direct member of Child, Bob sees it but not Board above it.
    assert!(bob.orgs().team_get(&org, &child).await.is_ok());
    assert!(bob.orgs().team_get(&org, &board).await.is_err());
    let err = alice
        .orgs()
        .team_update(&org, &board, None, None, Some(Some(&child)), None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("REJECTED"), "{err}");

    // A member creates top-level teams only with the capability.
    let err = bob
        .orgs()
        .team_create(&org, "Mine", TeamJoin::Everyone, None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("REJECTED"), "{err}");
    let _ok = alice
        .orgs()
        .capabilities(&org, 37, None, None, Some(&mut a))
        .await
        .unwrap();
    let mine = s(
        bob.orgs()
            .team_create(&org, "Mine", TeamJoin::Everyone, None)
            .await
            .map(wire)
            .unwrap(),
        "teamId",
    );
    assert_eq!(
        bob.orgs().team_get(&org, &mine).await.map(wire).unwrap()["myRole"],
        "lead"
    );

    // Deleting a team deletes what is under it, apps included (with the
    // deleter's passkey, since it holds an app).
    let gone = alice
        .orgs()
        .team_delete(&org, &board, Some(&mut a))
        .await
        .map(wire)
        .unwrap();
    assert_eq!(gone["teams"].as_array().unwrap().len(), 2, "{gone}");
    assert_eq!(gone["apps"], json!([child_app]));
    assert!(alice.apps().get(&child_app).await.is_err());
}

/// An organization with `alice` as owner and the others as members (redeemed
/// invites); answers the org id and each member's account id.
async fn org_with(
    alice: &Client,
    a: &mut SoftAuthenticator,
    members: &[&Client],
) -> (String, Vec<String>) {
    let s = |v: Value, k: &str| v[k].as_str().unwrap().to_owned();
    let org = s(
        alice.orgs().create("Acme", None).await.map(wire).unwrap(),
        "orgId",
    );
    let mut ids = Vec::new();
    for m in members {
        let token = s(
            alice
                .orgs()
                .invite_create(&org, typed(json!({ "role": "member" })), Some(&mut *a))
                .await
                .map(wire)
                .unwrap(),
            "token",
        );
        let _r = m.orgs().invite_redeem(&token).await.unwrap();
        ids.push(s(m.auth().me().await.map(wire).unwrap(), "accountId"));
    }
    (org, ids)
}

/// Every replica has applied the system log as it stands.
async fn all_applied(c: &Cluster) {
    for r in c.nodes.iter().flatten() {
        let applied = r.node.system.status().last_log.unwrap_or(0);
        assert!(
            r.node
                .system
                .wait_applied(applied, Duration::from_secs(5))
                .await
        );
    }
}

/// Removed from an organization, a person reaches none of its apps: not one
/// that lists their account (a private channel, a DM), not one they created,
/// and they receive none of its events.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn removal_from_an_organization_ends_account_listed_access() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let (org, ids) = org_with(&alice, &mut a, &[&bob]).await;
    let bob_id = ids[0].clone();
    // Bob may create apps here (CREATE_APP | JOIN_OPEN_TEAMS).
    let _ok = alice
        .orgs()
        .capabilities(&org, 5, Some(&bob_id), None, Some(&mut a))
        .await
        .map(wire)
        .unwrap();
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let listed = alice
        .apps().create(typed(json!({ "name": "dm", "bundle": &bundle, "org": org, "access": { "kind": "accounts", "accounts": [bob_id] } })))
        .await.map(|a| a.app_id)
        .unwrap();
    let bundle_b = bob
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let own = bob
        .apps().create(typed(json!({ "name": "mine", "bundle": &bundle_b, "org": org, "access": { "kind": "owner" } })))
        .await.map(|a| a.app_id)
        .unwrap();
    set(&bob, &listed, "in").await.unwrap();
    set(&bob, &own, "in").await.unwrap();
    let mut bob_events = bob.events(&listed, 0).await.unwrap();

    let _ok = alice
        .orgs()
        .remove_member(&org, &bob_id, Some(&mut a))
        .await
        .unwrap();
    all_applied(&c).await;

    for app in [&listed, &own] {
        let err = set(&bob, app, "after").await.unwrap_err();
        assert_eq!(err.code(), Some("NOT_FOUND"), "{app}: {err}");
    }
    set(&alice, &listed, "alice").await.unwrap();
    let mut got = Vec::new();
    while let Ok(Some(e)) = tokio::time::timeout(Duration::from_secs(2), bob_events.next()).await {
        got.push(e);
    }
    // The stream may still hold the event of Bob's own write before the removal.
    assert!(
        got.iter()
            .all(|(_id, _kind, data)| data["value"] != "alice"),
        "a removed member still received {got:?}"
    );
}

/// Removing someone from a team removes their direct rows in the teams below
/// it too (a private channel under an app's team).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn removal_from_a_team_reaches_its_sub_teams() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let (org, ids) = org_with(&alice, &mut a, &[&bob]).await;
    let bob_id = ids[0].clone();
    let s = |v: Value, k: &str| v[k].as_str().unwrap().to_owned();
    let team = |name: &str, parent: Option<&str>| {
        let (name, parent) = (name.to_owned(), parent.map(str::to_owned));
        let (alice, org) = (&alice, &org);
        async move {
            s(
                alice
                    .orgs()
                    .team_create(org, &name, TeamJoin::Invite, parent.as_deref())
                    .await
                    .map(wire)
                    .unwrap(),
                "teamId",
            )
        }
    };
    let chat = team("Chat", None).await;
    let private = team("design", Some(&chat)).await;
    for t in [&chat, &private] {
        let _ok = alice
            .orgs()
            .team_set_member(
                &org,
                t,
                typed(json!({ "account": bob_id, "role": "member" })),
                Some(&mut a),
            )
            .await
            .unwrap();
    }
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let channel = alice
        .apps().create(typed(json!({ "name": "design", "bundle": &bundle, "org": org, "access": { "kind": "team", "team": private } })))
        .await.map(|a| a.app_id)
        .unwrap();
    set(&bob, &channel, "in").await.unwrap();

    let _ok = alice
        .orgs()
        .team_remove_member(&org, &chat, &bob_id, Some(&mut a))
        .await
        .unwrap();
    let err = set(&bob, &channel, "after").await.unwrap_err();
    assert_eq!(err.code(), Some("NOT_FOUND"), "{err}");
    let view = alice
        .orgs()
        .team_get(&org, &private)
        .await
        .map(wire)
        .unwrap();
    assert!(
        !view.to_string().contains(&bob_id),
        "a row is left below: {view}"
    );
}

/// An inviter below admin names only teams they see: holding INVITE_MEMBERS
/// organization-wide does not open a restricted team they are not in.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_invite_names_only_teams_the_inviter_sees() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, mut b) = signed_up(&c, 1, "bob").await;
    let (org, ids) = org_with(&alice, &mut a, &[&bob]).await;
    let bob_id = ids[0].clone();
    // INVITE_MEMBERS | JOIN_OPEN_TEAMS, organization-wide.
    let _ok = alice
        .orgs()
        .capabilities(&org, 6, Some(&bob_id), None, Some(&mut a))
        .await
        .map(wire)
        .unwrap();
    let s = |v: Value, k: &str| v[k].as_str().unwrap().to_owned();
    let team = |name: &str, join: &str| {
        let (name, join) = (name.to_owned(), join.to_owned());
        let (alice, org) = (&alice, &org);
        async move {
            s(
                alice
                    .orgs()
                    .team_create(org, &name, typed(json!(join)), None)
                    .await
                    .map(wire)
                    .unwrap(),
                "teamId",
            )
        }
    };
    let open = team("Open", "everyone").await;
    let secret = team("Secret", "invite").await;
    let _ok = invite(&bob, &org, json!([]), &mut b).await.unwrap();
    let _ok = invite(&bob, &org, json!([open]), &mut b).await.unwrap();
    let err = invite(&bob, &org, json!([secret]), &mut b)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("REJECTED"), "{err}");
    let err = invite(&bob, &org, json!([open, secret]), &mut b)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("REJECTED"), "{err}");
}

/// `who` mints a member invite into `org` naming `teams`, with their passkey.
async fn invite(
    who: &Client,
    org: &str,
    teams: Value,
    pk: &mut SoftAuthenticator,
) -> Result<Value, Box<calimero_client::Error>> {
    who.orgs()
        .invite_create(
            org,
            typed(json!({ "role": "member", "teams": teams })),
            Some(&mut *pk),
        )
        .await
        .map(wire)
        .map_err(Box::new)
}

/// Deleting a team that holds apps takes the deleter's passkey, and the
/// request names the apps that go with it; an empty team goes without one.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn deleting_a_team_with_apps_takes_a_passkey_and_names_them() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let s = |v: Value, k: &str| v[k].as_str().unwrap().to_owned();
    let org = s(
        alice.orgs().create("Acme", None).await.map(wire).unwrap(),
        "orgId",
    );
    let team = |name: &str, parent: Option<&str>| {
        let (name, parent) = (name.to_owned(), parent.map(str::to_owned));
        let (alice, org) = (&alice, &org);
        async move {
            s(
                alice
                    .orgs()
                    .team_create(org, &name, TeamJoin::Invite, parent.as_deref())
                    .await
                    .map(wire)
                    .unwrap(),
                "teamId",
            )
        }
    };
    let empty = team("Empty", None).await;
    let gone = alice
        .orgs()
        .team_delete(&org, &empty, None)
        .await
        .map(wire)
        .unwrap();
    assert_eq!(gone["apps"], json!([]), "{gone}");

    let chat = team("Chat", None).await;
    let private = team("design", Some(&chat)).await;
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let channel = alice
        .apps().create(typed(json!({ "name": "design", "bundle": &bundle, "org": org, "access": { "kind": "team", "team": private } })))
        .await.map(|a| a.app_id)
        .unwrap();
    let err = alice
        .orgs()
        .team_delete(&org, &chat, None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("CONFIRM_REQUIRED"), "{err}");
    assert_eq!(err.details().unwrap()["apps"], json!([channel]), "{err}");
    assert!(alice.apps().get(&channel).await.is_ok());

    let gone = alice
        .orgs()
        .team_delete(&org, &chat, Some(&mut a))
        .await
        .map(wire)
        .unwrap();
    assert_eq!(gone["apps"], json!([channel]), "{gone}");
    assert!(alice.apps().get(&channel).await.is_err());
}

/// `who` changes `app`'s access to `access`.
async fn set_access(
    who: &Client,
    app: &str,
    access: Value,
) -> Result<Value, Box<calimero_client::Error>> {
    who.apps()
        .access(app, typed(json!(access)))
        .await
        .map(wire)
        .map_err(Box::new)
}

/// `who` sets `app`'s metadata.
async fn set_metadata(
    who: &Client,
    app: &str,
    metadata: Value,
) -> Result<Value, Box<calimero_client::Error>> {
    who.apps()
        .metadata(app, json!(metadata))
        .await
        .map(wire)
        .map_err(Box::new)
}

/// Removed from an organization, the owner of one of its apps manages it no
/// more: they cannot open it to the public (and so reach it again), transfer
/// it, delete it or set its metadata. The organization's admins still can,
/// so they clean it up: transfer one to a member, delete another.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_owner_removed_from_the_organization_no_longer_manages_its_app() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, mut b) = signed_up(&c, 1, "bob").await;
    let (carol, _c) = signed_up(&c, 2, "carol").await;
    let (org, ids) = org_with(&alice, &mut a, &[&bob, &carol]).await;
    let (bob_id, carol_id) = (ids[0].clone(), ids[1].clone());
    // Bob may create apps here (CREATE_APP | JOIN_OPEN_TEAMS).
    let _ok = alice
        .orgs()
        .capabilities(&org, 5, Some(&bob_id), None, Some(&mut a))
        .await
        .map(wire)
        .unwrap();
    let bundle = bob
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let mut apps = Vec::new();
    for name in ["kept", "dropped"] {
        let app = bob
            .apps()
            .create(typed(
                json!({ "name": name, "bundle": &bundle, "org": org, "access": { "kind": "org" } }),
            ))
            .await
            .map(|a| a.app_id)
            .unwrap();
        set(&bob, &app, "in").await.unwrap();
        apps.push(app);
    }
    let (kept, dropped) = (apps[0].clone(), apps[1].clone());

    let _ok = alice
        .orgs()
        .remove_member(&org, &bob_id, Some(&mut a))
        .await
        .unwrap();
    all_applied(&c).await;

    for app in [&kept, &dropped] {
        let err = set_access(&bob, app, json!({ "kind": "public" }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some("REJECTED"), "{app}: access: {err}");
        let err = set(&bob, app, "back").await.unwrap_err();
        assert_eq!(err.code(), Some("NOT_FOUND"), "{app}: reached: {err}");
        let err = set_metadata(&bob, app, json!({ "pinned": true }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some("REJECTED"), "{app}: metadata: {err}");
        let err = bob
            .apps()
            .transfer(app, &bob_id, Some(&mut b))
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some("FORBIDDEN"), "{app}: transfer: {err}");
        let err = bob.apps().delete(app).await.unwrap_err();
        assert_eq!(err.code(), Some("REJECTED"), "{app}: delete: {err}");
    }
    let record = alice.apps().get(&kept).await.map(wire).unwrap();
    assert_eq!(record["access"]["kind"], "org", "{record}");
    assert_eq!(record["metadata"], Value::Null, "{record}");

    // The organization's owner hands one to a member and deletes the other.
    let _ok = alice
        .apps()
        .transfer(&kept, &carol_id, Some(&mut a))
        .await
        .map(wire)
        .unwrap();
    all_applied(&c).await;
    let record = carol.apps().get(&kept).await.map(wire).unwrap();
    assert_eq!(record["owner"], json!(carol_id), "{record}");
    set(&carol, &kept, "carol's").await.unwrap();
    let _ok = alice.apps().delete(&dropped).await.unwrap();
    assert!(alice.apps().get(&dropped).await.is_err());
}

/// An owner who is a member of the app's organization manages it as before,
/// and so does the owner of an app in their personal organization, which goes
/// to the new owner's personal organization when handed to someone outside it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_member_owner_and_a_personal_owner_still_manage_their_apps() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, mut b) = signed_up(&c, 1, "bob").await;
    let (carol, mut cpk) = signed_up(&c, 2, "carol").await;
    let (org, ids) = org_with(&alice, &mut a, &[&bob, &carol]).await;
    let (bob_id, carol_id) = (ids[0].clone(), ids[1].clone());
    let _ok = alice
        .orgs()
        .capabilities(&org, 5, Some(&bob_id), None, Some(&mut a))
        .await
        .unwrap();
    let bundle = bob
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let in_org = bob
        .apps().create(typed(json!({ "name": "team-app", "bundle": &bundle, "org": org, "access": { "kind": "owner" } })))
        .await.map(|a| a.app_id)
        .unwrap();
    let personal = bob
        .apps()
        .create(typed(json!({ "name": "mine", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    for app in [&in_org, &personal] {
        let _ok = set_access(&bob, app, json!({ "kind": "public" }))
            .await
            .unwrap();
        let _ok = set_metadata(&bob, app, json!({ "pinned": true }))
            .await
            .unwrap();
        bob.apps()
            .transfer(app, &carol_id, Some(&mut b))
            .await
            .unwrap();
    }
    all_applied(&c).await;
    // It went to her personal organization.
    let carols = carol.apps().get(&personal).await.map(wire).unwrap()["org"].clone();
    assert_eq!(carols, json!(carol.personal_org().await.unwrap()));
    assert_ne!(carols, json!(org));
    for app in [&in_org, &personal] {
        let record = carol.apps().get(app).await.map(wire).unwrap();
        assert_eq!(record["owner"], json!(carol_id), "{record}");
        assert_eq!(record["metadata"], json!({ "pinned": true }), "{record}");
        let _ok = carol.apps().delete(app).await.unwrap();
    }
    let _unused = &mut cpk;
}

/// An app on a team is its team's, its owner's only while they are in it:
/// an owner removed from the team, who left it, or who lost it when an open
/// team closed reaches the app no more and manages it no more (no access
/// change, metadata, transfer or deletion), and nobody outside the
/// organization reaches it. The organization's admins transfer it; back in
/// the team, an owner has it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_owner_out_of_the_apps_team_no_longer_reaches_or_manages_it() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, mut b) = signed_up(&c, 1, "bob").await;
    let (carol, mut cpk) = signed_up(&c, 2, "carol").await;
    let (dave, _d) = signed_up(&c, 0, "dave").await;
    let (erin, _e) = signed_up(&c, 1, "erin").await;
    let (org, ids) = org_with(&alice, &mut a, &[&bob, &carol, &dave]).await;
    let (bob_id, carol_id, dave_id) = (ids[0].clone(), ids[1].clone(), ids[2].clone());
    let alice_id = alice.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let team = |name: &str, join: &str| {
        let (name, join) = (name.to_owned(), join.to_owned());
        let (alice, org) = (&alice, &org);
        async move {
            alice
                .orgs()
                .team_create(org, &name, typed(json!(join)), None)
                .await
                .map(wire)
                .unwrap()["teamId"]
                .as_str()
                .unwrap()
                .to_owned()
        }
    };
    // Chat is restricted, Blocks open to the organization.
    let chat = team("Chat", "invite").await;
    let blocks = team("Blocks", "everyone").await;
    for who in [&bob_id, &carol_id, &dave_id] {
        // CREATE_APP | JOIN_OPEN_TEAMS
        let _ok = alice
            .orgs()
            .capabilities(&org, 5, Some(who), None, Some(&mut a))
            .await
            .map(wire)
            .unwrap();
    }
    for who in [&bob_id, &carol_id] {
        let _ok = alice
            .orgs()
            .team_set_member(
                &org,
                &chat,
                typed(json!({ "account": who, "role": "member" })),
                Some(&mut a),
            )
            .await
            .unwrap();
    }
    let on = |t: &str| json!({ "org": org, "access": { "kind": "team", "team": t } });
    let mut apps = Vec::new();
    for (who, t) in [(&bob, &chat), (&carol, &chat), (&dave, &blocks)] {
        let bundle = who
            .packages()
            .bundle_put(kv_wasm())
            .await
            .map(|b| b.hash)
            .unwrap();
        let app = who
            .apps()
            .create(AppCreateRequest {
                name: Some("channel".to_owned()),
                bundle: Some(bundle),
                ..typed(on(t))
            })
            .await
            .map(|a| a.app_id)
            .unwrap();
        set(who, &app, "in").await.unwrap();
        apps.push(app);
    }
    let (bobs, carols, daves) = (apps[0].clone(), apps[1].clone(), apps[2].clone());

    // Kicked from Chat, Bob neither reaches nor manages his channel.
    let _ok = alice
        .orgs()
        .team_remove_member(&org, &chat, &bob_id, Some(&mut a))
        .await
        .map(wire)
        .unwrap();
    // Carol leaves Chat.
    let _ok = carol
        .orgs()
        .team_remove_member(&org, &chat, &carol_id, Some(&mut cpk))
        .await
        .map(wire)
        .unwrap();
    // Blocks closes: Dave, in it by inheritance only, is out.
    let _ok = alice
        .orgs()
        .team_update(&org, &blocks, None, Some(TeamJoin::Invite), None, None)
        .await
        .map(wire)
        .unwrap();
    all_applied(&c).await;
    for (who, app) in [(&bob, &bobs), (&carol, &carols), (&dave, &daves)] {
        let err = set(who, app, "after").await.unwrap_err();
        assert_eq!(err.code(), Some("NOT_FOUND"), "{app}: reached: {err}");
        assert!(who.apps().get(app).await.is_err(), "{app}");
    }
    let err = set(&erin, &bobs, "outside").await.unwrap_err();
    assert_eq!(err.code(), Some("NOT_FOUND"), "{err}");
    let err = set_access(&bob, &bobs, json!({ "kind": "public" }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("REJECTED"), "access: {err}");
    let err = set_metadata(&bob, &bobs, json!({ "pinned": true }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("REJECTED"), "metadata: {err}");
    let err = bob
        .apps()
        .transfer(&bobs, &bob_id, Some(&mut b))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("FORBIDDEN"), "transfer: {err}");
    let err = bob.apps().delete(&bobs).await.unwrap_err();
    assert_eq!(err.code(), Some("REJECTED"), "delete: {err}");

    // The organization's owner takes Bob's channel; Dave, added back to
    // Blocks, has his again.
    let _ok = alice
        .apps()
        .transfer(&bobs, &alice_id, Some(&mut a))
        .await
        .map(wire)
        .unwrap();
    let _ok = alice
        .orgs()
        .team_set_member(
            &org,
            &blocks,
            typed(json!({ "account": dave_id, "role": "member" })),
            Some(&mut a),
        )
        .await
        .unwrap();
    all_applied(&c).await;
    set(&alice, &bobs, "alice's").await.unwrap();
    set(&dave, &daves, "back").await.unwrap();
    let _ok = set_metadata(&dave, &daves, json!({ "pinned": true }))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_passkey_is_another_device_of_the_same_account() {
    let c = Cluster::start(3).await;
    let (laptop, mut pk) = signed_up(&c, 0, "alice").await;
    let mut phone_key = SoftAuthenticator::new("localhost", ORIGIN);
    let phone_cred = laptop.add_passkey(&mut phone_key, &mut pk).await.unwrap();
    let phone = c.client(1);
    let _s = phone.sign_in(&mut phone_key).await.unwrap();
    let (a, b) = (
        laptop.auth().me().await.map(wire).unwrap(),
        phone.auth().me().await.map(wire).unwrap(),
    );
    assert_eq!(a["accountId"], b["accountId"]);
    assert_eq!(b["passkeyId"], json!(phone_cred));
    assert_ne!(a["passkeyId"], b["passkeyId"]);
    assert_eq!(b["passkeys"].as_array().unwrap().len(), 2);
}

/// The `u64` a fixture returned (a decimal string in JSON, D20).
fn as_u64(v: &Value) -> u64 {
    v.as_str().unwrap().parse().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn views_refuse_confirm_methods() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
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
    let e = alice
        .apps()
        .view(
            &app,
            "wipe",
            Some(json!({})),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("APP_ERROR"), "{e}");
    assert_eq!(e.details().unwrap()["code"], "ConfirmNotAView", "{e}");
}

/// `POST /apps/{app}/methods/{method}` is `/intents` or `/views` under another spelling, and
/// `GET /apps/{app}/openapi.json` lists exactly those routes, under `/abi`'s access.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn methods_run_as_intents_and_views_and_the_openapi_document_lists_them() {
    let c = Cluster::start(3).await;
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

    // The document: one operation per client method, kinds and step-ups from the ABI.
    let doc = alice.apps().openapi(&app).await.map(wire).unwrap();
    assert_eq!(doc["openapi"], "3.1.0", "{doc}");
    assert_eq!(doc["info"]["version"], bundle.as_str());
    let set = &doc["paths"]["/apps/{app}/methods/set"]["post"];
    assert_eq!(set["operationId"], "set");
    assert_eq!(set["tags"], json!(["call"]));
    assert_eq!(
        doc["paths"]["/apps/{app}/methods/len"]["post"]["tags"],
        json!(["view"])
    );
    assert!(doc["paths"]["/apps/{app}/methods/clear"]["post"]["responses"]["412"].is_object());
    assert!(doc["paths"]["/apps/{app}/methods/init"].is_null());
    assert!(
        doc["paths"]["/apps/{app}/methods/beat"].is_null(),
        "a timer is not called"
    );
    assert!(doc["paths"]["/apps/{app}/events"]["get"].is_object());

    // A call: the same idempotency key through either route is one run.
    let by_route = alice
        .apps()
        .method(
            &app,
            "set",
            typed(json!({ "args": { "key": "a", "value": "1" }, "idempotencyKey": "k-1" })),
            None,
        )
        .await
        .map(wire)
        .unwrap();
    assert!(by_route["events"].is_array(), "{by_route}");
    let by_intents = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "a", "value": "2" })),
            Some("k-1"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(by_intents.index, by_route["index"].as_u64().unwrap());
    assert_eq!(
        strong_view(&alice, &app, "get", json!({ "key": "a" })).await,
        json!("1"),
        "the retry under the same key did not run again"
    );

    // A view: the answer `/views` gives (no `events`), and the same result.
    let len = alice
        .apps()
        .method(&app, "len", typed(json!({ "args": {} })), None)
        .await
        .map(wire)
        .unwrap();
    assert_eq!(
        len["result"],
        strong_view(&alice, &app, "len", json!({})).await
    );
    assert!(len.get("events").is_none(), "{len}");
    let stale = alice
        .apps().method(&app, "get", typed(json!({ "args": { "key": "a" }, "consistency": "stale", "minIndex": by_intents.index })), None)
        .await.map(wire)
        .unwrap();
    assert_eq!(stale["result"], json!("1"));

    // The app's own errors and the step-up come back as they do from `/intents`.
    let err = alice
        .apps()
        .method(&app, "clear", typed(json!({ "args": {} })), None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("CONFIRM_REQUIRED"), "{err}");
    let _r = alice
        .apps()
        .method(&app, "clear", typed(json!({ "args": {} })), Some(&mut pk))
        .await
        .unwrap();
    assert_eq!(strong_view(&alice, &app, "len", json!({})).await, json!(0));

    // Reserved names, `method` in the body and unknown fields are refused.
    let err = alice
        .apps()
        .method(&app, "__upgrade", typed(json!({ "args": {} })), None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("RESERVED_METHOD"), "{err}");
    let _err = alice
        .transport()
        .json(
            Request::new(Method::POST, format!("apps/{app}/methods/set"))
                .json(json!({ "method": "clear", "args": {} })),
        )
        .await
        .unwrap_err();

    // Someone the app does not reach gets what `/abi`, `/intents` and `/views` give them.
    let (bob, _bob_pk) = signed_up(&c, 1, "bob").await;
    let code = |r: Result<Value, calimero_client::Error>| r.unwrap_err().code().map(str::to_owned);
    let abi = code(bob.apps().abi(&app).await.map(wire));
    assert_eq!(code(bob.apps().openapi(&app).await.map(wire)), abi);
    let intents = bob
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "b", "value": "1" })),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(
        code(
            bob.apps()
                .method(
                    &app,
                    "set",
                    typed(json!({ "args": { "key": "b", "value": "1" } })),
                    None
                )
                .await
                .map(wire)
        ),
        intents.code().map(str::to_owned)
    );
    let views = bob
        .apps()
        .view(
            &app,
            "len",
            Some(json!({})),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(
        code(
            bob.apps()
                .method(&app, "len", typed(json!({ "args": {} })), None)
                .await
                .map(wire)
        ),
        views.code().map(str::to_owned)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn timeouts_are_not_logged() {
    let c = Cluster::start_with(3, |cfg| {
        cfg.budget = Some(calimero_app_host::Budget {
            fuel: u64::MAX,
            ..calimero_app_host::Budget::call()
        });
    })
    .await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
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
    let leader = c.app_leader(&app).unwrap();
    let applied = || {
        c.nodes[leader]
            .as_ref()
            .unwrap()
            .node
            .hosted(&app)
            .unwrap()
            .applied()
    };
    let before = applied();
    let e = alice
        .apps()
        .call(&app, "spin", Some(json!({})), Some("k-1"), None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("TIMEOUT"), "{e}");
    assert!(e.details().unwrap()["retryAfter"].is_u64(), "{e}");
    assert_eq!(applied(), before, "a timeout was logged");
    // The key was not spent: the same key on a fast method runs it.
    let r = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "a", "value": "1" })),
            Some("k-1"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(r.index, before + 1);
    let v = alice
        .apps()
        .view(
            &app,
            "get",
            Some(json!({ "key": "a" })),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap();
    assert_eq!(v.result, json!("1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn oversized_entry_is_host_denied() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
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
    let _r = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "a", "value": "1" })),
            None,
            None,
        )
        .await
        .unwrap();
    // 1,000 keys of 1,000 bytes, each with its event: within every per-run limit
    // (writes, events, event size), but about 2 MB of diff and events.
    let e = alice
        .apps()
        .call(
            &app,
            "fill",
            Some(json!({ "n": 1000, "size": 1000 })),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("TRAP"), "{e}");
    assert_eq!(e.details().unwrap()["kind"], "HostDenied", "{e}");
    let v = alice
        .apps()
        .view(
            &app,
            "len",
            Some(json!({})),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap();
    assert_eq!(v.result, json!(1));
    let _r = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "b", "value": "2" })),
            None,
            None,
        )
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn entry_timestamps_never_go_backwards() {
    let mut c = Cluster::start(3).await;
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
    let first = alice
        .apps()
        .call(&app, "stamp", Some(json!({})), None, None)
        .await
        .unwrap();
    let t1 = as_u64(&first.result);

    // The leader comes back with its clock a minute behind, and leads again.
    let slow = c.app_leader(&app).unwrap();
    c.restart(slow, |cfg| cfg.clock_offset_ms = -60_000).await;
    for _ in 0..30 {
        match c.app_leader(&app) {
            Some(l) if l == slow => break,
            Some(l) => c.restart(l, |_| {}).await,
            None => {}
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
    assert_eq!(
        c.app_leader(&app),
        Some(slow),
        "the slow node never led again"
    );

    let cl = c.client(slow);
    let _s = cl.sign_in(&mut pk).await.unwrap();
    let second = cl
        .apps()
        .call(&app, "stamp", Some(json!({})), None, None)
        .await
        .unwrap();
    let t2 = as_u64(&second.result);
    assert!(t2 > t1, "entry time went back: {t1} -> {t2}");

    let follower = (0..3).find(|i| *i != slow).unwrap();
    let fc = c.client(follower);
    let _s = fc.sign_in(&mut pk).await.unwrap();
    let seen = fc
        .apps()
        .view(
            &app,
            "timeNow",
            Some(json!({})),
            Some(ViewsRequestConsistency::Stale),
            Some(second.index),
        )
        .await
        .unwrap();
    assert!(as_u64(&seen.result) >= t2, "a view saw an earlier time");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn removed_member_stops_receiving_org_events() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let (carol, _c) = signed_up(&c, 2, "carol").await;
    let s = |v: Value, k: &str| v[k].as_str().unwrap().to_owned();
    let org = s(
        alice.orgs().create("Acme", None).await.map(wire).unwrap(),
        "orgId",
    );
    for member in [&bob, &carol] {
        let token = s(
            alice
                .orgs()
                .invite_create(&org, typed(json!({ "role": "member" })), Some(&mut a))
                .await
                .map(wire)
                .unwrap(),
            "token",
        );
        let _r = member.orgs().invite_redeem(&token).await.unwrap();
    }
    let bob_id = s(bob.auth().me().await.map(wire).unwrap(), "accountId");
    let bundle = alice
        .packages()
        .bundle_put(wasm("org-gated"))
        .await
        .map(|b| b.hash)
        .unwrap();
    let app = alice
        .apps()
        .create(typed(
            json!({ "name": "notes", "bundle": &bundle, "org": org }),
        ))
        .await
        .map(|a| a.app_id)
        .unwrap();

    let mut bob_events = bob.events(&app, 0).await.unwrap();
    let mut carol_events = carol.events(&app, 0).await.unwrap();
    let _r = alice
        .apps()
        .call(&app, "announce", Some(json!({ "text": "one" })), None, None)
        .await
        .unwrap();
    for stream in [&mut bob_events, &mut carol_events] {
        let (_id, kind, data) = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(kind, "Announced");
        assert_eq!(data["text"], "one");
    }

    let _r = alice
        .orgs()
        .remove_member(&org, &bob_id, Some(&mut a))
        .await
        .map(wire)
        .unwrap();
    // Every replica has applied the removal before the next event.
    for r in c.nodes.iter().flatten() {
        let applied = r.node.system.status().last_log.unwrap_or(0);
        assert!(
            r.node
                .system
                .wait_applied(applied, Duration::from_secs(5))
                .await
        );
    }
    let _r = alice
        .apps()
        .call(&app, "announce", Some(json!({ "text": "two" })), None, None)
        .await
        .unwrap();
    let (_id, _kind, data) = tokio::time::timeout(Duration::from_secs(5), carol_events.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(data["text"], "two");
    let got = tokio::time::timeout(Duration::from_secs(2), bob_events.next()).await;
    assert!(
        !matches!(got, Ok(Some(_))),
        "a removed member still received {got:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn unique_ids_differ_within_a_term_and_across_leaders() {
    let mut c = Cluster::start(3).await;
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
    let id = |v: &Value| -> (u64, u64, u64) {
        let parts: Vec<u64> = v
            .as_str()
            .unwrap()
            .split('.')
            .map(|p| p.parse().unwrap())
            .collect();
        (parts[0], parts[1], parts[2])
    };
    let a = id(&alice
        .apps()
        .call(&app, "newId", Some(json!({})), None, None)
        .await
        .unwrap()
        .result);
    let b = id(&alice
        .apps()
        .call(&app, "newId", Some(json!({})), None, None)
        .await
        .unwrap()
        .result);
    assert_eq!(a.0, b.0, "one term");
    assert_ne!(a, b);

    let leader = c.app_leader(&app).unwrap();
    c.stop(leader).await;
    let other = (0..3).find(|i| *i != leader).unwrap();
    let cl = c.client(other);
    let _s = cl.sign_in(&mut pk).await.unwrap();
    let mut after = None;
    for _ in 0..40 {
        if let Ok(r) = cl
            .apps()
            .call(&app, "newId", Some(json!({})), None, None)
            .await
        {
            after = Some(id(&r.result));
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let after = after.expect("no call after the leader change");
    assert!(after.0 > a.0, "{a:?} then {after:?}");
    assert_ne!(after, a);
    assert_ne!(after, b);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_system_caller_is_refused_where_the_abi_does_not_allow_it() {
    use calimero_node::node::CallRequest;
    use calimero_node::types::Caller;

    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
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
    let node = &c.nodes[1].as_ref().unwrap().node;
    let e = node
        .call_component(&CallRequest::new(
            app.clone(),
            "set".into(),
            json!({ "key": "s", "value": "v" }),
            Caller::System,
        ))
        .await
        .unwrap_err();
    assert_eq!(e.code, "APP_ERROR", "{e}");
    assert_eq!(e.details["code"], "Forbidden", "{e}");
    let v = alice
        .apps()
        .view(
            &app,
            "len",
            Some(json!({})),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap();
    assert_eq!(v.result, json!(0));
}

/// Refusals before the app runs carry the platform's codes (Ruling 32): an anonymous
/// caller of a method that is not public is `401 UNAUTHENTICATED`, also on a public app
/// (where the caller reaches the app and the ABI refuses), and an account the app does not
/// let in is `404 NOT_FOUND`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refusals_before_the_app_carry_the_platforms_codes() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let (bob, _bpk) = signed_up(&c, 1, "bob").await;
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let open = alice
        .apps()
        .create(typed(
            json!({ "name": "kv", "bundle": &bundle, "access": { "kind": "public" } }),
        ))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let closed = alice
        .apps()
        .create(typed(json!({ "name": "kv2", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let anonymous = c.client(2);
    let set = json!({ "key": "a", "value": "1" });
    for app in [&open, &closed] {
        let e = anonymous
            .apps()
            .call(app, "set", Some(set.clone()), None, None)
            .await
            .unwrap_err();
        assert_eq!(e.code(), Some("UNAUTHENTICATED"), "{app}: {e}");
        let e = anonymous
            .apps()
            .view(
                app,
                "len",
                Some(json!({})),
                Some(ViewsRequestConsistency::Strong),
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(e.code(), Some("UNAUTHENTICATED"), "{app}: {e}");
    }
    let e = bob
        .apps()
        .call(&closed, "set", Some(set), None, None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NOT_FOUND"), "{e}");
}

/// `(id_term, id_seq, n)` of a `unique_id` (D52).
fn id_parts(v: &Value) -> (u64, u64, u64) {
    let parts: Vec<u64> = v
        .as_str()
        .unwrap()
        .split('.')
        .map(|p| p.parse().unwrap())
        .collect();
    (parts[0], parts[1], parts[2])
}

/// A node that leads the same term again after a restart (a one-replica group:
/// openraft restores it as leader without an election) must not issue an
/// `id_seq` it issued before (D52, Ruling 29).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unique_ids_survive_a_restart_into_the_same_term() {
    let mut c = Cluster::start(1).await;
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
    let mut before = Vec::new();
    for _ in 0..3 {
        let r = alice
            .apps()
            .call(&app, "newId", Some(json!({})), None, None)
            .await
            .unwrap();
        before.push(id_parts(&r.result));
    }
    let term = before[0].0;

    c.restart(0, |_| {}).await;
    let cl = c.client(0);
    let mut signed = false;
    for _ in 0..40 {
        if cl.sign_in(&mut pk).await.is_ok() {
            signed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(signed, "no sign-in after the restart");
    let mut after = Vec::new();
    for _ in 0..3 {
        let mut got = None;
        for _ in 0..40 {
            if let Ok(r) = cl
                .apps()
                .call(&app, "newId", Some(json!({})), None, None)
                .await
            {
                got = Some(id_parts(&r.result));
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        after.push(got.expect("no call after the restart"));
    }
    // The test proves what it claims only if the restart kept the term.
    assert!(
        after.iter().all(|a| a.0 == term),
        "{before:?} then {after:?}"
    );
    for a in &after {
        assert!(!before.contains(a), "id reused: {before:?} then {after:?}");
    }
}

/// D52 in a three-replica group (Ruling 34): the app's leader restarts before any
/// follower's election timer fires, so openraft restores it as leader of its committed term
/// without an election (openraft 0.9.25 `Engine::startup`, "Previously it is a leader.
/// restore it as leader at once"). Preconditions, asserted: the term is the same on every
/// node before and after, and no other node reported itself leader, or any later term, at
/// any point of the window (each follower's group status sampled every 2 ms). Then the next
/// ids are in the same term and continue strictly after the last committed one.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn unique_ids_continue_when_a_leader_restarts_into_its_term_in_a_three_replica_group() {
    // Election timers long enough that a restart always beats them.
    let mut c = Cluster::start_with(3, |cfg| {
        cfg.raft.election_min_ms = 6_000;
        cfg.raft.election_max_ms = 8_000;
    })
    .await;
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
    let mut before = Vec::new();
    for _ in 0..3 {
        let r = alice
            .apps()
            .call(&app, "newId", Some(json!({})), None, None)
            .await
            .unwrap();
        before.push(id_parts(&r.result));
    }
    let leader = c.app_leader(&app).expect("an app leader");
    let status = |c: &Cluster, i: usize| {
        c.nodes[i]
            .as_ref()
            .unwrap()
            .node
            .hosted(&app)
            .unwrap()
            .status()
    };
    let term = status(&c, leader).term;
    assert!(
        before.iter().all(|b| b.0 == term),
        "{before:?} in term {term}"
    );
    let followers: Vec<usize> = (0..3).filter(|i| *i != leader).collect();
    for f in &followers {
        let s = status(&c, *f);
        assert_eq!(
            (s.term, s.leader),
            (term, Some(leader as u64 + 1)),
            "node {f}"
        );
    }

    // Sample the followers' view of the group for the whole restart window.
    let watched: Vec<_> = followers
        .iter()
        .map(|f| (*f, c.nodes[*f].as_ref().unwrap().node.hosted(&app).unwrap()))
        .collect();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let sampler = {
        let stop = Arc::clone(&stop);
        tokio::spawn(async move {
            let mut seen = Vec::new();
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                for (f, g) in &watched {
                    let s = g.status();
                    seen.push((*f, s.term, s.is_leader, s.leader));
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            seen
        })
    };

    let started = std::time::Instant::now();
    c.restart(leader, |_| {}).await;
    let restored = loop {
        let s = c.nodes[leader]
            .as_ref()
            .unwrap()
            .node
            .hosted(&app)
            .map(|g| g.status());
        if let Some(s) = s.filter(|s| s.is_leader) {
            break s;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "node {leader} did not resume leadership"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    assert_eq!(restored.term, term, "the restart kept the term");
    println!(
        "node {leader} resumed leading term {term} {:?} after the restart began",
        started.elapsed()
    );

    let other = followers[0];
    let cl = c.client(other);
    let _s = cl.sign_in(&mut pk).await.unwrap();
    let mut after = Vec::new();
    for _ in 0..3 {
        let r = cl
            .apps()
            .call(&app, "newId", Some(json!({})), None, None)
            .await
            .unwrap();
        after.push(id_parts(&r.result));
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let seen = sampler.await.unwrap();

    // Preconditions: nobody else led, no election started (a candidate's term would rise).
    assert!(seen.len() > 100, "{} samples", seen.len());
    for (f, t, is_leader, l) in &seen {
        assert!(!is_leader, "node {f} led during the window: {seen:?}");
        assert_eq!(*t, term, "node {f} saw term {t} during the window");
        assert!(
            l.is_none() || *l == Some(leader as u64 + 1),
            "node {f} followed {l:?}"
        );
    }
    for i in 0..3 {
        assert_eq!(status(&c, i).term, term, "node {i} after");
    }
    assert_eq!(c.app_leader(&app), Some(leader));

    // The outcome: same term, ids strictly after the last committed one, none reused.
    let last = *before.iter().max().unwrap();
    assert!(
        after.iter().all(|a| a.0 == term),
        "{before:?} then {after:?}"
    );
    assert!(after[0] > last, "{before:?} then {after:?}");
    for w in after.windows(2) {
        assert!(w[1] > w[0], "{after:?}");
    }
    for a in &after {
        assert!(!before.contains(a), "id reused: {before:?} then {after:?}");
    }
}

/// Args over 512 KiB are refused before anything runs (Ruling 30).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversized_args_are_refused_before_running() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
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
    let first = id_parts(
        &alice
            .apps()
            .call(&app, "newId", Some(json!({})), None, None)
            .await
            .unwrap()
            .result,
    );
    let leader = c.app_leader(&app).unwrap();
    let applied = || {
        c.nodes[leader]
            .as_ref()
            .unwrap()
            .node
            .hosted(&app)
            .unwrap()
            .applied()
    };
    let at = applied();
    let e = alice
        .apps()
        .call(
            &app,
            "put",
            Some(json!({ "key": "big", "value": "x".repeat(600 * 1024) })),
            Some("big-1"),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("ARGS_TOO_LARGE"), "{e}");
    assert_eq!(applied(), at, "a refused call was logged");
    // Nothing ran: the next execution takes the next id_seq.
    let next = id_parts(
        &alice
            .apps()
            .call(&app, "newId", Some(json!({})), None, None)
            .await
            .unwrap()
            .result,
    );
    assert_eq!(
        (next.0, next.1),
        (first.0, first.1 + 1),
        "{first:?} then {next:?}"
    );
}

/// Args within the limit whose writes push the entry over 1 MiB: the call is
/// logged as `Trap(HostDenied)` "entry too large", and the log moves on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn large_args_whose_entry_overflows_are_host_denied() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
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
    let e = alice
        .apps()
        .call(
            &app,
            "put",
            Some(json!({ "key": "big", "value": "x".repeat(450 * 1024) })),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("TRAP"), "{e}");
    assert_eq!(e.details().unwrap()["kind"], "HostDenied", "{e}");
    assert_eq!(e.details().unwrap()["detail"], "entry too large", "{e}");
    let _r = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "a", "value": "1" })),
            None,
            None,
        )
        .await
        .unwrap();
}

/// The node refuses what the ABI does not offer (Ruling 31): an unknown method,
/// and `init` through `/intents` or `/views`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_methods_and_init_are_refused_by_the_node() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
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
    let leader = c.app_leader(&app).unwrap();
    let applied = || {
        c.nodes[leader]
            .as_ref()
            .unwrap()
            .node
            .hosted(&app)
            .unwrap()
            .applied()
    };
    let at = applied();
    let e = alice
        .apps()
        .call(&app, "nope", Some(json!({})), None, None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("APP_ERROR"), "{e}");
    assert_eq!(e.details().unwrap()["code"], "UnknownMethod", "{e}");
    let e = alice
        .apps()
        .call(&app, "init", Some(json!({})), None, None)
        .await
        .unwrap_err();
    assert_eq!(e.details().unwrap()["code"], "WrongKind", "{e}");
    let e = alice
        .apps()
        .view(
            &app,
            "init",
            Some(json!({})),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.details().unwrap()["code"], "WrongKind", "{e}");
    assert_eq!(applied(), at, "a refused call was logged");
}

/// A compute method and its result method are the node's: no client can call either through
/// `/intents` or `/views`, and a refused call leaves no log entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compute_and_compute_result_methods_are_refused_to_clients() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice
        .packages()
        .bundle_put(wasm("compute"))
        .await
        .map(|b| b.hash)
        .unwrap();
    let app = alice
        .apps()
        .create(typed(json!({ "name": "compute", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let leader = c.app_leader(&app).unwrap();
    let applied = || {
        c.nodes[leader]
            .as_ref()
            .unwrap()
            .node
            .hosted(&app)
            .unwrap()
            .applied()
    };
    let at = applied();
    for method in ["digest", "onDone"] {
        let e = alice
            .apps()
            .call(&app, method, Some(json!({})), None, None)
            .await
            .unwrap_err();
        assert_eq!(e.details().unwrap()["code"], "WrongKind", "{method}: {e}");
        let e = alice
            .apps()
            .view(
                &app,
                method,
                Some(json!({})),
                Some(ViewsRequestConsistency::Strong),
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(e.details().unwrap()["code"], "WrongKind", "{method}: {e}");
    }
    assert_eq!(applied(), at, "a refused call was logged");
    let v = alice
        .apps()
        .view(
            &app,
            "done",
            Some(json!({})),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap();
    assert_eq!(v.result, json!("0"));
}

/// Upgrades switch the bundle, code only; a named migration is refused (D38).
/// The upgrade route is the node's: a passkey assertion over the exact
/// upgrade (412 first), and a refusal answered `422 UPGRADE_FAILED` with the
/// leader's answer in its details.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upgrades_switch_the_bundle_and_refuse_migrations() {
    let c = Cluster::start(3).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let v1 = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let app = alice
        .apps()
        .create(typed(json!({ "name": "kv", "bundle": &v1 })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let _r = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "a", "value": "kept" })),
            None,
            None,
        )
        .await
        .unwrap();
    // The same app with a custom section: another bundle hash.
    let mut wasm = kv_wasm();
    wasm.extend_from_slice(&[0, 4, 3, b'v', b'2', b'!']);
    let v2 = alice
        .packages()
        .bundle_put(wasm)
        .await
        .map(|b| b.hash)
        .unwrap();
    assert_ne!(v1, v2);

    let refused = alice
        .apps()
        .upgrade(
            &app,
            typed(json!({ "bundle": v2, "migrate": "migrate_v2" })),
            Some(&mut pk),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some("UPGRADE_FAILED"), "{refused}");
    let details = refused.details().unwrap();
    assert_eq!(details["status"], "appError", "{refused}");
    assert_eq!(
        details["error"]["code"], "MigrationsNotSupported",
        "{refused}"
    );
    assert_eq!(
        alice.apps().get(&app).await.map(wire).unwrap()["bundle"],
        json!(v1)
    );

    let up = alice
        .apps()
        .upgrade(&app, typed(json!({ "bundle": v2 })), Some(&mut pk))
        .await
        .map(wire)
        .unwrap();
    assert_eq!(up["status"], "ok", "{up}");
    assert_eq!(
        alice.apps().get(&app).await.map(wire).unwrap()["bundle"],
        json!(v2)
    );
    assert_eq!(
        strong_view(&alice, &app, "get", json!({ "key": "a" })).await,
        json!("kept")
    );
}

// ---- Timers (D34). The kv fixture's `beat` ticks every hour of app time; the tests
// move every node's app clock forward instead of sleeping.

const HOUR: i64 = 3_600_000;

/// One applied `beat` tick: `(scheduled, previous, at)`.
type BeatRow = (u64, Option<u64>, u64);

async fn beats(cl: &Client, app: &str) -> Vec<BeatRow> {
    let v = cl
        .apps()
        .view(
            app,
            "beats",
            Some(json!({})),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap()
        .result;
    v.as_array()
        .unwrap()
        .iter()
        .map(|b| {
            (
                as_u64(&b["scheduled"]),
                b["previous"].as_str().map(|p| p.parse().unwrap()),
                as_u64(&b["at"]),
            )
        })
        .collect()
}

/// Waits (up to `secs`) until `beats` has `n` rows.
async fn beats_reach(cl: &Client, app: &str, n: usize, secs: u64) -> Vec<BeatRow> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        if let Ok(v) = cl
            .apps()
            .view(
                app,
                "beats",
                Some(json!({})),
                Some(ViewsRequestConsistency::Strong),
                None,
            )
            .await
        {
            if v.result.as_array().is_some_and(|a| a.len() >= n) {
                return beats(cl, app).await;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "beats never reached {n}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Waits until node `i` has applied past `index` for `app`.
async fn applied_past(c: &Cluster, i: usize, app: &str, index: u64) {
    for _ in 0..100 {
        if c.applied_on(i, app) > index {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("node {i} never applied past {index}");
}

fn slots(rows: &[BeatRow]) -> Vec<u64> {
    rows.iter().map(|r| r.0).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ticks_run_once_per_slot() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
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
    for k in 1..=5 {
        c.advance_all(HOUR);
        let _rows = beats_reach(&alice, &app, k, 10).await;
    }
    // No slot runs twice, however long the leader keeps polling.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let rows = beats(&alice, &app).await;
    assert_eq!(rows.len(), 5, "{rows:?}");
    assert_eq!(rows[0].1, None, "the first tick has no previous");
    for w in rows.windows(2) {
        assert_eq!(
            w[1].0,
            w[0].0 + HOUR as u64,
            "slots are an hour apart: {rows:?}"
        );
        assert_eq!(w[1].1, Some(w[0].0), "previous is the prior slot: {rows:?}");
    }
    for (scheduled, _, at) in &rows {
        assert!(at >= scheduled, "a tick ran before its slot: {rows:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn advancing_the_app_clock_runs_the_due_slot_within_a_second() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
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
    let before = c.nodes[0].as_ref().unwrap().node.app_clock().now_ms();
    c.advance_all(10 * HOUR);
    let started = std::time::Instant::now();
    let rows = beats_reach(&alice, &app, 1, 5).await;
    let took = started.elapsed();
    assert!(took < Duration::from_millis(1200), "took {took:?}");
    // The latest due slot, one tick (no burst of ten).
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].0 > before + 9 * HOUR as u64, "{rows:?}");
    assert_eq!(rows[0].1, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_tick_runs_once_across_failover() {
    let mut c = Cluster::start(3).await;
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
    let leader = c.app_leader(&app).unwrap();
    let group = c.nodes[leader].as_ref().unwrap().node.hosted(&app).unwrap();
    let last = group.status().last_log.unwrap_or(0);
    let mut offset = HOUR;
    c.advance_all(HOUR);
    // Kill the leader as soon as it has proposed the tick (its log grew).
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while group.status().last_log.unwrap_or(0) <= last {
        assert!(std::time::Instant::now() < deadline, "no tick proposed");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    drop(group);
    c.stop(leader).await;

    // A new leader takes over and ticks the next slot.
    let other = (0..3).find(|i| *i != leader).unwrap();
    let cl = c.client(other);
    let _s = cl.sign_in(&mut pk).await.unwrap();
    for _ in 0..100 {
        if c.app_leader(&app).is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    offset += HOUR;
    c.advance_all(HOUR);
    let second = c.nodes[other].as_ref().unwrap().node.app_clock().now_ms();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let rows = loop {
        let rows = beats(&cl, &app).await;
        if rows.iter().any(|r| r.2 >= second) {
            break rows;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no tick after failover: {rows:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    // The old leader returns as a follower; nothing runs twice.
    c.restart(leader, |cfg| cfg.clock_offset_ms = offset).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let rows = {
        let later = beats(&cl, &app).await;
        assert!(later.len() >= rows.len());
        later
    };
    let mut seen = slots(&rows);
    seen.dedup();
    assert_eq!(seen.len(), rows.len(), "a slot ran twice: {rows:?}");
    let mut sorted = slots(&rows);
    sorted.sort_unstable();
    assert_eq!(sorted, slots(&rows), "slots out of order: {rows:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn missed_ticks_collapse_into_one() {
    let mut c = Cluster::start(3).await;
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
    c.advance_all(HOUR);
    let first = beats_reach(&alice, &app, 1, 10).await;

    // No leader for five slots: stop the app's leader and one more node.
    let leader = c.app_leader(&app).unwrap();
    let survivor = (0..3).find(|i| *i != leader).unwrap();
    let down: Vec<usize> = (0..3).filter(|i| *i != survivor).collect();
    for i in &down {
        c.stop(*i).await;
    }
    c.advance_all(5 * HOUR);
    tokio::time::sleep(Duration::from_millis(500)).await;
    for i in &down {
        c.restart(*i, |cfg| cfg.clock_offset_ms = 6 * HOUR).await;
    }
    let cl = c.client(survivor);
    let _s = cl.sign_in(&mut pk).await.unwrap();
    let rows = beats_reach(&cl, &app, 2, 20).await;
    tokio::time::sleep(Duration::from_millis(1000)).await;
    let rows_later = beats(&cl, &app).await;
    assert_eq!(
        rows_later, rows,
        "more than one tick for the gap: {rows_later:?}"
    );
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(
        rows[1].1,
        Some(first[0].0),
        "previous is the last applied slot"
    );
    assert!(
        rows[1].0 >= first[0].0 + 4 * HOUR as u64,
        "the catch-up tick is for the latest missed slot: {rows:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_failing_tick_counts_as_applied() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
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
    c.advance_all(HOUR);
    let first = beats_reach(&alice, &app, 1, 10).await;
    // The next two slots fail (the beat returns an AppError).
    let _r = alice
        .apps()
        .call(&app, "failBeats", Some(json!({ "n": 2 })), None, None)
        .await
        .unwrap();
    for _ in 0..2 {
        let leader = c.app_leader(&app).unwrap();
        let at = c.applied_on(leader, &app);
        c.advance_all(HOUR);
        // The failed tick is logged like any call.
        applied_past(&c, leader, &app, at).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(beats(&alice, &app).await.len(), 1, "a failed tick wrote");
    }
    c.advance_all(HOUR);
    let rows = beats_reach(&alice, &app, 2, 10).await;
    assert_eq!(rows[1].0, first[0].0 + 3 * HOUR as u64, "{rows:?}");
    assert_eq!(
        rows[1].1,
        Some(first[0].0 + 2 * HOUR as u64),
        "previous is the failed slot, not the last one that wrote: {rows:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_timed_out_tick_is_not_logged_and_its_slot_runs_again() {
    let c = Cluster::start_with(3, |cfg| {
        cfg.budget = Some(calimero_app_host::Budget {
            fuel: u64::MAX,
            wall_clock: Duration::from_millis(300),
            ..calimero_app_host::Budget::call()
        });
    })
    .await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
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
    // Ticks whose entry time is within the next hour and 1.5 s spin past the deadline.
    let stall = alice
        .apps()
        .call(
            &app,
            "stallBeats",
            Some(json!({ "ms": (HOUR + 1500).to_string() })),
            None,
            None,
        )
        .await
        .unwrap();
    let until = as_u64(&stall.result);
    let leader = c.app_leader(&app).unwrap();
    let at = c.applied_on(leader, &app);
    c.advance_all(HOUR);
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(
        c.applied_on(leader, &app),
        at,
        "a timed-out tick was logged"
    );
    assert!(beats(&alice, &app).await.is_empty());
    let rows = beats_reach(&alice, &app, 1, 10).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let rows_later = beats(&alice, &app).await;
    assert_eq!(rows_later, rows, "{rows_later:?}");
    let (scheduled, previous, ran_at) = rows[0];
    assert_eq!(previous, None);
    // The slot that timed out ran again, after the stall: same slot, later entry time.
    assert!(scheduled < until, "{rows:?} until {until}");
    assert!(ran_at >= until, "{rows:?} until {until}");
}

// ---- Final-review fixes (C1, I1, I2, I3).

/// The bundle the app group of `app` runs on node `i`.
fn running_bundle(c: &Cluster, i: usize, app: &str) -> Option<String> {
    c.nodes[i]
        .as_ref()
        .unwrap()
        .node
        .hosted(app)
        .unwrap()
        .read_local(|m, _| m.state.bundle.clone())
}

/// C1 (Ruling 36; Xabi's decision: keep the node's refusal of every
/// `__` method). The spec routed `__upgrade` sent through `/intents` to the upgrade path;
/// here `/intents` and `/views` refuse every `__` method (`400 RESERVED_METHOD`) before
/// the app's executor is looked at, so a component's bundle is never switched by naming
/// one, by anyone, its owner included; upgrades take `POST /apps/{app}/upgrade`.
/// (Renamed from proto's `upgrades_through_intents_need_the_owner_or_an_org_admin`.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upgrades_through_intents_are_refused_for_everyone() {
    let c = Cluster::start(3).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let v1 = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let mut other = kv_wasm();
    other.extend_from_slice(&[0, 4, 3, b'b', b'o', b'b']);
    let bobs = bob
        .packages()
        .bundle_put(other)
        .await
        .map(|b| b.hash)
        .unwrap();
    let public = alice
        .apps()
        .create(typed(
            json!({ "name": "kv", "bundle": &v1, "access": { "kind": "public" } }),
        ))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let anonymous = c.client(2);
    let upgrade = json!({ "bundle": bobs });
    for (who, cl) in [
        ("anonymous", &anonymous),
        ("another account", &bob),
        ("the owner", &alice),
    ] {
        for method in [
            "__upgrade",
            "__blob",
            "__blob_forget",
            "__blob_revoke",
            "__calimero_anything",
        ] {
            let e = cl
                .apps()
                .call(&public, method, Some(upgrade.clone()), None, None)
                .await
                .unwrap_err();
            assert_eq!(e.code(), Some("RESERVED_METHOD"), "{who} {method}: {e}");
            let e = cl
                .apps()
                .view(
                    &public,
                    method,
                    Some(upgrade.clone()),
                    Some(ViewsRequestConsistency::Strong),
                    None,
                )
                .await
                .unwrap_err();
            assert_eq!(
                e.code(),
                Some("RESERVED_METHOD"),
                "{who} {method} view: {e}"
            );
        }
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    for i in 0..3 {
        assert_eq!(running_bundle(&c, i, &public), Some(v1.clone()), "node {i}");
    }
    assert_eq!(
        alice.apps().get(&public).await.map(wire).unwrap()["bundle"],
        json!(v1)
    );
}

/// I2 (Ruling 40): the ABI checks read the bundle the app group runs, not the directory's
/// record of it. Here the two disagree (the directory's second write of an upgrade has
/// not happened): the directory says org-gated, the group runs kv.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_abi_gate_reads_the_bundle_the_app_runs() {
    let c = Cluster::start(3).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let gated = alice
        .packages()
        .bundle_put(wasm("org-gated"))
        .await
        .map(|b| b.hash)
        .unwrap();
    let kv = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let app = alice
        .apps()
        .create(typed(json!({ "name": "app", "bundle": &gated })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let leader = c.app_leader(&app).unwrap();
    let g = c.nodes[leader].as_ref().unwrap().node.hosted(&app).unwrap();
    let _p = g
        .propose(|m, _| {
            calimero_log::Proposal::Append(
                serde_json::to_vec(&calimero_node::component::ComponentCmd::Upgrade {
                    bundle: kv.clone(),
                    compute_bundle: None,
                    base_index: m.state.last_index,
                })
                .unwrap(),
            )
        })
        .await
        .unwrap();
    for i in 0..3 {
        for _ in 0..100 {
            if running_bundle(&c, i, &app).as_deref() == Some(kv.as_str()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(running_bundle(&c, i, &app), Some(kv.clone()), "node {i}");
    }
    assert_eq!(
        alice.apps().get(&app).await.map(wire).unwrap()["bundle"],
        json!(gated)
    );

    // `wipe` is a confirm method of the running bundle: it needs a consent.
    let e = alice
        .apps()
        .call(&app, "wipe", Some(json!({})), None, None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    // Straight to the node (no challenge flow): the leader refuses it too.
    let account = alice.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let e = c.nodes[2]
        .as_ref()
        .unwrap()
        .node
        .call_component(&calimero_node::node::CallRequest::new(
            app.clone(),
            "wipe".into(),
            json!({}),
            calimero_node::types::Caller::Account { account },
        ))
        .await
        .unwrap_err();
    assert_eq!(e.details["code"], "ConfirmRequired", "{e:?}");
    // Methods only the running bundle has are reached.
    let _r = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "a", "value": "1" })),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        strong_view(&alice, &app, "get", json!({ "key": "a" })).await,
        json!("1")
    );
}

/// I3 (Ruling 41): a read-only team member's call that only emits events is refused, like
/// one that writes keys; nothing is logged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_read_only_members_emit_only_call_is_refused() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let s = |v: Value, k: &str| v[k].as_str().unwrap().to_owned();
    let org = s(
        alice.orgs().create("Docs", None).await.map(wire).unwrap(),
        "orgId",
    );
    let token = s(
        alice
            .orgs()
            .invite_create(&org, typed(json!({ "role": "member" })), Some(&mut a))
            .await
            .map(wire)
            .unwrap(),
        "token",
    );
    let _r = bob.orgs().invite_redeem(&token).await.unwrap();
    let bob_id = s(bob.auth().me().await.map(wire).unwrap(), "accountId");
    let team = s(
        alice
            .orgs()
            .team_create(&org, "Team", TeamJoin::Everyone, None)
            .await
            .map(wire)
            .unwrap(),
        "teamId",
    );
    let _ok = alice
        .orgs()
        .team_set_member(
            &org,
            &team,
            typed(json!({ "account": bob_id, "role": "readOnly" })),
            Some(&mut a),
        )
        .await
        .unwrap();
    let bundle = alice
        .packages()
        .bundle_put(wasm("org-gated"))
        .await
        .map(|b| b.hash)
        .unwrap();
    let app = alice
        .apps().create(typed(json!({ "name": "notes", "bundle": &bundle, "org": org, "access": { "kind": "team", "team": team } })))
        .await.map(|a| a.app_id)
        .unwrap();
    let leader = c.app_leader(&app).unwrap();
    let before = c.applied_on(leader, &app);
    let e = bob
        .apps()
        .call(&app, "announce", Some(json!({ "text": "hi" })), None, None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"), "{e}");
    assert_eq!(
        c.applied_on(leader, &app),
        before,
        "the refused call was logged"
    );
    // Alice (no read-only row) announces as before.
    let _r = alice
        .apps()
        .call(&app, "announce", Some(json!({ "text": "hi" })), None, None)
        .await
        .unwrap();
}

/// I1 (Ruling 39): calls run off the async workers. With two workers and two calls that
/// each run to the 2 s deadline at once, other tasks (Raft heartbeats, HTTP) keep running.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slow_calls_do_not_stall_the_async_workers() {
    let c = Cluster::start_with(3, |cfg| {
        cfg.budget = Some(calimero_app_host::Budget {
            fuel: u64::MAX,
            ..calimero_app_host::Budget::call()
        });
    })
    .await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let a = alice
        .apps()
        .create(typed(json!({ "name": "a", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let b = alice
        .apps()
        .create(typed(json!({ "name": "b", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();

    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let probe = {
        let stop = Arc::clone(&stop);
        tokio::spawn(async move {
            let mut worst = Duration::ZERO;
            let mut last = std::time::Instant::now();
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(10)).await;
                let now = std::time::Instant::now();
                worst = worst.max(now - last);
                last = now;
            }
            worst
        })
    };
    let (ra, rb) = tokio::join!(
        alice.apps().call(&a, "spin", Some(json!({})), None, None),
        alice.apps().call(&b, "spin", Some(json!({})), None, None),
    );
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let worst = probe.await.unwrap();
    assert_eq!(ra.unwrap_err().code(), Some("TIMEOUT"));
    assert_eq!(rb.unwrap_err().code(), Some("TIMEOUT"));
    assert!(
        worst < Duration::from_millis(500),
        "a task waited {worst:?} for a worker"
    );
}

/// Ruling 44: concurrent first uses of a bundle wait for one prepare. Without this, 200
/// first calls compiled Rooms ~130 times at once and overflowed rayon's stacks in debug.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_first_uses_of_a_bundle_prepare_it_once() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let node = Arc::clone(&c.nodes[0].as_ref().unwrap().node);
    let before = node.prepares();
    let runs = (0..16).map(|_| {
        let (node, bundle) = (Arc::clone(&node), bundle.clone());
        tokio::spawn(async move { node.prepared_for(&bundle).await.map(|p| p.hash) })
    });
    for r in futures_util::future::join_all(runs).await {
        assert_eq!(r.unwrap().unwrap(), bundle);
    }
    assert_eq!(node.prepares() - before, 1, "prepared more than once");
}

/// C1 on the node's upgrade path (Xabi's decision: keep the node's check). Who
/// may upgrade is judged where the system log applies `UpgradeApp`, on every replica
/// (`SystemState::may_upgrade`: the owner, or `MANAGE_APPLICATION` in the app's
/// organization), and by the upgrade route before anything runs. Neither another
/// account (on a personal component app) nor an org member without
/// `MANAGE_APPLICATION` (on an org component app) gets an upgrade through, by the route
/// or by an `UpgradeApp` written straight into the system log past the front check,
/// through a follower or through the app's leader; each app keeps its bundle on every
/// replica and in the directory. The owner's own upgrade then goes through, so the
/// refusals are about authority. (The name is proto's; on this branch the check that
/// holds is the system log's, not a leader-side re-check.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_leader_checks_upgrade_authority_again() {
    use calimero_node::system::SystemCmd;

    let c = Cluster::start(3).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let v1 = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let personal = alice
        .apps()
        .create(typed(
            json!({ "name": "kv", "bundle": &v1, "access": { "kind": "public" } }),
        ))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let org = alice.orgs().create("Acme", None).await.map(wire).unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let in_org = alice
        .apps()
        .create(typed(
            json!({ "name": "kv-org", "bundle": &v1, "org": org }),
        ))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let mut other = kv_wasm();
    other.extend_from_slice(&[0, 4, 3, b'b', b'o', b'b']);
    let v2 = alice
        .packages()
        .bundle_put(other)
        .await
        .map(|b| b.hash)
        .unwrap();
    // Bob: another account. Carol: a member of Acme, without MANAGE_APPLICATION.
    let (bob, mut bob_pk) = signed_up(&c, 1, "bob").await;
    let (carol, mut carol_pk) = signed_up(&c, 2, "carol").await;
    let token = alice
        .orgs()
        .invite_create(
            &org,
            typed(json!({ "role": "member" })),
            Some(&mut alice_pk),
        )
        .await
        .map(wire)
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = carol.orgs().invite_redeem(&token).await.unwrap();
    let id_of = |v: Value| v["accountId"].as_str().unwrap().to_owned();
    let bob_id = id_of(bob.auth().me().await.map(wire).unwrap());
    let carol_id = id_of(carol.auth().me().await.map(wire).unwrap());

    for (app, who, id, pk) in [
        (&personal, "another account", &bob_id, &mut bob_pk),
        (
            &in_org,
            "a member without MANAGE_APPLICATION",
            &carol_id,
            &mut carol_pk,
        ),
    ] {
        let leader = c.app_leader(app).unwrap();
        let follower = (leader + 1) % 3;
        for i in [follower, leader] {
            // Through the upgrade route, on either node: refused before anything runs.
            let cl = c.client(i);
            let _s = cl.sign_in(pk).await.unwrap();
            let e = cl
                .apps()
                .upgrade(app, typed(json!({ "bundle": v2 })), Some(&mut *pk))
                .await
                .unwrap_err();
            assert_eq!(e.code(), Some("FORBIDDEN"), "{who}, route, node {i}: {e}");
            // Past the front check, straight into the system log, with the
            // actor's own valid consent: refused at apply, for authority.
            let statement =
                calimero_node::proof::Statement::new(app, "__upgrade", &json!({ "bundle": v2 }));
            let consent = serde_json::to_value(calimero_node::proof::Consent {
                assertion: pk.assert(&statement.challenge().unwrap()),
                statement,
                at: 0,
            })
            .unwrap();
            let e = c.nodes[i]
                .as_ref()
                .unwrap()
                .node
                .sys_write(&SystemCmd::UpgradeApp {
                    compute_bundle: None,
                    app_id: app.clone(),
                    bundle: v2.clone(),
                    by_operator: false,
                    actor: id.clone(),
                    at: calimero_node::types::now_secs(),
                    run: None,
                    consent,
                    release: None,
                    owner_in_org: true,
                    owner_in_team: true,
                })
                .await
                .unwrap_err();
            assert_eq!(e.code, "REJECTED", "{who}, log, node {i}: {e:?}");
            assert!(
                e.message.contains("needs its owner"),
                "{who}, node {i}: {e:?}"
            );
        }
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    for app in [&personal, &in_org] {
        for i in 0..3 {
            assert_eq!(
                running_bundle(&c, i, app),
                Some(v1.clone()),
                "{app} node {i}"
            );
        }
        assert_eq!(
            alice.apps().get(app).await.map(wire).unwrap()["bundle"],
            json!(v1)
        );
    }
    // The owner may.
    let up = alice
        .apps()
        .upgrade(
            &personal,
            typed(json!({ "bundle": v2 })),
            Some(&mut alice_pk),
        )
        .await
        .map(wire)
        .unwrap();
    assert_eq!(up["status"], "ok", "{up}");
    assert_eq!(
        alice.apps().get(&personal).await.map(wire).unwrap()["bundle"],
        json!(v2)
    );
}

/// Node `i`'s group of `app`, once the node (re)started it, up to ~10 s.
async fn hosted_on(
    c: &Cluster,
    i: usize,
    app: &str,
) -> Arc<calimero_log::Group<calimero_node::app::AppMachine>> {
    for _ in 0..200 {
        if let Some(g) = c.nodes[i].as_ref().unwrap().node.hosted(app) {
            return g;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("node {i} never hosted {app}");
}

/// Calls `newId` until one answers (the app may be electing), up to ~10 s.
async fn new_id(cl: &Client, app: &str) -> (u64, u64, u64) {
    for _ in 0..40 {
        if let Ok(r) = cl
            .apps()
            .call(app, "newId", Some(json!({})), None, None)
            .await
        {
            return id_parts(&r.result);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("no newId answer");
}

/// D52, the crash this branch's log makes possible, made deterministic. The log
/// does not sync the commit index or the applied batch on every write; one test
/// hook (`Engine::lose_unsynced_writes`) drops both for the app's group, as a
/// crash right after the leader appended an entry would: the entry is in the
/// synced log, but neither its commit nor its apply reaches the disk. The leader
/// restarts into the same term (a one-replica group), its counter memory gone,
/// with that entry past its saved commit index and applied index.
///
/// Whether the restarted leader re-applies the entry before its first proposal is
/// built is a second race (openraft re-commits the tail at startup); a second hook
/// (`calimero_log::testing::hold_apply`) holds every apply of the group from the
/// restart until the test releases it, so the first call is sent while the lost
/// entry is certainly not applied. Its id must not be the lost entry's, nor any
/// id issued before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unique_ids_survive_a_crash_before_the_commit_index_reaches_the_disk() {
    let mut c = Cluster::start(1).await;
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
    let mut issued = Vec::new();
    for _ in 0..3 {
        issued.push(new_id(&alice, &app).await);
    }
    let term = issued[0].0;
    let dir = c.configs[0].data_dir.clone();
    let node = &c.nodes[0].as_ref().unwrap().node;
    let group = node.hosted(&app).unwrap();
    let group_id = group.id();
    let durable = group.read_local(|m, _| m.state.component.head.last_id);
    node.engine.lose_unsynced_writes(group_id);
    // Appended and synced; its commit and apply never reach the disk.
    let lost = new_id(&alice, &app).await;
    issued.push(lost);
    assert!((lost.0, lost.1) > durable, "{lost:?} after {durable:?}");
    drop(group);

    calimero_log::testing::hold_apply(&dir, group_id);
    c.restart(0, |_| {}).await;
    let group = hosted_on(&c, 0, &app).await;
    let first = {
        let (url, app) = (c.urls[0].clone(), app.clone());
        let mut pk2 = SoftAuthenticator::new("localhost", ORIGIN);
        std::mem::swap(&mut pk, &mut pk2);
        tokio::spawn(async move {
            let cl = Client::connect(
                &url,
                Policy {
                    allow_mock: true,
                    ..Policy::default()
                },
            )
            .unwrap();
            let mut signed = false;
            for _ in 0..40 {
                if cl.sign_in(&mut pk2).await.is_ok() {
                    signed = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            assert!(signed, "no sign-in after the restart");
            (new_id(&cl, &app).await, pk2)
        })
    };
    // The precondition, held for a second while the first call is under way:
    // the lost entry is in the log, not in the applied state.
    tokio::time::sleep(Duration::from_millis(1000)).await;
    let held = group.read_local(|m, _| m.state.component.head.last_id);
    assert!(
        held < (lost.0, lost.1),
        "the lost entry applied while held: {held:?}"
    );
    assert!(
        group.status().last_log.unwrap_or(0) > group.applied(),
        "the restarted leader's log does not run past its applied state"
    );
    let conflicts = || {
        c.nodes[0]
            .as_ref()
            .unwrap()
            .node
            .conflicts
            .load(std::sync::atomic::Ordering::Relaxed)
    };
    let conflicts_before = conflicts();
    calimero_log::testing::release_apply(&dir, group_id);
    let (first, back) = first.await.unwrap();
    // The first proposal was built on the lost entry (the leader caught up with its
    // own log first), not on the state before it: no entry carrying the lost entry's
    // id was appended to be applied as a conflict and run again. (Without the
    // catch-up, `base_index` still keeps that reused id out of the state: the
    // stale entry applies after the lost one, as a no-op, and is re-run.)
    assert_eq!(
        conflicts() - conflicts_before,
        0,
        "the first proposal after the restart was built before the leader applied its own log"
    );
    pk = back;
    let cl = c.client(0);
    let _s = cl.sign_in(&mut pk).await.unwrap();
    let mut after = vec![first];
    for _ in 0..2 {
        after.push(new_id(&cl, &app).await);
    }
    println!("issued {issued:?}; durable {durable:?}; held {held:?}; after {after:?}");
    // The test proves what it claims only if the restart kept the term.
    assert!(
        after.iter().all(|a| a.0 == term),
        "{issued:?} then {after:?}"
    );
    for a in &after {
        assert!(!issued.contains(a), "id reused: {issued:?} then {after:?}");
        assert!(
            (a.0, a.1) > (lost.0, lost.1),
            "an id at or before the lost entry's: {issued:?} then {after:?}"
        );
    }
}

/// D52, D34 and the host records through a snapshot install: a follower that was
/// down while the log moved past what the leader keeps installs the leader's
/// snapshot, then leads. Its `last_id`, timer slots, `install_ts_ms` and
/// executor came with the snapshot: no unique id is reused, and no timer slot
/// runs twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_snapshot_install_carries_a_components_host_records() {
    let mut c = Cluster::start_with(3, |cfg| cfg.raft.snapshot_every = 20).await;
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
    let leader = c.app_leader(&app).unwrap();
    let lagging = (leader + 1) % 3;
    let other = (leader + 2) % 3;
    let mut issued = vec![new_id(&alice, &app).await];
    let lagging_applied = c.applied_on(lagging, &app);
    c.stop(lagging).await;

    // While it is down: a tick, and enough entries that the leader's log no
    // longer reaches back to where the stopped node is (it keeps 100 past a snapshot).
    c.advance_all(HOUR);
    let cl = c.client(other);
    let _s = cl.sign_in(&mut pk).await.unwrap();
    let ticked = beats_reach(&cl, &app, 1, 10).await;
    for _ in 0..300 {
        issued.push(new_id(&cl, &app).await);
    }
    let held = {
        let g = c.nodes[leader].as_ref().unwrap().node.hosted(&app).unwrap();
        g.snapshot_now().await.unwrap();
        g.read_local(|m, _| m.state.component.head.clone())
    };

    // It comes back (its own snapshots off, so a snapshot it holds is the leader's)
    // on app time as the others see it.
    c.restart(lagging, |cfg| {
        cfg.raft.snapshot_every = 1_000_000;
        cfg.clock_offset_ms = HOUR;
    })
    .await;
    let target = c.applied_on(leader, &app);
    let group = hosted_on(&c, lagging, &app).await;
    let node = &c.nodes[lagging].as_ref().unwrap().node;
    let snapshot = || {
        node.engine
            .snapshot_file(group.id())
            .unwrap()
            .map(|(index, _, _)| index)
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while group.applied() < target || snapshot().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "node {lagging} did not catch up to {target}: applied {}, snapshot {:?}",
            group.applied(),
            snapshot()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let installed = snapshot();
    assert!(
        installed.is_some_and(|i| i > lagging_applied),
        "no snapshot installed past {lagging_applied}: {installed:?}"
    );
    let restored = group.read_local(|m, _| m.state.component.head.clone());
    assert_eq!(restored.install_ts_ms, held.install_ts_ms);
    assert_eq!(restored.timers, held.timers);
    assert!(restored.last_id >= held.last_id, "{restored:?} vs {held:?}");
    drop(group);

    // It leads: the leader goes, and the restored node calls the election.
    c.stop(leader).await;
    let group = hosted_on(&c, lagging, &app).await;
    for _ in 0..100 {
        if c.app_leader(&app) == Some(lagging) {
            break;
        }
        let _r = group.raft().trigger().elect().await;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    drop(group);
    assert_eq!(c.app_leader(&app), Some(lagging), "the restored node leads");
    let cl = c.client(lagging);
    let _s = cl.sign_in(&mut pk).await.unwrap();
    // The slot applied before the snapshot is due again by the clock, never run again.
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert_eq!(beats(&cl, &app).await, ticked, "a slot ran twice");
    for _ in 0..3 {
        let id = new_id(&cl, &app).await;
        assert!(!issued.contains(&id), "id reused: {id:?}");
        issued.push(id);
    }
    // The next slot runs once.
    for r in c.nodes.iter().flatten() {
        r.node.advance_clock(HOUR);
    }
    let rows = beats_reach(&cl, &app, 2, 10).await;
    let mut seen = slots(&rows);
    seen.dedup();
    assert_eq!(seen.len(), rows.len(), "a slot ran twice: {rows:?}");
}

/// No new SDK 0.12 app: a core module (no `calimero:app` component) is refused as a
/// bundle (`400 UNSUPPORTED_BUNDLE`), whether uploaded raw or as a signed package's
/// service, so no app is created from one and no app is upgraded to one; the
/// refusal says to rebuild with calimero-sdk.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_core_module_bundle_is_refused_as_an_sdk_0_12_app() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    // `(module (memory (export "memory") 1))`: a valid core module, as SDK 0.12 built.
    let core = b"\0asm\x01\0\0\0\x05\x03\x01\x00\x01\x07\x0a\x01\x06memory\x02\x00".to_vec();
    let refused = |e: calimero_client::Error| {
        let status = match &e {
            calimero_client::Error::Api { status, .. } => *status,
            other => panic!("{other:?}"),
        };
        assert_eq!((status, e.code()), (400, Some("UNSUPPORTED_BUNDLE")), "{e}");
        assert!(e.to_string().contains("calimero-sdk"), "{e}");
    };
    refused(
        alice
            .packages()
            .bundle_put(core.clone())
            .await
            .map(|b| b.hash)
            .unwrap_err(),
    );
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[4; 32]);
    refused(
        alice
            .packages()
            .upload(mpk_declaring(
                "com.test.core",
                "0.1.0",
                &core,
                &publisher,
                &[],
            ))
            .await
            .unwrap_err(),
    );
    assert!(
        alice
            .packages()
            .list()
            .await
            .map(wire)
            .unwrap()
            .to_string()
            .find("com.test.core")
            .is_none(),
        "no package was recorded"
    );
    // The same node takes the component.
    let ok = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let _app = alice
        .apps()
        .create(typed(json!({ "name": "kv", "bundle": &ok })))
        .await
        .map(|a| a.app_id)
        .unwrap();
}

/// Decision 14: the endpoints that read an app's code or storage answer for a
/// component: `/abi` is the ABI the component embeds, `/storage` counts its
/// keys, and the blob routes refuse it (no blobs before R8).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_abi_storage_and_blob_routes_answer_for_a_component() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let component = kv_wasm();
    let embedded = calimero_app_build::abi_of(&component).unwrap().unwrap();
    let bundle = alice
        .packages()
        .bundle_put(component)
        .await
        .map(|b| b.hash)
        .unwrap();
    let app = alice
        .apps()
        .create(typed(json!({ "name": "kv", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let _r = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "a", "value": "1" })),
            None,
            None,
        )
        .await
        .unwrap();

    let abi = alice.apps().abi(&app).await.map(wire).unwrap();
    assert_eq!(abi["bundle"], json!(bundle));
    assert_eq!(abi["abi"], embedded, "not the component's own ABI");
    assert!(abi["abi"]["methods"]["set"].is_object());

    let storage = alice.apps().storage(&app).await.map(wire).unwrap();
    assert!(
        storage["keys"].as_u64().is_some_and(|k| k >= 1),
        "{storage}"
    );
    assert!(
        storage["bytes"].as_u64().is_some_and(|b| b > 0),
        "{storage}"
    );
    assert_eq!(storage["blobs"], json!(0));

    // The blob routes answer for a component (R8): nothing uploaded yet.
    assert_eq!(
        alice.apps().blob_list(&app).await.map(wire).unwrap()["blobs"],
        json!([])
    );
    let id = "00".repeat(32);
    let e = alice.apps().blob_info(&app, &id).await.unwrap_err();
    assert_eq!(e.code(), Some("NOT_FOUND"), "{e}");
    let e = alice
        .apps()
        .blob_download(&app, &id, None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NOT_FOUND"), "{e}");
}

/// R8: a component's blob is uploaded through the node, referred to from state by a
/// typed `BlobRef`, read and granted from runs, and held for as long as state refers
/// to it; once idle past its time every replica forgets it and the bytes go.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_components_blobs_are_held_by_references_and_granted() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
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

    let up = alice
        .transport()
        .json(
            Request::new(Method::POST, format!("apps/{app}/blobs"))
                .bytes("text/plain", b"hello".to_vec()),
        )
        .await
        .unwrap();
    let id = up["id"].as_str().unwrap().to_owned();
    assert_eq!(up["size"], json!(5));
    assert_eq!(
        alice.apps().blob_info(&app, &id).await.map(wire).unwrap()["size"],
        json!(5)
    );
    let r = alice
        .apps()
        .call(
            &app,
            "attach",
            Some(json!({ "name": "greeting", "blob": id })),
            None,
            None,
        )
        .await
        .unwrap();
    for i in 0..3 {
        applied_past(&c, i, &app, r.index - 1).await;
    }
    assert_eq!(
        strong_view(&alice, &app, "file", json!({ "name": "greeting" })).await,
        json!({ "id": id, "size": "5", "mime": "text/plain" })
    );
    assert_eq!(
        alice.apps().blob_download(&app, &id, None).await.unwrap(),
        b"hello"
    );

    // A grant from a run downloads without a session, on any node, until it expires.
    let grant = strong_view(
        &alice,
        &app,
        "share",
        json!({ "name": "greeting", "ttl": 60 }),
    )
    .await;
    let grant = grant.as_str().unwrap().to_owned();
    let anonymous = c.client(2);
    assert_eq!(
        anonymous
            .apps()
            .blob_download(&app, &id, Some(&grant))
            .await
            .unwrap(),
        b"hello"
    );
    let e = anonymous
        .apps()
        .blob_download(&app, &id, None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("UNAUTHENTICATED"), "{e}");
    let forged = format!("9{grant}");
    let e = anonymous
        .apps()
        .blob_download(&app, &id, Some(&forged))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NOT_FOUND"), "{e}");

    // A value may refer only to a blob the app holds.
    let e = alice
        .apps()
        .call(
            &app,
            "attach",
            Some(json!({ "name": "x", "blob": "11".repeat(32) })),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("TRAP"), "{e}");

    // An upload nobody refers to, and the referred one once detached, go after their
    // idle time; the referred one stays until then.
    let idle = alice
        .transport()
        .json(
            Request::new(Method::POST, format!("apps/{app}/blobs"))
                .bytes("text/plain", b"never attached".to_vec()),
        )
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    c.advance_all(2 * 3_600_000);
    let _r = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "tick", "value": "1" })),
            None,
            None,
        )
        .await
        .unwrap();
    let e = alice.apps().blob_info(&app, &idle).await.unwrap_err();
    assert_eq!(e.code(), Some("NOT_FOUND"), "{e}");
    assert_eq!(
        alice.apps().blob_info(&app, &id).await.map(wire).unwrap()["size"],
        json!(5)
    );
    let _r = alice
        .apps()
        .call(
            &app,
            "detach",
            Some(json!({ "name": "greeting" })),
            None,
            None,
        )
        .await
        .unwrap();
    c.advance_all(2 * 3_600_000);
    let _r = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "tick", "value": "2" })),
            None,
            None,
        )
        .await
        .unwrap();
    let mut gone = false;
    for _ in 0..50 {
        if alice.apps().blob_list(&app).await.map(wire).unwrap()["blobs"] == json!([])
            && alice.apps().blob_download(&app, &id, None).await.is_err()
        {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(gone, "an idle blob stayed");
    // The bytes left every node's store too (the leader collected them).
    for _ in 0..50 {
        if c.nodes.iter().flatten().all(|r| {
            r.node.engine.get_blob(&id).ok().flatten().is_none()
                && r.node.engine.get_blob(&idle).ok().flatten().is_none()
        }) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the bytes of a forgotten blob stayed in a store");
}

/// Spec section 7: the host keeps a full-text index of a table's declared fields from the
/// component's state, and views on any replica query it as of the state they read.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn views_search_a_components_tables_as_they_change() {
    let c = Cluster::start(3).await;
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
    let mut last = 0;
    for (id, title, body) in [
        ("1", "Groceries", "milk and eggs"),
        ("2", "Trip", "pack the eggs carefully"),
        ("3", "Work", "quarterly plans"),
    ] {
        last = alice
            .apps()
            .call(
                &app,
                "note",
                Some(json!({ "id": id, "title": title, "body": body })),
                None,
                None,
            )
            .await
            .unwrap()
            .index;
    }
    let sorted = |v: Value| {
        let mut ids: Vec<String> = serde_json::from_value(v).unwrap();
        ids.sort();
        ids
    };
    assert_eq!(
        sorted(strong_view(&alice, &app, "findNotes", json!({ "text": "eggs" })).await),
        ["1", "2"]
    );
    // Every replica builds its own index from the state it applied.
    for i in 0..3 {
        let cl = c.client(i);
        let _s = cl.sign_in(&mut pk).await.unwrap();
        let v = cl
            .apps()
            .view(
                &app,
                "findNotes",
                Some(json!({ "text": "quarterly" })),
                Some(ViewsRequestConsistency::Stale),
                Some(last),
            )
            .await
            .unwrap();
        assert_eq!(v.result, json!(["3"]), "node {i}");
    }
    // Updates and removals reach the index before the next view reads.
    let _r = alice
        .apps()
        .call(
            &app,
            "note",
            Some(json!({ "id": "3", "title": "Work", "body": "eggs too" })),
            None,
            None,
        )
        .await
        .unwrap();
    let _r = alice
        .apps()
        .call(&app, "dropNote", Some(json!({ "id": "1" })), None, None)
        .await
        .unwrap();
    assert_eq!(
        sorted(strong_view(&alice, &app, "findNotes", json!({ "text": "eggs" })).await),
        ["2", "3"]
    );
    // A call of a declared view runs as a strong view, so it may search too.
    let r = alice
        .apps()
        .call(
            &app,
            "findNotes",
            Some(json!({ "text": "eggs" })),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(sorted(r.result), ["2", "3"]);
}

/// D25/D29: a component's xcalls wait in its outbox and are delivered after the entry
/// commits, in order, as `Caller::App(source)` to a method that allows it; one that can
/// never be delivered (a missing target, another organization's app) is dead-lettered and
/// the source's `onXcallFailed` is told; a target's refusal is its final answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn component_xcalls_are_delivered_in_order_or_dead_lettered() {
    let c = Cluster::start(3).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let source = alice
        .apps()
        .create(typed(json!({ "name": "source", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let target = alice
        .apps()
        .create(typed(json!({ "name": "target", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let bobs = bob
        .apps()
        .create(typed(json!({ "name": "bobs", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();

    for i in 0..5 {
        let _r = alice
            .apps()
            .call(
                &source,
                "relay",
                Some(json!({ "app": target, "key": "k", "value": format!("{i}") })),
                None,
                None,
            )
            .await
            .unwrap();
    }
    // Delivered in order: the last value wins, and the outbox empties.
    let mut last = Value::Null;
    for _ in 0..100 {
        last = strong_view(&alice, &target, "get", json!({ "key": "k" })).await;
        let pending = alice.apps().xcalls(&source).await.map(wire).unwrap()["pending"].clone();
        if last == json!("4") && pending == json!(0) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(last, json!("4"));
    assert_eq!(
        alice.apps().xcalls(&source).await.map(wire).unwrap()["deadLetters"],
        json!([])
    );

    // The target refuses a method that allows no app: a final answer, no dead letter.
    let _r = alice
        .apps()
        .call(
            &source,
            "relayToSet",
            Some(json!({ "app": target, "key": "z", "value": "1" })),
            None,
            None,
        )
        .await
        .unwrap();
    // A missing target and another owner's app are dead-lettered at once.
    let _r = alice
        .apps()
        .call(
            &source,
            "relay",
            Some(json!({ "app": "no-such-app", "key": "a", "value": "1" })),
            None,
            None,
        )
        .await
        .unwrap();
    let _r = alice
        .apps()
        .call(
            &source,
            "relay",
            Some(json!({ "app": bobs, "key": "b", "value": "1" })),
            None,
            None,
        )
        .await
        .unwrap();
    let mut dead = Vec::new();
    for _ in 0..100 {
        let x = alice.apps().xcalls(&source).await.map(wire).unwrap();
        dead = x["deadLetters"].as_array().cloned().unwrap_or_default();
        if dead.len() == 2 && x["pending"] == json!(0) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(dead.len(), 2, "{dead:?}");
    assert_eq!(dead[0]["app"], json!("no-such-app"));
    assert!(dead[0]["reason"]
        .as_str()
        .unwrap()
        .contains("does not exist"));
    assert_eq!(dead[1]["app"], json!(bobs));
    assert!(dead[1]["reason"].as_str().unwrap().contains("organization"));
    assert_eq!(
        strong_view(&alice, &target, "get", json!({ "key": "z" })).await,
        json!(null),
        "a method that allows no app ran"
    );
    assert_eq!(
        strong_view(&bob, &bobs, "get", json!({ "key": "b" })).await,
        json!(null),
        "another owner's app was reached"
    );
    // The source's onXcallFailed heard of the dead letters.
    let mut failures = Value::Null;
    for _ in 0..50 {
        failures = strong_view(&alice, &source, "xcallFailures", json!({})).await;
        if failures.as_object().is_some_and(|f| !f.is_empty()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        failures["setFromApp"]
            .as_str()
            .is_some_and(|f| f.contains(&bobs)),
        "{failures}"
    );
    // Only the owner reads them.
    let e = bob.apps().xcalls(&source).await.unwrap_err();
    assert_eq!(e.code(), Some("NOT_FOUND"), "{e}");
}

/// The upgrade suite's case: an app's dead letters move with it. A new replica catches
/// up from the log, the send and its dead-lettering applied in one batch, and lists the
/// dead letter its leader wrote; a node that no longer holds a replica answers
/// `GET /apps/{app}/xcalls` from one, as the other app routes do.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn dead_letters_move_with_the_app_and_any_node_lists_them() {
    let c = Cluster::start(4).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let source = alice
        .apps()
        .create(typed(json!({ "name": "source", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let _r = alice
        .apps()
        .call(
            &source,
            "relay",
            Some(json!({ "app": "no-such-app", "key": "a", "value": "1" })),
            None,
            None,
        )
        .await
        .unwrap();
    let dead_of = |x: &Value| x["deadLetters"].as_array().cloned().unwrap_or_default();
    let mut before = Vec::new();
    for _ in 0..100 {
        before = dead_of(&alice.apps().xcalls(&source).await.map(wire).unwrap());
        if !before.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(before.len(), 1, "{before:?}");
    assert_eq!(before[0]["app"], json!("no-such-app"));

    // Move it off a follower onto the spare node, a learner catching up from the log.
    let info = alice.apps().get(&source).await.map(wire).unwrap();
    let replicas: Vec<u64> = serde_json::from_value(info["replicas"].clone()).unwrap();
    let spare = (1..=4).find(|n| !replicas.contains(n)).unwrap();
    let leader = c.app_leader(&source).unwrap() as u64 + 1;
    let off = *replicas.iter().find(|r| **r != leader).unwrap();
    let moved: Vec<u64> = replicas
        .iter()
        .copied()
        .filter(|r| *r != off)
        .chain([spare])
        .collect();
    let ops = c.client(leader as usize - 1);
    ops.use_api_key("ops-secret").await;
    let _ = ops
        .ops()
        .set_replicas(&source, typed(json!(moved)), Some(&common::operator_sign))
        .await
        .unwrap();

    // The new replica's own state, once it applied the dead-lettering: the letter is there.
    let node = &c.nodes[spare as usize - 1].as_ref().unwrap().node;
    let mut caught_up = None;
    for _ in 0..200 {
        caught_up = node.hosted(&source).and_then(|g| {
            g.read_local(|m, _| (m.state.component.head.xcall.first == 1).then(|| m.dead_letters()))
        });
        if caught_up.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let local = caught_up.expect("the new replica never applied the dead-lettering");
    assert_eq!(
        serde_json::to_value(&local).unwrap(),
        Value::Array(before.clone()),
        "the new replica lost the dead letter"
    );

    // Every node lists it: the new replica, and the node the app moved off, by forwarding.
    let session = alice.session().await.unwrap();
    for n in [spare, off] {
        let cl = c.client(n as usize - 1);
        cl.use_session(session.clone()).await;
        let x = cl
            .apps()
            .xcalls(&source)
            .await
            .map(wire)
            .unwrap_or_else(|e| panic!("node {n}: {e}"));
        assert_eq!(dead_of(&x), before, "node {n}");
    }
}

/// A signed `.mpk` of `wasm` as `package@version` whose manifest declares the outbound
/// URL prefixes `outbound` (S9), signed by `key`.
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
        compute: None,
        links: None,
        frontend: None,
        frontend_aliases: Vec::new(),
        upgrades: None,
        outbound: outbound.iter().map(|u| (*u).to_owned()).collect(),
        services: None,
        pipelines: Vec::new(),
        signature: None,
    };
    let mut manifest = serde_json::to_value(&manifest).unwrap();
    calimero_bundle::sign_manifest_json(&mut manifest, key).unwrap();
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

/// Polls view `method` until `done` holds of its result (at most 10 s).
async fn eventually(
    cl: &Client,
    app: &str,
    method: &str,
    args: Value,
    done: impl Fn(&Value) -> bool,
) -> Value {
    let mut last = Value::Null;
    for _ in 0..100 {
        last = strong_view(cl, app, method, args.clone()).await;
        if done(&last) {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{method} never got there: {last}");
}

/// D12, D17, D22, S9, R14: an action runs on the app's leader outside the log, reads
/// through a view, calls out over `wasi:http` only to a prefix the signed manifest
/// declares and the owner approved (with a secret the node fills in and redacts), and
/// writes through a keyed call to an `allow(action)` method no client can call. One that
/// fails (a refused request, an app error) fails for good and `onActionFailed` hears.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn actions_run_on_the_leader_and_reach_only_declared_approved_hosts() {
    const PREFIX: &str = "https://echo.outbound.test/v1/";
    const PACKAGE: &str = "com.calimero.v2-kv";
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let signer = calimero_bundle::derive_signer_id_did_key(publisher.verifying_key().as_bytes());
    let settings = calimero_outbound::OutboundSettings {
        enabled: true,
        hosts: vec!["echo.outbound.test".to_owned()],
        testing: Some(calimero_outbound::OutboundTestSettings {
            echo: true,
            turnstile: false,
            secrets: [(
                "ECHO_KEY".to_owned(),
                calimero_outbound::TestSecretSettings {
                    value: "s3cret-value".to_owned(),
                    packages: vec![PACKAGE.to_owned()],
                    signers: vec![signer],
                    hosts: vec!["echo.outbound.test".to_owned()],
                },
            )]
            .into(),
        }),
        ..calimero_outbound::OutboundSettings::default()
    };
    let service = calimero_outbound::OutboundService::from_settings(&settings).unwrap();
    let c = Cluster::start_with(3, |cfg| cfg.outbound.clone_from(&service)).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let _ = alice
        .packages()
        .upload(mpk_declaring(
            PACKAGE,
            "0.1.0",
            &kv_wasm(),
            &publisher,
            &[PREFIX],
        ))
        .await
        .unwrap();
    let app = alice
        .apps()
        .create(typed(json!({ "name": "kv", "package": PACKAGE })))
        .await
        .map(wire)
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "acme", "value": "acme.com" })),
            None,
            None,
        )
        .await
        .unwrap();

    // Not approved yet: the request is refused, the action fails, onActionFailed hears.
    let started = alice
        .apps()
        .call(&app, "enrich", Some(json!({ "key": "acme" })), None, None)
        .await
        .unwrap();
    let id = started.result["actionId"].as_str().unwrap().to_owned();
    let failures = eventually(&alice, &app, "actionFailures", json!({}), |f| {
        f["enrich"].is_string()
    })
    .await;
    let failure = failures["enrich"].as_str().unwrap();
    assert!(failure.starts_with(&id), "{failure}");
    assert!(failure.contains("NotApproved"), "{failure}");

    // Approved: the action calls out and writes what came back through its own call.
    let _ = alice
        .orgs()
        .outbound_approval(
            &alice.personal_org().await.unwrap(),
            PACKAGE,
            Some(typed(json!("manifest"))),
            Some(false),
            Some(&mut pk),
        )
        .await
        .map(wire)
        .unwrap();
    let id = alice
        .apps()
        .call(
            &app,
            "startEnrich",
            Some(json!({ "key": "acme" })),
            None,
            None,
        )
        .await
        .unwrap()
        .result;
    assert!(id.as_str().unwrap().starts_with("a."), "{id}");
    let got = eventually(&alice, &app, "enrichment", json!({ "key": "acme" }), |v| {
        v.is_string()
    })
    .await;
    let got = got.as_str().unwrap();
    assert!(got.starts_with("200 "), "{got}");
    assert!(got.contains("/v1/enrich?v=acme.com"), "{got}");
    // The secret went out and came back redacted.
    assert!(!got.contains("s3cret-value"), "{got}");
    assert!(got.contains("[redacted]"), "{got}");

    // No client calls an `allow(action)` method, and a call or view never calls out.
    let e = alice
        .apps()
        .call(
            &app,
            "applyEnrichment",
            Some(json!({ "key": "acme", "value": "forged" })),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("APP_ERROR"), "{e}");

    // An app error fails an action at once (D22).
    let _r = alice
        .apps()
        .call(&app, "explode", Some(json!({})), None, None)
        .await
        .unwrap();
    let failures = eventually(&alice, &app, "actionFailures", json!({}), |f| {
        f["explode"].is_string()
    })
    .await;
    assert!(
        failures["explode"].as_str().unwrap().contains("BeatFailed"),
        "{failures}"
    );
}

/// A synchronous xcall: the forms app (kv) verifies a captcha answer with the captcha
/// service, another app of the same owner, inside its own call, and writes only if it
/// passed. The challenge is spent in the captcha app's own log; a second use fails; an
/// app of another owner cannot reach the service.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_call_verifies_a_captcha_with_another_app_in_the_same_request() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let captcha_bundle = alice
        .packages()
        .bundle_put(wasm("captcha"))
        .await
        .map(|b| b.hash)
        .unwrap();
    let captcha = alice
        .apps()
        .create(typed(
            json!({ "name": "captcha", "bundle": &captcha_bundle }),
        ))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let kv_bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let forms = alice
        .apps()
        .create(typed(json!({ "name": "forms", "bundle": &kv_bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();

    let puzzle = alice
        .apps()
        .call(&captcha, "challenge", Some(json!({})), None, None)
        .await
        .unwrap()
        .result;
    let id = puzzle["id"].as_str().unwrap().to_owned();
    let answer = strong_view(&alice, &captcha, "answerOf", json!({ "id": id })).await;
    let answer = answer.as_str().unwrap().to_owned();
    let human = |value: &str, answer: &str| json!({ "key": "k", "value": value, "captcha": captcha, "challenge": id, "answer": answer });

    // A person cannot spend a challenge directly.
    let e = alice
        .apps()
        .call(
            &captcha,
            "verify",
            Some(json!({ "id": id, "answer": answer })),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.details().unwrap()["code"], "AppsOnly", "{e}");

    // The forms app's call verifies in the captcha app, then writes, in one request.
    let _r = alice
        .apps()
        .call(&forms, "setHuman", Some(human("v", &answer)), None, None)
        .await
        .unwrap();
    assert_eq!(
        strong_view(&alice, &forms, "get", json!({ "key": "k" })).await,
        json!("v")
    );
    // The captcha app logged the spend; a view through `xcall.view` sees it.
    assert_eq!(
        strong_view(&alice, &captcha, "status", json!({ "id": id })).await,
        json!("spent")
    );
    assert_eq!(
        strong_view(
            &alice,
            &forms,
            "captchaStatus",
            json!({ "captcha": captcha, "challenge": id })
        )
        .await,
        json!("spent")
    );

    // A spent challenge does not pass again, and nothing is written.
    let e = alice
        .apps()
        .call(&forms, "setHuman", Some(human("v2", &answer)), None, None)
        .await
        .unwrap_err();
    assert_eq!(e.details().unwrap()["code"], "CaptchaFailed", "{e}");
    assert_eq!(
        strong_view(&alice, &forms, "get", json!({ "key": "k" })).await,
        json!("v")
    );

    // Another owner's app cannot reach the service.
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let bobs_bundle = bob
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let bobs = bob
        .apps()
        .create(typed(json!({ "name": "forms", "bundle": &bobs_bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let e = bob
        .apps()
        .call(&bobs, "setHuman", Some(human("x", &answer)), None, None)
        .await
        .unwrap_err();
    assert_eq!(e.details().unwrap()["code"], "XcallFailed", "{e}");
    assert!(
        e.details().unwrap()["fields"]["message"]
            .as_str()
            .unwrap()
            .contains("no such app"),
        "{e}"
    );
}

/// D76: a call makes an outbound request itself. The forms app (kv) asks the captcha
/// service (a signed release declaring Turnstile's siteverify) inside its call; the
/// service checks the token with Turnstile from its own call, through the outbound gate
/// (declared, approved, the operator's host and secret), and the forms app writes only
/// on a pass. A spent token does not pass again.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_call_checks_a_turnstile_token_through_a_captcha_service() {
    const PACKAGE: &str = "com.calimero.captcha";
    const HOST: &str = "challenges.cloudflare.com";
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[11; 32]);
    let signer = calimero_bundle::derive_signer_id_did_key(publisher.verifying_key().as_bytes());
    let settings = calimero_outbound::OutboundSettings {
        enabled: true,
        hosts: vec![HOST.to_owned()],
        testing: Some(calimero_outbound::OutboundTestSettings {
            turnstile: true,
            secrets: [(
                "TURNSTILE_SECRET".to_owned(),
                calimero_outbound::TestSecretSettings {
                    // Cloudflare's test secret that passes every unspent token.
                    value: "1x0000000000000000000000000000000AA".to_owned(),
                    packages: vec![PACKAGE.to_owned()],
                    signers: vec![signer],
                    hosts: vec![HOST.to_owned()],
                },
            )]
            .into(),
            ..calimero_outbound::OutboundTestSettings::default()
        }),
        ..calimero_outbound::OutboundSettings::default()
    };
    let service = calimero_outbound::OutboundService::from_settings(&settings).unwrap();
    let c = Cluster::start_with(3, |cfg| cfg.outbound.clone_from(&service)).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let _ = alice
        .packages()
        .upload(mpk_declaring(
            PACKAGE,
            "0.1.0",
            &wasm("captcha"),
            &publisher,
            &[calimero_outbound::TURNSTILE_VERIFY_URL],
        ))
        .await
        .unwrap();
    let captcha = alice
        .apps()
        .create(typed(json!({ "name": "captcha", "package": PACKAGE })))
        .await
        .map(wire)
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    let kv_bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let forms = alice
        .apps()
        .create(typed(json!({ "name": "forms", "bundle": &kv_bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let human = |value: &str, token: &str| json!({ "key": "k", "value": value, "captcha": captcha, "token": token });

    // Not approved yet: the service's request is refused, the token is not passed.
    let e = alice
        .apps()
        .call(
            &forms,
            "setHumanTurnstile",
            Some(human("v", "tok-0")),
            Some("k-0"),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.details().unwrap()["code"], "CaptchaFailed", "{e}");

    // Approved: Turnstile accepts the token, checked from the service's own call, and
    // the forms app writes in the same request.
    let _ = alice
        .orgs()
        .outbound_approval(
            &alice.personal_org().await.unwrap(),
            PACKAGE,
            Some(typed(json!("manifest"))),
            Some(false),
            Some(&mut pk),
        )
        .await
        .map(wire)
        .unwrap();
    let first = alice
        .apps()
        .call(
            &forms,
            "setHumanTurnstile",
            Some(human("v", "tok-1")),
            Some("k-1"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        strong_view(&alice, &forms, "get", json!({ "key": "k" })).await,
        json!("v")
    );
    // The same call again replays its logged answer.
    let again = alice
        .apps()
        .call(
            &forms,
            "setHumanTurnstile",
            Some(human("v", "tok-1")),
            Some("k-1"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(again.index, first.index);
    // Another call with the spent token: Turnstile answers "timeout-or-duplicate".
    let e = alice
        .apps()
        .call(
            &forms,
            "setHumanTurnstile",
            Some(human("v2", "tok-1")),
            Some("k-2"),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.details().unwrap()["code"], "CaptchaFailed", "{e}");
    assert_eq!(
        strong_view(&alice, &forms, "get", json!({ "key": "k" })).await,
        json!("v")
    );
}

/// R8: the owner deletes a component's blob; it is gone whatever refers to it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_owner_deletes_a_components_blob() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
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
    let id = alice
        .apps()
        .blob_upload(&app, b"bytes".to_vec(), None)
        .await
        .map(wire)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = alice
        .apps()
        .call(
            &app,
            "attach",
            Some(json!({ "name": "f", "blob": id })),
            None,
            None,
        )
        .await
        .unwrap();
    let _d = alice.apps().blob_delete(&app, &id).await.unwrap();
    assert_eq!(
        alice.apps().blob_list(&app).await.map(wire).unwrap()["blobs"],
        json!([])
    );
    assert_eq!(
        strong_view(&alice, &app, "file", json!({ "name": "f" })).await,
        json!(null)
    );
    let e = alice
        .apps()
        .blob_download(&app, &id, None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NOT_FOUND"), "{e}");
}

/// The engine tests' `spin` guest (`crates/app-engine-wasmtime/tests/guests/spin`): its
/// `handle` never returns, so every run of it, `init` included, times out (D23).
fn spin_component() -> Vec<u8> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../app-engine-wasmtime/tests/guests/spin");
    let target = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("guests");
    calimero_app_build::build_component(&dir, &target)
        .unwrap_or_else(|e| panic!("build spin: {e}"))
        .component
}

/// Review item 2: a component app whose `init` has not applied (every run of its
/// bundle times out, D23, so `reconcile` keeps retrying) is still a component at
/// upgrade: the app record's executor decides until the group's state names one. A
/// component bundle is taken, and `init` then runs on it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_app_whose_init_has_not_applied_is_upgraded_by_its_recorded_executor() {
    // Unlimited fuel, so the spinning init hits its wall clock (a timeout, never
    // logged) instead of running out of fuel (a trap, logged as a failed init).
    let c = Cluster::start_with(3, |cfg| {
        cfg.budget = Some(calimero_app_host::Budget {
            fuel: u64::MAX,
            ..calimero_app_host::Budget::call()
        });
    })
    .await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let stuck = alice
        .packages()
        .bundle_put(spin_component())
        .await
        .map(|b| b.hash)
        .unwrap();
    let fixed = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    // Created; its init never applies (the create call itself waits 30 s for it,
    // so it runs beside the test).
    let creating = {
        let cl = c.client(0);
        let mut pk2 = SoftAuthenticator::new("localhost", ORIGIN);
        std::mem::swap(&mut pk, &mut pk2);
        let stuck = stuck.clone();
        let task = tokio::spawn(async move {
            let _s = cl.sign_in(&mut pk2).await.unwrap();
            let r = cl
                .apps()
                .create(typed(json!({ "name": "stuck", "bundle": stuck })))
                .await
                .map(wire);
            (r, pk2)
        });
        let mut app = None;
        for _ in 0..100 {
            app = c.nodes[0].as_ref().unwrap().node.sys(|s| {
                s.apps
                    .values()
                    .find(|a| a.name == "stuck")
                    .map(|a| a.app_id.clone())
            });
            if app.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        (app.expect("the app was recorded"), task)
    };
    let (app, task) = creating;
    let (created, back) = task.await.unwrap();
    pk = back;
    assert_eq!(
        created.unwrap_err().code(),
        Some("UNAVAILABLE"),
        "the stuck app started"
    );
    let _s = alice.sign_in(&mut pk).await.unwrap();
    for i in 0..3 {
        let g = hosted_on(&c, i, &app).await;
        assert_eq!(g.read_local(|m, _| m.state.executor()), None, "node {i}");
    }
    // A component bundle: taken, and init then runs on it.
    let up = alice
        .apps()
        .upgrade(&app, typed(json!({ "bundle": fixed })), Some(&mut pk))
        .await
        .map(wire)
        .unwrap();
    assert_eq!(up["status"], "ok", "{up}");
    let mut done = None;
    for _ in 0..80 {
        if let Ok(r) = alice
            .apps()
            .call(
                &app,
                "set",
                Some(json!({ "key": "a", "value": "1" })),
                None,
                None,
            )
            .await
        {
            done = Some(r);
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(done.is_some(), "the app never came up after the upgrade");
    for i in 0..3 {
        let g = hosted_on(&c, i, &app).await;
        for _ in 0..100 {
            if g.read_local(|m, _| m.state.initialized) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let (bundle, executor, initialized) = g.read_local(|m, _| {
            (
                m.state.bundle.clone(),
                m.state.executor(),
                m.state.initialized,
            )
        });
        assert_eq!(bundle, Some(fixed.clone()), "node {i}");
        assert_eq!(
            executor,
            Some(calimero_node::types::Executor::Component),
            "node {i}"
        );
        assert!(initialized, "node {i}");
    }
}

/// Review item 3, D75 (spec v0.26): `orgs-of` answers only the app's own organization.
/// Carol is a member of the app's org and the owner of another; a component in the
/// app's org learns only her role there, and an app in Alice's personal organization
/// (made when it named none) learns only that one: Alice's role, nothing of Carol's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_component_sees_no_organization_but_its_own() {
    use calimero_app_host::{Directory as _, OrgRole};
    use calimero_node::component::SysDirectory;

    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (carol, _c) = signed_up(&c, 1, "carol").await;
    let carol_id = carol.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let acme = alice.orgs().create("Acme", None).await.map(wire).unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let token = alice
        .orgs()
        .invite_create(&acme, typed(json!({ "role": "member" })), Some(&mut a))
        .await
        .map(wire)
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = carol.orgs().invite_redeem(&token).await.unwrap();
    let other = carol.orgs().create("Other", None).await.map(wire).unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let in_org = alice
        .apps()
        .create(typed(
            json!({ "name": "kv-org", "bundle": &bundle, "org": acme }),
        ))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let personal = alice
        .apps()
        .create(typed(json!({ "name": "kv", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let node = &c.nodes[0].as_ref().unwrap().node;
    // The node's replica has every org now.
    for _ in 0..50 {
        if node.sys(|s| s.orgs.contains_key(&other) && s.apps.contains_key(&personal)) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let dir = SysDirectory::new(Arc::downgrade(node), in_org.clone());
    assert_eq!(
        dir.orgs_of(&carol_id),
        vec![(acme.clone(), OrgRole::Member)],
        "a component saw an organization other than its own"
    );
    let dir = SysDirectory::new(Arc::downgrade(node), personal.clone());
    assert_eq!(
        dir.orgs_of(&carol_id),
        Vec::new(),
        "an app in a personal organization saw its caller's orgs"
    );
    let alice_id = alice.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let mine = node
        .sys(|s| s.personal_org_of(&alice_id).cloned())
        .expect("the instance made Alice's personal organization");
    assert_eq!(
        dir.orgs_of(&alice_id),
        vec![(mine, OrgRole::Owner)],
        "an app in a personal organization saw more than its own"
    );
}

/// Review nit: the blob routes judge access first. A caller who can't reach a private
/// component app gets `404` on every blob route (as on any route), never an answer
/// that would tell them it exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_blob_routes_hide_a_private_component_app_from_outsiders() {
    let c = Cluster::start(3).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
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
    let id = "00".repeat(32);
    let status = |e: calimero_client::Error| match e {
        calimero_client::Error::Api { status, code, .. } => (status, code),
        other => panic!("{other:?}"),
    };
    let not_found = (404, "NOT_FOUND".to_owned());
    assert_eq!(
        status(bob.apps().blob_list(&app).await.unwrap_err()),
        not_found,
        "list"
    );
    assert_eq!(
        status(bob.apps().blob_info(&app, &id).await.unwrap_err()),
        not_found,
        "info"
    );
    assert_eq!(
        status(bob.apps().blob_download(&app, &id, None).await.unwrap_err()),
        not_found,
        "download"
    );
    assert_eq!(
        status(bob.apps().blob_delete(&app, &id).await.unwrap_err()),
        not_found,
        "delete"
    );
    assert_eq!(
        status(
            bob.apps()
                .blob_upload(&app, b"x".to_vec(), None)
                .await
                .unwrap_err()
        ),
        not_found,
        "upload"
    );
    // Its owner gets the app's blobs (R8).
    assert_eq!(
        alice.apps().blob_list(&app).await.map(wire).unwrap()["blobs"],
        json!([])
    );
}

/// Review nit (D70): a same-key retry is answered from its record even when an upgrade
/// since removed the method: the replay comes before the ABI check.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retry_replays_even_after_an_upgrade_removed_its_method() {
    let c = Cluster::start(3).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let kv = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let gated = alice
        .packages()
        .bundle_put(wasm("org-gated"))
        .await
        .map(|b| b.hash)
        .unwrap();
    let app = alice
        .apps()
        .create(typed(json!({ "name": "kv", "bundle": &kv })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let first = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "a", "value": "1" })),
            Some("retry-me"),
            None,
        )
        .await
        .unwrap();
    let up = alice
        .apps()
        .upgrade(&app, typed(json!({ "bundle": gated })), Some(&mut pk))
        .await
        .map(wire)
        .unwrap();
    assert_eq!(up["status"], "ok", "{up}");
    // `set` is gone from the running bundle; a new call of it is refused...
    let e = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "a", "value": "2" })),
            Some("fresh"),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.details().unwrap()["code"], "UnknownMethod", "{e}");
    // ...but the retry of the committed one is its recorded answer, from any node.
    for i in 0..3 {
        let cl = c.client(i);
        let _s = cl.sign_in(&mut pk).await.unwrap();
        let again = cl
            .apps()
            .call(
                &app,
                "set",
                Some(json!({ "key": "a", "value": "1" })),
                Some("retry-me"),
                None,
            )
            .await
            .unwrap_or_else(|e| panic!("node {i}: {e}"));
        assert_eq!(again.index, first.index, "node {i}");
    }
}

/// Review nit, codes: a confirm method a stale front node did not know is confirm
/// (here: a node that hosts no replica reads the directory's bundle, org-gated,
/// while the group runs kv, whose `wipe` is a confirm method) still answers the
/// spec's step-up, `412 CONFIRM_REQUIRED` with a challenge,
/// not the leader's `422 ConfirmRequired`.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_confirm_method_a_stale_front_missed_still_asks_for_a_passkey() {
    let c = Cluster::start(4).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let gated = alice
        .packages()
        .bundle_put(wasm("org-gated"))
        .await
        .map(|b| b.hash)
        .unwrap();
    let kv = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let app = alice
        .apps()
        .create(typed(json!({ "name": "app", "bundle": &gated })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let leader = c.app_leader(&app).unwrap();
    let g = c.nodes[leader].as_ref().unwrap().node.hosted(&app).unwrap();
    let _p = g
        .propose(|m, _| {
            calimero_log::Proposal::Append(
                serde_json::to_vec(&calimero_node::component::ComponentCmd::Upgrade {
                    bundle: kv.clone(),
                    compute_bundle: None,
                    base_index: m.state.last_index,
                })
                .unwrap(),
            )
        })
        .await
        .unwrap();
    drop(g);
    let record = c.nodes[0].as_ref().unwrap().node.app(&app).await.unwrap();
    let outsider = (0..4)
        .find(|i| !record.replicas.contains(&(*i as u64 + 1)))
        .expect("a node without a replica");
    for i in (0..4).filter(|i| *i != outsider) {
        for _ in 0..100 {
            if running_bundle(&c, i, &app).as_deref() == Some(kv.as_str()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    let front = c.client(outsider);
    let _s = front.sign_in(&mut pk).await.unwrap();
    let e = front
        .apps()
        .call(&app, "wipe", Some(json!({})), None, None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    assert!(
        e.details().unwrap()["challengeId"].is_string(),
        "no challenge: {e}"
    );
}

/// Review nit, codes: a component the engine refuses at `POST /bundles` is a
/// `400 BAD_BUNDLE` (a core module is `400 UNSUPPORTED_BUNDLE`,
/// [`a_core_module_bundle_is_refused_as_an_sdk_0_12_app`]).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_component_upload_is_a_400() {
    let c = Cluster::start(3).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    // A component's header (layer 1) and nothing a component needs after it.
    let mut broken = b"\0asm\x0d\0\x01\0".to_vec();
    broken.extend_from_slice(&[0; 16]);
    let e = alice
        .packages()
        .bundle_put(broken)
        .await
        .map(|b| b.hash)
        .unwrap_err();
    let status = match &e {
        calimero_client::Error::Api { status, .. } => *status,
        other => panic!("{other:?}"),
    };
    assert_eq!((status, e.code()), (400, Some("BAD_BUNDLE")), "{e}");
}

// --- Usage (Q20): in the log, to the owner, in quotas -----------------------------------

/// `GET /apps/{app}/usage` through `cl`: today's row and the 30 days before it.
async fn app_usage(cl: &Client, app: &str) -> Value {
    cl.apps().usage(app).await.map(wire).unwrap()
}

/// A usage figure: a decimal string (D20), never a JSON number.
fn figure(v: &Value) -> u64 {
    v.as_str()
        .unwrap_or_else(|| panic!("not a decimal string: {v}"))
        .parse()
        .unwrap()
}

/// The runs and fuel of every day of an app's usage answer, today included.
fn usage_totals(u: &Value) -> (u64, u64) {
    let days = u["days"].as_array().unwrap().iter().chain([&u["today"]]);
    days.fold((0, 0), |(runs, fuel), d| {
        (runs + figure(&d["runs"]), fuel + figure(&d["fuel"]))
    })
}

/// Sets the cluster's `maxFuelPerDeveloperPerDay` through the ops API (a decimal
/// string, D20).
async fn set_fuel_limit(ops: &Client, limit: u64) {
    let mut quotas = ops.ops().quotas_get().await.map(wire).unwrap();
    quotas["maxFuelPerDeveloperPerDay"] = json!(limit.to_string());
    let _ok = ops
        .ops()
        .quotas_set(typed(quotas), Some(&common::operator_sign))
        .await
        .unwrap();
}

/// Waits until every running node reads `limit` as the fuel quota.
async fn fuel_limit_everywhere(c: &Cluster, limit: u64) {
    for _ in 0..100 {
        if c.nodes.iter().flatten().all(|r| {
            r.node.sys(|s| {
                serde_json::to_value(&s.quotas).unwrap()["maxFuelPerDeveloperPerDay"]
                    == json!(limit.to_string())
            })
        }) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the fuel quota never reached every node");
}

/// Waits until every running node has applied what the app's leader has, so a
/// usage read on any replica counts every run that committed.
async fn applied_everywhere(c: &Cluster, app: &str) {
    let target = c.applied_on(c.app_leader(app).unwrap(), app);
    for i in (0..c.nodes.len()).filter(|i| c.nodes[*i].is_some()) {
        let g = hosted_on(c, i, app).await;
        for _ in 0..200 {
            if g.applied() >= target {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(g.applied() >= target, "node {i} never applied {target}");
    }
}

/// Each running node's own answer for the app's usage, once it applied `target`.
async fn usage_on_every_node(
    c: &Cluster,
    pk: &mut SoftAuthenticator,
    app: &str,
    target: u64,
) -> Vec<Value> {
    let mut out = Vec::new();
    for i in 0..c.nodes.len() {
        if c.nodes[i].is_none() {
            continue;
        }
        let g = hosted_on(c, i, app).await;
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while g.applied() < target {
            assert!(
                std::time::Instant::now() < deadline,
                "node {i} never applied {target}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let cl = c.client(i);
        let _s = cl.sign_in(pk).await.unwrap();
        out.push(app_usage(&cl, app).await);
    }
    out
}

/// Q20: every logged run's usage is in its entry, and a replica only adds up what
/// the entries say. A replica that installed a snapshot, and the replicas left after
/// the leader goes, answer the same book, wall-clock milliseconds included, and it
/// counts each run once: the `init`, every call that committed and the tick.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn usage_survives_a_leader_change_and_a_snapshot_install() {
    let mut c = Cluster::start_with(3, |cfg| cfg.raft.snapshot_every = 20).await;
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
    let leader = c.app_leader(&app).unwrap();
    let lagging = (leader + 1) % 3;
    let other = (leader + 2) % 3;
    let mut calls = 0;
    for v in ["a", "b", "c"] {
        set(&alice, &app, v).await.unwrap();
        calls += 1;
    }
    let lagging_applied = c.applied_on(lagging, &app);
    c.stop(lagging).await;

    // While it is down: a tick, and enough calls that the leader's log no longer
    // reaches back to it (it keeps 100 entries past a snapshot).
    c.advance_all(HOUR);
    let cl = c.client(other);
    let _s = cl.sign_in(&mut pk).await.unwrap();
    let _t = beats_reach(&cl, &app, 1, 10).await;
    for _ in 0..300 {
        let _id = new_id(&cl, &app).await;
        calls += 1;
    }
    {
        let g = c.nodes[leader].as_ref().unwrap().node.hosted(&app).unwrap();
        g.snapshot_now().await.unwrap();
    }
    c.restart(lagging, |cfg| {
        cfg.raft.snapshot_every = 1_000_000;
        cfg.clock_offset_ms = HOUR;
    })
    .await;
    let target = c.applied_on(leader, &app);
    let group = hosted_on(&c, lagging, &app).await;
    let node = &c.nodes[lagging].as_ref().unwrap().node;
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let snapshot = || {
        node.engine
            .snapshot_file(group.id())
            .unwrap()
            .map(|(index, _, _)| index)
    };
    while group.applied() < target || snapshot().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "node {lagging} did not catch up"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        snapshot().is_some_and(|i| i > lagging_applied),
        "no snapshot installed"
    );
    drop(group);

    let answers = usage_on_every_node(&c, &mut pk, &app, target).await;
    assert_eq!(answers.len(), 3);
    assert!(
        answers.iter().all(|a| *a == answers[0]),
        "replicas differ: {answers:#?}"
    );
    let (runs, fuel) = usage_totals(&answers[0]);
    assert_eq!(
        runs,
        1 + calls + 1,
        "init, the calls and the tick, once each"
    );
    assert!(fuel > 0, "{answers:#?}");

    // The leader goes; the two left elect one and take more calls.
    c.stop(leader).await;
    let survivor = c.client(other);
    let _s = survivor.sign_in(&mut pk).await.unwrap();
    for v in ["d", "e", "f", "g", "h"] {
        set(&survivor, &app, v).await.unwrap();
        calls += 1;
    }
    let new_leader = c.app_leader(&app).expect("a new leader");
    let target = c.applied_on(new_leader, &app);
    let answers = usage_on_every_node(&c, &mut pk, &app, target).await;
    assert_eq!(answers.len(), 2);
    assert_eq!(
        answers[0], answers[1],
        "replicas differ after the leader change"
    );
    let (runs_after, fuel_after) = usage_totals(&answers[0]);
    assert_eq!(runs_after, 1 + calls + 1);
    assert!(fuel_after > fuel);
}

/// The usage of the developer `app` is billed to: its organization's (an
/// instance made without naming one is in its owner's personal organization),
/// else the account's own, for an app from before outside any organization.
async fn developer_usage(cl: &Client, app: &str) -> Value {
    let org = cl.apps().get(app).await.map(wire).unwrap()["org"]
        .as_str()
        .unwrap()
        .to_owned();
    cl.orgs().usage(&org).await.map(wire).unwrap()
}

/// Q20: a developer at its daily fuel quota gets `429 QUOTA_EXCEEDED` on a call to
/// any of its apps, before the call runs (here the owner's personal organization,
/// where her instances go); its apps' timers still tick and `init` still runs
/// (Xabi), and both still count.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_account_over_its_daily_fuel_gets_429_and_its_timers_still_tick() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let ops = c.client(1);
    ops.use_api_key("ops-secret").await;
    // One unit of fuel: the app's own `init` uses it up.
    set_fuel_limit(&ops, 1).await;
    fuel_limit_everywhere(&c, 1).await;
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

    let err = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "k", "value": "v" })),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("QUOTA_EXCEEDED"), "{err}");
    assert!(
        matches!(err, calimero_client::Error::Api { status: 429, .. }),
        "{err}"
    );
    let me = alice.auth().usage().await.map(wire).unwrap();
    let dev = developer_usage(&alice, &app).await;
    let before = figure(&dev["developerFuelToday"]);
    assert!(before >= 1, "{dev}");
    // Only `init` ran: it is the whole of the account's day, and the app's.
    let u = app_usage(&alice, &app).await;
    assert_eq!(figure(&u["today"]["runs"]), 1, "{u}");
    assert_eq!(figure(&u["today"]["fuel"]), before, "{u} vs {dev}");
    let _writes = figure(&dev["developerWritesToday"]);
    let _bytes = figure(&me["bundleBytes"]);
    assert_eq!(me["quotas"]["maxFuelPerDeveloperPerDay"], json!("1"));
    assert!(
        alice
            .apps()
            .view(
                &app,
                "get",
                Some(json!({ "key": "k" })),
                Some(ViewsRequestConsistency::Strong),
                None
            )
            .await
            .unwrap()
            .result
            .is_null(),
        "the refused call ran"
    );

    // Its timer ticks anyway, and the tick counts.
    c.advance_all(HOUR);
    let _rows = beats_reach(&alice, &app, 1, 10).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let u = app_usage(&alice, &app).await;
        if usage_totals(&u).0 >= 2 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the tick never counted: {u}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let after = figure(&developer_usage(&alice, &app).await["developerFuelToday"]);
    assert!(
        after > before,
        "the tick's fuel did not count: {before} → {after}"
    );

    // A second app's `init` runs while over the quota; its calls are refused too.
    let second = alice
        .apps()
        .create(typed(json!({ "name": "kv2", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let u = app_usage(&alice, &second).await;
    assert_eq!(figure(&u["today"]["runs"]), 1, "{u}");
    let err = alice
        .apps()
        .call(
            &second,
            "set",
            Some(json!({ "key": "k", "value": "v" })),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("QUOTA_EXCEEDED"), "{err}");
}

/// Q20: a limit of 0 (the default) never refuses; a refusal is not logged, so the
/// same idempotency key runs once the limit is lifted.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_fuel_limit_of_zero_never_refuses() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let ops = c.client(1);
    ops.use_api_key("ops-secret").await;
    let quotas = ops.ops().quotas_get().await.map(wire).unwrap();
    assert_eq!(quotas["maxFuelPerDeveloperPerDay"], json!("0"), "{quotas}");
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
    for i in 0..20 {
        let _r = alice
            .apps()
            .call(
                &app,
                "set",
                Some(json!({ "key": format!("k{i}"), "value": "v" })),
                None,
                None,
            )
            .await
            .unwrap();
    }
    let used = figure(&developer_usage(&alice, &app).await["developerFuelToday"]);
    assert!(used > 0);

    set_fuel_limit(&ops, used).await;
    fuel_limit_everywhere(&c, used).await;
    let err = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "late", "value": "v" })),
            Some("late-key"),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("QUOTA_EXCEEDED"), "{err}");

    set_fuel_limit(&ops, 0).await;
    fuel_limit_everywhere(&c, 0).await;
    let _r = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "late", "value": "v" })),
            Some("late-key"),
            None,
        )
        .await
        .unwrap();
    let got = alice
        .apps()
        .view(
            &app,
            "get",
            Some(json!({ "key": "late" })),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap()
        .result;
    assert_eq!(got, json!("v"), "the key the refusal did not spend ran");
}

/// Q20: an app's usage is its owner's and its organization's admins' to read; a
/// member gets 403, someone who doesn't reach the app 404.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn only_the_owner_and_org_admins_read_an_apps_usage() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (carol, _c) = signed_up(&c, 1, "carol").await;
    let (dave, _d) = signed_up(&c, 2, "dave").await;
    let (eve, _e) = signed_up(&c, 0, "eve").await;
    let acme = alice.orgs().create("Acme", None).await.map(wire).unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    for (who, role) in [(&carol, "member"), (&dave, "admin")] {
        let token = alice
            .orgs()
            .invite_create(&acme, typed(json!({ "role": role })), Some(&mut a))
            .await
            .map(wire)
            .unwrap()["token"]
            .as_str()
            .unwrap()
            .to_owned();
        let _r = who.orgs().invite_redeem(&token).await.unwrap();
    }
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let in_org = alice
        .apps()
        .create(typed(
            json!({ "name": "kv-org", "bundle": &bundle, "org": acme }),
        ))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let personal = alice
        .apps()
        .create(typed(json!({ "name": "kv", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    // The member reaches the org app (its call runs), but its usage is not theirs.
    let _r = carol
        .apps()
        .call(
            &in_org,
            "set",
            Some(json!({ "key": "k", "value": "v" })),
            None,
            None,
        )
        .await
        .unwrap();
    applied_everywhere(&c, &in_org).await;

    let owner = app_usage(&alice, &in_org).await;
    assert_eq!(figure(&owner["today"]["runs"]), 2, "{owner}");
    assert_eq!(owner["days"].as_array().unwrap().len(), 30, "{owner}");
    let admin = dave.apps().usage(&in_org).await.map(wire).unwrap();
    assert_eq!(figure(&admin["today"]["runs"]), 2, "{admin}");
    let err = carol.apps().usage(&in_org).await.unwrap_err();
    assert_eq!(err.code(), Some("FORBIDDEN"), "{err}");
    let err = eve.apps().usage(&in_org).await.unwrap_err();
    assert_eq!(err.code(), Some("NOT_FOUND"), "{err}");
    let _o = app_usage(&alice, &personal).await;
    for who in [&carol, &dave] {
        let err = who.apps().usage(&personal).await.unwrap_err();
        assert_eq!(err.code(), Some("NOT_FOUND"), "{err}");
    }

    // An admin made a member reads it no longer (403); removed from the org, they
    // no longer reach the app at all (404). Access is judged fresh on every read.
    let dave_id = dave.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = alice
        .orgs()
        .set_member(&acme, &dave_id, OrgRole::Member, Some(&mut a))
        .await
        .unwrap();
    let err = dave.apps().usage(&in_org).await.unwrap_err();
    assert_eq!(err.code(), Some("FORBIDDEN"), "a demoted admin: {err}");
    let _r = alice
        .orgs()
        .remove_member(&acme, &dave_id, Some(&mut a))
        .await
        .unwrap();
    let err = dave.apps().usage(&in_org).await.unwrap_err();
    assert_eq!(err.code(), Some("NOT_FOUND"), "a removed member: {err}");
}

/// An organization named `name` that `owner` creates; answers its id.
async fn new_org(owner: &Client, name: &str) -> String {
    owner.orgs().create(name, None).await.map(wire).unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// `who` joins `org` as `role` through an invite `admin` mints with its passkey.
async fn join_org(
    admin: &Client,
    admin_pk: &mut SoftAuthenticator,
    who: &Client,
    org: &str,
    role: &str,
) {
    let token = admin
        .orgs()
        .invite_create(org, typed(json!({ "role": role })), Some(&mut *admin_pk))
        .await
        .map(wire)
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = who.orgs().invite_redeem(&token).await.unwrap();
}

/// D78: an org app's runs are the organization's, not its owner's. An instance made
/// without naming an organization is in the owner's personal organization, so its
/// runs are that organization's too; the owner's `/auth/usage` counts only apps
/// outside any organization, and each organization's admins read its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_org_apps_runs_count_toward_the_org_not_its_owner() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let acme = new_org(&alice, "Acme").await;
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let in_org = alice
        .apps()
        .create(typed(
            json!({ "name": "kv-org", "bundle": &bundle, "org": acme }),
        ))
        .await
        .map(|a| a.app_id)
        .unwrap();
    for v in ["a", "b", "c"] {
        set(&alice, &in_org, v).await.unwrap();
    }
    let app_fuel = figure(&app_usage(&alice, &in_org).await["today"]["fuel"]);
    assert!(app_fuel > 0);
    let org = alice.orgs().usage(&acme).await.map(wire).unwrap();
    assert_eq!(figure(&org["developerFuelToday"]), app_fuel, "{org}");
    assert!(figure(&org["developerWritesToday"]) >= 3, "{org}");
    assert_eq!(org["apps"], json!(1), "{org}");
    let me = alice.auth().usage().await.map(wire).unwrap();
    assert_eq!(
        figure(&me["developerFuelToday"]),
        0,
        "the owner paid for the org's app: {me}"
    );

    // One in her personal organization is that organization's, and only it.
    let personal = alice
        .apps()
        .create(typed(json!({ "name": "kv", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    set(&alice, &personal, "p").await.unwrap();
    let mine = figure(&app_usage(&alice, &personal).await["today"]["fuel"]);
    let home = alice.personal_org().await.unwrap();
    assert_ne!(home, acme);
    let theirs = alice.orgs().usage(&home).await.map(wire).unwrap();
    assert_eq!(figure(&theirs["developerFuelToday"]), mine, "{theirs}");
    let me = alice.auth().usage().await.map(wire).unwrap();
    assert_eq!(figure(&me["developerFuelToday"]), 0, "{me}");
    let org = alice.orgs().usage(&acme).await.map(wire).unwrap();
    assert_eq!(figure(&org["developerFuelToday"]), app_fuel, "{org}");
}

/// D78: the daily fuel cap holds per developer. An org app made from a release has
/// the org as its developer: over the cap it gets 429, while its owner's apps in her
/// personal organization, another developer under the cap, still run.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_org_over_its_fuel_cap_gets_429_while_its_owners_personal_apps_run() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let ops = c.client(1);
    ops.use_api_key("ops-secret").await;
    let acme = new_org(&alice, "Acme").await;
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let personal = alice
        .apps()
        .create(typed(json!({ "name": "kv", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let in_org = alice
        .apps()
        .create(typed(
            json!({ "name": "kv-org", "bundle": &bundle, "org": acme }),
        ))
        .await
        .map(|a| a.app_id)
        .unwrap();
    // The org burns far more than the owner's personal app will.
    let home = alice.personal_org().await.unwrap();
    let mine = figure(&alice.orgs().usage(&home).await.map(wire).unwrap()["developerFuelToday"]);
    let mut org_fuel = 0;
    for i in 0..400 {
        set(&alice, &in_org, &format!("v{i}")).await.unwrap();
        org_fuel =
            figure(&alice.orgs().usage(&acme).await.map(wire).unwrap()["developerFuelToday"]);
        if org_fuel > 20 * mine.max(1) {
            break;
        }
    }
    assert!(
        org_fuel > 20 * mine.max(1),
        "org {org_fuel}, personal {mine}"
    );
    set_fuel_limit(&ops, org_fuel).await;
    fuel_limit_everywhere(&c, org_fuel).await;

    let err = alice
        .apps()
        .call(
            &in_org,
            "set",
            Some(json!({ "key": "k", "value": "x" })),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("QUOTA_EXCEEDED"), "{err}");
    alice
        .apps()
        .call(
            &personal,
            "set",
            Some(json!({ "key": "k", "value": "x" })),
            None,
            None,
        )
        .await
        .unwrap();
}

/// D78: an organization's usage is its admins' (and owner's) to read; a member gets
/// 403, someone outside it 404, and a demoted admin 403 at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn only_an_orgs_admins_read_its_usage() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (carol, _c) = signed_up(&c, 1, "carol").await;
    let (dave, _d) = signed_up(&c, 2, "dave").await;
    let (eve, _e) = signed_up(&c, 0, "eve").await;
    let acme = new_org(&alice, "Acme").await;
    join_org(&alice, &mut a, &carol, &acme, "member").await;
    join_org(&alice, &mut a, &dave, &acme, "admin").await;

    let owner = alice.orgs().usage(&acme).await.map(wire).unwrap();
    let _fuel = figure(&owner["developerFuelToday"]);
    let admin = dave.orgs().usage(&acme).await.map(wire).unwrap();
    assert_eq!(admin["developerFuelToday"], owner["developerFuelToday"]);
    let err = carol.orgs().usage(&acme).await.unwrap_err();
    assert_eq!(err.code(), Some("FORBIDDEN"), "a member: {err}");
    let err = eve.orgs().usage(&acme).await.unwrap_err();
    assert_eq!(err.code(), Some("NOT_FOUND"), "an outsider: {err}");

    let dave_id = dave.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = alice
        .orgs()
        .set_member(&acme, &dave_id, OrgRole::Member, Some(&mut a))
        .await
        .unwrap();
    let err = dave.orgs().usage(&acme).await.unwrap_err();
    assert_eq!(err.code(), Some("FORBIDDEN"), "a demoted admin: {err}");
}

// --- Sandi's 10-30 review of the fuel cap -------------------------------------------------

/// Two apps of `cl`'s account on different nodes of a cluster that keeps one replica per
/// app: `(app, its node's index)` for each.
async fn apps_apart(c: &Cluster, cl: &Client, bundle: &str) -> ((String, usize), (String, usize)) {
    let node = &c.nodes[0].as_ref().unwrap().node;
    let mut placed: Vec<(String, usize)> = Vec::new();
    for i in 0..8 {
        let app = cl
            .apps()
            .create(typed(
                json!({ "name": &format!("kv{i}"), "bundle": bundle }),
            ))
            .await
            .map(|a| a.app_id)
            .unwrap();
        let at = node
            .sys_until(|s| s.apps.get(&app).map(|a| a.replicas.clone()))
            .await
            .unwrap();
        let idx = usize::try_from(at[0]).unwrap() - 1;
        if let Some(first) = placed.iter().find(|(_, n)| *n != idx) {
            return (first.clone(), (app, idx));
        }
        placed.push((app, idx));
    }
    panic!("every app landed on one node: {placed:?}");
}

/// Stops node `i` and leaves its port accepting connections that never answer: a
/// peer that hangs rather than refuses.
async fn hang(c: &mut Cluster, i: usize) {
    let addr = c.configs[i].listen;
    c.stop(i).await;
    let listener = std::net::TcpListener::bind(addr).unwrap();
    drop(std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming().flatten() {
            held.push(stream);
        }
    }));
}

/// Calls `app` through `cl` until one answers (the system may be electing).
async fn warm(cl: &Client, app: &str) {
    for _ in 0..80 {
        if cl
            .apps()
            .call(
                app,
                "set",
                Some(json!({ "key": "warm", "value": "w" })),
                None,
                None,
            )
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("{app} never answered");
}

/// Sandi 10-30 (1): the cap never asks a peer during a call, so a replica of another
/// of the developer's apps that hangs adds nothing to the call.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_fuel_cap_asks_no_peer_during_a_call_while_a_replica_hangs() {
    let mut c = Cluster::start_with(3, |cfg| cfg.replicas = 1).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let ((a, ia), (_b, ib)) = apps_apart(&c, &alice, &bundle).await;
    let ops = c.client(ia);
    ops.use_api_key("ops-secret").await;
    set_fuel_limit(&ops, 1_000_000_000_000_000).await;
    fuel_limit_everywhere(&c, 1_000_000_000_000_000).await;

    hang(&mut c, ib).await;
    let cl = c.client(ia);
    let _s = cl.sign_in(&mut pk).await.unwrap();
    warm(&cl, &a).await;
    let started = std::time::Instant::now();
    cl.apps()
        .call(
            &a,
            "set",
            Some(json!({ "key": "k", "value": "v" })),
            None,
            None,
        )
        .await
        .unwrap();
    let took = started.elapsed();
    assert!(
        took < Duration::from_secs(2),
        "the call waited on the hung replica: {took:?}"
    );
}

/// Sandi 10-30 (1): a developer over its cap is refused within one refresh, and a peer
/// that stops answering keeps the figure it last reported, so the cap does not fail
/// open the moment one replica is down.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_over_cap_developer_is_refused_within_one_refresh_while_a_peer_hangs() {
    let mut c = Cluster::start_with(3, |cfg| cfg.replicas = 1).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let ((a, ia), (b, ib)) = apps_apart(&c, &alice, &bundle).await;
    let ops = c.client(ia);
    ops.use_api_key("ops-secret").await;
    set_fuel_limit(&ops, 1_000_000_000_000_000).await;
    fuel_limit_everywhere(&c, 1_000_000_000_000_000).await;

    // B burns well past what A's `init` took.
    for i in 0..20 {
        set(&alice, &b, &format!("v{i}")).await.unwrap();
    }
    let b_fuel = figure(&app_usage(&alice, &b).await["today"]["fuel"]);
    let a_fuel = figure(&app_usage(&alice, &a).await["today"]["fuel"]);
    assert!(a_fuel < b_fuel, "a {a_fuel}, b {b_fuel}");
    set_fuel_limit(&ops, b_fuel).await;
    fuel_limit_everywhere(&c, b_fuel).await;
    // One refresh carries B's figure to A's node.
    tokio::time::sleep(calimero_node::node::USAGE_REFRESH + Duration::from_secs(1)).await;

    hang(&mut c, ib).await;
    let cl = c.client(ia);
    let _s = cl.sign_in(&mut pk).await.unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let err = loop {
        match cl
            .apps()
            .call(
                &a,
                "set",
                Some(json!({ "key": "k", "value": "v" })),
                None,
                None,
            )
            .await
        {
            Err(e) if e.code() == Some("QUOTA_EXCEEDED") => break e,
            other => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "never refused while B's node hangs: {other:?}"
                );
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    };
    assert!(
        matches!(err, calimero_client::Error::Api { status: 429, .. }),
        "{err}"
    );
}

/// Sandi 10-30 (4): an xcall delivered into an app whose developer is over its fuel
/// cap is refused like a client's call (`429 QUOTA_EXCEEDED`), and does not run.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_delivered_xcall_into_an_over_cap_developers_app_is_refused() {
    use calimero_node::component::xcall::XcallFrom;
    use calimero_node::node::CallRequest;
    use calimero_node::types::Caller;

    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let ops = c.client(1);
    ops.use_api_key("ops-secret").await;
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let source = alice
        .apps()
        .create(typed(json!({ "name": "from", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let target = alice
        .apps()
        .create(typed(json!({ "name": "to", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    // The two apps' `init` runs use more than the one unit allowed below.
    set_fuel_limit(&ops, 1).await;
    fuel_limit_everywhere(&c, 1).await;

    let node = &c.nodes[0].as_ref().unwrap().node;
    let req = CallRequest {
        idempotency_key: Some(format!("node:xcall-{source}-1")),
        xcall: Some(XcallFrom {
            source: source.clone(),
            seq: 1,
        }),
        ..CallRequest::new(
            target.clone(),
            // `allow(app = "*")`: the target would take it from the source.
            "setFromApp".into(),
            json!({ "key": "k", "value": "x" }),
            Caller::App {
                app: source.clone(),
            },
        )
    };
    let err = node.call_component(&req).await.expect_err("the xcall ran");
    assert_eq!(err.code, "QUOTA_EXCEEDED", "{err:?}");
    assert_eq!(err.status, 429);
    let got = alice
        .apps()
        .view(
            &target,
            "get",
            Some(json!({ "key": "k" })),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap()
        .result;
    assert!(got.is_null(), "the refused xcall wrote: {got}");
}

/// Sandi 10-30 (5): a read-only member's call to any method but a view is refused
/// before it runs, so it burns no fuel uncounted; a view still answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_read_only_members_call_is_refused_before_it_runs() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let s = |v: Value, k: &str| v[k].as_str().unwrap().to_owned();
    let org = new_org(&alice, "Docs").await;
    join_org(&alice, &mut a, &bob, &org, "member").await;
    let bob_id = s(bob.auth().me().await.map(wire).unwrap(), "accountId");
    let team = s(
        alice
            .orgs()
            .team_create(&org, "Team", TeamJoin::Everyone, None)
            .await
            .map(wire)
            .unwrap(),
        "teamId",
    );
    let _ok = alice
        .orgs()
        .team_set_member(
            &org,
            &team,
            typed(json!({ "account": bob_id, "role": "readOnly" })),
            Some(&mut a),
        )
        .await
        .unwrap();
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let app = alice
        .apps().create(typed(json!({ "name": "kv", "bundle": &bundle, "org": org, "access": { "kind": "team", "team": team } })))
        .await.map(|a| a.app_id)
        .unwrap();
    let runs = |u: &Value| usage_totals(u).0;
    let before = runs(&app_usage(&alice, &app).await);

    // `stamp` writes nothing, but it is not a view: it does not run for him.
    let err = bob
        .apps()
        .call(&app, "stamp", Some(json!({})), None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("FORBIDDEN"), "{err}");
    // A view called through /intents still answers.
    let len = bob
        .apps()
        .call(&app, "len", Some(json!({})), None, None)
        .await
        .unwrap();
    assert_eq!(len.result, json!(0));
    assert_eq!(runs(&app_usage(&alice, &app).await), before);
    // Alice (no read-only row) runs it.
    let _r = alice
        .apps()
        .call(&app, "stamp", Some(json!({})), None, None)
        .await
        .unwrap();
}

/// Sandi 10-30 (6): the fuel cap is a decimal string in JSON (D20, D76), kept exact
/// past 2^53; a number is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_fuel_cap_is_a_decimal_string() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let ops = c.client(1);
    ops.use_api_key("ops-secret").await;
    let big = "9007199254740993"; // 2^53 + 1
    let _ok = ops
        .ops()
        .quotas_set(
            typed(json!({ "maxFuelPerDeveloperPerDay": big })),
            Some(&common::operator_sign),
        )
        .await
        .map(wire)
        .unwrap();
    // `ops/quotas` and `/auth/usage` read the replica of the node they reach: wait
    // until every node applied the write.
    fuel_limit_everywhere(&c, 9_007_199_254_740_993).await;
    let quotas = ops.ops().quotas_get().await.map(wire).unwrap();
    assert_eq!(quotas["maxFuelPerDeveloperPerDay"], json!(big), "{quotas}");
    let me = alice.auth().usage().await.map(wire).unwrap();
    assert_eq!(
        me["quotas"]["maxFuelPerDeveloperPerDay"],
        json!(big),
        "{me}"
    );

    // A number, not the decimal string the route takes: sent raw, as the
    // request type cannot carry it.
    let err = ops
        .transport()
        .json(
            Request::new(Method::POST, "ops/quotas")
                .json(json!({ "maxFuelPerDeveloperPerDay": 5 })),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, calimero_client::Error::Api { status: 400, .. }),
        "{err}"
    );
    for bad in ["", "-1", "01", "1e3"] {
        let err = ops
            .ops()
            .quotas_set(typed(json!({ "maxFuelPerDeveloperPerDay": bad })), None)
            .await
            .unwrap_err();
        assert!(
            matches!(err, calimero_client::Error::Api { status: 400, .. }),
            "{bad:?}: {err}"
        );
    }
}

/// Sandi 10-30 (3) / option A: a fuel cap is written only once every node runs this
/// release or newer, as D71 gates `base_seq`; an older node would drop it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_fuel_cap_waits_until_every_node_runs_this_release() {
    let mut c = Cluster::start_with(3, |cfg| {
        cfg.release = if cfg.node_id == 3 { "0.9.0" } else { "1.0.0" }.into();
    })
    .await;
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    let err = ops
        .ops()
        .quotas_set(typed(json!({ "maxFuelPerDeveloperPerDay": "1000" })), None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("UPGRADE_PENDING"), "{err}");
    assert!(
        matches!(err, calimero_client::Error::Api { status: 409, .. }),
        "{err}"
    );
    let behind = &err.details().unwrap()["nodes"];
    assert_eq!(
        behind,
        &json!([{ "node": 3, "release": "0.9.0" }]),
        "{err:?}"
    );
    // Other quotas, and a cap of 0, are written as before.
    let _ok = ops
        .ops()
        .quotas_set(
            typed(json!({ "maxAppsPerAccount": 7 })),
            Some(&common::operator_sign),
        )
        .await
        .map(wire)
        .unwrap();
    let quotas = ops.ops().quotas_get().await.map(wire).unwrap();
    assert_eq!(quotas["maxFuelPerDeveloperPerDay"], json!("0"));

    // Node 3 runs the release now: the cap is written.
    c.restart(2, |cfg| cfg.release = "1.0.0".into()).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        match ops
            .ops()
            .quotas_set(
                typed(json!({ "maxFuelPerDeveloperPerDay": "1000" })),
                Some(&common::operator_sign),
            )
            .await
            .map(wire)
        {
            Ok(_) => break,
            Err(e) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "still refused after the upgrade: {e}"
                );
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
    fuel_limit_everywhere(&c, 1000).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn org_quotas_wait_for_the_release_then_merge_and_clear() {
    let mut c = Cluster::start_with(3, |cfg| {
        cfg.release = if cfg.node_id == 3 { "0.9.0" } else { "1.0.0" }.into();
    })
    .await;
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    let err = ops
        .ops()
        .quotas_set(typed(json!({ "org": "x", "maxConcurrentRuns": 1 })), None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("UPGRADE_PENDING"), "{err}");
    c.restart(2, |cfg| cfg.release = "1.0.0".into()).await;
    let set = |body: serde_json::Value| {
        let ops = &ops;
        async move {
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            loop {
                match ops
                    .transport()
                    .approved(
                        Request::new(Method::POST, "ops/quotas").json(body.clone()),
                        None,
                        Some(&common::operator_sign),
                    )
                    .await
                {
                    Ok(_) => return,
                    Err(e) => {
                        assert!(std::time::Instant::now() < deadline, "still refused: {e}");
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                }
            }
        }
    };
    set(json!({ "org": "x", "maxConcurrentRuns": 1 })).await;
    set(json!({ "org": "x", "computeFuel": "99" })).await;
    let shown = ops.ops().quotas_get().await.map(wire).unwrap();
    assert_eq!(
        shown["orgs"],
        json!({ "x": { "maxConcurrentRuns": 1, "computeFuel": "99" } })
    );
    set(json!({ "org": "x", "maxConcurrentRuns": null, "computeFuel": null })).await;
    assert_eq!(
        ops.ops().quotas_get().await.map(wire).unwrap()["orgs"],
        json!({})
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pools_wait_until_every_node_runs_this_release() {
    let mut c = Cluster::start_with(3, |cfg| {
        cfg.release = if cfg.node_id == 3 { "0.9.0" } else { "1.0.0" }.into();
    })
    .await;
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    // A pool names the job nodes it is for: a replica given one is refused before anything
    // is signed, so no older node is asked to read a field it drops.
    let err = ops
        .ops()
        .register_node(9, "http://127.0.0.1:1", None, Some("standard"), None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("POOL_FOR_JOB_NODES"), "{err}");
    let pools = json!({ "pools": { "standard": { "runsPerNode": 2, "maxMemoryMib": 4096 } } });
    let err = ops
        .ops()
        .pools_set(typed(pools["pools"].clone()), None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("UPGRADE_PENDING"), "{err}");
    assert!(
        matches!(err, calimero_client::Error::Api { status: 409, .. }),
        "{err}"
    );
    c.restart(2, |cfg| cfg.release = "1.0.0".into()).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        match ops
            .ops()
            .pools_set(typed(pools["pools"].clone()), Some(&common::operator_sign))
            .await
        {
            Ok(_) => break,
            Err(e) => {
                assert!(std::time::Instant::now() < deadline, "still refused: {e}");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
    let shown = ops.ops().pools_get().await.map(wire).unwrap();
    assert_eq!(shown["pools"], pools["pools"]);
    assert_eq!(shown["capacity"], json!({ "standard": 0 }));
}

#[tokio::test(flavor = "multi_thread")]
async fn prices_wait_until_every_node_runs_this_release() {
    let mut c = Cluster::start_with(3, |cfg| {
        cfg.release = if cfg.node_id == 3 { "0.9.0" } else { "1.0.0" }.into();
    })
    .await;
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    assert_eq!(ops.ops().prices_get().await.map(wire).unwrap(), json!({}));
    let table = |version: u64, rate: u64| {
        json!({ "table": {
            "version": version, "fuelPerVcpuSecond": rate, "vcpuSecond": 100, "gibSecond": 10,
            "connectorGib": 1, "storageGibMonth": 2, "egressGib": 3,
        } })
    };
    let err = ops
        .ops()
        .prices_set(typed(table(1, 1_000_000_000)["table"].clone()), None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("UPGRADE_PENDING"), "{err}");
    assert!(
        matches!(err, calimero_client::Error::Api { status: 409, .. }),
        "{err}"
    );
    c.restart(2, |cfg| cfg.release = "1.0.0".into()).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        match ops
            .ops()
            .prices_set(
                typed(table(1, 1_000_000_000)["table"].clone()),
                Some(&common::operator_sign),
            )
            .await
        {
            Ok(_) => break,
            Err(e) => {
                assert!(std::time::Instant::now() < deadline, "still refused: {e}");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
    assert_eq!(
        ops.ops().prices_get().await.map(wire).unwrap(),
        table(1, 1_000_000_000)["table"]
    );
    // A version that does not follow the stored one, and a zero rate, are refused after signing.
    for (bad, code) in [
        (table(1, 1_000_000_000), "REJECTED"),
        (table(2, 0), "REJECTED"),
    ] {
        let err = ops
            .ops()
            .prices_set(typed(bad["table"].clone()), Some(&common::operator_sign))
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some(code), "{err}");
    }
    assert_eq!(
        ops.ops().prices_get().await.map(wire).unwrap()["version"],
        1
    );
}
