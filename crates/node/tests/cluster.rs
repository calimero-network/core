//! A three-node cluster in one process, driven only through the client, over
//! sealed channels, with the real kv-store app (the cluster itself is in
//! `harness/cluster.rs`, shared with `benches/cluster_throughput.rs`).
//!
//! Needs `kv_store.wasm`: `cargo build -p kv-store --target wasm32-unknown-unknown --profile app-release`,
//! or set `KV_STORE_WASM`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use calimero_client::{Client, Consistency, Error, Policy};
use calimero_identity::testing::SoftAuthenticator;
use calimero_node::run;
use futures_util::StreamExt;
use serde_json::{json, Value};

#[path = "harness/cluster.rs"]
mod harness;

use harness::{free_port, kv_wasm, node_config, signed_up, Cluster, ORIGIN};

fn wasm(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/wasm32-unknown-unknown/app-release")
        .join(format!("{name}.wasm"));
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!("{}: {e}; build it with cargo build --target wasm32-unknown-unknown --profile app-release", path.display())
    })
}

impl Cluster {
    /// Start one more node, not a founder: it knows the cluster's addresses
    /// but joins only once the operator registers it (`/ops/nodes`).
    async fn join(&mut self) -> usize {
        self.join_with(|_| {}).await
    }

    /// [`Self::join`], the node's configuration changed by `configure`. As in a
    /// deployment, the node starts first: on an empty data directory it waits
    /// for the cluster key, serving `/health`, until the operator registers it
    /// (`/ops/nodes`, no zone yet) and a peer delivers the key to its address.
    async fn join_with(&mut self, configure: impl Fn(&mut calimero_node::Config)) -> usize {
        let id = self.urls.len() as u64 + 1;
        let url = format!("http://127.0.0.1:{}/", free_port());
        self.urls.push(url.clone());
        let peers: BTreeMap<u64, String> = self
            .urls
            .iter()
            .enumerate()
            .map(|(i, u)| (i as u64 + 1, u.clone()))
            .collect();
        let dir = tempfile::tempdir().unwrap();
        let mut config = node_config(id, &peers, dir.path());
        config.cluster.clone_from(&self.name);
        config.bootstrap = false;
        configure(&mut config);
        let starting = tokio::spawn(run(config));
        let mut waiting = Value::Null;
        for _ in 0..100 {
            if let Ok(r) = reqwest::get(format!("{url}health")).await {
                waiting = r.json().await.unwrap_or_default();
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(waiting["waiting"].is_string(), "{waiting}");
        let at = self.nodes.iter().position(Option::is_some).unwrap();
        let _ = ops_at(self, at)
            .await
            .post("ops/nodes", json!({ "id": id, "addr": url, "zone": "" }))
            .await
            .unwrap();
        self.nodes.push(Some(starting.await.unwrap().unwrap()));
        self._dirs.push(dir);
        self.nodes.len() - 1
    }

    /// Stop node `i` and start it again on its data directory, as a restart
    /// (or a rolling upgrade of its binary) does.
    async fn restart(&mut self, i: usize) {
        self.stop(i).await;
        let peers: BTreeMap<u64, String> = self
            .urls
            .iter()
            .enumerate()
            .map(|(i, u)| (i as u64 + 1, u.clone()))
            .collect();
        let mut config = node_config(i as u64 + 1, &peers, self._dirs[i].path());
        config.cluster.clone_from(&self.name);
        config.bootstrap = false;
        // The stopped node's tasks may hold its store for a moment.
        for _ in 0..50 {
            if let Ok(r) = run(config.clone()).await {
                self.nodes[i] = Some(r);
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("node {} did not restart", i + 1);
    }
}

/// `session` without its refresh token: a client holding it cannot refresh,
/// so what it is answered is what the node judged of the session itself.
fn without_refresh(session: calimero_client::Session) -> calimero_client::Session {
    calimero_client::Session {
        refresh_token: String::new(),
        ..session
    }
}

/// Sends `set key=value` through `cl` until it commits, a node answers 401,
/// or about 10 s pass (an election, a forward to a stopped node).
async fn set_through(cl: &Client, app: &str, key: &str, value: &str) -> Result<(), String> {
    let mut wrote = Err(String::new());
    for _ in 0..100 {
        wrote = cl
            .call(app, "set", json!({ "key": key, "value": value }), None)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string());
        match &wrote {
            Ok(()) => break,
            Err(e) if e.starts_with("401") => break,
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    wrote
}

/// Failover: a session outlives the node that issued it. Every node attests
/// each peer's session-signing key ahead of time, so with the issuer gone
/// another node still verifies the session, without a refresh (merobox
/// `kv-store-failover` and `frozen-leader` write through a survivor with the
/// session the dead or frozen node issued).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_session_outlives_the_node_that_issued_it() {
    let mut c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let (app, _) = deploy(&alice, "kv", kv_wasm(), json!({})).await;
    set_through(&alice, &app, "a", "1").await.unwrap();
    let session = without_refresh(alice.session().await.unwrap());
    // Node 2 has never been shown a session node 1 issued.
    c.stop(0).await;
    let survivor = c.client(1);
    survivor.use_session(session).await;
    set_through(&survivor, &app, "b", "2")
        .await
        .expect("a write through a survivor with the stopped issuer's session");
}

/// A node's sessions survive its restart on the same data directory (a crash,
/// or a rolling upgrade of its binary): the key it signs them with is kept,
/// wrapped under its K_r, and attested again at start (merobox
/// `rolling-upgrade`).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn sessions_survive_a_restart_of_the_node_that_issued_them() {
    let mut c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let (app, _) = deploy(&alice, "kv", kv_wasm(), json!({})).await;
    set_through(&alice, &app, "a", "1").await.unwrap();
    let session = without_refresh(alice.session().await.unwrap());
    c.restart(0).await;
    for i in [0, 1] {
        let cl = c.client(i);
        cl.use_session(session.clone()).await;
        set_through(&cl, &app, &format!("k{i}"), "v")
            .await
            .unwrap_or_else(|e| panic!("node {}: {e}", i + 1));
    }
}

/// Once a node is removed, the sessions it issued verify nowhere: no node
/// keeps a removed node's signing key.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_removed_nodes_sessions_are_refused() {
    let mut c = Cluster::start(3).await;
    let new = c.join().await;
    let new_id = new as u64 + 1;
    // Signed up on the new node: it issued the session.
    let mut issued = None;
    for _ in 0..100 {
        let cl = c.client(new);
        let mut pk = SoftAuthenticator::new("localhost", ORIGIN);
        if let Ok(s) = cl.sign_up(&mut pk, "alice").await {
            issued = Some(s);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let full = issued.expect("signed up on the joined node");
    let session = without_refresh(full.clone());
    let other = c.client(0);
    other.use_session(session).await;
    let _me = other.me().await.expect("another node takes the session");

    let mut removed = false;
    for _ in 0..100 {
        let leader = system_leader(&c).await;
        if ops_at(&c, leader)
            .await
            .delete(&format!("ops/nodes/{new_id}"))
            .await
            .is_ok()
        {
            removed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(removed, "node {new_id} was never removed");
    let mut answer = Ok(Value::Null);
    for _ in 0..50 {
        answer = other.me().await;
        if answer.is_err() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let e = answer.expect_err("a removed node's session must be refused");
    assert!(e.to_string().starts_with("401"), "{e}");

    // A client holding the refresh token refreshes on that 401 on its own,
    // once, on the node it talks to (one that applied the removal), and sends
    // the request again.
    let refreshing = c.client(0);
    refreshing.use_session(full.clone()).await;
    let me = refreshing
        .me()
        .await
        .expect("the client refreshes the session on its own");
    assert!(me["accountId"].is_string(), "{me}");
    let now = refreshing.session().await.unwrap();
    assert_ne!(now.session_token, full.session_token, "a new session token");
    assert_ne!(now.refresh_token, full.refresh_token, "a new refresh token");
}

/// A passkey ceremony begun on one node finishes on another (a load balancer
/// in front of the cluster may send the two requests anywhere), and a
/// challenge is still used once.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_sign_in_begun_on_one_node_finishes_on_another() {
    use calimero_client::Passkey;
    let c = Cluster::start(3).await;
    let (_alice, mut pk) = signed_up(&c, 0, "alice").await;
    let begin = c
        .client(0)
        .post("auth/passkey/login/begin", json!({}))
        .await
        .unwrap();
    let challenge =
        calimero_identity::from_b64url(begin["options"]["challenge"].as_str().unwrap()).unwrap();
    let finish = json!({
        "challengeId": begin["challengeId"],
        "assertion": pk.get(&challenge),
    });
    let s = c
        .client(1)
        .post("auth/passkey/login/finish", finish.clone())
        .await
        .unwrap();
    assert!(s["sessionToken"].is_string(), "{s}");
    for i in [0, 2] {
        let e = c
            .client(i)
            .post("auth/passkey/login/finish", finish.clone())
            .await
            .unwrap_err();
        assert_eq!(e.code(), Some("UNKNOWN_CHALLENGE"), "{e}");
    }
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

/// A node that took its keys from mero-kms reports what it verified of that
/// KMS, but only to a client that asks (`purpose=kms`), and only inside the
/// quote: the record is bound into the report data with the transport key, so
/// a relay that swaps, adds or strips it fails the same check that attests the
/// key. Not asking (every client before this) gets the report data it always
/// has, and a node with no record answers as an older node does.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_reports_the_kms_it_verified_inside_its_quote() {
    use calimero_client::attest::{report_data, verify, AttestationDoc, KmsKeys};
    use calimero_tee_attestation::KmsEvidence;

    let reported = KmsKeys {
        storage: Some(KmsEvidence {
            kms_host: "kms.internal:8080".into(),
            release: "2.4.1".into(),
            profile: Some("locked-read-only".into()),
            mrtd: "aa".repeat(48),
            rtmr0: "00".repeat(48),
            rtmr1: "11".repeat(48),
            rtmr2: "22".repeat(48),
            rtmr3: "33".repeat(48),
            tcb_status: Some("UpToDate".into()),
            quote_sha256: "44".repeat(32),
            verified_at: 1_790_000_000,
            mock: false,
        }),
        disk: None,
    };
    let given = reported.clone();
    let c = Cluster::start_with(1, move |cfg| cfg.kms_keys = given.clone()).await;
    let plain = reqwest::Client::new();
    let mock = Policy {
        allow_mock: true,
        ..Policy::default()
    };
    let attest = |url: String, purpose: Option<&'static str>| {
        let plain = plain.clone();
        async move {
            let nonce = calimero_identity::random::<32>();
            let mut request = format!("{url}attestation?nonce={}", hex::encode(nonce));
            if let Some(p) = purpose {
                request.push_str(&format!("&purpose={p}"));
            }
            let doc: AttestationDoc = plain
                .get(request)
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .await
                .unwrap();
            (nonce, doc)
        }
    };

    let (nonce, doc) = attest(c.urls[0].clone(), Some("kms")).await;
    assert_eq!(doc.kms.as_ref(), Some(&reported));
    verify(&doc, &nonce, &mock)
        .await
        .expect("the record is bound into the quote");
    let mut swapped = doc.clone();
    if let Some(storage) = swapped.kms.as_mut().and_then(|k| k.storage.as_mut()) {
        storage.release = "2.3.1".into();
    }
    assert!(
        verify(&swapped, &nonce, &mock).await.is_err(),
        "a swapped record"
    );
    let stripped = AttestationDoc {
        kms: None,
        ..doc.clone()
    };
    assert!(
        verify(&stripped, &nonce, &mock).await.is_err(),
        "a stripped record"
    );

    // Not asked: no record, and the report data every older client checks.
    let (nonce, doc) = attest(c.urls[0].clone(), None).await;
    assert!(doc.kms.is_none());
    let quote =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &doc.quote).unwrap();
    let key: [u8; 32] = hex::decode(&doc.transport_key).unwrap().try_into().unwrap();
    let header = calimero_client::attest::MOCK_QUOTE_HEADER.len();
    assert_eq!(
        quote[header..header + 64],
        report_data(&nonce, &[0; 32], &key),
        "unchanged for a client that does not ask"
    );
    verify(&doc, &nonce, &mock)
        .await
        .expect("an unasked attestation");

    // A node with no record answers `purpose=kms` as a node from before it.
    let bare = Cluster::start(1).await;
    let (nonce, doc) = attest(bare.urls[0].clone(), Some("kms")).await;
    assert!(doc.kms.is_none());
    verify(&doc, &nonce, &mock)
        .await
        .expect("no record, no binding");
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

/// A quota post changes the limits it names and keeps the others; a key that
/// names no limit is refused instead of ignored (mdma reset `maxAccounts` on
/// every save by posting the four it knew).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_quota_post_changes_only_what_it_names() {
    let c = Cluster::start(3).await;
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    let _ok = ops
        .post("ops/quotas", json!({ "maxAccounts": 7 }))
        .await
        .unwrap();
    let _ok = ops
        .post("ops/quotas", json!({ "maxOrgsPerAccount": 3 }))
        .await
        .unwrap();
    let q = ops.get("ops/quotas").await.unwrap();
    assert_eq!(q["maxAccounts"], json!(7), "{q}");
    assert_eq!(q["maxOrgsPerAccount"], json!(3), "{q}");
    let e = ops
        .post("ops/quotas", json!({ "maxAcounts": 1 }))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("maxAcounts"), "{e}");
    assert_eq!(ops.get("ops/quotas").await.unwrap(), q);
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
/// Entries the running nodes appended that applied as conflicts.
fn conflicts(c: &Cluster) -> u64 {
    c.nodes
        .iter()
        .flatten()
        .map(|r| r.node.conflicts.load(std::sync::atomic::Ordering::Relaxed))
        .sum()
}

/// A new leader's blank entry, like a membership entry, changes no app
/// state. A build's base counted them while `apply` compared it with the last
/// app entry, so the first call after every leader change (and the init after
/// the group formed) applied as a conflict and ran again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_first_call_after_a_leader_change_commits_without_a_rerun() {
    let mut c = Cluster::start(3).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
    let _r = alice
        .call(&app, "set", json!({ "key": "a", "value": "1" }), None)
        .await
        .unwrap();
    let after_init = conflicts(&c);

    let leader = c.app_leader(&app).unwrap();
    c.stop(leader).await;
    let before = conflicts(&c);
    let other = (0..3).find(|i| *i != leader).unwrap();
    let alice_other = c.client(other);
    let _s = alice_other.sign_in(&mut alice_pk).await.unwrap();
    let mut done = false;
    for _ in 0..40 {
        if alice_other
            .call(&app, "set", json!({ "key": "a", "value": "2" }), None)
            .await
            .is_ok()
        {
            done = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(done, "no write after the leader change");
    assert_eq!(
        conflicts(&c),
        before,
        "the first call after the leader change applied as a conflict"
    );
    assert_eq!(
        after_init, 0,
        "the init or the first call applied as a conflict"
    );
}

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

/// A blob reaches the other nodes base64-encoded inside a sealed request
/// capped at 64 MiB, so the node refuses one too large to spread (413) before
/// keeping any of it, instead of storing it and failing to reach a majority.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_blob_too_large_to_reach_the_other_nodes_is_refused_up_front() {
    let c = Cluster::start(3).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let (files, _) = deploy(&alice, "files", mero_wasm("blobs", "blobs"), json!({})).await;
    let e = alice
        .upload_blob(&files, vec![7u8; 48 * 1024 * 1024])
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("TOO_LARGE"), "{e}");
    let listed = alice.get(&format!("apps/{files}/blobs")).await.unwrap();
    assert_eq!(listed["blobs"], json!([]), "{listed}");
}

/// An upload past the sealed channel's own cap is answered `413 TOO_LARGE`
/// naming the cap, not dropped mid-upload: merobox's 100 and 250 MiB blob
/// sweep saw the connection closed with no answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_upload_past_the_sealed_cap_is_answered_413() {
    let c = Cluster::start(3).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let (files, _) = deploy(&alice, "files", mero_wasm("blobs", "blobs"), json!({})).await;
    let cap = calimero_sealed::server::MAX_SEALED_BYTES;
    for size in [cap + 1, 100 << 20] {
        let e = alice
            .upload_blob(&files, vec![7u8; size])
            .await
            .unwrap_err();
        let Error::Api {
            status, message, ..
        } = &e
        else {
            panic!("{size} bytes: not an answer from the node: {e:#}");
        };
        assert_eq!((*status, e.code()), (413, Some("TOO_LARGE")), "{e}");
        assert!(message.contains("64 MiB"), "{e}");
    }
    // The session still works after a refusal.
    let listed = alice.get(&format!("apps/{files}/blobs")).await.unwrap();
    assert_eq!(listed["blobs"], json!([]), "{listed}");
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
    // A write may not search (search is for views).
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

async fn set(cl: &Client, app: &str, v: &str) -> Result<(), Box<calimero_client::Error>> {
    cl.call(app, "set", json!({ "key": "k", "value": v }), None)
        .await
        .map(drop)
        .map_err(Box::new)
}

/// Member capabilities: a member who is not an admin creates apps in the
/// organization only with `CREATE_APP`, invites only with `INVITE_MEMBERS`,
/// and upgrades another's org app only with `MANAGE_APPLICATION` (none by
/// default), with their own passkey.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_member_needs_the_capability_to_create_apps_or_invite() {
    const CREATE_APP: u32 = 1 << 0;
    const INVITE_MEMBERS: u32 = 1 << 1;
    const MANAGE_APPLICATION: u32 = 1 << 4;
    let c = Cluster::start(3).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let (bob, mut bob_pk) = signed_up(&c, 1, "bob").await;
    let org = alice.post("orgs", json!({ "name": "Docs" })).await.unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let token = alice
        .post(&format!("orgs/{org}/invites"), json!({ "role": "member" }))
        .await
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = bob
        .post(&format!("invites/{token}/redeem"), json!({}))
        .await
        .unwrap();
    let bob_id = bob.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let refused = |e: calimero_client::Error, why: &str| {
        let e = e.to_string();
        assert!(e.contains(why), "{e}");
    };
    let bundle = bob.upload_bundle(kv_wasm()).await.unwrap();

    let in_org = json!({ "org": org });
    refused(
        bob.create_app("b", &bundle, in_org.clone())
            .await
            .unwrap_err(),
        "CREATE_APP",
    );
    refused(
        bob.post(&format!("orgs/{org}/invites"), json!({ "role": "member" }))
            .await
            .unwrap_err(),
        "INVITE_MEMBERS",
    );

    let _ok = alice
        .post(
            &format!("orgs/{org}/capabilities"),
            json!({
                "account": bob_id,
                "capabilities": CREATE_APP | INVITE_MEMBERS | MANAGE_APPLICATION,
            }),
        )
        .await
        .unwrap();
    let _bobs = bob.create_app("b", &bundle, in_org.clone()).await.unwrap();
    let _invite = bob
        .post(&format!("orgs/{org}/invites"), json!({ "role": "member" }))
        .await
        .unwrap();
    // Never above the member's own role.
    refused(
        bob.post(&format!("orgs/{org}/invites"), json!({ "role": "admin" }))
            .await
            .unwrap_err(),
        "above your own role",
    );
    // Another's org app: only with MANAGE_APPLICATION.
    let alices = alice.create_app("a", &bundle, in_org).await.unwrap();
    let next = bob.upload_bundle(variant(&kv_wasm(), "v2")).await.unwrap();
    let _ok = alice
        .post(
            &format!("orgs/{org}/capabilities"),
            json!({ "account": bob_id, "capabilities": CREATE_APP | INVITE_MEMBERS }),
        )
        .await
        .unwrap();
    refused(
        bob.post_confirmed(
            &format!("apps/{alices}/upgrade"),
            json!({ "bundle": next }),
            &mut bob_pk,
        )
        .await
        .unwrap_err(),
        "MANAGE_APPLICATION",
    );
    let _ok = alice
        .post(
            &format!("orgs/{org}/capabilities"),
            json!({ "account": bob_id, "capabilities": MANAGE_APPLICATION }),
        )
        .await
        .unwrap();
    let _upgraded = bob
        .post_confirmed(
            &format!("apps/{alices}/upgrade"),
            json!({ "bundle": next }),
            &mut bob_pk,
        )
        .await
        .unwrap();
    // Never a personal app.
    let personal = alice.create_app("p", &bundle, json!({})).await.unwrap();
    refused(
        bob.post_confirmed(
            &format!("apps/{personal}/upgrade"),
            json!({ "bundle": next }),
            &mut bob_pk,
        )
        .await
        .unwrap_err(),
        "403",
    );
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
    mpk_full(package, version, wasm, key, outbound, &[], None)
}

/// [`mpk`] declaring `outbound` and shipping `frontend` files (paths under
/// `frontend/`, the first is the entry).
fn mpk_full(
    package: &str,
    version: &str,
    wasm: &[u8],
    key: &ed25519_dalek::SigningKey,
    outbound: &[&str],
    frontend: &[(&str, &[u8])],
    upgrades: Option<calimero_bundle::UpgradeAuthority>,
) -> Vec<u8> {
    use sha2::Digest;
    let artifact = |path: &str, bytes: &[u8]| calimero_bundle::BundleArtifact {
        path: path.to_owned(),
        hash: hex::encode(sha2::Sha256::digest(bytes)),
        size: bytes.len() as u64,
    };
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
        wasm: Some(artifact("app.wasm", wasm)),
        abi: None,
        links: None,
        frontend: (!frontend.is_empty()).then(|| calimero_bundle::BundleFrontend {
            entry: frontend[0].0.to_owned(),
            files: frontend.iter().map(|(p, b)| artifact(p, b)).collect(),
        }),
        frontend_aliases: Vec::new(),
        upgrades,
        outbound: outbound.iter().map(|u| (*u).to_owned()).collect(),
        services: None,
        signature: None,
    };
    let mut manifest = serde_json::to_value(&manifest).unwrap();
    calimero_bundle::sign_manifest_json(&mut manifest, key).unwrap();
    let mut entries = vec![
        (
            "manifest.json".to_owned(),
            serde_json::to_vec(&manifest).unwrap(),
        ),
        ("app.wasm".to_owned(), wasm.to_vec()),
    ];
    entries.extend(frontend.iter().map(|(p, b)| ((*p).to_owned(), b.to_vec())));
    let refs: Vec<(&str, Vec<u8>)> = entries
        .iter()
        .map(|(p, b)| (p.as_str(), b.clone()))
        .collect();
    pack(&refs)
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
        cfg.allow_package_uploads = false;
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
    // Nor an uploaded package, signed or not, from an account or the
    // operator: every release comes from the registry, so any node that lacks
    // it fetches it there, and none lives only where it was uploaded.
    let signed = mpk("com.test.kv", "1.0.0", &kv, &publisher);
    let e = alice.upload_package(signed.clone()).await.unwrap_err();
    assert_eq!(e.code(), Some("UPLOADS_DISABLED"), "{e}");
    let e = ops_at(&c, 0)
        .await
        .ops_upload_package(signed)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("UPLOADS_DISABLED"), "{e}");

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

/// The names of the packages `cl` lists, with the versions it sees of each.
async fn listed(cl: &Client) -> BTreeMap<String, Vec<String>> {
    cl.packages()
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let versions = p["versions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v["version"].as_str().unwrap().to_owned())
                .collect();
            (p["package"].as_str().unwrap().to_owned(), versions)
        })
        .collect()
}

