//! A three-node cluster in one process, driven only through the client, over
//! sealed channels, with the real v2 fixture components (`apps/v2-fixtures/*`,
//! built on first use by `common::component`).

mod common;

use std::sync::Arc;
use std::time::Duration;

use calimero_client::{Client, Consistency, Policy};
use calimero_identity::testing::SoftAuthenticator;
use common::{signed_up, Cluster, ORIGIN};
use futures_util::StreamExt;
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
        .call(&app, "getResult", json!({ "key": "missing" }), None)
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
        .create_api_key(json!({ "label": "agent" }), &mut pk)
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
    let (alice, mut apk) = signed_up(&c, 0, "alice").await;
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
        .post_confirmed(
            &format!("orgs/{org}/invites"),
            json!({ "role": "member", "teams": [team] }),
            &mut apk,
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
        .delete_confirmed(&format!("orgs/{org}/members/{bob_id}"), &mut apk)
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
        .create_api_key(json!({ "readOnly": true, "apps": [app] }), &mut apk)
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
    let (_index, data) = calimero_node::backup::latest(&dir, node.cluster_key(), &app)
        .unwrap()
        .unwrap();
    // The app's rows are in it, opened with the storage key (the log's sealed
    // snapshot file, as for an app on wasmer).
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
        .post_operator_signed(
            &format!("ops/apps/{app}/replicas"),
            json!({ "replicas": new }),
            &common::operator_sign,
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
        .post_operator_signed(
            &format!("ops/apps/{app}/replicas"),
            json!({ "replicas": new }),
            &common::operator_sign,
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
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let ops = c.client(1);
    ops.use_api_key("ops-secret").await;
    let mut quotas = ops.get("ops/quotas").await.unwrap();
    assert_eq!(quotas["maxAppsPerAccount"], json!(100));
    quotas["maxAppsPerAccount"] = json!(1);
    quotas["maxApiKeysPerAccount"] = json!(1);
    let _ok = ops
        .post_operator_signed("ops/quotas", quotas, &common::operator_sign)
        .await
        .unwrap();

    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let _first = alice.create_app("one", &bundle, json!({})).await.unwrap();
    let err = alice
        .create_app("two", &bundle, json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("QUOTA_EXCEEDED"), "{err}");

    let _key = alice.create_api_key(json!({}), &mut pk).await.unwrap();
    let err = alice.create_api_key(json!({}), &mut pk).await.unwrap_err();
    assert_eq!(err.code(), Some("QUOTA_EXCEEDED"), "{err}");
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
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
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
            .post_confirmed(
                &format!("orgs/{org}/invites"),
                json!({ "role": "member", "teams": [team] }),
                &mut a,
            )
            .await
            .unwrap(),
        "token",
    );
    let _r = bob
        .post(&format!("invites/{invite}/redeem"), json!({}))
        .await
        .unwrap();

    let bundle = alice.upload_bundle(wasm("org-gated")).await.unwrap();
    let app = alice
        .create_app("notes", &bundle, json!({ "org": org }))
        .await
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
        .call(&app, "setPinned", json!({ "text": "hello" }), None)
        .await
        .unwrap();
    assert_eq!(strong_view(&bob, &app, "pinned", json!({})).await, "hello");
    let err = bob
        .call(&app, "setPinned", json!({ "text": "mine" }), None)
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
        .delete_confirmed(
            &format!("orgs/{org}/teams/{team}/members/{alice_id}"),
            &mut a,
        )
        .await
        .unwrap();
    let _ok = bob
        .call(
            &app,
            "setTeamNote",
            json!({ "team": team, "text": "ship it" }),
            None,
        )
        .await
        .unwrap();
    let err = alice
        .call(
            &app,
            "setTeamNote",
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

async fn set(cl: &Client, app: &str, v: &str) -> Result<(), Box<calimero_client::Error>> {
    cl.call(app, "set", json!({ "key": "k", "value": v }), None)
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
        alice.post("orgs", json!({ "name": "Docs" })).await.unwrap(),
        "orgId",
    );
    let token = s(
        alice
            .post_confirmed(
                &format!("orgs/{org}/invites"),
                json!({ "role": "member" }),
                &mut a,
            )
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
        .delete_confirmed(&format!("orgs/{org}/teams/{open}/members/{bob_id}"), &mut a)
        .await
        .unwrap();
    let err = set(&bob, &open_app, "x").await.unwrap_err();
    assert_eq!(err.code(), Some("NOT_FOUND"), "{err}");
    let _ok = alice
        .post_confirmed(
            &format!("orgs/{org}/teams/{open}/members"),
            json!({ "account": bob_id }),
            &mut a,
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
        .post_confirmed(
            &format!("orgs/{org}/teams/{secret}/members"),
            json!({ "account": bob_id, "role": "member" }),
            &mut a,
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
        .post_confirmed(
            &format!("orgs/{org}/teams/{child}/members"),
            json!({ "account": bob_id, "role": "readOnly" }),
            &mut a,
        )
        .await
        .unwrap();
    let err = set(&bob, &child_app, "ro").await.unwrap_err();
    assert_eq!(err.code(), Some("FORBIDDEN"), "{err}");
    // Leaving does not shed the role: he does not come back by inheritance.
    let _ok = bob
        .delete_confirmed(
            &format!("orgs/{org}/teams/{child}/members/{bob_id}"),
            &mut b,
        )
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
        .post_confirmed(
            &format!("orgs/{org}/teams/{child}/members"),
            json!({ "account": bob_id, "role": "readOnly" }),
            &mut a,
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
        .post_confirmed(
            &format!("orgs/{org}/capabilities"),
            json!({ "capabilities": 37 }),
            &mut a,
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
    let (laptop, mut pk) = signed_up(&c, 0, "alice").await;
    let mut phone_key = SoftAuthenticator::new("localhost", ORIGIN);
    let phone_cred = laptop.add_passkey(&mut phone_key, &mut pk).await.unwrap();
    let phone = c.client(1);
    let _s = phone.sign_in(&mut phone_key).await.unwrap();
    let (a, b) = (laptop.me().await.unwrap(), phone.me().await.unwrap());
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
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
    let e = alice
        .view(&app, "wipe", json!({}), Consistency::Strong)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("APP_ERROR"), "{e}");
    assert_eq!(e.details().unwrap()["code"], "ConfirmNotAView", "{e}");
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
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
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
        .call(&app, "spin", json!({}), Some("k-1"))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("TIMEOUT"), "{e}");
    assert!(e.details().unwrap()["retryAfter"].is_u64(), "{e}");
    assert_eq!(applied(), before, "a timeout was logged");
    // The key was not spent: the same key on a fast method runs it.
    let r = alice
        .call(
            &app,
            "set",
            json!({ "key": "a", "value": "1" }),
            Some("k-1"),
        )
        .await
        .unwrap();
    assert_eq!(r.index, before + 1);
    let v = alice
        .view(&app, "get", json!({ "key": "a" }), Consistency::Strong)
        .await
        .unwrap();
    assert_eq!(v.result, json!("1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn oversized_entry_is_host_denied() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
    let _r = alice
        .call(&app, "set", json!({ "key": "a", "value": "1" }), None)
        .await
        .unwrap();
    // 1,000 keys of 1,000 bytes, each with its event: within every per-run limit
    // (writes, events, event size), but about 2 MB of diff and events.
    let e = alice
        .call(&app, "fill", json!({ "n": 1000, "size": 1000 }), None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("TRAP"), "{e}");
    assert_eq!(e.details().unwrap()["kind"], "HostDenied", "{e}");
    let v = alice
        .view(&app, "len", json!({}), Consistency::Strong)
        .await
        .unwrap();
    assert_eq!(v.result, json!(1));
    let _r = alice
        .call(&app, "set", json!({ "key": "b", "value": "2" }), None)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn entry_timestamps_never_go_backwards() {
    let mut c = Cluster::start(3).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
    let first = alice.call(&app, "stamp", json!({}), None).await.unwrap();
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
    let second = cl.call(&app, "stamp", json!({}), None).await.unwrap();
    let t2 = as_u64(&second.result);
    assert!(t2 > t1, "entry time went back: {t1} -> {t2}");

    let follower = (0..3).find(|i| *i != slow).unwrap();
    let fc = c.client(follower);
    let _s = fc.sign_in(&mut pk).await.unwrap();
    let seen = fc
        .view(
            &app,
            "timeNow",
            json!({}),
            Consistency::Stale {
                min_index: second.index,
            },
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
        alice.post("orgs", json!({ "name": "Acme" })).await.unwrap(),
        "orgId",
    );
    for member in [&bob, &carol] {
        let token = s(
            alice
                .post_confirmed(
                    &format!("orgs/{org}/invites"),
                    json!({ "role": "member" }),
                    &mut a,
                )
                .await
                .unwrap(),
            "token",
        );
        let _r = member
            .post(&format!("invites/{token}/redeem"), json!({}))
            .await
            .unwrap();
    }
    let bob_id = s(bob.me().await.unwrap(), "accountId");
    let bundle = alice.upload_bundle(wasm("org-gated")).await.unwrap();
    let app = alice
        .create_app("notes", &bundle, json!({ "org": org }))
        .await
        .unwrap();

    let mut bob_events = bob.events(&app, 0).await.unwrap();
    let mut carol_events = carol.events(&app, 0).await.unwrap();
    let _r = alice
        .call(&app, "announce", json!({ "text": "one" }), None)
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
        .delete_confirmed(&format!("orgs/{org}/members/{bob_id}"), &mut a)
        .await
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
        .call(&app, "announce", json!({ "text": "two" }), None)
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
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
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
        .call(&app, "newId", json!({}), None)
        .await
        .unwrap()
        .result);
    let b = id(&alice
        .call(&app, "newId", json!({}), None)
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
        if let Ok(r) = cl.call(&app, "newId", json!({}), None).await {
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
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
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
        .view(&app, "len", json!({}), Consistency::Strong)
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
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let open = alice
        .create_app("kv", &bundle, json!({ "access": { "kind": "public" } }))
        .await
        .unwrap();
    let closed = alice.create_app("kv2", &bundle, json!({})).await.unwrap();
    let anonymous = c.client(2);
    let set = json!({ "key": "a", "value": "1" });
    for app in [&open, &closed] {
        let e = anonymous
            .call(app, "set", set.clone(), None)
            .await
            .unwrap_err();
        assert_eq!(e.code(), Some("UNAUTHENTICATED"), "{app}: {e}");
        let e = anonymous
            .view(app, "len", json!({}), Consistency::Strong)
            .await
            .unwrap_err();
        assert_eq!(e.code(), Some("UNAUTHENTICATED"), "{app}: {e}");
    }
    let e = bob.call(&closed, "set", set, None).await.unwrap_err();
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
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
    let mut before = Vec::new();
    for _ in 0..3 {
        let r = alice.call(&app, "newId", json!({}), None).await.unwrap();
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
            if let Ok(r) = cl.call(&app, "newId", json!({}), None).await {
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
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
    let mut before = Vec::new();
    for _ in 0..3 {
        let r = alice.call(&app, "newId", json!({}), None).await.unwrap();
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
        let r = cl.call(&app, "newId", json!({}), None).await.unwrap();
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
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
    let first = id_parts(
        &alice
            .call(&app, "newId", json!({}), None)
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
        .call(
            &app,
            "put",
            json!({ "key": "big", "value": "x".repeat(600 * 1024) }),
            Some("big-1"),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("ARGS_TOO_LARGE"), "{e}");
    assert_eq!(applied(), at, "a refused call was logged");
    // Nothing ran: the next execution takes the next id_seq.
    let next = id_parts(
        &alice
            .call(&app, "newId", json!({}), None)
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
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
    let e = alice
        .call(
            &app,
            "put",
            json!({ "key": "big", "value": "x".repeat(450 * 1024) }),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("TRAP"), "{e}");
    assert_eq!(e.details().unwrap()["kind"], "HostDenied", "{e}");
    assert_eq!(e.details().unwrap()["detail"], "entry too large", "{e}");
    let _r = alice
        .call(&app, "set", json!({ "key": "a", "value": "1" }), None)
        .await
        .unwrap();
}

/// The node refuses what the ABI does not offer (Ruling 31): an unknown method,
/// and `init` through `/intents` or `/views`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_methods_and_init_are_refused_by_the_node() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
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
    let e = alice.call(&app, "nope", json!({}), None).await.unwrap_err();
    assert_eq!(e.code(), Some("APP_ERROR"), "{e}");
    assert_eq!(e.details().unwrap()["code"], "UnknownMethod", "{e}");
    let e = alice.call(&app, "init", json!({}), None).await.unwrap_err();
    assert_eq!(e.details().unwrap()["code"], "WrongKind", "{e}");
    let e = alice
        .view(&app, "init", json!({}), Consistency::Strong)
        .await
        .unwrap_err();
    assert_eq!(e.details().unwrap()["code"], "WrongKind", "{e}");
    assert_eq!(applied(), at, "a refused call was logged");
}

/// Upgrades switch the bundle, code only; a named migration is refused (D38).
/// The upgrade route is the node's: a passkey assertion over the exact
/// upgrade (412 first), and a refusal answered `422 UPGRADE_FAILED` with the
/// leader's answer in its details.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upgrades_switch_the_bundle_and_refuse_migrations() {
    let c = Cluster::start(3).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let v1 = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &v1, json!({})).await.unwrap();
    let _r = alice
        .call(&app, "set", json!({ "key": "a", "value": "kept" }), None)
        .await
        .unwrap();
    // The same app with a custom section: another bundle hash.
    let mut wasm = kv_wasm();
    wasm.extend_from_slice(&[0, 4, 3, b'v', b'2', b'!']);
    let v2 = alice.upload_bundle(wasm).await.unwrap();
    assert_ne!(v1, v2);

    let refused = alice
        .upgrade_app(
            &app,
            json!({ "bundle": v2, "migrate": "migrate_v2" }),
            &mut pk,
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
        alice.get(&format!("apps/{app}")).await.unwrap()["bundle"],
        json!(v1)
    );

    let up = alice
        .upgrade_app(&app, json!({ "bundle": v2 }), &mut pk)
        .await
        .unwrap();
    assert_eq!(up["status"], "ok", "{up}");
    assert_eq!(
        alice.get(&format!("apps/{app}")).await.unwrap()["bundle"],
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
        .view(app, "beats", json!({}), Consistency::Strong)
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
        if let Ok(v) = cl.view(app, "beats", json!({}), Consistency::Strong).await {
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
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
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
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
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
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
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
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
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
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
    c.advance_all(HOUR);
    let first = beats_reach(&alice, &app, 1, 10).await;
    // The next two slots fail (the beat returns an AppError).
    let _r = alice
        .call(&app, "failBeats", json!({ "n": 2 }), None)
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
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
    // Ticks whose entry time is within the next hour and 1.5 s spin past the deadline.
    let stall = alice
        .call(
            &app,
            "stallBeats",
            json!({ "ms": (HOUR + 1500).to_string() }),
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
    let v1 = alice.upload_bundle(kv_wasm()).await.unwrap();
    let mut other = kv_wasm();
    other.extend_from_slice(&[0, 4, 3, b'b', b'o', b'b']);
    let bobs = bob.upload_bundle(other).await.unwrap();
    let public = alice
        .create_app("kv", &v1, json!({ "access": { "kind": "public" } }))
        .await
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
            "__calimero_anything",
        ] {
            let e = cl
                .call(&public, method, upgrade.clone(), None)
                .await
                .unwrap_err();
            assert_eq!(e.code(), Some("RESERVED_METHOD"), "{who} {method}: {e}");
            let e = cl
                .view(&public, method, upgrade.clone(), Consistency::Strong)
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
        alice.get(&format!("apps/{public}")).await.unwrap()["bundle"],
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
    let gated = alice.upload_bundle(wasm("org-gated")).await.unwrap();
    let kv = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("app", &gated, json!({})).await.unwrap();
    let leader = c.app_leader(&app).unwrap();
    let g = c.nodes[leader].as_ref().unwrap().node.hosted(&app).unwrap();
    let _p = g
        .propose(|m, _| {
            calimero_log::Proposal::Append(
                serde_json::to_vec(&calimero_node::component::ComponentCmd::Upgrade {
                    bundle: kv.clone(),
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
        alice.get(&format!("apps/{app}")).await.unwrap()["bundle"],
        json!(gated)
    );

    // `wipe` is a confirm method of the running bundle: it needs a consent.
    let e = alice.call(&app, "wipe", json!({}), None).await.unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    // Straight to the node (no challenge flow): the leader refuses it too.
    let account = alice.me().await.unwrap()["accountId"]
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
        .call(&app, "set", json!({ "key": "a", "value": "1" }), None)
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
        alice.post("orgs", json!({ "name": "Docs" })).await.unwrap(),
        "orgId",
    );
    let token = s(
        alice
            .post_confirmed(
                &format!("orgs/{org}/invites"),
                json!({ "role": "member" }),
                &mut a,
            )
            .await
            .unwrap(),
        "token",
    );
    let _r = bob
        .post(&format!("invites/{token}/redeem"), json!({}))
        .await
        .unwrap();
    let bob_id = s(bob.me().await.unwrap(), "accountId");
    let team = s(
        alice
            .post(
                &format!("orgs/{org}/teams"),
                json!({ "name": "Team", "join": "everyone" }),
            )
            .await
            .unwrap(),
        "teamId",
    );
    let _ok = alice
        .post_confirmed(
            &format!("orgs/{org}/teams/{team}/members"),
            json!({ "account": bob_id, "role": "readOnly" }),
            &mut a,
        )
        .await
        .unwrap();
    let bundle = alice.upload_bundle(wasm("org-gated")).await.unwrap();
    let app = alice
        .create_app(
            "notes",
            &bundle,
            json!({ "org": org, "access": { "kind": "team", "team": team } }),
        )
        .await
        .unwrap();
    let leader = c.app_leader(&app).unwrap();
    let before = c.applied_on(leader, &app);
    let e = bob
        .call(&app, "announce", json!({ "text": "hi" }), None)
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
        .call(&app, "announce", json!({ "text": "hi" }), None)
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
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let a = alice.create_app("a", &bundle, json!({})).await.unwrap();
    let b = alice.create_app("b", &bundle, json!({})).await.unwrap();

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
        alice.call(&a, "spin", json!({}), None),
        alice.call(&b, "spin", json!({}), None),
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
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
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
    let v1 = alice.upload_bundle(kv_wasm()).await.unwrap();
    let personal = alice
        .create_app("kv", &v1, json!({ "access": { "kind": "public" } }))
        .await
        .unwrap();
    let org = alice.post("orgs", json!({ "name": "Acme" })).await.unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let in_org = alice
        .create_app("kv-org", &v1, json!({ "org": org }))
        .await
        .unwrap();
    let mut other = kv_wasm();
    other.extend_from_slice(&[0, 4, 3, b'b', b'o', b'b']);
    let v2 = alice.upload_bundle(other).await.unwrap();
    // Bob: another account. Carol: a member of Acme, without MANAGE_APPLICATION.
    let (bob, mut bob_pk) = signed_up(&c, 1, "bob").await;
    let (carol, mut carol_pk) = signed_up(&c, 2, "carol").await;
    let token = alice
        .post_confirmed(
            &format!("orgs/{org}/invites"),
            json!({ "role": "member" }),
            &mut alice_pk,
        )
        .await
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = carol
        .post(&format!("invites/{token}/redeem"), json!({}))
        .await
        .unwrap();
    let id_of = |v: Value| v["accountId"].as_str().unwrap().to_owned();
    let bob_id = id_of(bob.me().await.unwrap());
    let carol_id = id_of(carol.me().await.unwrap());

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
                .upgrade_app(app, json!({ "bundle": v2 }), pk)
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
            })
            .unwrap();
            let e = c.nodes[i]
                .as_ref()
                .unwrap()
                .node
                .sys_write(&SystemCmd::UpgradeApp {
                    app_id: app.clone(),
                    bundle: v2.clone(),
                    by_operator: false,
                    actor: id.clone(),
                    at: calimero_node::types::now_secs(),
                    run: None,
                    consent,
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
            alice.get(&format!("apps/{app}")).await.unwrap()["bundle"],
            json!(v1)
        );
    }
    // The owner may.
    let up = alice
        .upgrade_app(&personal, json!({ "bundle": v2 }), &mut alice_pk)
        .await
        .unwrap();
    assert_eq!(up["status"], "ok", "{up}");
    assert_eq!(
        alice.get(&format!("apps/{personal}")).await.unwrap()["bundle"],
        json!(v2)
    );
}

/// The core-module fixture (`apps/wasmer-fixtures/kv-store`, SDK 0.12), which runs on wasmer.
fn wasmer_kv() -> Vec<u8> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/wasm32-unknown-unknown/app-release/kv_store.wasm");
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}; cargo build -p kv-store --target wasm32-unknown-unknown --profile app-release",
            path.display()
        )
    })
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
        if let Ok(r) = cl.call(app, "newId", json!({}), None).await {
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
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
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
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
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

/// Writes round `round` (`k{round}` = `v{round}`) to both apps through `cl`, retrying
/// while the apps elect; answers the two entries' indexes.
async fn write_both(cl: &Client, apps: [&str; 2], round: usize) -> [u64; 2] {
    let (key, value) = (format!("k{round}"), format!("v{round}"));
    let mut out = [0; 2];
    for (slot, app) in apps.into_iter().enumerate() {
        let mut done = None;
        for _ in 0..60 {
            if let Ok(r) = cl
                .call(app, "set", json!({ "key": key, "value": value }), None)
                .await
            {
                done = Some(r.index);
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        out[slot] = done.unwrap_or_else(|| panic!("{app} took no write in round {round}"));
    }
    out
}

/// Every running node's own replica answers every round of both apps, once it has
/// applied the given index of each.
async fn read_everywhere(
    c: &Cluster,
    pk: &mut SoftAuthenticator,
    apps: [&str; 2],
    rounds: std::ops::Range<usize>,
    indexes: [u64; 2],
) {
    for (i, r) in c.nodes.iter().enumerate() {
        if r.is_none() {
            continue;
        }
        let cl = c.client(i);
        let _s = cl.sign_in(pk).await.unwrap();
        for round in rounds.clone() {
            let key = format!("k{round}");
            for (app, min_index) in apps.into_iter().zip(indexes) {
                let v = cl
                    .view(
                        app,
                        "get",
                        json!({ "key": key }),
                        Consistency::Stale { min_index },
                    )
                    .await
                    .unwrap_or_else(|e| panic!("node {i} {app}: {e:?}"));
                assert_eq!(v.result, json!(format!("v{round}")), "node {i} {app} {key}");
            }
        }
    }
}

/// One cluster runs a core module on wasmer and a component on wasmtime side by
/// side: writes to them in turns, each reads back on every node, including after
/// a leader change.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn wasmer_and_component_apps_share_a_cluster() {
    use calimero_node::types::Executor;

    let mut c = Cluster::start(3).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let v1 = alice.upload_bundle(wasmer_kv()).await.unwrap();
    let v2 = alice.upload_bundle(kv_wasm()).await.unwrap();
    let old = alice.create_app("old", &v1, json!({})).await.unwrap();
    let new = alice.create_app("new", &v2, json!({})).await.unwrap();
    let apps = [old.as_str(), new.as_str()];
    for (app, executor) in [(&old, Executor::Wasmer), (&new, Executor::Component)] {
        let record = c.nodes[0].as_ref().unwrap().node.app(app).await.unwrap();
        assert_eq!(record.executor, executor, "{app}");
    }
    let mut last = [0; 2];
    for round in 0..6 {
        last = write_both(&alice, apps, round).await;
    }
    read_everywhere(&c, &mut pk, apps, 0..6, last).await;

    // A leader change: the node leading the wasmer app stops (it may lead both).
    let down = c.app_leader(&old).unwrap();
    c.stop(down).await;
    let survivor = (0..3).find(|i| *i != down).unwrap();
    let cl = c.client(survivor);
    let _s = cl.sign_in(&mut pk).await.unwrap();
    for round in 6..10 {
        last = write_both(&cl, apps, round).await;
    }
    read_everywhere(&c, &mut pk, apps, 0..10, last).await;
    // The stopped node comes back and catches up on both, each in its own format.
    c.restart(down, |_| {}).await;
    read_everywhere(&c, &mut pk, apps, 0..10, last).await;
    for i in 0..3 {
        let node = &c.nodes[i].as_ref().unwrap().node;
        let executors = (
            node.hosted(&old)
                .unwrap()
                .read_local(|m, _| m.state.executor()),
            node.hosted(&new)
                .unwrap()
                .read_local(|m, _| m.state.executor()),
        );
        assert_eq!(
            executors,
            (Some(Executor::Wasmer), Some(Executor::Component)),
            "node {i}"
        );
    }
}

/// Decision 2: an upgrade never moves an app between executors, either way,
/// whatever its state; the app stays on its bundle and keeps working.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upgrades_across_executors_are_refused_both_ways() {
    let c = Cluster::start(3).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let v1 = alice.upload_bundle(wasmer_kv()).await.unwrap();
    let v2 = alice.upload_bundle(kv_wasm()).await.unwrap();
    // One app with state on each executor, and one fresh one (no state of its own).
    let old = alice.create_app("old", &v1, json!({})).await.unwrap();
    let new = alice.create_app("new", &v2, json!({})).await.unwrap();
    let fresh = alice.create_app("fresh", &v2, json!({})).await.unwrap();
    for app in [&old, &new] {
        let _r = alice
            .call(app, "set", json!({ "key": "a", "value": "kept" }), None)
            .await
            .unwrap();
    }
    for (app, from, to) in [(&old, &v1, &v2), (&new, &v2, &v1), (&fresh, &v2, &v1)] {
        let e = alice
            .upgrade_app(app, json!({ "bundle": to }), &mut pk)
            .await
            .unwrap_err();
        assert_eq!(e.code(), Some("UPGRADE_FAILED"), "{app}: {e}");
        assert!(
            e.details().unwrap()["error"]
                .as_str()
                .is_some_and(|m| m.contains("across executors")),
            "{app}: {e}"
        );
        assert_eq!(
            alice.get(&format!("apps/{app}")).await.unwrap()["bundle"],
            json!(from),
            "{app}"
        );
        for i in 0..3 {
            assert_eq!(
                running_bundle(&c, i, app).as_deref(),
                Some(from.as_str()),
                "node {i}"
            );
        }
    }
    for app in [&old, &new] {
        let v = alice
            .view(app, "get", json!({ "key": "a" }), Consistency::Strong)
            .await
            .unwrap();
        assert_eq!(v.result, json!("kept"), "{app}");
    }
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
    let bundle = alice.upload_bundle(component).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
    let _r = alice
        .call(&app, "set", json!({ "key": "a", "value": "1" }), None)
        .await
        .unwrap();

    let abi = alice.get(&format!("apps/{app}/abi")).await.unwrap();
    assert_eq!(abi["bundle"], json!(bundle));
    assert_eq!(abi["abi"], embedded, "not the component's own ABI");
    assert!(abi["abi"]["methods"]["set"].is_object());

    let storage = alice.get(&format!("apps/{app}/storage")).await.unwrap();
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
    assert_eq!(alice.list_blobs(&app).await.unwrap()["blobs"], json!([]));
    let id = "00".repeat(32);
    let e = alice.blob_info(&app, &id).await.unwrap_err();
    assert_eq!(e.code(), Some("NOT_FOUND"), "{e}");
    let e = alice.download_blob(&app, &id).await.unwrap_err();
    assert_eq!(e.code(), Some("NOT_FOUND"), "{e}");
}

/// R8: a component's blob is uploaded through the node, referred to from state by a
/// typed `BlobRef`, read and granted from runs, and held for as long as state refers
/// to it; once idle past its time every replica forgets it and the bytes go.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_components_blobs_are_held_by_references_and_granted() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();

    let up = alice
        .upload_blob_as(&app, "text/plain", b"hello".to_vec())
        .await
        .unwrap();
    let id = up["id"].as_str().unwrap().to_owned();
    assert_eq!(up["size"], json!(5));
    assert_eq!(alice.blob_info(&app, &id).await.unwrap()["size"], json!(5));
    let r = alice
        .call(
            &app,
            "attach",
            json!({ "name": "greeting", "blob": id }),
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
    assert_eq!(alice.download_blob(&app, &id).await.unwrap(), b"hello");

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
            .download_blob_granted(&app, &id, &grant)
            .await
            .unwrap(),
        b"hello"
    );
    let e = anonymous.download_blob(&app, &id).await.unwrap_err();
    assert_eq!(e.code(), Some("UNAUTHENTICATED"), "{e}");
    let forged = format!("9{grant}");
    let e = anonymous
        .download_blob_granted(&app, &id, &forged)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NOT_FOUND"), "{e}");

    // A value may refer only to a blob the app holds.
    let e = alice
        .call(
            &app,
            "attach",
            json!({ "name": "x", "blob": "11".repeat(32) }),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("TRAP"), "{e}");

    // An upload nobody refers to, and the referred one once detached, go after their
    // idle time; the referred one stays until then.
    let idle = alice
        .upload_blob_as(&app, "text/plain", b"never attached".to_vec())
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    c.advance_all(2 * 3_600_000);
    let _r = alice
        .call(&app, "set", json!({ "key": "tick", "value": "1" }), None)
        .await
        .unwrap();
    let e = alice.blob_info(&app, &idle).await.unwrap_err();
    assert_eq!(e.code(), Some("NOT_FOUND"), "{e}");
    assert_eq!(alice.blob_info(&app, &id).await.unwrap()["size"], json!(5));
    let _r = alice
        .call(&app, "detach", json!({ "name": "greeting" }), None)
        .await
        .unwrap();
    c.advance_all(2 * 3_600_000);
    let _r = alice
        .call(&app, "set", json!({ "key": "tick", "value": "2" }), None)
        .await
        .unwrap();
    let mut gone = false;
    for _ in 0..50 {
        if alice.list_blobs(&app).await.unwrap()["blobs"] == json!([])
            && alice.download_blob(&app, &id).await.is_err()
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
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
    let mut last = 0;
    for (id, title, body) in [
        ("1", "Groceries", "milk and eggs"),
        ("2", "Trip", "pack the eggs carefully"),
        ("3", "Work", "quarterly plans"),
    ] {
        last = alice
            .call(
                &app,
                "note",
                json!({ "id": id, "title": title, "body": body }),
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
            .view(
                &app,
                "findNotes",
                json!({ "text": "quarterly" }),
                Consistency::Stale { min_index: last },
            )
            .await
            .unwrap();
        assert_eq!(v.result, json!(["3"]), "node {i}");
    }
    // Updates and removals reach the index before the next view reads.
    let _r = alice
        .call(
            &app,
            "note",
            json!({ "id": "3", "title": "Work", "body": "eggs too" }),
            None,
        )
        .await
        .unwrap();
    let _r = alice
        .call(&app, "dropNote", json!({ "id": "1" }), None)
        .await
        .unwrap();
    assert_eq!(
        sorted(strong_view(&alice, &app, "findNotes", json!({ "text": "eggs" })).await),
        ["2", "3"]
    );
    // A call of a declared view runs as a strong view, so it may search too.
    let r = alice
        .call(&app, "findNotes", json!({ "text": "eggs" }), None)
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
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let source = alice
        .create_app("source", &bundle, json!({}))
        .await
        .unwrap();
    let target = alice
        .create_app("target", &bundle, json!({}))
        .await
        .unwrap();
    let bobs = bob.create_app("bobs", &bundle, json!({})).await.unwrap();

    for i in 0..5 {
        let _r = alice
            .call(
                &source,
                "relay",
                json!({ "app": target, "key": "k", "value": format!("{i}") }),
                None,
            )
            .await
            .unwrap();
    }
    // Delivered in order: the last value wins, and the outbox empties.
    let mut last = Value::Null;
    for _ in 0..100 {
        last = strong_view(&alice, &target, "get", json!({ "key": "k" })).await;
        let pending = alice.app_xcalls(&source).await.unwrap()["pending"].clone();
        if last == json!("4") && pending == json!(0) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(last, json!("4"));
    assert_eq!(
        alice.app_xcalls(&source).await.unwrap()["deadLetters"],
        json!([])
    );

    // The target refuses a method that allows no app: a final answer, no dead letter.
    let _r = alice
        .call(
            &source,
            "relayToSet",
            json!({ "app": target, "key": "z", "value": "1" }),
            None,
        )
        .await
        .unwrap();
    // A missing target and another owner's app are dead-lettered at once.
    let _r = alice
        .call(
            &source,
            "relay",
            json!({ "app": "no-such-app", "key": "a", "value": "1" }),
            None,
        )
        .await
        .unwrap();
    let _r = alice
        .call(
            &source,
            "relay",
            json!({ "app": bobs, "key": "b", "value": "1" }),
            None,
        )
        .await
        .unwrap();
    let mut dead = Vec::new();
    for _ in 0..100 {
        let x = alice.app_xcalls(&source).await.unwrap();
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
    let e = bob.app_xcalls(&source).await.unwrap_err();
    assert_eq!(e.code(), Some("NOT_FOUND"), "{e}");
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
        links: None,
        frontend: None,
        frontend_aliases: Vec::new(),
        upgrades: None,
        outbound: outbound.iter().map(|u| (*u).to_owned()).collect(),
        services: None,
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
        .upload_package(mpk_declaring(
            PACKAGE,
            "0.1.0",
            &kv_wasm(),
            &publisher,
            &[PREFIX],
        ))
        .await
        .unwrap();
    let app = alice
        .post("apps", json!({ "name": "kv", "package": PACKAGE }))
        .await
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = alice
        .call(
            &app,
            "set",
            json!({ "key": "acme", "value": "acme.com" }),
            None,
        )
        .await
        .unwrap();

    // Not approved yet: the request is refused, the action fails, onActionFailed hears.
    let started = alice
        .call(&app, "enrich", json!({ "key": "acme" }), None)
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
        .approve_outbound(None, PACKAGE, None, false, &mut pk)
        .await
        .unwrap();
    let id = alice
        .call(&app, "startEnrich", json!({ "key": "acme" }), None)
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
        .call(
            &app,
            "applyEnrichment",
            json!({ "key": "acme", "value": "forged" }),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("APP_ERROR"), "{e}");

    // An app error fails an action at once (D22).
    let _r = alice.call(&app, "explode", json!({}), None).await.unwrap();
    let failures = eventually(&alice, &app, "actionFailures", json!({}), |f| {
        f["explode"].is_string()
    })
    .await;
    assert!(
        failures["explode"].as_str().unwrap().contains("BeatFailed"),
        "{failures}"
    );
}

/// R8: the owner deletes a component's blob; it is gone whatever refers to it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_owner_deletes_a_components_blob() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
    let id = alice.upload_blob(&app, b"bytes".to_vec()).await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = alice
        .call(&app, "attach", json!({ "name": "f", "blob": id }), None)
        .await
        .unwrap();
    let _d = alice.delete_blob(&app, &id).await.unwrap();
    assert_eq!(alice.list_blobs(&app).await.unwrap()["blobs"], json!([]));
    assert_eq!(
        strong_view(&alice, &app, "file", json!({ "name": "f" })).await,
        json!(null)
    );
    let e = alice.download_blob(&app, &id).await.unwrap_err();
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
/// wasmer bundle is refused; a component bundle is taken, and `init` then runs on it.
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
    let stuck = alice.upload_bundle(spin_component()).await.unwrap();
    let fixed = alice.upload_bundle(kv_wasm()).await.unwrap();
    let wasmer = alice.upload_bundle(wasmer_kv()).await.unwrap();
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
                .post("apps", json!({ "name": "stuck", "bundle": stuck }))
                .await;
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
    // A wasmer bundle: refused, nothing switched.
    let e = alice
        .upgrade_app(&app, json!({ "bundle": wasmer }), &mut pk)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("UPGRADE_FAILED"), "{e}");
    assert!(
        e.details().unwrap()["error"]
            .as_str()
            .is_some_and(|m| m.contains("across executors")),
        "{e}"
    );
    for i in 0..3 {
        let g = hosted_on(&c, i, &app).await;
        assert_eq!(
            g.read_local(|m, _| m.state.bundle.clone()),
            None,
            "node {i}"
        );
    }
    // A component bundle: taken, and init then runs on it.
    let up = alice
        .upgrade_app(&app, json!({ "bundle": fixed }), &mut pk)
        .await
        .unwrap();
    assert_eq!(up["status"], "ok", "{up}");
    let mut done = None;
    for _ in 0..80 {
        if let Ok(r) = alice
            .call(&app, "set", json!({ "key": "a", "value": "1" }), None)
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
/// app's org learns only her role there, and a personal app learns nothing about its
/// callers' organizations.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_component_sees_no_organization_but_its_own() {
    use calimero_app_host::{Directory as _, OrgRole};
    use calimero_node::component::SysDirectory;

    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (carol, _c) = signed_up(&c, 1, "carol").await;
    let carol_id = carol.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let acme = alice.post("orgs", json!({ "name": "Acme" })).await.unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let token = alice
        .post_confirmed(
            &format!("orgs/{acme}/invites"),
            json!({ "role": "member" }),
            &mut a,
        )
        .await
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = carol
        .post(&format!("invites/{token}/redeem"), json!({}))
        .await
        .unwrap();
    let other = carol
        .post("orgs", json!({ "name": "Other" }))
        .await
        .unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let in_org = alice
        .create_app("kv-org", &bundle, json!({ "org": acme }))
        .await
        .unwrap();
    let personal = alice.create_app("kv", &bundle, json!({})).await.unwrap();
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
        "a personal app saw its caller's orgs"
    );
    let alice_id = alice.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        dir.orgs_of(&alice_id),
        Vec::new(),
        "a personal app saw its owner's orgs"
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
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
    let id = "00".repeat(32);
    let status = |e: calimero_client::Error| match e {
        calimero_client::Error::Api { status, code, .. } => (status, code),
        other => panic!("{other:?}"),
    };
    let not_found = (404, "NOT_FOUND".to_owned());
    assert_eq!(
        status(bob.list_blobs(&app).await.unwrap_err()),
        not_found,
        "list"
    );
    assert_eq!(
        status(bob.blob_info(&app, &id).await.unwrap_err()),
        not_found,
        "info"
    );
    assert_eq!(
        status(bob.download_blob(&app, &id).await.unwrap_err()),
        not_found,
        "download"
    );
    assert_eq!(
        status(bob.delete_blob(&app, &id).await.unwrap_err()),
        not_found,
        "delete"
    );
    assert_eq!(
        status(bob.upload_blob(&app, b"x".to_vec()).await.unwrap_err()),
        not_found,
        "upload"
    );
    // Its owner gets the app's blobs (R8).
    assert_eq!(alice.list_blobs(&app).await.unwrap()["blobs"], json!([]));
}

/// Review nit (D70): a same-key retry is answered from its record even when an upgrade
/// since removed the method: the replay comes before the ABI check.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retry_replays_even_after_an_upgrade_removed_its_method() {
    let c = Cluster::start(3).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let kv = alice.upload_bundle(kv_wasm()).await.unwrap();
    let gated = alice.upload_bundle(wasm("org-gated")).await.unwrap();
    let app = alice.create_app("kv", &kv, json!({})).await.unwrap();
    let first = alice
        .call(
            &app,
            "set",
            json!({ "key": "a", "value": "1" }),
            Some("retry-me"),
        )
        .await
        .unwrap();
    let up = alice
        .upgrade_app(&app, json!({ "bundle": gated }), &mut pk)
        .await
        .unwrap();
    assert_eq!(up["status"], "ok", "{up}");
    // `set` is gone from the running bundle; a new call of it is refused...
    let e = alice
        .call(
            &app,
            "set",
            json!({ "key": "a", "value": "2" }),
            Some("fresh"),
        )
        .await
        .unwrap_err();
    assert_eq!(e.details().unwrap()["code"], "UnknownMethod", "{e}");
    // ...but the retry of the committed one is its recorded answer, from any node.
    for i in 0..3 {
        let cl = c.client(i);
        let _s = cl.sign_in(&mut pk).await.unwrap();
        let again = cl
            .call(
                &app,
                "set",
                json!({ "key": "a", "value": "1" }),
                Some("retry-me"),
            )
            .await
            .unwrap_or_else(|e| panic!("node {i}: {e}"));
        assert_eq!(again.index, first.index, "node {i}");
    }
}

/// Review nit, codes: a confirm method a stale front node did not know is confirm
/// (here: a node that hosts no replica reads the directory's bundle, org-gated,
/// while the group runs kv, whose `wipe` is a confirm method) still answers the
/// spec's step-up, `412 CONFIRM_REQUIRED` with a challenge, as the wasmer path does,
/// not the leader's `422 ConfirmRequired`.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_confirm_method_a_stale_front_missed_still_asks_for_a_passkey() {
    let c = Cluster::start(4).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let gated = alice.upload_bundle(wasm("org-gated")).await.unwrap();
    let kv = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("app", &gated, json!({})).await.unwrap();
    let leader = c.app_leader(&app).unwrap();
    let g = c.nodes[leader].as_ref().unwrap().node.hosted(&app).unwrap();
    let _p = g
        .propose(|m, _| {
            calimero_log::Proposal::Append(
                serde_json::to_vec(&calimero_node::component::ComponentCmd::Upgrade {
                    bundle: kv.clone(),
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
    let e = front.call(&app, "wipe", json!({}), None).await.unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    assert!(
        e.details().unwrap()["challengeId"].is_string(),
        "no challenge: {e}"
    );
}

/// Review nit, codes: a component the engine refuses at `POST /bundles` is a
/// `400 BAD_BUNDLE`, as a core module wasmer can't compile is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_component_upload_is_a_400_like_wasmers() {
    let c = Cluster::start(3).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    // A component's header (layer 1) and nothing a component needs after it.
    let mut broken = b"\0asm\x0d\0\x01\0".to_vec();
    broken.extend_from_slice(&[0; 16]);
    let e = alice.upload_bundle(broken).await.unwrap_err();
    let status = match &e {
        calimero_client::Error::Api { status, .. } => *status,
        other => panic!("{other:?}"),
    };
    assert_eq!((status, e.code()), (400, Some("BAD_BUNDLE")), "{e}");
}