/// On a cluster many organizations share, a release is listed only to who may
/// use it: the account that installed it, members of an organization it is
/// in, accounts that reach an app running it; the operator lists only its own
/// installs, which every account sees. For anyone else it is as if it were not installed: not listed,
/// `404` by name, and creating an app from it (by release or by its WASM's
/// hash) answers what a release the cluster lacks answers. Before, every
/// signed-in account listed every package the cluster held.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_private_package_is_listed_only_to_accounts_that_may_use_it() {
    let registry = FakeRegistry::default();
    let url = registry.serve().await;
    let c = Cluster::start_with(3, |cfg| {
        cfg.registry = Some(url.clone());
        cfg.allow_unsigned_bundles = false;
    })
    .await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let kv = kv_wasm();
    // Alice's organization (Acme, with Amy) and Bob, outside it, in his own.
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let (amy, _m) = signed_up(&c, 1, "amy").await;
    let (bob, _b) = signed_up(&c, 2, "bob").await;
    let bob_id = bob.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let acme = alice.post("orgs", json!({ "name": "Acme" })).await.unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let invite = alice
        .post(&format!("orgs/{acme}/invites"), json!({ "role": "member" }))
        .await
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = amy
        .post(&format!("invites/{invite}/redeem"), json!({}))
        .await
        .unwrap();
    let _o = bob.post("orgs", json!({ "name": "Bobco" })).await.unwrap();

    // Alice uploads a private package (no registry has it).
    let secret = variant(&kv, "secret");
    let _ = alice
        .upload_package(mpk("com.acme.secret", "1.0.0", &secret, &publisher))
        .await
        .unwrap();
    let only_secret = BTreeMap::from([("com.acme.secret".to_owned(), vec!["1.0.0".to_owned()])]);
    assert_eq!(listed(&alice).await, only_secret);
    assert_eq!(listed(&amy).await, only_secret, "a member of Alice's org");

    // Bob, in another organization, sees nothing of it.
    assert_eq!(listed(&bob).await, BTreeMap::new());
    let e = bob.package("com.acme.secret").await.unwrap_err();
    assert!(e.to_string().starts_with("404"), "{e}");
    // Approving its outbound hosts answers as for a missing package, before
    // any passkey ceremony.
    for package in ["com.acme.secret", "com.never.installed"] {
        let e = bob
            .post("auth/outbound", json!({ "package": package }))
            .await
            .unwrap_err();
        assert!(e.to_string().starts_with("404"), "{package}: {e}");
    }
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    let ops_listed = ops.get("ops/packages").await.unwrap()["packages"].clone();
    assert_eq!(ops_listed, json!([]), "the operator installed none");

    // Creating from it answers as for a release the cluster lacks.
    let named = |package: &str, version: Option<&str>| json!({ "name": "x", "package": package, "version": version });
    let hidden = bob
        .post("apps", named("com.acme.secret", Some("1.0.0")))
        .await
        .unwrap_err();
    let missing = bob
        .post("apps", named("com.never.installed", Some("1.0.0")))
        .await
        .unwrap_err();
    assert!(hidden.to_string().starts_with("404"), "{hidden}");
    assert_eq!(hidden.code(), missing.code(), "{hidden} / {missing}");
    let hidden = bob
        .post("apps", named("com.acme.secret", None))
        .await
        .unwrap_err();
    let missing = bob
        .post("apps", named("com.never.installed", None))
        .await
        .unwrap_err();
    assert_eq!(hidden.code(), Some("VERSION_REQUIRED"), "{hidden}");
    assert_eq!(hidden.code(), missing.code(), "{hidden} / {missing}");
    // Its WASM's hash is no way in either.
    let hash = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&secret));
    let by_hash = bob
        .post("apps", json!({ "name": "x", "bundle": hash }))
        .await
        .unwrap_err();
    let unknown = bob
        .post("apps", json!({ "name": "x", "bundle": "00".repeat(32) }))
        .await
        .unwrap_err();
    assert_eq!(by_hash.to_string(), unknown.to_string());
    // A failed attempt installed nothing that Bob now lists.
    assert_eq!(listed(&bob).await, BTreeMap::new());

    // Alice and Amy create apps from it.
    let _ = alice
        .post("apps", named("com.acme.secret", Some("1.0.0")))
        .await
        .unwrap();
    let shared = amy
        .post(
            "apps",
            json!({
                "name": "shared", "package": "com.acme.secret",
                "access": { "kind": "accounts", "accounts": [bob_id] },
            }),
        )
        .await
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    // Once Bob reaches an app running it, he sees that release.
    assert_eq!(listed(&bob).await, only_secret, "Bob reaches {shared}");
    let _ = amy.delete_app(&shared).await.unwrap();
    assert_eq!(listed(&bob).await, BTreeMap::new());

    // A registry release is private to its installers too; Bob installing the
    // same release himself makes it his as well.
    registry.publish(
        "com.test.kv",
        "1.0.0",
        mpk("com.test.kv", "1.0.0", &kv, &publisher),
    );
    let _ = alice.install_package("com.test.kv", "1.0.0").await.unwrap();
    assert!(!listed(&bob).await.contains_key("com.test.kv"));
    let _ = bob.install_package("com.test.kv", "1.0.0").await.unwrap();
    assert_eq!(
        listed(&bob).await,
        BTreeMap::from([("com.test.kv".to_owned(), vec!["1.0.0".to_owned()])])
    );
    let _ = bob
        .post("apps", named("com.test.kv", Some("1.0.0")))
        .await
        .unwrap();

    // The operator lists only the releases it installed; those are the
    // cluster's own catalog, which every account sees and may run.
    let ops_kv = variant(&kv, "ops");
    registry.publish(
        "com.test.ops",
        "1.0.0",
        mpk("com.test.ops", "1.0.0", &ops_kv, &publisher),
    );
    let _ = ops
        .post(
            "ops/packages/install",
            json!({ "package": "com.test.ops", "version": "1.0.0" }),
        )
        .await
        .unwrap();
    let ops_listed = ops.get("ops/packages").await.unwrap()["packages"].clone();
    let names: Vec<&str> = ops_listed
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["package"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["com.test.ops"], "{ops_listed}");
    assert!(listed(&alice).await.contains_key("com.test.ops"));
    assert!(listed(&bob).await.contains_key("com.test.ops"));
}

/// A refresh token presented again is a stolen copy, and the session ends,
/// whichever node it is presented to: a replica that has not applied the
/// exchange yet must not answer "unknown" and let the thief keep trying.
/// `read_wait: 0` stands for a replica further behind than a lookup waits.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_reused_refresh_token_revokes_the_session_on_a_lagging_node() {
    let c = Cluster::start_with(3, |cfg| cfg.read_wait = Duration::ZERO).await;
    for round in 0..10 {
        let (owner, _pk) = signed_up(&c, 0, &format!("owner{round}")).await;
        let stolen = owner.session().await.unwrap();
        let _rotated = owner.refresh().await.unwrap();
        let thief = c.client(1 + round % 2);
        thief.use_session(stolen).await;
        let e = thief.refresh().await.unwrap_err().to_string();
        assert!(e.contains("already used"), "round {round}: {e}");
        let e = owner.me().await.unwrap_err().to_string();
        assert!(e.starts_with("401"), "round {round}: {e}");
    }
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

// ---------------------------------------------------------------- placement

/// Registers each of `c`'s nodes again with the zone `zones` names for it
/// (`POST /ops/nodes`), and waits until node 0, where the tests create apps,
/// has applied every one.
async fn put_in_zones(c: &Cluster, zones: &[&str]) {
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    for (i, zone) in zones.iter().enumerate() {
        let _ = ops
            .post(
                "ops/nodes",
                json!({ "id": i as u64 + 1, "addr": c.urls[i], "zone": zone }),
            )
            .await
            .unwrap();
    }
    let node = &c.nodes[0].as_ref().unwrap().node;
    for _ in 0..100 {
        let applied = node.sys(|s| {
            zones
                .iter()
                .enumerate()
                .all(|(i, z)| s.nodes.get(&(i as u64 + 1)).is_some_and(|n| n.zone == *z))
        });
        if applied {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("node 1 never applied the zones");
}

/// The replicas `POST /apps` placed a new kv app on, sorted.
async fn placed(cl: &Client, bundle: &str, name: &str) -> Vec<u64> {
    let created = cl
        .post("apps", json!({ "name": name, "bundle": bundle }))
        .await
        .unwrap();
    let mut replicas: Vec<u64> = serde_json::from_value(created["replicas"].clone()).unwrap();
    replicas.sort_unstable();
    replicas
}

/// A new app's replicas span as many zones as the cluster has, the least
/// loaded node first within a zone: with nodes in zones a, a, b and c, an app
/// lands on one node of a, the node of b and the node of c, never on both of
/// a; the next app takes the a node the first one left empty. Before, placement
/// took the three least-loaded nodes whatever their zone ([1, 2, 3]: both of a).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_new_app_is_placed_across_zones() {
    let c = Cluster::start(4).await;
    put_in_zones(&c, &["a", "a", "b", "c"]).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();

    // Nothing is placed yet: node 1 (lower id) for zone a, then b and c.
    assert_eq!(placed(&alice, &bundle, "first").await, vec![1, 3, 4]);
    // Node 2 holds nothing, node 1 one app: zone a's replica goes to node 2.
    assert_eq!(placed(&alice, &bundle, "second").await, vec![2, 3, 4]);
    // Zones before load: b and c hold two apps each, a's nodes one each, and
    // the third app still takes b and c rather than both of a.
    assert_eq!(placed(&alice, &bundle, "third").await, vec![1, 3, 4]);
}

/// Spreading across zones never places a replica on a node whose release the
/// owner has not approved: with zone c's only node on an unapproved release,
/// the app spans a and b and fills its third replica from a again.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn placement_across_zones_keeps_to_approved_releases() {
    let c = Cluster::start_with(4, |cfg| {
        cfg.tee_release = Some(if cfg.node_id == 4 { "2.0.0" } else { "1.0.0" }.into());
    })
    .await;
    put_in_zones(&c, &["a", "a", "b", "c"]).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    assert_eq!(placed(&alice, &bundle, "kv").await, vec![1, 2, 3]);
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

/// The outbound gate, with the owner's approval in place of v1's namespace
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

    // Bob sees no release of Alice's private upload: approving it answers as
    // for a missing package. Once he installs the same release himself, his
    // approval for his own apps reaches nothing of Alice's.
    let e = bob
        .approve_outbound(None, PACKAGE, None, false, &mut bob_pk)
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("404"), "{e}");
    let _ = bob
        .upload_package(mpk_declaring(
            PACKAGE,
            "0.1.0",
            &wasm("outbound_probe"),
            &publisher,
            &[PREFIX],
        ))
        .await
        .unwrap();
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

    // Usage: of the four probes only the approved one left the node, counted
    // on the node that ran it (`calimero_app_outbound_requests_total`).
    let alice_id = alice.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut sent = 0.0;
    for i in 0..3 {
        let ops = c.client(i);
        ops.use_api_key("ops-secret").await;
        let text = ops.get_text("ops/metrics").await.unwrap();
        sent += sample(
            &text,
            "calimero_app_outbound_requests_total",
            &format!("app=\"{app}\",org=\"account:{alice_id}\""),
        )
        .unwrap_or(0.0);
    }
    assert_eq!(sent, 1.0);
}

// ---------------------------------------------------------------- governance

/// Who decides releases, app upgrades and outbound hosts is chosen per
/// deployment, each one either the operator's or the owner's, and a user
/// proves which: every replica binds the governance into its receipt-key
/// quote, and a replica claiming other rules is refused.
/// The node's own commands (`__upgrade`, `__blob`, `__blob_forget`) are not
/// methods anyone can call: through `/intents` they would switch an app's
/// bundle with no owner approval, or mark another app's blob as one's own.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_nodes_own_commands_cannot_be_called_as_methods() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let (bob, _bpk) = signed_up(&c, 1, "bob").await;
    let kv = kv_wasm();
    let (public, _) = deploy(
        &alice,
        "public",
        kv.clone(),
        json!({ "access": { "kind": "public" } }),
    )
    .await;
    let other = alice.upload_bundle(variant(&kv, "other")).await.unwrap();
    let before = alice.get(&format!("apps/{public}")).await.unwrap()["bundle"].clone();

    for (who, caller) in [("bob", &bob), ("the owner", &alice)] {
        let r = caller
            .call(&public, "__upgrade", json!({ "bundle": other }), None)
            .await;
        assert!(r.is_err(), "{who} upgraded through /intents: {r:?}");
    }
    assert_eq!(
        alice.get(&format!("apps/{public}")).await.unwrap()["bundle"],
        before,
        "the bundle is unchanged"
    );

    // Alice's private blob, and bob's own app.
    let (private, _) = deploy(&alice, "private", kv.clone(), json!({})).await;
    let up = alice
        .upload_blob(&private, b"alice's".to_vec())
        .await
        .unwrap();
    let id = up["id"].as_str().unwrap().to_owned();
    let (mine, _) = deploy(&bob, "mine", kv.clone(), json!({})).await;
    for method in ["__blob", "__blob_forget"] {
        let r = bob
            .call(&mine, method, json!({ "id": id, "size": 7 }), None)
            .await;
        assert!(r.is_err(), "{method} through /intents: {r:?}");
    }
    assert!(
        bob.download_blob(&mine, &id).await.is_err(),
        "bob cannot read alice's blob through his app"
    );
    assert!(
        bob.view(
            &public,
            "__upgrade",
            json!({}),
            calimero_client::Consistency::Strong
        )
        .await
        .is_err(),
        "nor as a view"
    );
}

/// An upgrade that turns an identity-gated field (an `AuthoredMap`) into a
/// plain one (an `UnorderedMap`) would strip every entry's authorship, so it
/// is refused and the app stays where it was, by the identity-downgrade gate
/// (#2587).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_upgrade_that_strips_authorship_is_refused() {
    let c = Cluster::start(3).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let v1 = alice
        .upload_bundle(mero_wasm(
            "migrations/scenario-identity-downgrade-v1",
            "scenario_identity_downgrade_v1",
        ))
        .await
        .unwrap();
    let app = alice.create_app("wiki", &v1, json!({})).await.unwrap();
    let v2 = alice
        .upload_bundle(mero_wasm(
            "migrations/scenario-identity-downgrade-v2",
            "scenario_identity_downgrade_v2",
        ))
        .await
        .unwrap();
    let out = alice
        .upgrade_app(&app, json!({ "bundle": v2 }), &mut pk)
        .await;
    match &out {
        Ok(v) => assert_ne!(v["status"], "ok", "the downgrade was applied: {v}"),
        Err(e) => assert!(e.to_string().contains("authorship"), "{e}"),
    }
    let a = alice.get(&format!("apps/{app}")).await.unwrap();
    assert_eq!(
        a["bundle"],
        json!(v1),
        "the app stays on the release it ran"
    );
}

/// A write too large for one log entry is refused at once as `413
/// TOO_LARGE`, whichever node takes it: the leader's own refusal is not a
/// "no leader yet" to retry.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_write_too_large_for_the_log_is_refused_at_once_on_every_node() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let (app, _) = deploy(
        &alice,
        "kv",
        kv_wasm(),
        json!({ "access": { "kind": "public" } }),
    )
    .await;
    let big = "x".repeat(calimero_log::MAX_ENTRY_BYTES + 1024);
    for i in 0..3 {
        let (cl, _) = signed_up(&c, i, &format!("user{i}")).await;
        let started = std::time::Instant::now();
        let r = cl
            .call(&app, "set", json!({ "key": "big", "value": big }), None)
            .await;
        let e = r.expect_err("an entry over the cap is refused");
        assert_eq!(e.code(), Some("TOO_LARGE"), "node {i}: {e}");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "node {i} answered after {:?}",
            started.elapsed()
        );
    }
}

/// Where the operator upgrades apps, the operator installs the releases it
/// upgrades to: from an uploaded `.mpk`, with its ops token, no account
/// needed. It uninstalls only what it installed.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_operator_installs_the_releases_it_upgrades_to() {
    use calimero_node::system::{Authority, Governance};
    let c = Cluster::start_with(3, |cfg| {
        cfg.governance = Governance {
            app_upgrades: Authority::Operator,
            ..Governance::default()
        };
    })
    .await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let kv = kv_wasm();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[4; 32]);
    let ops = c.client(1);
    ops.use_api_key("ops-secret").await;

    let first = ops
        .ops_upload_package(mpk("com.test.ops", "1.0.0", &kv, &publisher))
        .await
        .unwrap_or_else(|e| panic!("the operator installs a release: {e}"));
    assert_eq!(first["version"], "1.0.0");
    let created = alice
        .post("apps", json!({ "name": "kv", "package": "com.test.ops" }))
        .await
        .unwrap();
    let app = created["appId"].as_str().unwrap().to_owned();

    let _ = ops
        .ops_upload_package(mpk(
            "com.test.ops",
            "1.1.0",
            &variant(&kv, "next"),
            &publisher,
        ))
        .await
        .unwrap();
    let listed = ops.get("ops/packages").await.unwrap();
    assert!(listed.to_string().contains("1.1.0"), "{listed}");
    let _ = harness::ops_signed(
        &ops,
        &format!("ops/apps/{app}/upgrade"),
        json!({ "version": "1.1.0" }),
    )
    .await
    .unwrap_or_else(|e| panic!("the operator upgrades to its release: {e}"));
    let a = alice.get("apps?package=com.test.ops").await.unwrap();
    assert_eq!(a["apps"][0]["version"], "1.1.0", "{a}");

    // A release an account installed stays that account's.
    let _ = alice
        .upload_package(mpk("com.test.mine", "1.0.0", &kv, &publisher))
        .await
        .unwrap();
    assert!(ops
        .ops_uninstall_package("com.test.mine", None)
        .await
        .is_err());
    // Its own, unused one goes.
    let _ = ops
        .ops_uninstall_package("com.test.ops", Some("1.0.0"))
        .await
        .unwrap();
    // Without the token, nothing.
    assert!(c
        .client(2)
        .ops_upload_package(mpk("com.test.ops", "1.2.0", &kv, &publisher))
        .await
        .is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_deployment_decides_who_upgrades_and_users_can_prove_it() {
    use calimero_node::system::{Authority, Governance};
    let (owner, operator) = (Authority::Owner, Authority::Operator);
    let deployments = [
        Governance {
            releases: owner,
            app_upgrades: owner,
            outbound: owner,
        },
        Governance {
            releases: operator,
            app_upgrades: operator,
            outbound: operator,
        },
        // A managed cluster whose operator rolls out node releases, while
        // each app's owner keeps its code and its outbound hosts.
        Governance {
            releases: operator,
            app_upgrades: owner,
            outbound: owner,
        },
    ];
    for g in deployments {
        let op = |a: Authority| a == Authority::Operator;
        let configured = g.clone();
        let c = Cluster::start_with(3, move |cfg| cfg.governance = configured.clone()).await;
        let (alice, mut pk) = signed_up(&c, 0, "alice").await;
        let kv = kv_wasm();
        let (app, _ctx) = deploy(&alice, "kv", kv.clone(), json!({})).await;
        let next = alice.upload_bundle(variant(&kv, "next")).await.unwrap();
        let publisher = ed25519_dalek::SigningKey::from_bytes(&[3; 32]);
        let _ = alice
            .upload_package(mpk_declaring(
                "com.test.gov",
                "0.1.0",
                &variant(&kv, "pkg"),
                &publisher,
                &["https://api.example.com/v1/"],
            ))
            .await
            .unwrap();

        // What the user proves.
        let proven = alice.verify_app_cluster(&app).await.unwrap();
        assert_eq!(proven.keys.len(), 3, "{g:?}");
        assert_eq!(proven.governance, g, "{g:?}");
        let nonce = calimero_identity::random::<32>();
        let docs = alice
            .get(&format!(
                "apps/{app}/attestation?nonce={}",
                hex::encode(nonce)
            ))
            .await
            .unwrap();
        let mut doc: calimero_client::attest::AttestationDoc =
            serde_json::from_value(docs["replicas"][0]["attestation"].clone()).unwrap();
        let mock = Policy {
            allow_mock: true,
            ..Policy::default()
        };
        assert!(
            calimero_client::attest::verify_receipt_key(&doc, &nonce, &mock)
                .await
                .is_ok()
        );
        let mut forged = g.clone();
        forged.app_upgrades = if op(g.app_upgrades) { owner } else { operator };
        doc.governance = Some(forged);
        assert!(
            calimero_client::attest::verify_receipt_key(&doc, &nonce, &mock)
                .await
                .is_err(),
            "{g:?}: a forged governance verified"
        );

        let ops = c.client(1);
        ops.use_api_key("ops-secret").await;
        let status = ops.get("ops/status").await.unwrap();
        assert_eq!(status["governance"], json!(g), "{g:?}");

        // App upgrades: exactly one of the two may.
        let by_operator = harness::ops_signed(
            &ops,
            &format!("ops/apps/{app}/upgrade"),
            json!({ "bundle": next }),
        )
        .await;
        assert_eq!(
            by_operator.is_ok(),
            op(g.app_upgrades),
            "{g:?}: {by_operator:?}"
        );
        let by_owner = alice
            .upgrade_app(&app, json!({ "bundle": next }), &mut pk)
            .await;
        assert_eq!(by_owner.is_ok(), !op(g.app_upgrades), "{g:?}: {by_owner:?}");
        let a = alice.get(&format!("apps/{app}")).await.unwrap();
        assert_eq!(
            a["bundle"],
            json!(next),
            "{g:?}: upgraded by whoever holds it"
        );
        let _ = alice
            .call(&app, "set", json!({ "key": "k", "value": "v" }), None)
            .await
            .unwrap();

        // Release approvals.
        let ops_release =
            harness::ops_signed(&ops, "ops/releases", json!({ "version": "9.0.0" })).await;
        assert_eq!(
            ops_release.is_ok(),
            op(g.releases),
            "{g:?}: {ops_release:?}"
        );
        let owner_release = alice.approve_release(None, "9.0.0", &mut pk).await;
        assert_eq!(
            owner_release.is_ok(),
            !op(g.releases),
            "{g:?}: {owner_release:?}"
        );

        // Outbound approvals.
        let ops_outbound =
            harness::ops_signed(&ops, "ops/outbound", json!({ "package": "com.test.gov" })).await;
        assert_eq!(
            ops_outbound.is_ok(),
            op(g.outbound),
            "{g:?}: {ops_outbound:?}"
        );
        let owner_outbound = alice
            .approve_outbound(None, "com.test.gov", None, false, &mut pk)
            .await;
        assert_eq!(
            owner_outbound.is_ok(),
            !op(g.outbound),
            "{g:?}: {owner_outbound:?}"
        );
    }
}

// ---------------------------------------------------------------- orgs, metadata

async fn account_id(cl: &Client) -> String {
    cl.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Org metadata, member metadata, app metadata, device labels and
/// owner-delete: who may set each, what reads them back, and that deleting an
/// organization takes its apps and invites, with the owner's passkey.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn org_metadata_labels_and_deletion_work_like_v1() {
    let c = Cluster::start(3).await;
    let (alice, mut apk) = signed_up(&c, 0, "alice").await;
    let (bob, _bpk) = signed_up(&c, 1, "bob").await;
    let bob_id = account_id(&bob).await;
    let alice_id = account_id(&alice).await;
    let org = alice.post("orgs", json!({ "name": "Acme" })).await.unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ = alice
        .post(
            &format!("orgs/{org}/members"),
            json!({ "account": bob_id, "role": "member" }),
        )
        .await
        .unwrap();

    // Organization metadata: an admin's; a member reads it.
    let _ = alice
        .set_org_metadata(&org, json!({ "logo": "acme.svg" }))
        .await
        .unwrap();
    assert!(bob
        .set_org_metadata(&org, json!({ "logo": "x" }))
        .await
        .is_err());
    let o = bob.get(&format!("orgs/{org}")).await.unwrap();
    assert_eq!(o["metadata"]["logo"], json!("acme.svg"));

    // Member metadata: the member's own, or an admin's; not another member's.
    let _ = bob
        .set_member_metadata(&org, &bob_id, json!({ "title": "Engineer" }))
        .await
        .unwrap();
    assert!(bob
        .set_member_metadata(&org, &alice_id, json!({ "title": "x" }))
        .await
        .is_err());
    let _ = alice
        .set_member_metadata(&org, &bob_id, json!({ "title": "Lead" }))
        .await
        .unwrap();
    let o = alice.get(&format!("orgs/{org}")).await.unwrap();
    assert_eq!(o["memberMetadata"][&bob_id]["title"], json!("Lead"));
    // Over 16 KiB is refused.
    let big = "x".repeat(17 * 1024);
    assert!(alice
        .set_org_metadata(&org, json!({ "big": big }))
        .await
        .is_err());

    // App metadata: its owner's or an org admin's; it reads back with the app.
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice
        .post(
            "apps",
            json!({ "name": "kv", "bundle": bundle, "org": org, "metadata": { "icon": "a" } }),
        )
        .await
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        alice.get(&format!("apps/{app}")).await.unwrap()["metadata"]["icon"],
        json!("a")
    );
    assert!(bob
        .set_app_metadata(&app, json!({ "icon": "b" }))
        .await
        .is_err());
    let _ = alice
        .set_app_metadata(&app, json!({ "icon": "c" }))
        .await
        .unwrap();
    assert_eq!(
        bob.get(&format!("apps/{app}")).await.unwrap()["metadata"]["icon"],
        json!("c")
    );

    // Passkey labels.
    let cred = alice.me().await.unwrap()["passkeys"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ = alice.label_passkey(&cred, "laptop").await.unwrap();
    assert_eq!(
        alice.me().await.unwrap()["passkeys"][0]["label"],
        json!("laptop")
    );
    assert!(bob.label_passkey(&cred, "mine").await.is_err());

    // Deleting the organization: only the owner, with a passkey; its apps go.
    let invite = alice
        .post(&format!("orgs/{org}/invites"), json!({ "role": "member" }))
        .await
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(bob.delete(&format!("orgs/{org}")).await.is_err());
    let e = alice.delete(&format!("orgs/{org}")).await.unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    let gone = alice.delete_org(&org, &mut apk).await.unwrap();
    assert_eq!(gone["apps"], json!([app]));
    assert!(alice.get(&format!("orgs/{org}")).await.is_err());
    assert!(alice.get(&format!("apps/{app}")).await.is_err());
    let (carol, _cpk) = signed_up(&c, 2, "carol").await;
    assert!(carol
        .post(&format!("invites/{invite}/redeem"), json!({}))
        .await
        .is_err());
}

// ---------------------------------------------------------------- blobs, storage

/// Blob list, info and delete, app storage size, the app's ABI and
/// usage. A deleted blob is the app's no longer; its bytes leave every node
/// once no other app holds the same content, and never while one does.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn blobs_list_and_delete_and_storage_is_inspectable() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let (bob, _bpk) = signed_up(&c, 1, "bob").await;
    let (a1, _) = deploy(
        &alice,
        "one",
        kv_wasm(),
        json!({ "access": { "kind": "public" } }),
    )
    .await;
    let (a2, _) = deploy(&alice, "two", kv_wasm(), json!({})).await;
    let bytes = b"the same picture".to_vec();
    let up = alice.upload_blob(&a1, bytes.clone()).await.unwrap();
    let id = up["id"].as_str().unwrap().to_owned();
    let _ = alice.upload_blob(&a2, bytes.clone()).await.unwrap();
    let other = alice.upload_blob(&a1, b"another".to_vec()).await.unwrap();

    let listed = alice.list_blobs(&a1).await.unwrap();
    assert_eq!(listed["blobs"].as_array().unwrap().len(), 2, "{listed}");
    let info = alice.blob_info(&a1, &id).await.unwrap();
    assert_eq!(info["size"], json!(bytes.len()));
    // The base58 id apps print works too.
    let b58 = up["blobId"].as_str().unwrap();
    assert_eq!(alice.blob_info(&a1, b58).await.unwrap()["id"], json!(id));

    let _ = alice
        .call(&a1, "set", json!({ "key": "k", "value": "v" }), None)
        .await
        .unwrap();
    let storage = alice.app_storage(&a1).await.unwrap();
    assert_eq!(storage["blobs"], json!(2), "{storage}");
    assert_eq!(storage["blobBytes"], json!(bytes.len() + b"another".len()));
    assert!(storage["keys"].as_u64().unwrap() >= 1, "{storage}");
    let abi = alice.app_abi(&a1).await.unwrap();
    assert!(abi.get("bundle").is_some(), "{abi}");
    let usage = alice.usage().await.unwrap();
    assert_eq!(usage["apps"], json!(2), "{usage}");
    assert!(usage["quotas"]["maxAppsPerAccount"].as_u64().is_some());

    // Only the owner (or an org admin) deletes; a caller of a public app cannot.
    assert!(bob.delete_blob(&a1, &id).await.is_err());
    let _ = alice.delete_blob(&a1, &id).await.unwrap();
    assert!(alice.blob_info(&a1, &id).await.is_err());
    assert!(alice.download_blob(&a1, &id).await.is_err());
    // The other app holds the same bytes: they stay everywhere they were.
    assert_eq!(alice.download_blob(&a2, &id).await.unwrap(), bytes);
    let held_somewhere = |c: &Cluster| {
        c.nodes
            .iter()
            .flatten()
            .filter(|r| r.node.engine.get_blob(&id).unwrap().is_some())
            .count()
    };
    assert!(held_somewhere(&c) >= 2, "a majority kept the shared bytes");

    // Once the last app lets it go, no node keeps the bytes.
    let _ = alice.delete_blob(&a2, &id).await.unwrap();
    assert_eq!(held_somewhere(&c), 0, "the bytes outlived every app");
    // The app's other blob is untouched.
    let other_id = other["id"].as_str().unwrap();
    assert_eq!(
        alice.download_blob(&a1, other_id).await.unwrap(),
        b"another".to_vec()
    );
}

// ---------------------------------------------------------------- upgrade runs, aliases

async fn finished_run(cl: &Client, org: &str, run: &str) -> Value {
    for _ in 0..200 {
        let r = cl.upgrade_run(org, run).await.unwrap();
        if r["finished"] == json!(true) {
            return r;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("upgrade run {run} never finished");
}

/// Org upgrade runs (with cascade), their status and retry, and each app's
/// last upgrade; and aliases.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn org_upgrades_run_report_and_retry_and_aliases_name_apps() {
    let c = Cluster::start_with(3, |cfg| cfg.allow_unsigned_bundles = true).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[4; 32]);
    let kv = kv_wasm();
    let _ = alice
        .upload_package(mpk("com.test.kv", "0.1.0", &kv, &publisher))
        .await
        .unwrap();
    let org = alice.post("orgs", json!({ "name": "Acme" })).await.unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let team = |name: &str, parent: Option<&str>| {
        let alice = &alice;
        let org = org.clone();
        let body = json!({ "name": name, "join": "invite", "parent": parent });
        async move {
            alice
                .post(&format!("orgs/{org}/teams"), body)
                .await
                .unwrap()["teamId"]
                .as_str()
                .unwrap()
                .to_owned()
        }
    };
    let eng = team("eng", None).await;
    let web = team("web", Some(&eng)).await;
    let ops = team("ops", None).await;
    let mut apps = BTreeMap::new();
    for (name, t) in [("top", &eng), ("below", &web), ("elsewhere", &ops)] {
        let app = alice
            .post(
                "apps",
                json!({ "name": name, "package": "com.test.kv", "org": org,
                        "access": { "kind": "team", "team": t } }),
            )
            .await
            .unwrap()["appId"]
            .as_str()
            .unwrap()
            .to_owned();
        let _prev = apps.insert(name, app);
    }
    let _ = alice
        .upload_package(mpk("com.test.kv", "0.2.0", &variant(&kv, "v2"), &publisher))
        .await
        .unwrap();

    // Eng and the teams below it, not ops.
    let run = alice
        .upgrade_org(
            &org,
            json!({ "package": "com.test.kv", "version": "0.2.0", "team": eng, "cascade": true }),
            &mut pk,
        )
        .await
        .unwrap();
    let run_id = run["runId"].as_str().unwrap().to_owned();
    assert_eq!(run["apps"].as_array().unwrap().len(), 2, "{run}");
    let done = finished_run(&alice, &org, &run_id).await;
    assert_eq!(done["done"].as_array().unwrap().len(), 2, "{done}");
    assert_eq!(done["failed"], json!({}));
    for name in ["top", "below"] {
        let a = alice.get(&format!("apps/{}", apps[name])).await.unwrap();
        assert_eq!(a["lastUpgrade"]["ok"], json!(true), "{name}: {a}");
        assert_eq!(a["lastUpgrade"]["run"], json!(run_id));
    }
    let untouched = alice
        .get(&format!("apps/{}", apps["elsewhere"]))
        .await
        .unwrap();
    assert!(untouched.get("lastUpgrade").is_none(), "{untouched}");
    // Still serving after the run.
    let _ = alice
        .call(
            &apps["below"],
            "set",
            json!({ "key": "k", "value": "v" }),
            None,
        )
        .await
        .unwrap();

    // A failed upgrade is recorded with its reason, and a retry runs it again.
    let f1 = mero_wasm(
        "migrations/scenario-migration-check-fail-v1",
        "scenario_migration_check_fail_v1",
    );
    let f2 = mero_wasm(
        "migrations/scenario-migration-check-fail-v2",
        "scenario_migration_check_fail_v2",
    );
    let _ = alice
        .upload_package(mpk("com.test.checked", "1.0.0", &f1, &publisher))
        .await
        .unwrap();
    let checked = alice
        .post(
            "apps",
            json!({ "name": "checked", "package": "com.test.checked", "org": org }),
        )
        .await
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    for k in ["x", "y"] {
        let _ = alice
            .call(&checked, "set_item", json!({ "key": k, "value": k }), None)
            .await
            .unwrap();
    }
    let _ = alice
        .upload_package(mpk("com.test.checked", "2.0.0", &f2, &publisher))
        .await
        .unwrap();
    let run = alice
        .upgrade_org(
            &org,
            json!({ "package": "com.test.checked", "version": "2.0.0" }),
            &mut pk,
        )
        .await
        .unwrap();
    let failed = finished_run(&alice, &org, run["runId"].as_str().unwrap()).await;
    assert!(failed["failed"].get(&checked).is_some(), "{failed}");
    let a = alice.get(&format!("apps/{checked}")).await.unwrap();
    assert_eq!(a["lastUpgrade"]["ok"], json!(false), "{a}");
    assert_ne!(a["lastUpgrade"]["error"], Value::Null, "{a}");
    let retry = alice
        .retry_upgrade_run(&org, run["runId"].as_str().unwrap(), &mut pk)
        .await
        .unwrap();
    assert_eq!(retry["apps"], json!([checked]), "{retry}");
    let again = finished_run(&alice, &org, retry["runId"].as_str().unwrap()).await;
    assert!(again["failed"].get(&checked).is_some(), "{again}");

    // Aliases: this account's names for apps.
    let _ = alice.set_alias("main", &apps["top"]).await.unwrap();
    assert_eq!(
        alice.alias("main").await.unwrap()["appId"],
        json!(apps["top"])
    );
    assert_eq!(
        alice.aliases().await.unwrap()["aliases"]["main"],
        json!(apps["top"])
    );
    let (bob, _bpk) = signed_up(&c, 1, "bob").await;
    assert!(bob.alias("main").await.is_err(), "aliases are per account");
    assert!(
        bob.set_alias("x", &apps["top"]).await.is_err(),
        "an app bob cannot reach"
    );
    let _ = alice.delete_alias("main").await.unwrap();
    assert!(alice.alias("main").await.is_err());
}

// ---------------------------------------------------------------- presence

/// Ephemeral presence: members on different nodes see each other, live;
/// the newest seq wins and a retract shows them leaving; a heartbeat keeps an
/// entry alive on every node and silence expires it everywhere. Nothing of it
/// is in the app's log.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn presence_reaches_every_node_and_expires() {
    let c = Cluster::start(3).await;
    let (alice, _apk) = signed_up(&c, 0, "alice").await;
    let (bob, _bpk) = signed_up(&c, 1, "bob").await;
    let (carol, _cpk) = signed_up(&c, 2, "carol").await;
    let (app, _) = deploy(
        &alice,
        "doc",
        kv_wasm(),
        json!({ "access": { "kind": "public" } }),
    )
    .await;
    let alice_id = account_id(&alice).await;

    let mut stream = Box::pin(bob.presence_stream(&app).await.unwrap());
    let (_, kind, snap) = stream.next().await.unwrap();
    assert_eq!(kind, "snapshot");
    assert_eq!(snap, json!([]));

    let _ = alice
        .publish_presence(&app, 1, Some(b"cursor:3"))
        .await
        .unwrap();
    let (_, kind, d) = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kind, "presence");
    assert_eq!(d["account"], json!(alice_id));
    assert_eq!(d["state"], json!(hex::encode(b"cursor:3")));
    // A third node has it too.
    let mut seen = false;
    for _ in 0..100 {
        if carol.presence(&app).await.unwrap()["present"]
            .as_array()
            .unwrap()
            .len()
            == 1
        {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(seen, "the third node never saw alice");

    // An older seq does not win; a retract shows alice leaving.
    let _ = alice
        .publish_presence(&app, 0, Some(b"stale"))
        .await
        .unwrap();
    let _ = alice.publish_presence(&app, 2, None).await.unwrap();
    let (_, _, d) = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d["state"], Value::Null, "{d}");
    assert_eq!(bob.presence(&app).await.unwrap()["present"], json!([]));

    // Heartbeats keep an entry alive past the TTL on every node; silence ends it.
    let _ = alice
        .publish_presence(&app, 3, Some(b"here"))
        .await
        .unwrap();
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let _ = alice
            .publish_presence(&app, 3, Some(b"here"))
            .await
            .unwrap();
    }
    for cl in [&bob, &carol] {
        assert_eq!(
            cl.presence(&app).await.unwrap()["present"]
                .as_array()
                .unwrap()
                .len(),
            1,
            "kept alive by heartbeats"
        );
    }
    tokio::time::sleep(Duration::from_millis(
        calimero_node::presence::PRESENCE_TTL_MS + 1_500,
    ))
    .await;
    for cl in [&alice, &bob, &carol] {
        assert_eq!(cl.presence(&app).await.unwrap()["present"], json!([]));
    }
}

// ---------------------------------------------------------------- app frontends

/// App frontends: each package's frontend on its own origin, served from the
/// release's verified `.mpk` under a strict CSP, with client routes and deep
/// links; and a passkey session opened from that origin is bound to the
/// package, reaching its apps and nothing else of the account.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn app_frontends_are_served_and_their_sessions_are_bound() {
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, _apk) = signed_up(&c, 0, "alice").await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[6; 32]);
    let html: &[u8] = b"<!doctype html><html><head><script type=\"module\" src=\"./assets/app.js\"></script></head><body></body></html>";
    let _ = alice
        .upload_package(mpk_full(
            "com.test.site",
            "0.1.0",
            &kv_wasm(),
            &publisher,
            &[],
            &[
                ("frontend/index.html", html),
                ("frontend/assets/app.js", b"console.log(1)"),
            ],
            None,
        ))
        .await
        .unwrap();
    let _ = alice
        .upload_package(mpk(
            "com.test.other",
            "0.1.0",
            &variant(&kv_wasm(), "o"),
            &publisher,
        ))
        .await
        .unwrap();
    let other = alice
        .post(
            "apps",
            json!({ "name": "other", "package": "com.test.other", "access": { "kind": "public" } }),
        )
        .await
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();

    let label = calimero_node::app_host::label("com.test.site");
    let port = url::Url::parse(&c.urls[0]).unwrap().port().unwrap();
    let host = format!("{label}.localhost");
    let origin = format!("http://{host}:{port}");
    let web = reqwest::Client::builder()
        .resolve(&host, ([127, 0, 0, 1], port).into())
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let get = |path: &str, accept: &str| {
        web.get(format!("{origin}{path}"))
            .header("accept", accept)
            .send()
    };

    let root = get("/", "text/html").await.unwrap();
    assert_eq!(root.status(), 302);
    assert_eq!(root.headers()["location"], "/0.1.0/");
    let page = get("/0.1.0/", "text/html").await.unwrap();
    assert_eq!(page.status(), 200);
    let csp = page.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(csp.contains("default-src 'self'"), "{csp}");
    assert!(
        csp.contains(&c.urls[0].trim_end_matches('/').to_owned()),
        "the API is reachable: {csp}"
    );
    assert_eq!(page.headers()["content-type"], "text/html; charset=utf-8");
    assert_eq!(page.bytes().await.unwrap().as_ref(), html);
    let js = get("/0.1.0/assets/app.js", "*/*").await.unwrap();
    assert_eq!(js.status(), 200);
    assert!(js.headers()["cache-control"]
        .to_str()
        .unwrap()
        .contains("immutable"));
    // A client route reloaded, and a deep link's relative asset.
    let route = get("/0.1.0/docs/42", "text/html").await.unwrap();
    assert_eq!(route.status(), 200);
    assert_eq!(route.bytes().await.unwrap().as_ref(), html);
    let deep = get("/0.1.0/docs/42/assets/app.js", "*/*").await.unwrap();
    assert_eq!(deep.status(), 200);
    // Nothing but files: no node route, no other method, no other package.
    assert_eq!(get("/health", "*/*").await.unwrap().status(), 404);
    assert_eq!(
        get("/attestation?nonce=00", "*/*").await.unwrap().status(),
        404
    );
    let post = web.post(format!("{origin}/0.1.0/")).send().await.unwrap();
    assert_eq!(post.status(), 404);
    let known = get("/.well-known/calimero.json", "*/*").await.unwrap();
    let known: Value = known.json().await.unwrap();
    assert_eq!(known["package"], json!("com.test.site"));
    assert_eq!(known["versions"], json!(["0.1.0"]));
    let stranger = format!("{}.localhost", "a".repeat(52));
    let web2 = reqwest::Client::builder()
        .resolve(&stranger, ([127, 0, 0, 1], port).into())
        .build()
        .unwrap();
    let r = web2
        .get(format!("http://{stranger}:{port}/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404, "an unknown label never reaches the node");

    // Signing up on the app's origin opens a session bound to its package.
    let carol = c.client(1);
    let mut carol_key = SoftAuthenticator::new("localhost", &origin);
    let _s = carol.sign_up(&mut carol_key, "carol").await.unwrap();
    let mine = carol
        .post(
            "apps",
            json!({ "name": "mine", "package": "com.test.site" }),
        )
        .await
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ = carol
        .call(&mine, "set", json!({ "key": "k", "value": "v" }), None)
        .await
        .unwrap();
    // Not another package's app, even a public one; not the account's keys.
    assert!(carol
        .call(&other, "set", json!({ "key": "k", "value": "v" }), None)
        .await
        .is_err());
    assert!(carol.get(&format!("apps/{other}")).await.is_err());
    assert!(carol
        .post("apps", json!({ "name": "x", "package": "com.test.other" }))
        .await
        .is_err());
    assert!(carol.create_api_key(json!({ "label": "k" })).await.is_err());
    let listed = carol.get("apps").await.unwrap();
    assert!(
        listed["apps"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["appId"] != json!(other)),
        "{listed}"
    );
    // The binding survives a refresh.
    let _ = carol.refresh().await.unwrap();
    assert!(carol.create_api_key(json!({ "label": "k" })).await.is_err());
    assert!(carol
        .call(&other, "set", json!({ "key": "k", "value": "v" }), None)
        .await
        .is_err());

    // The same person on the cluster's own origin is not bound.
    let carol_home = c.client(2);
    let mut home_key = SoftAuthenticator::with_key(&carol_key, "localhost", ORIGIN);
    let _s = carol_home.sign_in(&mut home_key).await.unwrap();
    let _ = carol_home
        .call(&other, "set", json!({ "key": "k", "value": "v" }), None)
        .await
        .unwrap();
    assert!(carol_home
        .create_api_key(json!({ "label": "k" }))
        .await
        .is_ok());
}

// ---------------------------------------------------------------- joining nodes

/// A node registered with `/ops/nodes` after the cluster started joins the
/// system group as a learner, learns the directory, and can host replicas
/// (mdma's `test_a_node_added_to_a_running_cluster_hosts_replicas`).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_node_added_to_a_running_cluster_hosts_replicas() {
    let mut c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let (app, _) = deploy(&alice, "kv", kv_wasm(), json!({})).await;
    let _ = alice
        .call(&app, "set", json!({ "key": "a", "value": "1" }), None)
        .await
        .unwrap();
    let new = c.join().await;
    let new_id = new as u64 + 1;
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    let _ = ops
        .post(
            "ops/nodes",
            json!({ "id": new_id, "addr": c.urls[new], "zone": "b" }),
        )
        .await
        .unwrap();

    // It joins the system group and learns the directory.
    let joined = &c.nodes[new].as_ref().unwrap().node;
    let mut knows = false;
    for _ in 0..200 {
        if joined.sys(|s| s.apps.contains_key(&app)) {
            knows = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(knows, "the new node never learned the directory");
    let leader_node = (0..3)
        .find(|i| c.nodes[*i].as_ref().unwrap().node.system.status().is_leader)
        .unwrap();
    let status = c.nodes[leader_node].as_ref().unwrap().node.system.status();
    assert!(status.learners.contains(&new_id), "{status:?}");

    // The operator moves a replica onto it, and it serves the app.
    let replicas: Vec<u64> = alice.get(&format!("apps/{app}")).await.unwrap()["replicas"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    let new_set: Vec<u64> = replicas[1..].iter().copied().chain([new_id]).collect();
    let app_leader = c.app_leader(&app).unwrap();
    let ops_leader = c.client(app_leader);
    ops_leader.use_api_key("ops-secret").await;
    let _ = ops_leader
        .post(
            &format!("ops/apps/{app}/replicas"),
            json!({ "replicas": new_set }),
        )
        .await
        .unwrap();
    let session = alice.session().await.unwrap();
    let cl = c.client(new);
    cl.use_session(session).await;
    let mut served = Value::Null;
    for _ in 0..100 {
        if let Ok(v) = cl
            .view(
                &app,
                "get",
                json!({ "key": "a" }),
                Consistency::Stale { min_index: 0 },
            )
            .await
        {
            if v.result == json!("1") {
                served = v.result;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(served, json!("1"), "the new replica never served the app");
    assert!(c.nodes[new].as_ref().unwrap().node.hosted(&app).is_some());
}

/// A replica move sent to a node that holds no replica of the app is redirected
/// before anything is written (it used to record the new replica set, then
/// answer 503, and the node started an empty group).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_replica_move_sent_to_a_node_without_the_app_writes_nothing() {
    let mut c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let (app, _) = deploy(&alice, "kv", kv_wasm(), json!({})).await;
    let new = c.join().await;
    let new_id = new as u64 + 1;
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    let _ = ops
        .post(
            "ops/nodes",
            json!({ "id": new_id, "addr": c.urls[new], "zone": "b" }),
        )
        .await
        .unwrap();
    let joined = &c.nodes[new].as_ref().unwrap().node;
    for _ in 0..200 {
        if joined.sys(|s| s.apps.contains_key(&app)) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let before = alice.get(&format!("apps/{app}")).await.unwrap()["replicas"].clone();
    let outside = c.client(new);
    outside.use_api_key("ops-secret").await;
    let e = outside
        .post(
            &format!("ops/apps/{app}/replicas"),
            json!({ "replicas": [2, 3, new_id] }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NOT_LEADER"), "{e}");
    assert!(e.details().unwrap()["leaderUrl"].is_string(), "{e}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let after = alice.get(&format!("apps/{app}")).await.unwrap()["replicas"].clone();
    assert_eq!(before, after, "nothing was written");
    assert!(joined.hosted(&app).is_none(), "no empty group was started");
}

/// `DELETE /ops/nodes/{id}`: refused while an app keeps a replica on the node;
/// once the operator moved them, the node leaves the directory and the system
/// group (a registered learner takes its vote), and is not registered again
/// from the other nodes' `--peers`.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_node_is_removed_once_no_app_keeps_a_replica_on_it() {
    let mut c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let (app, _) = deploy(&alice, "kv", kv_wasm(), json!({})).await;
    let _ = alice
        .call(&app, "set", json!({ "key": "a", "value": "1" }), None)
        .await
        .unwrap();
    let new = c.join().await;
    let new_id = new as u64 + 1;
    let ops = |i: usize| {
        let cl = c.client(i);
        async move {
            cl.use_api_key("ops-secret").await;
            cl
        }
    };
    let _ = ops(0)
        .await
        .post(
            "ops/nodes",
            json!({ "id": new_id, "addr": c.urls[new], "zone": "b" }),
        )
        .await
        .unwrap();
    let sys_leader = || {
        (0..3)
            .find(|i| c.nodes[*i].as_ref().unwrap().node.system.status().is_leader)
            .unwrap()
    };
    for _ in 0..200 {
        let st = c.nodes[sys_leader()].as_ref().unwrap().node.system.status();
        if st.learners.contains(&new_id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // A node that is not the system leader, holding one of the app's replicas.
    let leader = sys_leader();
    let gone = (0..3).find(|i| *i != leader).unwrap();
    let gone_id = gone as u64 + 1;

    let e = ops(leader)
        .await
        .delete(&format!("ops/nodes/{gone_id}"))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("replica"), "{e}");
    let e = ops(gone)
        .await
        .delete(&format!("ops/nodes/{gone_id}"))
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("421"), "{e}");

    // Move the app off it, then remove it.
    let keep: Vec<u64> = (1..=new_id).filter(|id| *id != gone_id).collect();
    let app_leader = c.app_leader(&app).unwrap();
    let _ = ops(app_leader)
        .await
        .post(
            &format!("ops/apps/{app}/replicas"),
            json!({ "replicas": keep }),
        )
        .await
        .unwrap();
    let _ = ops(leader)
        .await
        .delete(&format!("ops/nodes/{gone_id}"))
        .await
        .unwrap();
    let node = &c.nodes[leader].as_ref().unwrap().node;
    let st = node.system.status();
    assert!(!st.voters.contains(&gone_id), "{st:?}");
    assert!(!st.learners.contains(&gone_id), "{st:?}");
    assert!(
        st.voters.contains(&new_id),
        "the learner takes its vote: {st:?}"
    );
    // Reconcile does not register it again from --peers.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(node.sys(|s| !s.nodes.contains_key(&gone_id)));
    // The app keeps serving.
    let v = alice
        .view(&app, "get", json!({ "key": "a" }), Consistency::Strong)
        .await
        .unwrap();
    assert_eq!(v.result, json!("1"));
}

/// A client of node `i` holding the operator's token.
async fn ops_at(c: &Cluster, i: usize) -> Client {
    let cl = c.client(i);
    cl.use_api_key("ops-secret").await;
    cl
}

/// The node ids `cl` sees in `ops/placement` for `app`, and each node's app count.
async fn placement(cl: &Client, app: &str) -> (Vec<u64>, BTreeMap<u64, u64>) {
    let p = cl.get("ops/placement").await.unwrap();
    let replicas = p["apps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["app"] == json!(app))
        .map(|a| {
            a["replicas"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap())
                .collect()
        })
        .unwrap_or_default();
    let counts = p["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| (n["id"].as_u64().unwrap(), n["apps"].as_u64().unwrap()))
        .collect();
    (replicas, counts)
}

/// The index of the running node that leads the system group, waiting for an election.
async fn system_leader(c: &Cluster) -> usize {
    for _ in 0..200 {
        if let Some(i) = c
            .nodes
            .iter()
            .position(|r| r.as_ref().is_some_and(|r| r.node.system.status().is_leader))
        {
            return i;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no system leader was elected");
}

/// A cluster started on ONE node grows: registered learners become system
/// voters once there are enough to make 3 (never 2), the app moves to 3
/// replicas, the cluster survives losing its first node, a fourth node stays a
/// learner, shows in `/ops/placement` with no apps, and takes a replica.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_cluster_started_on_one_node_grows_and_survives_losing_it() {
    let mut c = Cluster::start(1).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let (app, _) = deploy(
        &alice,
        "doc",
        kv_wasm(),
        json!({ "access": { "kind": "public" } }),
    )
    .await;
    let _ = alice
        .call(&app, "set", json!({ "key": "a", "value": "1" }), None)
        .await
        .unwrap();
    // A blob, held by node 1 alone for now.
    let blob = alice
        .upload_blob(&app, b"held by one node".to_vec())
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (replicas, counts) = placement(&ops_at(&c, 0).await, &app).await;
    assert_eq!(replicas, vec![1], "one node, one replica");
    assert_eq!(counts, BTreeMap::from([(1, 1)]));

    // Every voter set the first node's system group goes through.
    let first = std::sync::Arc::clone(&c.nodes[0].as_ref().unwrap().node);
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<usize>::new()));
    let watch = {
        let seen = std::sync::Arc::clone(&seen);
        let mut rx = first.system.raft().metrics();
        let first = std::sync::Arc::clone(&first);
        tokio::spawn(async move {
            while rx.changed().await.is_ok() {
                seen.lock()
                    .unwrap()
                    .push(first.system.status().voters.len());
            }
        })
    };

    // Node 2 joins as a learner; alone it is not promoted (2 voters tolerate no failure).
    let two = c.join().await;
    let _ = ops_at(&c, 0)
        .await
        .post(
            "ops/nodes",
            json!({ "id": 2, "addr": c.urls[two], "zone": "b" }),
        )
        .await
        .unwrap();
    for _ in 0..200 {
        if first.system.status().learners.contains(&2) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(2000)).await;
    let st = first.system.status();
    assert_eq!(st.voters, vec![1], "{st:?}");
    assert_eq!(st.learners, vec![2], "{st:?}");

    // Node 3 joins: both learners become voters in one change.
    let three = c.join().await;
    let _ = ops_at(&c, 0)
        .await
        .post(
            "ops/nodes",
            json!({ "id": 3, "addr": c.urls[three], "zone": "c" }),
        )
        .await
        .unwrap();
    let mut voters = Vec::new();
    for _ in 0..300 {
        let st = first.system.status();
        voters = st.voters;
        if voters == [1, 2, 3] && st.learners.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(voters, vec![1, 2, 3], "learners became system voters");
    watch.abort();
    let _ = watch.await;
    let sizes = seen.lock().unwrap().clone();
    assert!(!sizes.contains(&2), "never 2 voters: {sizes:?}");
    drop(first);

    // The app moves to 3 replicas; the same move again changes nothing.
    let leader = c.app_leader(&app).unwrap();
    let moved = ops_at(&c, leader)
        .await
        .post(
            &format!("ops/apps/{app}/replicas"),
            json!({ "replicas": [1, 2, 3] }),
        )
        .await
        .unwrap();
    assert_eq!(moved["status"]["voters"], json!([1, 2, 3]), "{moved}");
    let again = ops_at(&c, leader)
        .await
        .post(
            &format!("ops/apps/{app}/replicas"),
            json!({ "replicas": [3, 2, 1] }),
        )
        .await
        .unwrap();
    assert_eq!(again["unchanged"], json!(true), "{again}");
    let (replicas, _) = placement(&ops_at(&c, 1).await, &app).await;
    assert_eq!(replicas, vec![1, 2, 3]);

    // Node 1 stops: the others elect a system leader and serve the app.
    let session = alice.session().await.unwrap();
    c.stop(0).await;
    let _ = system_leader(&c).await;
    let alice2 = c.client(1);
    alice2.use_session(session).await;
    let mut wrote = Err(String::new());
    for _ in 0..100 {
        // Node 1 signed the session: with it gone, the session refreshes
        // into one a live node signs.
        if wrote.as_ref().is_err_and(|e| e.starts_with("401")) {
            let _ = alice2.refresh().await;
        }
        wrote = alice2
            .call(&app, "set", json!({ "key": "b", "value": "2" }), None)
            .await
            .map_err(|e| e.to_string());
        if wrote.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let _ = wrote.expect("a call commits after node 1 stopped");
    // The move handed the bundle and the blob over: node 1 held them alone.
    assert_eq!(
        alice2.download_blob(&app, &blob).await.unwrap(),
        b"held by one node".to_vec()
    );
    let anyone = c.client(2);
    let v = anyone
        .view(&app, "get", json!({ "key": "b" }), Consistency::Strong)
        .await
        .unwrap();
    assert_eq!(v.result, json!("2"), "a read without signing in");

    // Node 4 joins: voters stay 3, and it shows in the placement with no apps.
    let four = c.join().await;
    let _ = ops_at(&c, 1)
        .await
        .post(
            "ops/nodes",
            json!({ "id": 4, "addr": c.urls[four], "zone": "b" }),
        )
        .await
        .unwrap();
    let mut st = c.nodes[1].as_ref().unwrap().node.system.status();
    for _ in 0..200 {
        let l = system_leader(&c).await;
        st = c.nodes[l].as_ref().unwrap().node.system.status();
        if st.learners.contains(&4) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(st.learners.contains(&4), "{st:?}");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let l = system_leader(&c).await;
    let st = c.nodes[l].as_ref().unwrap().node.system.status();
    assert_eq!(st.voters, vec![1, 2, 3], "no growth past 3: {st:?}");
    let (_, counts) = placement(&ops_at(&c, 1).await, &app).await;
    assert_eq!(counts.get(&4), Some(&0), "{counts:?}");
    let p = ops_at(&c, 2).await.get("ops/placement").await.unwrap();
    let n4 = p["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == json!(4))
        .unwrap()
        .clone();
    assert_eq!(n4["zone"], json!("b"), "{p}");
    assert!(n4.get("addr").is_none(), "ids and counts only: {p}");
    assert!(
        p["apps"][0].get("name").is_none(),
        "ids and counts only: {p}"
    );

    // The operator (or mdma) moves the app onto it, off the stopped node.
    let mut moved = Err(String::new());
    for _ in 0..50 {
        let Some(leader) = c.app_leader(&app) else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        moved = ops_at(&c, leader)
            .await
            .post(
                &format!("ops/apps/{app}/replicas"),
                json!({ "replicas": [2, 3, 4] }),
            )
            .await
            .map_err(|e| e.to_string());
        if moved.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let _ = moved.unwrap();
    let (replicas, counts) = placement(&ops_at(&c, 1).await, &app).await;
    assert_eq!(replicas, vec![2, 3, 4]);
    assert_eq!(counts.get(&4), Some(&1), "{counts:?}");
    let on_four = c.client(four);
    let mut served = Value::Null;
    for _ in 0..100 {
        if let Ok(v) = on_four
            .view(
                &app,
                "get",
                json!({ "key": "a" }),
                Consistency::Stale { min_index: 0 },
            )
            .await
        {
            served = v.result;
            if served == json!("1") {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(served, json!("1"), "the new replica serves the app");
}

/// A rebalancer may send the same move twice, or race another: a move to the
/// replica set the app already has changes nothing, and a second move while
/// one is under way is refused (`409 MOVE_IN_PROGRESS`) instead of mixing
/// two replica sets into the directory.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn replica_moves_are_idempotent_and_one_at_a_time() {
    let mut c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let (app, _) = deploy(&alice, "kv", kv_wasm(), json!({})).await;
    let new = c.join().await;
    let _ = ops_at(&c, 0)
        .await
        .post("ops/nodes", json!({ "id": 4, "addr": c.urls[new] }))
        .await
        .unwrap();
    let leader = c.app_leader(&app).unwrap();
    let lead = ops_at(&c, leader).await;
    let path = format!("ops/apps/{app}/replicas");

    let same = lead
        .post(&path, json!({ "replicas": [1, 2, 3] }))
        .await
        .unwrap();
    assert_eq!(same["unchanged"], json!(true), "{same}");

    // Two moves at once, both keeping the leader: one runs, one is refused.
    let me = leader as u64 + 1;
    let others: Vec<u64> = (1..=3).filter(|id| *id != me).collect();
    let a = json!({ "replicas": [me, others[0], 4] });
    let b = json!({ "replicas": [me, others[1], 4] });
    let second = ops_at(&c, leader).await;
    let (ra, rb) = tokio::join!(lead.post(&path, a), second.post(&path, b));
    let (won, lost) = match (ra, rb) {
        (Ok(w), Err(l)) | (Err(l), Ok(w)) => (w, l),
        other => panic!("exactly one move runs: {other:?}"),
    };
    assert_eq!(lost.code(), Some("MOVE_IN_PROGRESS"), "{lost}");
    let voters = won["status"]["voters"].clone();
    let recorded = alice.get(&format!("apps/{app}")).await.unwrap()["replicas"].clone();
    let mut recorded: Vec<u64> = recorded
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    recorded.sort_unstable();
    assert_eq!(
        json!(recorded),
        voters,
        "the directory names the group's voters"
    );
}

/// A release upgrade, as the runbook does it: nodes on the next mero-tee
/// release join, every app moves onto them, and the founders leave. The next
/// release's nodes take their storage key from that release's mero-kms, whose
/// root is its own, so their K_r is not the founders'. The apps' state, a value
/// sealed to the app's TEE (`TeeSecret`), a member's sealed share and a backup
/// taken before the upgrade must all still open on the new nodes.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn nodes_on_the_next_release_keep_the_data_of_the_ones_before() {
    let mut c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let (kv, _) = deploy(&alice, "kv", kv_wasm(), json!({})).await;
    let _ = alice
        .call(&kv, "set", json!({ "key": "a", "value": "1" }), None)
        .await
        .unwrap();
    let (cards, _) = deploy(
        &alice,
        "cards",
        mero_wasm("tee-cards", "tee_cards"),
        json!({}),
    )
    .await;
    let _ = eventually(&alice, &cards, "cards_left", json!({}), |v| !v.is_null()).await;
    let _ = alice.call(&cards, "sit", json!({}), None).await.unwrap();
    let _ = alice.call(&cards, "draw", json!({}), None).await.unwrap();
    let hand = eventually(&alice, &cards, "my_hand", json!({}), |v| {
        v.as_array().is_some_and(|h| h.len() == 1)
    })
    .await;
    let old_leader = c.app_leader(&kv).unwrap();
    let old = &c.nodes[old_leader].as_ref().unwrap().node;
    assert!(old.backup().await.unwrap() >= 1);
    let backups = old.config.backup_dir.clone().unwrap();

    // The next release: its own KMS, so another K_r.
    let mut next = Vec::new();
    for _ in 0..3 {
        next.push(c.join_with(|config| config.storage_key = [43; 32]).await);
    }
    let next_ids: Vec<u64> = next.iter().map(|i| *i as u64 + 1).collect();
    let sys_leader = |c: &Cluster| {
        c.nodes.iter().enumerate().find_map(|(i, r)| {
            r.as_ref()
                .filter(|r| r.node.system.status().is_leader)
                .map(|_| i)
        })
    };
    let mut caught_up = false;
    for _ in 0..200 {
        caught_up = next.iter().all(|i| {
            c.nodes[*i]
                .as_ref()
                .unwrap()
                .node
                .sys(|s| s.apps.contains_key(&kv) && s.apps.contains_key(&cards))
        });
        if caught_up {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        caught_up,
        "the next release's nodes never caught up with the system group"
    );

    // Every app moves onto the next release's nodes.
    for app in [&kv, &cards] {
        let mut moved = None;
        for _ in 0..100 {
            if let Some(leader) = c.app_leader(app) {
                match ops_at(&c, leader)
                    .await
                    .post(
                        &format!("ops/apps/{app}/replicas"),
                        json!({ "replicas": next_ids }),
                    )
                    .await
                {
                    Ok(v) => {
                        moved = Some(v);
                        break;
                    }
                    Err(e) => eprintln!("move {app}: {e}"),
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(moved.is_some(), "{app} never moved to the next release");
    }

    // The founders leave.
    for gone in 0..3_usize {
        let mut removed = false;
        for _ in 0..100 {
            if let Some(leader) = sys_leader(&c) {
                match ops_at(&c, leader)
                    .await
                    .delete(&format!("ops/nodes/{}", gone + 1))
                    .await
                {
                    Ok(_) => {
                        removed = true;
                        break;
                    }
                    Err(e) => eprintln!("remove node {}: {e}", gone + 1),
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(removed, "node {} was never removed", gone + 1);
        c.stop(gone).await;
    }

    // Only the next release's nodes are left, and they hold everything.
    let cl = c.client(next[0]);
    cl.use_session(alice.session().await.unwrap()).await;
    // The founders signed alice's session: with them gone, it refreshes into
    // one a node of the next release signs.
    let _fresh = cl.refresh().await.unwrap();
    let v = eventually(&cl, &kv, "get", json!({ "key": "a" }), |v| v == &json!("1")).await;
    assert_eq!(v, json!("1"), "the app's state");
    let mine = eventually(&cl, &cards, "my_hand", json!({}), |v| v == &hand).await;
    assert_eq!(mine, hand, "a member's sealed share");
    // Dealing opens the deck, which is sealed to the app's TEE.
    let _ = cl.call(&cards, "draw", json!({}), None).await.unwrap();
    let hand = eventually(&cl, &cards, "my_hand", json!({}), |v| {
        v.as_array().is_some_and(|h| h.len() == 2)
    })
    .await;
    assert_eq!(hand.as_array().map(Vec::len), Some(2), "the TEE's deck");
    let new = &c.nodes[next[0]].as_ref().unwrap().node;
    let (_index, rows) = calimero_node::backup::latest(&backups, new.cluster_key(), &kv)
        .expect("a backup from before the upgrade opens on the next release")
        .unwrap();
    assert!(!rows.is_empty());
}

/// `internal/cluster-key` is reached without the cluster secret (a node of the
/// next release has none yet), so it judges the request alone, and never
/// answers with the key: it delivers it to the address the cluster has for the
/// node id asked about. Asking under another node's id, another cluster's, an
/// unknown or a removed one gets the asker nothing. A data directory whose
/// wrapped key another K_r cannot open stops the node.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_cluster_key_goes_only_to_the_address_of_a_node_the_cluster_knows() {
    use calimero_node::cluster_key::Request;

    let mut c = Cluster::start(3).await;
    let ask = |cluster: &str, node: u64| {
        serde_json::to_value(Request {
            cluster: cluster.to_owned(),
            node,
            token: hex::encode([1_u8; 32]),
        })
        .unwrap()
    };
    let cl = c.client(0);
    let name = c.name.clone();
    // Node 2's id: the key goes to node 2's address, which (running already)
    // takes no delivery, and the asker gets no key.
    let e = cl
        .post("internal/cluster-key", ask(&name, 2))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("could not be delivered"), "{e}");
    assert!(!e.to_string().contains(&hex::encode(
        c.nodes[0].as_ref().unwrap().node.cluster_key()
    )));
    let e = cl
        .post("internal/cluster-key", ask(&name, 9))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("not a node of this cluster"), "{e}");
    let e = cl
        .post("internal/cluster-key", ask("elsewhere", 2))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("another cluster"), "{e}");
    // A node that joined, then was removed.
    let new = c.join().await;
    let new_id = new as u64 + 1;
    let leader = (0..3)
        .find(|i| c.nodes[*i].as_ref().unwrap().node.system.status().is_leader)
        .unwrap();
    let _ = ops_at(&c, leader)
        .await
        .delete(&format!("ops/nodes/{new_id}"))
        .await
        .unwrap();
    let e = c
        .client(leader)
        .post("internal/cluster-key", ask(&name, new_id))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("not a node of this cluster"), "{e}");

    // The same data directory booted with another release's K_r stops.
    c.stop(1).await;
    let peers: BTreeMap<u64, String> = c
        .urls
        .iter()
        .enumerate()
        .map(|(i, u)| (i as u64 + 1, u.clone()))
        .collect();
    let mut config = node_config(2, &peers, c._dirs[1].path());
    config.cluster.clone_from(&c.name);
    config.bootstrap = false;
    config.storage_key = [43; 32];
    let e = run(config)
        .await
        .err()
        .expect("another K_r must not open it");
    assert!(
        format!("{e:#}").contains("another mero-tee release"),
        "{e:#}"
    );
}

/// Ownership transfer: the owner hands an app to another account
/// with a passkey assertion; the new owner then decides, the old one no longer.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_owner_transfers_an_app_with_their_passkey() {
    let c = Cluster::start(3).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let bob_id = bob.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let (app, _) = deploy(&alice, "kv", kv_wasm(), json!({})).await;
    let e = alice
        .post(&format!("apps/{app}/transfer"), json!({ "to": bob_id }))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    let e = bob
        .post(&format!("apps/{app}/transfer"), json!({ "to": bob_id }))
        .await
        .unwrap_err();
    assert!(!e.to_string().contains("CONFIRM_REQUIRED"), "{e}");
    let _ = alice
        .transfer_app(&app, &bob_id, &mut alice_pk)
        .await
        .unwrap();
    let a = bob.get(&format!("apps/{app}")).await.unwrap();
    assert_eq!(a["owner"], json!(bob_id), "{a}");
    assert!(alice.delete(&format!("apps/{app}")).await.is_err());
}

/// `--ops-token-sha256`: the node holds only the token's hash, and the token
/// (not the hash) opens the Ops API; and its Prometheus metrics.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_ops_token_can_be_given_as_a_hash_and_metrics_count() {
    use sha2::Digest;
    let hash = hex::encode(sha2::Sha256::digest(b"hashed-secret"));
    let c = Cluster::start_with(3, |cfg| {
        cfg.ops_token = None;
        cfg.ops_token_sha256 = Some(hash.clone());
    })
    .await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let (_app, _) = deploy(&alice, "kv", kv_wasm(), json!({})).await;
    let ops = c.client(0);
    ops.use_api_key("hashed-secret").await;
    assert!(ops.get("ops/status").await.is_ok());
    let wrong = c.client(0);
    wrong.use_api_key(&hash).await;
    assert!(
        wrong.get("ops/status").await.is_err(),
        "the hash is not the credential"
    );
    wrong.use_api_key("ops-secret").await;
    assert!(wrong.get("ops/status").await.is_err());

    let metrics = ops.get_text("ops/metrics").await.unwrap();
    assert!(metrics.contains("calimero_apps 1"), "{metrics}");
    assert!(metrics.contains("calimero_accounts 1"), "{metrics}");
    assert!(metrics.contains("calimero_system_voters 3"), "{metrics}");
    assert!(
        metrics.contains("# TYPE calimero_hosted_groups gauge"),
        "{metrics}"
    );
    assert!(
        alice.get_text("ops/metrics").await.is_err(),
        "operators only"
    );
}

/// `--metrics-listen`: the same counts as `/ops/metrics`, in plain HTTP on an
/// address of the operator's choosing, for a Prometheus scraper (which cannot
/// open the sealed channel). Off unless configured; it serves nothing else.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn metrics_can_be_scraped_on_their_own_listener() {
    let c = Cluster::start_with(3, |cfg| {
        cfg.metrics_listen = Some("127.0.0.1:0".parse().unwrap());
    })
    .await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let (_app, _) = deploy(&alice, "kv", kv_wasm(), json!({})).await;
    let addr = c.nodes[0].as_ref().unwrap().metrics_addr.unwrap();
    let http = reqwest::Client::new();
    let r = http
        .get(format!("http://{addr}/metrics"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let text = r.text().await.unwrap();
    assert!(text.contains("calimero_apps 1"), "{text}");
    assert!(text.contains("calimero_system_voters 3"), "{text}");
    let other = http
        .get(format!("http://{addr}/ops/status"))
        .send()
        .await
        .unwrap();
    assert_eq!(other.status(), 404, "only /metrics");
    let default = Cluster::start(3).await;
    assert!(default.nodes[0].as_ref().unwrap().metrics_addr.is_none());
}

/// The value of the `metric{labels}` series in a Prometheus text, if present.
/// `labels` is the inside of the braces, in the order the node writes them.
fn sample(text: &str, metric: &str, labels: &str) -> Option<f64> {
    let head = format!("{metric}{{{labels}}} ");
    text.lines()
        .find_map(|l| l.strip_prefix(&head))
        .map(|v| v.parse().unwrap())
}

/// Per-app usage for billing on `/metrics`: each run is counted on the node
/// that ran its WASM (calls and strong views on the app's leader, a stale view
/// on the replica that answered it), never on the replicas that applied the
/// entry, so the cluster's sum is the number of runs. Series are labelled
/// with the app's id and its payer: the organization, or `account:{owner}`
/// for a personal app. State bytes are a gauge every replica reports alike.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn app_usage_is_counted_on_the_node_that_ran_the_wasm() {
    let c = Cluster::start_with(3, |cfg| {
        cfg.metrics_listen = Some("127.0.0.1:0".parse().unwrap());
    })
    .await;
    let (alice, _pk) = signed_up(&c, 0, "alice-the-billed").await;
    let alice_id = alice.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let org = alice.post("orgs", json!({ "name": "Acme" })).await.unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let (app, _) = deploy(&alice, "kv", kv_wasm(), json!({ "org": org })).await;
    let (personal, _) = deploy(&alice, "mine", kv_wasm(), json!({})).await;
    for i in 0..3 {
        let _ = alice
            .call(
                &app,
                "set",
                json!({ "key": format!("k{i}"), "value": "v" }),
                None,
            )
            .await
            .unwrap();
    }
    for _ in 0..2 {
        let v = alice
            .view(&app, "get", json!({ "key": "k0" }), Consistency::Strong)
            .await
            .unwrap();
        assert_eq!(v.result, json!("v"));
    }
    let _ = alice
        .call(&personal, "set", json!({ "key": "a", "value": "b" }), None)
        .await
        .unwrap();

    // A stale view runs on the replica that answers it, here a follower.
    let leader = c.app_leader(&app).unwrap();
    let follower = (0..3).find(|i| *i != leader).unwrap();
    let applied_on = |i: usize| {
        c.nodes[i]
            .as_ref()
            .unwrap()
            .node
            .hosted(&app)
            .unwrap()
            .applied()
    };
    let at = applied_on(leader);
    let stale = c.nodes[follower]
        .as_ref()
        .unwrap()
        .node
        .view(&calimero_node::node::ViewRequest {
            app_id: app.clone(),
            method: "get".to_owned(),
            args: json!({ "key": "k0" }),
            caller: calimero_node::types::Caller::Account {
                account: alice_id.clone(),
            },
            strong: false,
            min_index: at,
        })
        .await
        .unwrap();
    assert_eq!(stale.result, json!("v"));

    // Every replica applied every entry; then each samples its state.
    for i in 0..3 {
        for _ in 0..100 {
            if applied_on(i) >= at {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    for r in c.nodes.iter().flatten() {
        r.node.sample_usage();
    }
    let http = reqwest::Client::new();
    let mut texts = Vec::new();
    for r in c.nodes.iter().flatten() {
        let addr = r.metrics_addr.unwrap();
        let text = http
            .get(format!("http://{addr}/metrics"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        texts.push(text);
    }
    let in_org = format!("app=\"{app}\",org=\"{org}\"");
    let mine = format!("app=\"{personal}\",org=\"account:{alice_id}\"");
    let calls = |labels: &str, kind: &str| -> Vec<f64> {
        texts
            .iter()
            .map(|t| {
                sample(
                    t,
                    "calimero_app_calls_total",
                    &format!("{labels},kind=\"{kind}\""),
                )
                .unwrap_or(0.0)
            })
            .collect()
    };
    // `init` and the three `set`s: four mutations in all, not one per replica.
    let mutations = calls(&in_org, "mutation");
    assert_eq!(mutations.iter().sum::<f64>(), 4.0, "{mutations:?}");
    let views = calls(&in_org, "view");
    assert_eq!(views.iter().sum::<f64>(), 3.0, "{views:?}");
    assert!(
        views[follower] >= 1.0,
        "the stale view ran on {follower}: {views:?}"
    );
    assert_eq!(calls(&in_org, "followup").iter().sum::<f64>(), 0.0);
    assert_eq!(calls(&mine, "mutation").iter().sum::<f64>(), 2.0);
    for (i, t) in texts.iter().enumerate() {
        let ran = mutations[i] + views[i];
        let cpu = sample(t, "calimero_app_wasm_cpu_seconds_total", &in_org);
        let memory = sample(t, "calimero_app_wasm_memory_byte_seconds_total", &in_org);
        if ran > 0.0 {
            assert!(cpu.unwrap() > 0.0, "node {i}:\n{t}");
            assert!(memory.unwrap() > 0.0, "node {i}:\n{t}");
        } else {
            assert_eq!(cpu, None, "node {i} ran nothing:\n{t}");
        }
        assert_eq!(
            sample(t, "calimero_app_outbound_requests_total", &in_org),
            None,
            "a series appears once it counts something"
        );
        assert!(t.contains("# TYPE calimero_app_calls_total counter"), "{t}");
        assert!(t.contains("# TYPE calimero_app_state_bytes gauge"), "{t}");
        assert!(!t.contains("alice-the-billed"), "a name leaked:\n{t}");
    }
    let state: Vec<f64> = texts
        .iter()
        .map(|t| sample(t, "calimero_app_state_bytes", &in_org).unwrap())
        .collect();
    assert!(state[0] > 0.0, "{state:?}");
    assert!(
        state.iter().all(|s| *s == state[0]),
        "replicas agree: {state:?}"
    );
    assert_eq!(
        sample(&texts[0], "calimero_app_blob_bytes", &in_org),
        None,
        "no blobs, no series"
    );
}

// ---------------------------------------------------------------- inspection

/// `merod inspect`: a stopped node's data directory, read
/// offline. The raw layout without the key; with it, each group's metadata.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_stopped_nodes_store_can_be_inspected() {
    let mut c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let (app, _) = deploy(&alice, "kv", kv_wasm(), json!({})).await;
    let _ = alice
        .call(&app, "set", json!({ "key": "a", "value": "1" }), None)
        .await
        .unwrap();
    c.stop(1).await;
    let dir = c._dirs[1].path().to_owned();
    let raw = calimero_node::inspect::inspect(&dir, None).unwrap();
    let groups = raw["groups"].as_object().unwrap();
    assert!(groups.contains_key("1"), "the system group: {raw}");
    assert!(groups.len() >= 2, "the app's group too: {raw}");
    assert!(raw["blobs"].as_u64().unwrap() >= 1, "the bundle: {raw}");
    assert!(groups["1"]["logEntries"].as_u64().unwrap() > 0);
    let opened = calimero_node::inspect::inspect(&dir, Some([42u8; 32])).unwrap();
    assert!(
        opened["groups"]["1"]["values"]
            .as_object()
            .is_some_and(|v| !v.is_empty()),
        "{opened}"
    );
}

/// The client's own calls for what the Python client and merobox reach through
/// it: an app from a package release, a confirmed DELETE (an organization) and
/// signing out on every device.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_client_creates_from_a_package_deletes_confirmed_and_signs_out_everywhere() {
    let c = Cluster::start(3).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let _ = alice
        .upload_package(mpk("com.test.client", "1.0.0", &kv_wasm(), &publisher))
        .await
        .unwrap();
    let app = alice
        .create_app_from_package("kv", "com.test.client", Some("1.0.0"), None, json!({}))
        .await
        .unwrap();
    let _ = alice
        .call(&app, "set", json!({ "key": "a", "value": "1" }), None)
        .await
        .unwrap();

    let org = alice
        .post("orgs", json!({ "name": "doomed" }))
        .await
        .unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = format!("orgs/{org}");
    let refused = alice.delete(&path).await.unwrap_err();
    assert_eq!(refused.code(), Some("CONFIRM_REQUIRED"));
    let _ = alice.delete_confirmed(&path, &mut pk).await.unwrap();
    let status = |e: calimero_client::Error| match e {
        calimero_client::Error::Api { status, .. } => status,
        calimero_client::Error::WrongNode { .. } => 421,
        calimero_client::Error::Transport(e) => panic!("{e}"),
    };
    assert_eq!(status(alice.get(&path).await.unwrap_err()), 404);

    let laptop = c.client(1);
    let _s = laptop.sign_in(&mut pk).await.unwrap();
    let _ = alice.sign_out_everywhere().await.unwrap();
    assert_eq!(status(laptop.get("auth/me").await.unwrap_err()), 401);
}

/// D70: the client gives every call a key of its own (resent unchanged on a
/// 503), so two calls made without one are two calls, never one replayed.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn calls_without_a_key_each_run_once() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 1, "alice").await;
    let (app, _) = deploy(&alice, "kv", kv_wasm(), json!({})).await;
    let a = alice
        .call(&app, "set", json!({ "key": "k", "value": "1" }), None)
        .await
        .unwrap();
    let b = alice
        .call(&app, "set", json!({ "key": "k", "value": "1" }), None)
        .await
        .unwrap();
    assert!(
        b.index > a.index,
        "the second call ran: {} then {}",
        a.index,
        b.index
    );
    let again = alice
        .call(
            &app,
            "set",
            json!({ "key": "k", "value": "2" }),
            Some("mine"),
        )
        .await
        .unwrap();
    let replay = alice
        .call(
            &app,
            "set",
            json!({ "key": "k", "value": "3" }),
            Some("mine"),
        )
        .await
        .unwrap();
    assert_eq!(replay.index, again.index, "a given key is answered once");
}

/// A grant or a revocation committed through one node holds at once for a call
/// through another: access is judged on the app record as of the fresh system
/// read the request makes, not on the record read before it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn access_changed_through_one_node_holds_at_once_through_another() {
    let c = Cluster::start(3).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let bob_id = bob.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
    let access = format!("apps/{app}/access");
    for round in 0..20 {
        let _ok = alice
            .post(&access, json!({ "access": { "kind": "owner" } }))
            .await
            .unwrap();
        let refused = bob
            .call(&app, "set", json!({ "key": "k", "value": "no" }), None)
            .await
            .expect_err("a revoked account is refused at once");
        assert_eq!(
            refused.code(),
            Some("NOT_FOUND"),
            "round {round}: {refused}"
        );
        let _ok = alice
            .post(
                &access,
                json!({ "access": { "kind": "accounts", "accounts": [bob_id] } }),
            )
            .await
            .unwrap();
        let _r = bob
            .call(&app, "set", json!({ "key": "k", "value": "yes" }), None)
            .await
            .unwrap_or_else(|e| {
                panic!("round {round}: a granted account is admitted at once: {e}")
            });
    }
}

/// What `/ops/metrics` adds for dashboards: requests by route template and
/// status, error codes, latency by route group, Raft per group and disk
/// sizes. No id, name or value reaches a label.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn metrics_count_requests_errors_and_raft_without_naming_anyone() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice-the-metrics-user").await;
    let alice_id = alice.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let (app, _) = deploy(&alice, "kv", kv_wasm(), json!({})).await;
    for i in 0..3 {
        let _ = alice
            .call(
                &app,
                "set",
                json!({ "key": format!("k{i}"), "value": "v" }),
                None,
            )
            .await
            .unwrap();
    }
    let refused = alice.get("apps/no-such-app").await.expect_err("404");
    assert_eq!(refused.code(), Some("NOT_FOUND"), "{refused}");

    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    let metrics = ops.get_text("ops/metrics").await.unwrap();
    let line = |prefix: &str| {
        metrics
            .lines()
            .find(|l| l.starts_with(prefix))
            .unwrap_or_else(|| panic!("no `{prefix}` in\n{metrics}"))
            .to_owned()
    };
    let calls = line(
        "http_requests_total{method=\"POST\",path=\"/apps/{app}/intents\",status=\"2xx\",group=\"calls\"}",
    );
    let n: u64 = calls.rsplit(' ').next().unwrap().parse().unwrap();
    assert!(n >= 3, "{calls}");
    let _ = line("calimero_http_errors_total{group=\"apps\",code=\"NOT_FOUND\"}");
    let _ = line(
        "http_request_duration_seconds_bucket{method=\"POST\",path=\"/apps/{app}/intents\",status=\"2xx\",group=\"calls\",le=\"+Inf\"}",
    );
    let _ = line("http_request_duration_seconds_count{method=\"POST\",path=\"/internal/raft\"");
    let _ = line("calimero_group_term{group=\"system\"}");
    let _ = line("calimero_group_leader{group=\"system\"}");
    let _ = line("calimero_group_apply_lag{group=\"system\"}");
    let _ = line("storage_disk_usage_bytes{store=\"datastore\"}");
    let _ = line("merod_build_info{version=");
    let _ = line("process_start_time_seconds ");
    if cfg!(target_os = "linux") {
        let _ = line("process_resident_memory_bytes ");
        let _ = line("process_cpu_seconds_total ");
    }
    assert!(metrics.contains("# TYPE http_request_duration_seconds histogram"));
    // Ids are labels of the per-app usage series (`calimero_app_*`) only, which
    // bill an app to its payer; a name or a request's path never is.
    assert!(metrics.contains(&format!("app=\"{app}\",org=\"account:{alice_id}\"")));
    let elsewhere: String = metrics
        .lines()
        .filter(|l| !l.starts_with("calimero_app_"))
        .collect::<Vec<_>>()
        .join("\n");
    for secret in [app.as_str(), alice_id.as_str()] {
        assert!(
            !elsewhere.contains(secret),
            "{secret} leaked outside the usage series"
        );
    }
    for secret in ["alice-the-metrics-user", "no-such-app"] {
        assert!(
            !metrics.contains(secret),
            "{secret} leaked into the metrics"
        );
    }
}

// ---------------------------------------------------------------- release governance

const APPROVE_RELEASES: u32 = 1 << 10;

/// An account's id, from `/auth/me`.
async fn id_of(cl: &Client) -> String {
    cl.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// `who` joins `org` at `role` through an invite `inviter` mints.
async fn join(inviter: &Client, who: &Client, org: &str, role: &str) {
    let token = inviter
        .post(&format!("orgs/{org}/invites"), json!({ "role": role }))
        .await
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = who
        .post(&format!("invites/{token}/redeem"), json!({}))
        .await
        .unwrap();
}

async fn new_org(cl: &Client, name: &str) -> String {
    cl.post("orgs", json!({ "name": name })).await.unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn ops(c: &Cluster) -> Client {
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    ops
}

/// Approving an organization's releases is `APPROVE_RELEASES`: the owner and
/// admins hold it, a member only when granted, refused (403, before any
/// passkey ceremony) once revoked; an outsider is told nothing (404); the
/// operator still cannot approve on a cluster whose owners decide.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn approving_an_organizations_releases_needs_approve_releases() {
    let c = Cluster::start(3).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let (bob, mut bob_pk) = signed_up(&c, 1, "bob").await;
    let (carol, mut carol_pk) = signed_up(&c, 2, "carol").await;
    let (dan, mut dan_pk) = signed_up(&c, 0, "dan").await;
    let org = new_org(&alice, "Docs").await;
    join(&alice, &bob, &org, "member").await;
    join(&alice, &carol, &org, "admin").await;
    let bob_id = id_of(&bob).await;
    let caps = format!("orgs/{org}/capabilities");

    let e = bob
        .approve_release(Some(&org), "9.0.0", &mut bob_pk)
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("403"), "{e}");
    assert!(e.to_string().contains("APPROVE_RELEASES"), "{e}");

    let _ok = alice
        .post(
            &caps,
            json!({ "account": bob_id, "capabilities": APPROVE_RELEASES }),
        )
        .await
        .unwrap();
    let _ok = bob
        .approve_release(Some(&org), "9.0.0", &mut bob_pk)
        .await
        .unwrap();
    let _ok = carol
        .approve_release(Some(&org), "9.1.0", &mut carol_pk)
        .await
        .unwrap();
    let _ok = alice
        .approve_release(Some(&org), "9.2.0", &mut alice_pk)
        .await
        .unwrap();
    let view = alice.get(&format!("orgs/{org}")).await.unwrap();
    let approvers: Vec<String> = serde_json::from_value(view["releaseApprovers"].clone()).unwrap();
    assert!(approvers.contains(&bob_id), "{view}");
    assert_eq!(approvers.len(), 3, "{view}");
    for v in ["9.0.0", "9.1.0", "9.2.0"] {
        assert!(view["approvedReleases"]
            .as_array()
            .unwrap()
            .contains(&json!(v)));
    }

    let _ok = alice
        .post(&caps, json!({ "account": bob_id, "capabilities": 0 }))
        .await
        .unwrap();
    let e = bob
        .approve_release(Some(&org), "9.3.0", &mut bob_pk)
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("403"), "revoked: {e}");

    let e = dan
        .approve_release(Some(&org), "9.3.0", &mut dan_pk)
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("404"), "an outsider: {e}");

    let e = ops(&c)
        .await
        .post("ops/releases", json!({ "version": "9.4.0" }))
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("403"), "the operator: {e}");
}

/// Set (`Some`) or lift (`None`) 2.0.0's deadline, signed by the operator.
async fn set_deadline(c: &Cluster, after: Option<&str>) -> Result<Value, Box<Error>> {
    harness::ops_signed(
        &ops(c).await,
        "ops/releases/2.0.0/deadline",
        json!({ "after": after }),
    )
    .await
    .map_err(Box::new)
}

/// A release deadline stops new work for whoever has not approved the
/// release: before it everything goes on; after it a new app, an install and
/// an upgrade are refused `409 RELEASE_DEADLINE` naming the release, while
/// the existing app keeps answering; an account that approved it is
/// unaffected, approving lifts the stop at once, and new apps then go only
/// to nodes on the newer release. The deadlines are public.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_release_deadline_stops_new_work_but_not_running_apps() {
    let c = Cluster::start_with(3, |cfg| {
        cfg.tee_release = Some(if cfg.node_id == 3 { "2.0.0" } else { "1.0.0" }.into());
    })
    .await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let (bob, mut bob_pk) = signed_up(&c, 1, "bob").await;
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
    set(&alice, &app, "v1").await.unwrap();

    // Announced for later: nothing changes yet, and anyone can read it.
    let _ok = set_deadline(&c, Some("2999-01-01T00:00:00Z"))
        .await
        .unwrap();
    let _early = alice.create_app("early", &bundle, json!({})).await.unwrap();
    let public = c.client(1).get("releases").await.unwrap();
    assert_eq!(
        public["deadlines"][0]["version"],
        json!("2.0.0"),
        "{public}"
    );
    assert_eq!(
        public["deadlines"][0]["after"],
        json!("2999-01-01T00:00:00Z")
    );
    let e = ops(&c)
        .await
        .post("ops/releases/2.0.0/deadline", json!({ "after": "soon" }))
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("400"), "{e}");

    // Bob approves 2.0.0 in time; then the deadline passes.
    let _ok = bob
        .approve_release(None, "2.0.0", &mut bob_pk)
        .await
        .unwrap();
    let _ok = set_deadline(&c, Some("2001-01-01T00:00:00Z"))
        .await
        .unwrap();

    let stopped = |e: Error| {
        assert_eq!(e.code(), Some("RELEASE_DEADLINE"), "{e}");
        assert!(e.to_string().contains("2.0.0"), "{e}");
    };
    stopped(
        alice
            .create_app("late", &bundle, json!({}))
            .await
            .unwrap_err(),
    );
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[21; 32]);
    stopped(
        alice
            .upload_package(mpk("com.test.late", "0.1.0", &kv_wasm(), &publisher))
            .await
            .unwrap_err(),
    );
    let next = alice
        .upload_bundle(variant(&kv_wasm(), "v2"))
        .await
        .unwrap();
    stopped(
        alice
            .post(&format!("apps/{app}/upgrade"), json!({ "bundle": next }))
            .await
            .unwrap_err(),
    );
    // The running app keeps answering, reads and writes.
    set(&alice, &app, "v2").await.unwrap();
    assert_eq!(
        strong_view(&alice, &app, "get", json!({ "key": "k" })).await,
        json!("v2")
    );

    // Bob, who approved, carries on: on the newer node only.
    let bobs = bob.upload_bundle(kv_wasm()).await.unwrap();
    let created = bob
        .post("apps", json!({ "name": "b", "bundle": bobs }))
        .await
        .unwrap();
    assert_eq!(created["replicas"], json!([3]), "{created}");

    // Approving lifts it for Alice at once.
    let _ok = alice
        .approve_release(None, "2.0.0", &mut alice_pk)
        .await
        .unwrap();
    let created = alice
        .post("apps", json!({ "name": "late", "bundle": bundle }))
        .await
        .unwrap();
    assert_eq!(created["replicas"], json!([3]), "{created}");

    // Lifted by the operator: gone from the public rules.
    // Unsigned, it is not lifted; the operator's signature lifts it.
    let e = ops(&c)
        .await
        .delete("ops/releases/2.0.0/deadline")
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("OPERATOR_SIGNATURE_REQUIRED"), "{e}");
    let _ok = set_deadline(&c, None).await.unwrap();
    let public = c.client(1).get("releases").await.unwrap();
    assert_eq!(public["deadlines"], json!([]), "{public}");
}

/// The binding of `key` to `org`: the key's signature over
/// `calimero-publisher-binding/v1\n{cluster}\n{org}\n{signer}\n{generation}`,
/// with the cluster's name and the key's generation as the node names them.
async fn publisher_binding(cl: &Client, key: &ed25519_dalek::SigningKey, org: &str) -> Value {
    use ed25519_dalek::Signer as _;
    let signer = calimero_bundle::derive_signer_id_did_key(key.verifying_key().as_bytes());
    let next = cl.get(&format!("publishers/{signer}")).await.unwrap();
    let message = format!(
        "calimero-publisher-binding/v1\n{}\n{org}\n{signer}\n{}",
        next["cluster"].as_str().unwrap(),
        next["generation"]
    );
    json!({
        "signerId": signer,
        "publicKey": calimero_identity::b64url(key.verifying_key().as_bytes()),
        "signature": calimero_identity::b64url(&key.sign(message.as_bytes()).to_bytes()),
    })
}

/// A publisher binds its package's key with the key's signature, then
/// upgrades its customers' instances of a `publisher` package, which the
/// customer may not (and nobody may before the binding); a later release may
/// give upgrades to owners, never back.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_publisher_upgrades_the_instances_of_its_publisher_packages() {
    use calimero_bundle::UpgradeAuthority::{Owner, Publisher};
    let c = Cluster::start(3).await;
    let (pam, mut pam_pk) = signed_up(&c, 0, "pam").await;
    let (cora, mut cora_pk) = signed_up(&c, 1, "cora").await;
    let publisher = new_org(&pam, "Publisher").await;
    let customer = new_org(&cora, "Customer").await;
    let key = ed25519_dalek::SigningKey::from_bytes(&[31; 32]);
    let release = |version: &str, wasm: &[u8], upgrades| {
        mpk_full(
            "com.test.pub",
            version,
            wasm,
            &key,
            &[],
            &[],
            Some(upgrades),
        )
    };
    let kv = kv_wasm();
    let first = release("1.0.0", &kv, Publisher);
    let _ok = pam.upload_package(first.clone()).await.unwrap();
    let _ok = cora.upload_package(first).await.unwrap();
    let inst = cora
        .create_app_from_package("i", "com.test.pub", None, None, json!({ "org": customer }))
        .await
        .unwrap();

    // Until the key is bound, nobody upgrades a `publisher` package.
    let e = cora
        .post(
            &format!("apps/{inst}/upgrade"),
            json!({ "version": "1.0.0" }),
        )
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("409"), "{e}");
    // The reports say so: the release declares `publisher`, but with the key
    // unbound the instance's own organization approves its node releases.
    let a = reported_app(&cora, &format!("orgs/{customer}/apps"), &inst).await;
    assert_eq!(a["upgradesHeldBy"], json!("publisher"), "{a}");
    assert_eq!(a["releasesApprovedBy"], json!("owner"), "{a}");
    assert_eq!(a["publisher"]["org"], Value::Null, "{a}");

    // Only the key's own signature over this organization's binding counts.
    let e = cora
        .post_confirmed(
            &format!("orgs/{customer}/publishers"),
            publisher_binding(&cora, &key, &publisher).await,
            &mut cora_pk,
        )
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("403"), "{e}");
    let binding = publisher_binding(&pam, &key, &publisher).await;
    let _ok = pam
        .post_confirmed(
            &format!("orgs/{publisher}/publishers"),
            binding.clone(),
            &mut pam_pk,
        )
        .await
        .unwrap();
    let view = pam.get(&format!("orgs/{publisher}")).await.unwrap();
    assert_eq!(view["publishes"][0], binding["signerId"]);
    // Bound: the publisher's approvals now decide the customer's instance.
    let a = reported_app(&cora, &format!("orgs/{customer}/apps"), &inst).await;
    assert_eq!(a["releasesApprovedBy"], json!("publisher"), "{a}");
    assert_eq!(a["publisher"]["org"], json!(publisher), "{a}");
    let ops = c.client(2);
    ops.use_api_key("ops-secret").await;
    let a = reported_app(&ops, "ops/apps", &inst).await;
    assert_eq!(a["releasesApprovedBy"], json!("publisher"), "{a}");
    assert_eq!(a["upgradesHeldBy"], json!("publisher"), "{a}");
    // The same signature again is spent.
    let e = pam
        .post_confirmed(
            &format!("orgs/{publisher}/publishers"),
            binding,
            &mut pam_pk,
        )
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("403"), "{e}");

    let _ok = pam
        .upload_package(release("1.1.0", &variant(&kv, "v2"), Publisher))
        .await
        .unwrap();
    // The customer no longer upgrades it.
    let e = cora
        .post_confirmed(
            &format!("apps/{inst}/upgrade"),
            json!({ "version": "1.1.0" }),
            &mut cora_pk,
        )
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("403"), "{e}");
    // The publisher does, for every instance.
    let run = pam
        .post_confirmed(
            &format!("orgs/{publisher}/packages/com.test.pub/upgrade"),
            json!({ "version": "1.1.0" }),
            &mut pam_pk,
        )
        .await
        .unwrap();
    let run = finished_run(&pam, &publisher, run["runId"].as_str().unwrap()).await;
    assert_eq!(run["done"], json!([inst]), "{run}");

    // Upgrades go to owners in a later release, never back.
    let _ok = pam
        .upload_package(release("1.2.0", &variant(&kv, "v3"), Owner))
        .await
        .unwrap();
    let e = pam
        .upload_package(release("1.3.0", &variant(&kv, "v4"), Publisher))
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("409"), "{e}");
}

// ---------------------------------------------------------------- version and approval reports

/// The entry for `app` in a paged app report (`ops/apps`, `orgs/{org}/apps`,
/// `auth/apps`), reading every page.
async fn reported_app(cl: &Client, route: &str, app: &str) -> Value {
    let mut after = String::new();
    for _ in 0..100 {
        let sep = if route.contains('?') { '&' } else { '?' };
        let page = cl
            .get(&format!("{route}{sep}limit=1{after}"))
            .await
            .unwrap_or_else(|e| panic!("{route}: {e}"));
        let apps = page["apps"].as_array().unwrap();
        assert!(apps.len() <= 1, "limit=1 answers one at most: {page}");
        if let Some(a) = apps.iter().find(|a| a["app"] == json!(app)) {
            return a.clone();
        }
        match page["next"].as_str() {
            Some(next) => after = format!("&after={next}"),
            None => panic!("{app} not in {route}"),
        }
    }
    panic!("{route} never ended");
}

/// What runs where, as the operator and an organization's members read it:
/// each app's package release (to the operator only for a release it can see,
/// else its bundle digest alone), its last upgrade with the kind of actor (an
/// account id or name never reaches the operator), the releases that may hold
/// its data and, per replica, whether that node's release is one of them. A
/// member reads its organization's apps with the upgrader's account; a
/// non-member reads nothing; an account reads its own apps with the releases
/// it has not approved yet and the newer releases it could upgrade to.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn app_versions_and_release_approvals_are_reported_without_names() {
    let c = Cluster::start_with(3, |cfg| {
        // The operator rolled node 3 onto a new release.
        cfg.tee_release = Some(if cfg.node_id == 3 { "2.0.0" } else { "1.0.0" }.into());
    })
    .await;
    let (alice, mut apk) = signed_up(&c, 0, "alice-the-owner").await;
    let (bob, _bpk) = signed_up(&c, 1, "bob-the-member").await;
    let (carol, _cpk) = signed_up(&c, 1, "carol-the-outsider").await;
    let alice_id = account_id(&alice).await;
    let bob_id = account_id(&bob).await;
    let org = alice
        .post("orgs", json!({ "name": "Acme Secret Org" }))
        .await
        .unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ = alice
        .post(
            &format!("orgs/{org}/members"),
            json!({ "account": bob_id, "role": "member" }),
        )
        .await
        .unwrap();
    let kv = kv_wasm();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let _ = alice
        .upload_package(mpk("com.test.track", "1.0.0", &kv, &publisher))
        .await
        .unwrap();
    let _ = alice
        .upload_package(mpk(
            "com.test.track",
            "1.1.0",
            &variant(&kv, "next"),
            &publisher,
        ))
        .await
        .unwrap();
    let created = alice
        .post(
            "apps",
            json!({
                "name": "tracked-secret-name",
                "package": "com.test.track",
                "version": "1.0.0",
                "org": org,
                "access": { "kind": "org" },
            }),
        )
        .await
        .unwrap();
    let app = created["appId"].as_str().unwrap().to_owned();
    let _ = alice
        .upgrade_app(&app, json!({ "version": "1.1.0" }), &mut apk)
        .await
        .unwrap();

    // The operator: ids, digests and kinds, never a name or an account.
    let ops = c.client(1);
    ops.use_api_key("ops-secret").await;
    let a = reported_app(&ops, "ops/apps", &app).await;
    assert_eq!(a["scope"], json!({ "kind": "org", "org": org }), "{a}");
    assert_eq!(
        a["package"],
        Value::Null,
        "a release the operator did not install stays unnamed: {a}"
    );
    assert_eq!(a["publisher"], Value::Null, "{a}");
    assert!(a["bundle"].as_str().is_some_and(|b| b.len() == 64), "{a}");
    assert_eq!(a["upgradesHeldBy"], json!("owner"), "{a}");
    assert_eq!(a["lastUpgrade"]["actorKind"], json!("owner"), "{a}");
    assert_eq!(a["lastUpgrade"]["ok"], json!(true), "{a}");
    assert_eq!(a["lastUpgrade"]["to"]["bundle"], a["bundle"], "{a}");
    assert!(a["lastUpgrade"].get("actor").is_none(), "{a}");
    assert_eq!(a["approvedReleases"], json!(["1.0.0"]), "{a}");
    let replicas = a["replicas"].as_array().unwrap();
    assert_eq!(replicas.len(), 2, "{a}");
    for r in replicas {
        assert_eq!(r["release"], json!("1.0.0"), "{a}");
        assert_eq!(r["approved"], json!(true), "{a}");
    }
    let report = ops.get("ops/apps").await.unwrap();
    assert_eq!(
        report["governance"]["appUpgrades"],
        json!("owner"),
        "{report}"
    );
    let orgs = ops.get("ops/orgs").await.unwrap();
    let o = orgs["orgs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["org"] == json!(org))
        .unwrap_or_else(|| panic!("{org} in {orgs}"));
    assert_eq!(o["approvedReleases"], json!(["1.0.0"]), "{orgs}");
    for secret in [
        alice_id.as_str(),
        bob_id.as_str(),
        "alice-the-owner",
        "tracked-secret-name",
        "Acme Secret Org",
        "com.test.track",
    ] {
        assert!(
            !report.to_string().contains(secret) && !orgs.to_string().contains(secret),
            "{secret} reached the operator"
        );
    }
    // The placement read keeps its shape.
    let p = ops.get("ops/placement").await.unwrap();
    assert!(p["apps"][0].get("replicas").is_some(), "{p}");
    assert!(p["nodes"][0].get("release").is_some(), "{p}");

    // A member: the package, who upgraded it, and the release not approved yet.
    let route = format!("orgs/{org}/apps");
    let m = bob.get(&route).await.unwrap();
    assert_eq!(m["scope"], json!({ "kind": "org", "org": org }), "{m}");
    assert_eq!(m["approvedReleases"], json!(["1.0.0"]), "{m}");
    assert_eq!(m["governance"]["releases"], json!("owner"), "{m}");
    let cluster: Vec<(String, bool)> = m["clusterReleases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["release"].as_str().unwrap().to_owned(),
                r["approved"].as_bool().unwrap(),
            )
        })
        .collect();
    assert!(cluster.contains(&("2.0.0".into(), false)), "{m}");
    assert!(cluster.contains(&("1.0.0".into(), true)), "{m}");
    let a = reported_app(&bob, &route, &app).await;
    assert_eq!(a["name"], json!("tracked-secret-name"), "{a}");
    assert_eq!(a["package"]["package"], json!("com.test.track"), "{a}");
    assert_eq!(a["package"]["version"], json!("1.1.0"), "{a}");
    assert!(
        a["publisher"]["signer"]
            .as_str()
            .is_some_and(|k| k.starts_with("did:key:")),
        "{a}"
    );
    assert_eq!(a["lastUpgrade"]["from"]["version"], json!("1.0.0"), "{a}");
    assert_eq!(a["lastUpgrade"]["to"]["version"], json!("1.1.0"), "{a}");
    assert_eq!(a["lastUpgrade"]["actorKind"], json!("owner"), "{a}");
    assert_eq!(a["lastUpgrade"]["actor"], json!(alice_id), "{a}");
    // Someone outside the organization reads nothing of it.
    let e = carol.get(&route).await.unwrap_err();
    assert!(e.to_string().starts_with("404"), "{e}");
    let e = ops.get(&route).await.unwrap_err();
    assert!(
        e.to_string().starts_with("401") || e.to_string().starts_with("404"),
        "{e}"
    );

    // The owner approves 2.0.0: both views show it.
    let _ = alice
        .approve_release(Some(&org), "2.0.0", &mut apk)
        .await
        .unwrap();
    let m = bob.get(&route).await.unwrap();
    assert_eq!(m["approvedReleases"], json!(["1.0.0", "2.0.0"]), "{m}");
    let orgs = ops.get("ops/orgs").await.unwrap();
    assert!(
        orgs["orgs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["org"] == json!(org) && o["approvedReleases"] == json!(["1.0.0", "2.0.0"])),
        "{orgs}"
    );

    // An account's own app: its approvals, the releases it has not approved,
    // and the newer release it could upgrade to.
    let mine = alice
        .post(
            "apps",
            json!({ "name": "mine", "package": "com.test.track", "version": "1.0.0" }),
        )
        .await
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    let own = alice.get("auth/apps").await.unwrap();
    assert_eq!(own["scope"], json!({ "kind": "account" }), "{own}");
    assert_eq!(own["approvedReleases"], json!(["1.0.0"]), "{own}");
    assert!(
        own["clusterReleases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["release"] == json!("2.0.0") && r["approved"] == json!(false)),
        "{own}"
    );
    let a = reported_app(&alice, "auth/apps", &mine).await;
    assert_eq!(a["package"]["version"], json!("1.0.0"), "{a}");
    assert_eq!(a["availableVersions"], json!(["1.1.0"]), "{a}");
    assert_eq!(a["lastUpgrade"], Value::Null, "{a}");
    assert!(
        !own.to_string().contains(&app),
        "an organization's app is not the account's own: {own}"
    );
    // Another account sees none of alice's.
    let theirs = carol.get("auth/apps").await.unwrap();
    assert_eq!(theirs["apps"], json!([]), "{theirs}");
    // The page size is bounded.
    let e = alice.get("auth/apps?limit=100000").await.unwrap_err();
    assert!(e.to_string().starts_with("400"), "{e}");
}

/// Where the operator upgrades apps, the reports name the release it
/// installed and the actor kind `operator`, to the operator and to the owner.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_operator_upgrade_is_reported_as_the_operators() {
    use calimero_node::system::{Authority, Governance};
    let c = Cluster::start_with(3, |cfg| {
        cfg.governance = Governance {
            app_upgrades: Authority::Operator,
            ..Governance::default()
        };
    })
    .await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let kv = kv_wasm();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[8; 32]);
    let ops = c.client(1);
    ops.use_api_key("ops-secret").await;
    let _ = ops
        .ops_upload_package(mpk("com.test.opsup", "1.0.0", &kv, &publisher))
        .await
        .unwrap();
    let app = alice
        .post("apps", json!({ "name": "kv", "package": "com.test.opsup" }))
        .await
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ = ops
        .ops_upload_package(mpk(
            "com.test.opsup",
            "1.1.0",
            &variant(&kv, "next"),
            &publisher,
        ))
        .await
        .unwrap();
    let _ = harness::ops_signed(
        &ops,
        &format!("ops/apps/{app}/upgrade"),
        json!({ "version": "1.1.0" }),
    )
    .await
    .unwrap();

    let a = reported_app(&ops, "ops/apps", &app).await;
    assert_eq!(a["scope"], json!({ "kind": "account" }), "{a}");
    assert_eq!(a["upgradesHeldBy"], json!("operator"), "{a}");
    assert_eq!(
        a["package"],
        json!({ "package": "com.test.opsup", "version": "1.1.0" }),
        "a release the operator installed is named: {a}"
    );
    assert!(
        a["publisher"]["signer"]
            .as_str()
            .is_some_and(|k| k.starts_with("did:key:")),
        "{a}"
    );
    assert_eq!(a["lastUpgrade"]["actorKind"], json!("operator"), "{a}");
    assert_eq!(a["lastUpgrade"]["from"]["version"], json!("1.0.0"), "{a}");

    let a = reported_app(&alice, "auth/apps", &app).await;
    assert_eq!(a["lastUpgrade"]["actorKind"], json!("operator"), "{a}");
    assert_eq!(a["lastUpgrade"]["actor"], json!("operator"), "{a}");
    assert_eq!(a["availableVersions"], json!([]), "{a}");
    let own = alice.get("auth/apps").await.unwrap();
    assert_eq!(own["governance"]["appUpgrades"], json!("operator"), "{own}");
}

/// A system write the group cannot commit answers `503 UNAVAILABLE` in
/// bounded time instead of waiting forever. eu-shared-2 (2026-10-30): its
/// system group could not commit, and a sign-up and a node registration each
/// waited until the proxy in front of merod cut them at 30 s, so the caller
/// saw a dropped connection rather than why.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_system_write_that_cannot_commit_answers_unavailable() {
    let mut c = Cluster::start(3).await;
    let leader = (0..3)
        .find(|i| c.nodes[*i].as_ref().unwrap().node.system.status().is_leader)
        .expect("a system leader");
    // A first account, so the cluster is known to commit while it has its voters.
    let _ = signed_up(&c, leader, "alice").await;
    for i in (0..3).filter(|i| *i != leader) {
        c.stop(i).await;
    }

    let bob = c.client(leader);
    let mut pk = SoftAuthenticator::new("localhost", ORIGIN);
    let started = std::time::Instant::now();
    let answer = tokio::time::timeout(Duration::from_secs(60), bob.sign_up(&mut pk, "bob")).await;
    let waited = started.elapsed();
    let e = answer
        .expect("the write must be answered, not left hanging past 60 s")
        .expect_err("a write the system group cannot commit must fail");
    assert_eq!(e.code(), Some("UNAVAILABLE"), "{e}");
    assert!(
        waited < Duration::from_secs(25),
        "answered only after {waited:?}"
    );
}
