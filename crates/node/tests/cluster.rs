//! A three-node cluster in one process, driven only through the client, over
//! sealed channels, with the kv fixture component (the cluster itself is in
//! `harness/cluster.rs`, shared with `benches/cluster_throughput.rs`).

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use calimero_client::types::{AppCreateRequest, OrgRole, TeamJoin, ViewsRequestConsistency};
use calimero_client::{Client, Error, Policy, Request};
use calimero_identity::testing::SoftAuthenticator;
use calimero_node::run;
use futures_util::StreamExt;
use serde_json::{json, Value};

#[path = "harness/cluster.rs"]
mod harness;

use harness::{free_port, kv_wasm, node_config, signed_up, typed, wire, Cluster, ORIGIN};
use reqwest::Method;

const EVENTUALLY_DEADLINE: Duration = Duration::from_secs(60); // sized for a loaded CI runner

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
            .ops()
            .register_node(
                id,
                &url,
                Some(""),
                None,
                None,
                Some(&harness::operator_sign),
            )
            .await
            .map(wire)
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
            .apps()
            .call(
                app,
                "set",
                Some(json!({ "key": key, "value": value })),
                None,
                None,
            )
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

/// A system write waits for a leader until its deadline (`SYSTEM_WRITE_TIMEOUT`), not
/// for a fixed number of tries. The system leader and one more node stop, so the one
/// left has no leader; a write sent there then commits once a stopped node is back,
/// about 8 s later. Before, it gave up after 50 tries of 150 ms with `503 the system
/// group has no leader`, which a client signing in right after a node died saw
/// (calimero-client-py `test_keeps_serving_after_a_node_dies`).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_system_write_waits_for_a_leader_until_its_deadline() {
    let mut c = Cluster::start(3).await;
    let (_alice, _pk) = signed_up(&c, 0, "alice").await;
    let leader = (0..3)
        .find(|i| c.nodes[*i].as_ref().unwrap().node.system.status().is_leader)
        .expect("a system leader");
    let survivor = (leader + 1) % 3;
    let other = (leader + 2) % 3;
    c.stop(leader).await;
    c.stop(other).await;
    let node = std::sync::Arc::clone(&c.nodes[survivor].as_ref().unwrap().node);
    let started = std::time::Instant::now();
    // A second genesis: refused when it applies, which proves it reached a leader.
    let genesis = |node: std::sync::Arc<calimero_node::Node>| async move {
        node.sys_write(&calimero_node::system::SystemCmd::Genesis {
            session_proofs: true,
            governance: calimero_node::system::Governance::default(),
            release: None,
            operator_key: None,
            nodes: Vec::new(),
        })
        .await
    };
    let write = tokio::spawn(genesis(std::sync::Arc::clone(&node)));
    tokio::time::sleep(Duration::from_secs(8)).await;
    assert!(
        !write.is_finished(),
        "the write gave up without a leader before its deadline"
    );
    c.restart(other).await;
    let mut e = write
        .await
        .unwrap()
        .expect_err("a second genesis is refused");
    // The restarted node may not have rejoined and elected inside the first write's
    // remaining 2 s on a loaded runner; a write sent now must still reach the leader.
    if e.code == "UNAVAILABLE" {
        e = genesis(node)
            .await
            .expect_err("a second genesis is refused");
    }
    assert_eq!(e.code, "REJECTED", "after {:?}: {e:?}", started.elapsed());
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
    let _me = other
        .auth()
        .me()
        .await
        .expect("another node takes the session");

    let mut removed = false;
    for _ in 0..100 {
        let leader = system_leader(&c).await;
        if ops_at(&c, leader)
            .await
            .ops()
            .remove_node(&new_id.to_string(), Some(&harness::operator_sign))
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
        answer = other.auth().me().await.map(wire);
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
        .auth()
        .me()
        .await
        .map(wire)
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
        .auth()
        .login_begin(None)
        .await
        .map(wire)
        .unwrap();
    let challenge =
        calimero_identity::from_b64url(begin["options"]["challenge"].as_str().unwrap()).unwrap();
    let finish = json!({
        "challengeId": begin["challengeId"],
        "assertion": pk.get(&challenge),
    });
    let s = c
        .client(1)
        .transport()
        .json(Request::new(Method::POST, "auth/passkey/login/finish").json(finish.clone()))
        .await
        .unwrap();
    assert!(s["sessionToken"].is_string(), "{s}");
    for i in [0, 2] {
        let e = c
            .client(i)
            .transport()
            .json(Request::new(Method::POST, "auth/passkey/login/finish").json(finish.clone()))
            .await
            .unwrap_err();
        assert_eq!(e.code(), Some("UNKNOWN_CHALLENGE"), "{e}");
    }
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

/// A quota post changes the limits it names and keeps the others; a key that
/// names no limit is refused instead of ignored (mdma reset `maxAccounts` on
/// every save by posting the four it knew).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_quota_post_changes_only_what_it_names() {
    let c = Cluster::start(3).await;
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    let _ok = ops
        .ops()
        .quotas_set(
            typed(json!({ "maxAccounts": 7 })),
            Some(&harness::operator_sign),
        )
        .await
        .unwrap();
    let _ok = ops
        .ops()
        .quotas_set(
            typed(json!({ "maxOrgsPerAccount": 3 })),
            Some(&harness::operator_sign),
        )
        .await
        .unwrap();
    let q = ops.ops().quotas_get().await.map(wire).unwrap();
    assert_eq!(q["maxAccounts"], json!(7), "{q}");
    assert_eq!(q["maxOrgsPerAccount"], json!(3), "{q}");
    let e = ops
        .transport()
        .approved(
            Request::new(Method::POST, "ops/quotas").json(json!({ "maxAcounts": 1 })),
            None,
            Some(&harness::operator_sign),
        )
        .await
        .unwrap_err();
    assert!(e.to_string().contains("maxAcounts"), "{e}");
    assert_eq!(ops.ops().quotas_get().await.map(wire).unwrap(), q);
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
            .apps()
            .call(
                &app,
                "set",
                Some(json!({ "key": "a", "value": "2" })),
                None,
                None,
            )
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
    let bundle = cl
        .packages()
        .bundle_put(wasm)
        .await
        .map(|b| b.hash)
        .unwrap();
    let created = cl
        .apps()
        .create(AppCreateRequest {
            name: Some(name.to_owned()),
            bundle: Some(bundle),
            ..typed(options)
        })
        .await
        .unwrap();
    (created.app_id, created.context_id)
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
    let deadline = tokio::time::Instant::now() + EVENTUALLY_DEADLINE;
    while tokio::time::Instant::now() < deadline {
        if let Ok(v) = cl
            .apps()
            .view(
                app,
                method,
                Some(args.clone()),
                Some(ViewsRequestConsistency::Strong),
                None,
            )
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
    let (files, _) = deploy(&alice, "files", kv_wasm(), json!({})).await;
    let e = alice
        .apps()
        .blob_upload(&files, vec![7u8; 48 * 1024 * 1024], None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("TOO_LARGE"), "{e}");
    let listed = alice.apps().blob_list(&files).await.map(wire).unwrap();
    assert_eq!(listed["blobs"], json!([]), "{listed}");
}

/// An upload past the sealed channel's own cap is answered `413 TOO_LARGE`
/// naming the cap, not dropped mid-upload: merobox's 100 and 250 MiB blob
/// sweep saw the connection closed with no answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_upload_past_the_sealed_cap_is_answered_413() {
    let c = Cluster::start(3).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let (files, _) = deploy(&alice, "files", kv_wasm(), json!({})).await;
    let cap = calimero_sealed::server::MAX_SEALED_BYTES;
    for size in [cap + 1, 100 << 20] {
        let e = alice
            .apps()
            .blob_upload(&files, vec![7u8; size], None)
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
    let listed = alice.apps().blob_list(&files).await.map(wire).unwrap();
    assert_eq!(listed["blobs"], json!([]), "{listed}");
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

/// An owner who opens their own one-use invite link, out of curiosity,
/// gains nothing from it, so they spend nothing: the new account it was made
/// for still joins with it, and only then is it used up (`400 REJECTED`,
/// saying so). Before, the owner's redemption spent the only use and the
/// invitee was told "unknown invite".
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_owner_opening_their_own_invite_leaves_it_for_the_invitee() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let (carol, _c) = signed_up(&c, 2, "carol").await;
    let org = alice.orgs().create("Docs", None).await.map(wire).unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let token = alice
        .orgs()
        .invite_create(&org, typed(json!({ "role": "member" })), Some(&mut a))
        .await
        .map(wire)
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let own = alice.orgs().invite_redeem(&token).await.map(wire).unwrap();
    assert_eq!(own["org"], json!(org), "{own}");
    let joined = bob.orgs().invite_redeem(&token).await.map(wire).unwrap();
    assert_eq!(joined["org"], json!(org), "{joined}");
    let bob_id = bob.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let alice_id = alice.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let listed = alice.orgs().get(&org).await.map(wire).unwrap();
    assert_eq!(listed["members"][&bob_id], "member", "{listed}");
    assert_eq!(listed["members"][&alice_id], "owner", "{listed}");
    let e = carol.orgs().invite_redeem(&token).await.unwrap_err();
    assert_eq!(e.code(), Some("REJECTED"), "{e}");
    assert!(e.to_string().contains("used up"), "{e}");
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
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, mut bob_pk) = signed_up(&c, 1, "bob").await;
    let org = alice.orgs().create("Docs", None).await.map(wire).unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let token = alice
        .orgs()
        .invite_create(&org, typed(json!({ "role": "member" })), Some(&mut a))
        .await
        .map(wire)
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = bob.orgs().invite_redeem(&token).await.unwrap();
    let bob_id = bob.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let refused = |e: calimero_client::Error, why: &str| {
        let e = e.to_string();
        assert!(e.contains(why), "{e}");
    };
    let bundle = bob
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();

    let in_org = |name: &str| AppCreateRequest {
        name: Some(name.to_owned()),
        bundle: Some(bundle.clone()),
        org: Some(org.clone()),
        ..AppCreateRequest::default()
    };
    refused(
        bob.apps().create(in_org("b")).await.unwrap_err(),
        "CREATE_APP",
    );
    refused(
        bob.orgs()
            .invite_create(&org, typed(json!({ "role": "member" })), Some(&mut bob_pk))
            .await
            .unwrap_err(),
        "INVITE_MEMBERS",
    );

    let _ok = alice
        .orgs()
        .capabilities(
            &org,
            u64::from(CREATE_APP | INVITE_MEMBERS | MANAGE_APPLICATION),
            Some(&bob_id),
            None,
            Some(&mut a),
        )
        .await
        .unwrap();
    let _bobs = bob.apps().create(in_org("b")).await.unwrap();
    let _invite = bob
        .orgs()
        .invite_create(&org, typed(json!({ "role": "member" })), Some(&mut bob_pk))
        .await
        .unwrap();
    // Never above the member's own role.
    refused(
        bob.orgs()
            .invite_create(&org, typed(json!({ "role": "admin" })), Some(&mut bob_pk))
            .await
            .unwrap_err(),
        "above your own role",
    );
    // Another's org app: only with MANAGE_APPLICATION.
    let alices = alice
        .apps()
        .create(in_org("a"))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let next = bob
        .packages()
        .bundle_put(variant(&kv_wasm(), "v2"))
        .await
        .map(|b| b.hash)
        .unwrap();
    let _ok = alice
        .orgs()
        .capabilities(
            &org,
            u64::from(CREATE_APP | INVITE_MEMBERS),
            Some(&bob_id),
            None,
            Some(&mut a),
        )
        .await
        .unwrap();
    refused(
        bob.apps()
            .upgrade(&alices, typed(json!({ "bundle": next })), Some(&mut bob_pk))
            .await
            .unwrap_err(),
        "MANAGE_APPLICATION",
    );
    let _ok = alice
        .orgs()
        .capabilities(
            &org,
            u64::from(MANAGE_APPLICATION),
            Some(&bob_id),
            None,
            Some(&mut a),
        )
        .await
        .unwrap();
    let _upgraded = bob
        .apps()
        .upgrade(&alices, typed(json!({ "bundle": next })), Some(&mut bob_pk))
        .await
        .unwrap();
    // Never a personal app.
    let personal = alice
        .apps()
        .create(typed(json!({ "name": "p", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    refused(
        bob.apps()
            .upgrade(
                &personal,
                typed(json!({ "bundle": next })),
                Some(&mut bob_pk),
            )
            .await
            .unwrap_err(),
        "403",
    );
}

// ---------------------------------------------------------------- packages

/// A signed single-service `.mpk`, the shape `mero-sign` signs.
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
        compute: None,
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
    let e = alice
        .packages()
        .bundle_put(kv.clone())
        .await
        .map(|b| b.hash)
        .unwrap_err();
    assert!(e.to_string().starts_with("403"), "{e}");
    // Nor an uploaded package, signed or not, from an account or the
    // operator: every release comes from the registry, so any node that lacks
    // it fetches it there, and none lives only where it was uploaded.
    let signed = mpk("com.test.kv", "1.0.0", &kv, &publisher);
    let e = alice.packages().upload(signed.clone()).await.unwrap_err();
    assert_eq!(e.code(), Some("UPLOADS_DISABLED"), "{e}");
    let e = ops_at(&c, 0)
        .await
        .ops()
        .package_upload(signed)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("UPLOADS_DISABLED"), "{e}");

    // Install by name; the node that installs it verifies it first.
    let installed = alice
        .packages()
        .install("com.test.kv", "1.0.0")
        .await
        .map(wire)
        .unwrap();
    assert_eq!(installed["version"], json!("1.0.0"));
    assert_eq!(
        installed["signerId"],
        json!(calimero_bundle::derive_signer_id_did_key(
            publisher.verifying_key().as_bytes()
        ))
    );
    let created = alice
        .apps()
        .create(typed(
            json!({ "name": "kv", "package": "com.test.kv", "version": "1.0.0" }),
        ))
        .await
        .map(wire)
        .unwrap();
    let app = created["appId"].as_str().unwrap().to_owned();
    let _ = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "k", "value": "v1" })),
            None,
            None,
        )
        .await
        .unwrap();

    // Each node runs the app from its own copy, fetched from the registry
    // (never pushed by a peer) and verified again: one fetch per node.
    let session = alice.session().await.unwrap();
    for i in 0..3 {
        let cl = c.client(i);
        cl.use_session(session.clone()).await;
        let v = cl
            .apps()
            .view(
                &app,
                "get",
                Some(json!({ "key": "k" })),
                Some(ViewsRequestConsistency::Stale),
                Some(0),
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
        .packages()
        .install("com.test.kv", "1.1.0")
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
        .packages()
        .install("com.test.kv", "1.2.0")
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
        .apps()
        .upgrade(
            &app,
            typed(json!({ "version": "1.0.1" })),
            Some(&mut alice_key),
        )
        .await
        .map(wire)
        .unwrap();
    assert_eq!(up["status"], json!("ok"), "{up}");
    let listed = alice
        .apps()
        .list(Some("com.test.kv"), None)
        .await
        .map(wire)
        .unwrap();
    assert_eq!(listed["apps"][0]["version"], json!("1.0.1"), "{listed}");
    let v = alice
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
    assert_eq!(v.result, json!("v1"), "state survives the upgrade");

    // A release some node fetches later must be the one the fleet installed:
    // a registry that swaps the bytes is refused, never run.
    let v2 = variant(&kv, "two");
    registry.publish(
        "com.test.kv",
        "2.0.0",
        mpk("com.test.kv", "2.0.0", &v2, &publisher),
    );
    let _ = alice
        .packages()
        .install("com.test.kv", "2.0.0")
        .await
        .unwrap();
    let created = alice
        .apps()
        .create(typed(
            json!({ "name": "kv2", "package": "com.test.kv", "version": "2.0.0" }),
        ))
        .await
        .map(wire)
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
            .apps()
            .view(
                &app2,
                "get",
                Some(json!({ "key": "k" })),
                Some(ViewsRequestConsistency::Stale),
                Some(0),
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
        .packages()
        .version_uninstall("com.test.kv", "1.0.1")
        .await
        .unwrap_err();
    assert!(e.to_string().contains("still runs"), "{e}");
    let _ = alice
        .packages()
        .version_uninstall("com.test.kv", "1.0.0")
        .await
        .unwrap();
    let _ = alice.apps().delete(&app).await.unwrap();
    let _ = alice.apps().delete(&app2).await.unwrap();
    let e = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "k", "value": "x" })),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("404"), "{e}");
    let _ = alice.packages().uninstall("com.test.kv").await.unwrap();
    assert!(alice.packages().list().await.unwrap().packages.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_uploaded_package_is_verified_and_reaches_the_other_nodes() {
    let c = Cluster::start_with(3, |cfg| cfg.allow_unsigned_bundles = false).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let kv = kv_wasm();

    // An unsigned archive is refused.
    let unsigned = pack(&[("app.wasm", kv.clone())]);
    let e = alice.packages().upload(unsigned).await.unwrap_err();
    assert_eq!(e.code(), Some("BAD_PACKAGE"), "{e}");

    let installed = alice
        .packages()
        .upload(mpk("com.test.up", "0.1.0", &kv, &publisher))
        .await
        .map(wire)
        .unwrap();
    assert_eq!(installed["package"], json!("com.test.up"));
    let created = alice
        .apps()
        .create(typed(json!({ "name": "up", "package": "com.test.up" })))
        .await
        .map(wire)
        .unwrap();
    let app = created["appId"].as_str().unwrap().to_owned();
    let _ = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "a", "value": "b" })),
            None,
            None,
        )
        .await
        .unwrap();
    let session = alice.session().await.unwrap();
    for i in 0..3 {
        let cl = c.client(i);
        cl.use_session(session.clone()).await;
        let v = cl
            .apps()
            .view(
                &app,
                "get",
                Some(json!({ "key": "a" })),
                Some(ViewsRequestConsistency::Stale),
                Some(0),
            )
            .await
            .unwrap();
        assert_eq!(v.result, json!("b"), "node {i}");
    }
    let p = alice.packages().get("com.test.up").await.map(wire).unwrap();
    assert_eq!(p["latest"], json!("0.1.0"));
    assert_eq!(p["versions"][0]["source"], json!("upload"));
}

/// The names of the packages `cl` lists, with the versions it sees of each.
async fn listed(cl: &Client) -> BTreeMap<String, Vec<String>> {
    cl.packages()
        .list()
        .await
        .unwrap()
        .packages
        .into_iter()
        .map(|p| {
            let versions = p.versions.into_iter().map(|v| v.version).collect();
            (p.package, versions)
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
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (amy, _m) = signed_up(&c, 1, "amy").await;
    let (bob, _b) = signed_up(&c, 2, "bob").await;
    let bob_id = bob.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let acme = alice.orgs().create("Acme", None).await.map(wire).unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let invite = alice
        .orgs()
        .invite_create(&acme, typed(json!({ "role": "member" })), Some(&mut a))
        .await
        .map(wire)
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = amy.orgs().invite_redeem(&invite).await.unwrap();
    let _o = bob.orgs().create("Bobco", None).await.unwrap();

    // Alice uploads a private package (no registry has it).
    let secret = variant(&kv, "secret");
    let _ = alice
        .packages()
        .upload(mpk("com.acme.secret", "1.0.0", &secret, &publisher))
        .await
        .unwrap();
    let only_secret = BTreeMap::from([("com.acme.secret".to_owned(), vec!["1.0.0".to_owned()])]);
    assert_eq!(listed(&alice).await, only_secret);
    assert_eq!(listed(&amy).await, only_secret, "a member of Alice's org");

    // Bob, in another organization, sees nothing of it.
    assert_eq!(listed(&bob).await, BTreeMap::new());
    let e = bob.packages().get("com.acme.secret").await.unwrap_err();
    assert!(e.to_string().starts_with("404"), "{e}");
    // Approving its outbound hosts answers as for a missing package, before
    // any passkey ceremony.
    for package in ["com.acme.secret", "com.never.installed"] {
        let e = bob
            .auth()
            .account_outbound_approval(package, None, None, None)
            .await
            .unwrap_err();
        assert!(e.to_string().starts_with("404"), "{package}: {e}");
    }
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    let ops_listed = ops.ops().package_list().await.map(wire).unwrap()["packages"].clone();
    assert_eq!(ops_listed, json!([]), "the operator installed none");

    // Creating from it answers as for a release the cluster lacks.
    let named = |package: &str, version: Option<&str>| AppCreateRequest {
        name: Some("x".to_owned()),
        package: Some(package.to_owned()),
        version: version.map(str::to_owned),
        ..AppCreateRequest::default()
    };
    let hidden = bob
        .apps()
        .create(named("com.acme.secret", Some("1.0.0")))
        .await
        .unwrap_err();
    let missing = bob
        .apps()
        .create(named("com.never.installed", Some("1.0.0")))
        .await
        .unwrap_err();
    assert!(hidden.to_string().starts_with("404"), "{hidden}");
    assert_eq!(hidden.code(), missing.code(), "{hidden} / {missing}");
    let hidden = bob
        .apps()
        .create(named("com.acme.secret", None))
        .await
        .unwrap_err();
    let missing = bob
        .apps()
        .create(named("com.never.installed", None))
        .await
        .unwrap_err();
    assert_eq!(hidden.code(), Some("VERSION_REQUIRED"), "{hidden}");
    assert_eq!(hidden.code(), missing.code(), "{hidden} / {missing}");
    // Its WASM's hash is no way in either.
    let hash = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&secret));
    let by_hash = bob
        .apps()
        .create(typed(json!({ "name": "x", "bundle": hash })))
        .await
        .unwrap_err();
    let unknown = bob
        .apps()
        .create(typed(json!({ "name": "x", "bundle": "00".repeat(32) })))
        .await
        .unwrap_err();
    assert_eq!(by_hash.to_string(), unknown.to_string());
    // A failed attempt installed nothing that Bob now lists.
    assert_eq!(listed(&bob).await, BTreeMap::new());

    // Alice and Amy create apps from it.
    let _ = alice
        .apps()
        .create(named("com.acme.secret", Some("1.0.0")))
        .await
        .map(wire)
        .unwrap();
    let shared = amy
        .apps().create(typed(json!({ "name": "shared", "package": "com.acme.secret", "access": { "kind": "accounts", "accounts": [bob_id] } })))
        .await.map(wire)
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    // Once Bob reaches an app running it, he sees that release.
    assert_eq!(listed(&bob).await, only_secret, "Bob reaches {shared}");
    let _ = amy.apps().delete(&shared).await.unwrap();
    assert_eq!(listed(&bob).await, BTreeMap::new());

    // A registry release is private to its installers too; Bob installing the
    // same release himself makes it his as well.
    registry.publish(
        "com.test.kv",
        "1.0.0",
        mpk("com.test.kv", "1.0.0", &kv, &publisher),
    );
    let _ = alice
        .packages()
        .install("com.test.kv", "1.0.0")
        .await
        .unwrap();
    assert!(!listed(&bob).await.contains_key("com.test.kv"));
    let _ = bob
        .packages()
        .install("com.test.kv", "1.0.0")
        .await
        .unwrap();
    assert_eq!(
        listed(&bob).await,
        BTreeMap::from([("com.test.kv".to_owned(), vec!["1.0.0".to_owned()])])
    );
    let _ = bob
        .apps()
        .create(named("com.test.kv", Some("1.0.0")))
        .await
        .map(wire)
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
        .ops()
        .package_install("com.test.ops", "1.0.0")
        .await
        .unwrap();
    let ops_listed = ops.ops().package_list().await.map(wire).unwrap()["packages"].clone();
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
        let e = owner.auth().me().await.unwrap_err().to_string();
        assert!(e.starts_with("401"), "round {round}: {e}");
    }
}

/// Xil's review, issues 5, 6 and 7, each reproduced as the review ran it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn sessions_confirmations_and_key_scopes_hold() {
    let c = Cluster::start(3).await;
    let (laptop, mut pk) = signed_up(&c, 0, "alice").await;

    // 5. Removing a passkey ends the sessions opened with it.
    let mut phone_key = SoftAuthenticator::new("localhost", ORIGIN);
    let phone_cred = laptop.add_passkey(&mut phone_key, &mut pk).await.unwrap();
    let phone = c.client(1);
    let _s = phone.sign_in(&mut phone_key).await.unwrap();
    assert!(phone.auth().me().await.is_ok());
    let _ok = laptop
        .auth()
        .passkey_remove(&phone_cred, Some(&mut pk))
        .await
        .unwrap();
    let e = phone.auth().me().await.unwrap_err();
    assert!(e.to_string().starts_with("401"), "{e}");
    let e = phone.refresh().await.unwrap_err();
    assert!(e.to_string().starts_with("401"), "{e}");
    assert!(
        laptop.auth().me().await.is_ok(),
        "the other device stays signed in"
    );

    // 5. A refresh token used twice is a stolen copy: the session ends.
    let before = laptop.session().await.unwrap();
    let _after = laptop.refresh().await.unwrap();
    let thief = c.client(2);
    thief.use_session(before).await;
    let e = thief.refresh().await.unwrap_err();
    assert!(e.to_string().contains("already used"), "{e}");
    let e = laptop.auth().me().await.unwrap_err();
    assert!(e.to_string().starts_with("401"), "{e}");

    // 6. A confirm method (the kv fixture's `clear`) does not answer through /views.
    let (bob, mut pk) = signed_up(&c, 0, "bob").await;
    let (app, _ctx) = deploy(&bob, "kv", kv_wasm(), json!({})).await;
    let e = bob
        .apps()
        .view(
            &app,
            "clear",
            Some(json!({})),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("APP_ERROR"), "{e}");
    assert_eq!(e.details().unwrap()["code"], "ConfirmNotAView", "{e}");

    // 7. A read-only key installs nothing; an app-scoped key starts no app.
    let (_id, read_only) = bob
        .auth()
        .apikey_create(typed(json!({ "readOnly": true })), Some(&mut pk))
        .await
        .map(|k| (k.key_id, k.key))
        .unwrap();
    let svc = c.client(0);
    svc.use_api_key(&read_only).await;
    let e = svc
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap_err();
    assert!(e.to_string().starts_with("403"), "{e}");
    let (_id, scoped) = bob
        .auth()
        .apikey_create(
            typed(json!({ "apps": [app], "methods": ["set"] })),
            Some(&mut pk),
        )
        .await
        .map(|k| (k.key_id, k.key))
        .unwrap();
    let svc = c.client(0);
    svc.use_api_key(&scoped).await;
    let bundle = bob
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let e = svc
        .apps()
        .create(typed(json!({ "name": "new", "bundle": bundle })))
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
        alice.auth().me().await.map(wire).unwrap()["approvedReleases"],
        json!(["1.0.0"])
    );
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
    let mut replicas: Vec<u64> = serde_json::from_value(created["replicas"].clone()).unwrap();
    replicas.sort_unstable();
    assert_eq!(
        replicas,
        vec![1, 2],
        "the unapproved node is placed nothing"
    );
    let _ = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "k", "value": "v1" })),
            None,
            None,
        )
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
        .ops()
        .set_replicas(&app, typed(json!([1, 2, 3])), Some(&harness::operator_sign))
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("403"), "{e}");

    // An upgrade needs the owner's passkey over this upgrade.
    let e = alice
        .apps()
        .upgrade(&app, typed(json!({ "bundle": bundle })), None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    let (bob, _bob_key) = signed_up(&c, 1, "bob").await;
    let e = bob
        .apps()
        .upgrade(&app, typed(json!({ "bundle": bundle })), None)
        .await
        .unwrap_err();
    assert!(
        e.to_string().starts_with("403") || e.to_string().starts_with("404"),
        "{e}"
    );

    // The owner approves 2.0.0: now the app may move there, and node 3 serves it.
    let _ok = alice
        .auth()
        .account_release_approval("2.0.0", None, Some(&mut key))
        .await
        .map(wire)
        .unwrap();
    let leader = c.app_leader(&app).unwrap();
    let _moved = ops(leader)
        .await
        .ops()
        .set_replicas(&app, typed(json!([1, 2, 3])), Some(&harness::operator_sign))
        .await
        .unwrap();
    let session = alice.session().await.unwrap();
    let on3 = c.client(2);
    on3.use_session(session).await;
    let mut seen = None;
    for _ in 0..100 {
        if let Ok(v) = on3
            .apps()
            .view(
                &app,
                "get",
                Some(json!({ "key": "k" })),
                Some(ViewsRequestConsistency::Stale),
                Some(0),
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
        .auth()
        .account_release_approval("2.0.0", Some(false), Some(&mut key))
        .await
        .map(wire)
        .unwrap();
    let _ = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "k", "value": "v2" })),
            None,
            None,
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let v = on3
        .apps()
        .view(
            &app,
            "get",
            Some(json!({ "key": "k" })),
            Some(ViewsRequestConsistency::Stale),
            Some(0),
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
            .ops()
            .register_node(
                i as u64 + 1,
                &c.urls[i],
                Some(zone),
                None,
                None,
                Some(&harness::operator_sign),
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
        .apps()
        .create(typed(json!({ "name": name, "bundle": bundle })))
        .await
        .map(wire)
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
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();

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
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
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
    let receipt = call.receipt.clone();
    assert!(receipt.verify(&keys[&receipt.node], &call.result));

    let view = alice
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
    let receipt = view.receipt.clone();
    assert_eq!(receipt.index, view.index);
    assert!(receipt.verify(&keys[&receipt.node], &view.result));
    assert!(!receipt.verify(&keys[&receipt.node], &json!("forged")));
}

// ---------------------------------------------------------------- governance

/// Who decides releases, app upgrades and outbound hosts is chosen per
/// deployment, each one either the operator's or the owner's, and a user
/// proves which: every replica binds the governance into its receipt-key
/// quote, and a replica claiming other rules is refused.
/// The node's own commands (`__upgrade`, `__blob`, `__blob_forget`, `__blob_revoke`) are not
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
    let other = alice
        .packages()
        .bundle_put(variant(&kv, "other"))
        .await
        .map(|b| b.hash)
        .unwrap();
    let before = alice.apps().get(&public).await.map(wire).unwrap()["bundle"].clone();

    for (who, caller) in [("bob", &bob), ("the owner", &alice)] {
        let r = caller
            .apps()
            .call(
                &public,
                "__upgrade",
                Some(json!({ "bundle": other })),
                None,
                None,
            )
            .await;
        assert!(r.is_err(), "{who} upgraded through /intents: {r:?}");
    }
    assert_eq!(
        alice.apps().get(&public).await.map(wire).unwrap()["bundle"],
        before,
        "the bundle is unchanged"
    );

    // Alice's private blob, and bob's own app.
    let (private, _) = deploy(&alice, "private", kv.clone(), json!({})).await;
    let up = alice
        .apps()
        .blob_upload(&private, b"alice's".to_vec(), None)
        .await
        .map(wire)
        .unwrap();
    let id = up["id"].as_str().unwrap().to_owned();
    let (mine, _) = deploy(&bob, "mine", kv.clone(), json!({})).await;
    for method in ["__blob", "__blob_forget", "__blob_revoke"] {
        let r = bob
            .apps()
            .call(
                &mine,
                method,
                Some(json!({ "id": id, "size": 7 })),
                None,
                None,
            )
            .await;
        assert!(r.is_err(), "{method} through /intents: {r:?}");
    }
    assert!(
        bob.apps().blob_download(&mine, &id, None).await.is_err(),
        "bob cannot read alice's blob through his app"
    );
    assert!(
        bob.apps()
            .view(
                &public,
                "__upgrade",
                Some(json!({})),
                Some(ViewsRequestConsistency::Strong),
                None,
            )
            .await
            .is_err(),
        "nor as a view"
    );
}

/// A write too large for one log entry is refused at once, whichever node
/// takes it: a component's arguments past `MAX_ARGS_BYTES` are `413
/// ARGS_TOO_LARGE`, and the leader's own refusal is not a "no leader yet" to
/// retry.
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
    // One past the arguments cap (a trapped entry if proposed), one past the log's.
    let sizes = [
        calimero_node::node::MAX_ARGS_BYTES + 1024,
        calimero_log::MAX_ENTRY_BYTES + 1024,
    ];
    let logs = |c: &Cluster| -> Vec<Option<u64>> {
        c.nodes
            .iter()
            .map(|r| {
                let g = r.as_ref().unwrap().node.hosted(&app)?;
                Some(g.status().last_log.unwrap_or(0))
            })
            .collect()
    };
    // Snapshot only once every replica holds the app's log as far as its
    // leader does: a follower still appending the deploy's entries would grow
    // between this snapshot and the check below with no write proposed.
    let settled = |logs: &[Option<u64>]| {
        let held: Vec<u64> = logs.iter().flatten().copied().collect();
        !held.is_empty() && held.iter().all(|n| *n == held[0])
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !settled(&logs(&c)) && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let before = logs(&c);
    assert!(
        before.iter().any(Option::is_some),
        "some node hosts the app"
    );
    assert!(
        settled(&before),
        "the app's replicas agree on its log: {before:?}"
    );
    for i in 0..3 {
        let (cl, _) = signed_up(&c, i, &format!("user{i}")).await;
        for size in sizes {
            let started = std::time::Instant::now();
            let big = "x".repeat(size);
            let r = cl
                .apps()
                .call(
                    &app,
                    "set",
                    Some(json!({ "key": "big", "value": big })),
                    None,
                    None,
                )
                .await;
            let e = r.expect_err("an entry over the cap is refused");
            // A component's arguments are capped below the log's entry cap.
            assert_eq!(
                e.code(),
                Some("ARGS_TOO_LARGE"),
                "node {i}, {size} bytes: {e}"
            );
            // A node's first call prepares the bundle, which is slow on a loaded
            // machine; a forward that was retried would outlast this.
            assert!(
                started.elapsed() < calimero_node::node::SYSTEM_WRITE_TIMEOUT,
                "node {i} answered after {:?} ({size} bytes)",
                started.elapsed()
            );
        }
    }
    assert_eq!(logs(&c), before, "a refused write is never proposed");
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
        .ops()
        .package_upload(mpk("com.test.ops", "1.0.0", &kv, &publisher))
        .await
        .map(wire)
        .unwrap_or_else(|e| panic!("the operator installs a release: {e}"));
    assert_eq!(first["version"], "1.0.0");
    let created = alice
        .apps()
        .create(typed(json!({ "name": "kv", "package": "com.test.ops" })))
        .await
        .map(wire)
        .unwrap();
    let app = created["appId"].as_str().unwrap().to_owned();

    let _ = ops
        .ops()
        .package_upload(mpk(
            "com.test.ops",
            "1.1.0",
            &variant(&kv, "next"),
            &publisher,
        ))
        .await
        .unwrap();
    let listed = ops.ops().package_list().await.map(wire).unwrap();
    assert!(listed.to_string().contains("1.1.0"), "{listed}");
    let _ = harness::ops_signed(
        &ops,
        &format!("ops/apps/{app}/upgrade"),
        json!({ "version": "1.1.0" }),
    )
    .await
    .unwrap_or_else(|e| panic!("the operator upgrades to its release: {e}"));
    let a = alice
        .apps()
        .list(Some("com.test.ops"), None)
        .await
        .map(wire)
        .unwrap();
    assert_eq!(a["apps"][0]["version"], "1.1.0", "{a}");

    // A release an account installed stays that account's.
    let _ = alice
        .packages()
        .upload(mpk("com.test.mine", "1.0.0", &kv, &publisher))
        .await
        .map(wire)
        .unwrap();
    assert!(ops.ops().package_uninstall("com.test.mine").await.is_err());
    // Its own, unused one goes.
    let _ = ops
        .ops()
        .package_version_uninstall("com.test.ops", "1.0.0")
        .await
        .map(wire)
        .unwrap();
    // Without the token, nothing.
    assert!(c
        .client(2)
        .ops()
        .package_upload(mpk("com.test.ops", "1.2.0", &kv, &publisher))
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
            ..Governance::default()
        },
        Governance {
            releases: operator,
            app_upgrades: operator,
            outbound: operator,
            ..Governance::default()
        },
        // A managed cluster whose operator rolls out node releases, while
        // each app's owner keeps its code and its outbound hosts.
        Governance {
            releases: operator,
            app_upgrades: owner,
            outbound: owner,
            ..Governance::default()
        },
    ];
    for g in deployments {
        let op = |a: Authority| a == Authority::Operator;
        let configured = g.clone();
        let c = Cluster::start_with(3, move |cfg| cfg.governance = configured.clone()).await;
        let (alice, mut pk) = signed_up(&c, 0, "alice").await;
        let kv = kv_wasm();
        let (app, _ctx) = deploy(&alice, "kv", kv.clone(), json!({})).await;
        let next = alice
            .packages()
            .bundle_put(variant(&kv, "next"))
            .await
            .map(|b| b.hash)
            .unwrap();
        let publisher = ed25519_dalek::SigningKey::from_bytes(&[3; 32]);
        let _ = alice
            .packages()
            .upload(mpk_declaring(
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
            .apps()
            .attestation(&app, &hex::encode(nonce))
            .await
            .map(wire)
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
        let status = ops.ops().status().await.map(wire).unwrap();
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
            .apps()
            .upgrade(&app, typed(json!({ "bundle": next })), Some(&mut pk))
            .await
            .map(wire);
        assert_eq!(by_owner.is_ok(), !op(g.app_upgrades), "{g:?}: {by_owner:?}");
        let a = alice.apps().get(&app).await.map(wire).unwrap();
        assert_eq!(
            a["bundle"],
            json!(next),
            "{g:?}: upgraded by whoever holds it"
        );
        let _ = alice
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

        // Release approvals.
        let ops_release =
            harness::ops_signed(&ops, "ops/releases", json!({ "version": "9.0.0" })).await;
        assert_eq!(
            ops_release.is_ok(),
            op(g.releases),
            "{g:?}: {ops_release:?}"
        );
        let owner_release = alice
            .auth()
            .account_release_approval("9.0.0", None, Some(&mut pk))
            .await
            .map(wire);
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
            .auth()
            .account_outbound_approval(
                "com.test.gov",
                Some(typed(json!("manifest"))),
                Some(false),
                Some(&mut pk),
            )
            .await
            .map(wire);
        assert_eq!(
            owner_outbound.is_ok(),
            !op(g.outbound),
            "{g:?}: {owner_outbound:?}"
        );
    }
}

// ---------------------------------------------------------------- orgs, metadata

async fn account_id(cl: &Client) -> String {
    cl.auth().me().await.map(wire).unwrap()["accountId"]
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
    let org = alice.orgs().create("Acme", None).await.map(wire).unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ = alice
        .orgs()
        .set_member(&org, &bob_id, OrgRole::Member, Some(&mut apk))
        .await
        .unwrap();

    // Organization metadata: an admin's; a member reads it.
    let _ = alice
        .orgs()
        .metadata(&org, json!({ "logo": "acme.svg" }))
        .await
        .map(wire)
        .unwrap();
    assert!(bob
        .orgs()
        .metadata(&org, json!({ "logo": "x" }))
        .await
        .is_err());
    let o = bob.orgs().get(&org).await.map(wire).unwrap();
    assert_eq!(o["metadata"]["logo"], json!("acme.svg"));

    // Member metadata: the member's own, or an admin's; not another member's.
    let _ = bob
        .orgs()
        .member_metadata(&org, &bob_id, json!({ "title": "Engineer" }))
        .await
        .map(wire)
        .unwrap();
    assert!(bob
        .orgs()
        .member_metadata(&org, &alice_id, json!({ "title": "x" }))
        .await
        .is_err());
    let _ = alice
        .orgs()
        .member_metadata(&org, &bob_id, json!({ "title": "Lead" }))
        .await
        .unwrap();
    let o = alice.orgs().get(&org).await.map(wire).unwrap();
    assert_eq!(o["memberMetadata"][&bob_id]["title"], json!("Lead"));
    // Over 16 KiB is refused.
    let big = "x".repeat(17 * 1024);
    assert!(alice
        .orgs()
        .metadata(&org, json!({ "big": big }))
        .await
        .is_err());

    // App metadata: its owner's or an org admin's; it reads back with the app.
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let app = alice
        .apps()
        .create(typed(
            json!({ "name": "kv", "bundle": bundle, "org": org, "metadata": { "icon": "a" } }),
        ))
        .await
        .map(wire)
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        alice.apps().get(&app).await.map(wire).unwrap()["metadata"]["icon"],
        json!("a")
    );
    assert!(bob
        .apps()
        .metadata(&app, json!({ "icon": "b" }))
        .await
        .is_err());
    let _ = alice
        .apps()
        .metadata(&app, json!({ "icon": "c" }))
        .await
        .unwrap();
    assert_eq!(
        bob.apps().get(&app).await.map(wire).unwrap()["metadata"]["icon"],
        json!("c")
    );

    // Passkey labels.
    let cred = alice.auth().me().await.map(wire).unwrap()["passkeys"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ = alice.auth().passkey_label(&cred, "laptop").await.unwrap();
    assert_eq!(
        alice.auth().me().await.map(wire).unwrap()["passkeys"][0]["label"],
        json!("laptop")
    );
    assert!(bob.auth().passkey_label(&cred, "mine").await.is_err());

    // Deleting the organization: only the owner, with a passkey; its apps go.
    let invite = alice
        .orgs()
        .invite_create(&org, typed(json!({ "role": "member" })), Some(&mut apk))
        .await
        .map(wire)
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(bob.orgs().delete(&org, None).await.is_err());
    let e = alice.orgs().delete(&org, None).await.unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    let gone = alice
        .orgs()
        .delete(&org, Some(&mut apk))
        .await
        .map(wire)
        .unwrap();
    assert_eq!(gone["apps"], json!([app]));
    assert!(alice.orgs().get(&org).await.is_err());
    assert!(alice.apps().get(&app).await.is_err());
    let (carol, _cpk) = signed_up(&c, 2, "carol").await;
    assert!(carol.orgs().invite_redeem(&invite).await.is_err());
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
    let up = alice
        .apps()
        .blob_upload(&a1, bytes.clone(), None)
        .await
        .map(wire)
        .unwrap();
    let id = up["id"].as_str().unwrap().to_owned();
    let _ = alice
        .apps()
        .blob_upload(&a2, bytes.clone(), None)
        .await
        .unwrap();
    let other = alice
        .apps()
        .blob_upload(&a1, b"another".to_vec(), None)
        .await
        .map(wire)
        .unwrap();

    let listed = alice.apps().blob_list(&a1).await.map(wire).unwrap();
    assert_eq!(listed["blobs"].as_array().unwrap().len(), 2, "{listed}");
    let info = alice.apps().blob_info(&a1, &id).await.map(wire).unwrap();
    assert_eq!(info["size"], json!(bytes.len()));
    // The base58 id apps print works too.
    let b58 = up["blobId"].as_str().unwrap();
    assert_eq!(
        alice.apps().blob_info(&a1, b58).await.map(wire).unwrap()["id"],
        json!(id)
    );

    let _ = alice
        .apps()
        .call(
            &a1,
            "set",
            Some(json!({ "key": "k", "value": "v" })),
            None,
            None,
        )
        .await
        .unwrap();
    let storage = alice.apps().storage(&a1).await.map(wire).unwrap();
    assert_eq!(storage["blobs"], json!(2), "{storage}");
    assert_eq!(storage["blobBytes"], json!(bytes.len() + b"another".len()));
    assert!(storage["keys"].as_u64().unwrap() >= 1, "{storage}");
    let abi = alice.apps().abi(&a1).await.map(wire).unwrap();
    assert!(abi.get("bundle").is_some(), "{abi}");
    let usage = alice.auth().usage().await.map(wire).unwrap();
    assert_eq!(usage["apps"], json!(2), "{usage}");
    assert!(usage["quotas"]["maxAppsPerAccount"].as_u64().is_some());

    // Only the owner (or an org admin) deletes; a caller of a public app cannot.
    assert!(bob.apps().blob_delete(&a1, &id).await.is_err());
    let _ = alice.apps().blob_delete(&a1, &id).await.unwrap();
    assert!(alice.apps().blob_info(&a1, &id).await.is_err());
    assert!(alice.apps().blob_download(&a1, &id, None).await.is_err());
    // The other app holds the same bytes: they stay everywhere they were.
    assert_eq!(
        alice.apps().blob_download(&a2, &id, None).await.unwrap(),
        bytes
    );
    let held_somewhere = |c: &Cluster| {
        c.nodes
            .iter()
            .flatten()
            .filter(|r| r.node.engine.get_blob(&id).unwrap().is_some())
            .count()
    };
    assert!(held_somewhere(&c) >= 2, "a majority kept the shared bytes");

    // Once the last app lets it go, no node keeps the bytes.
    let _ = alice.apps().blob_delete(&a2, &id).await.map(wire).unwrap();
    assert_eq!(held_somewhere(&c), 0, "the bytes outlived every app");
    // The app's other blob is untouched.
    let other_id = other["id"].as_str().unwrap();
    assert_eq!(
        alice
            .apps()
            .blob_download(&a1, other_id, None)
            .await
            .unwrap(),
        b"another".to_vec()
    );
}

// ---------------------------------------------------------------- upgrade runs, aliases

async fn finished_run(cl: &Client, org: &str, run: &str) -> Value {
    for _ in 0..200 {
        let r = cl.orgs().upgrade_status(org, run).await.map(wire).unwrap();
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
        .packages()
        .upload(mpk("com.test.kv", "0.1.0", &kv, &publisher))
        .await
        .unwrap();
    let org = alice.orgs().create("Acme", None).await.map(wire).unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let team = |name: &str, parent: Option<&str>| {
        let alice = &alice;
        let org = org.clone();
        let (name, parent) = (name.to_owned(), parent.map(str::to_owned));
        async move {
            alice
                .orgs()
                .team_create(&org, &name, TeamJoin::Invite, parent.as_deref())
                .await
                .map(wire)
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
            .apps().create(typed(json!({ "name": name, "package": "com.test.kv", "org": org, "access": { "kind": "team", "team": t } })))
            .await.map(wire)
            .unwrap()["appId"]
            .as_str()
            .unwrap()
            .to_owned();
        let _prev = apps.insert(name, app);
    }
    let _ = alice
        .packages()
        .upload(mpk("com.test.kv", "0.2.0", &variant(&kv, "v2"), &publisher))
        .await
        .map(wire)
        .unwrap();

    // Eng and the teams below it, not ops.
    let run = alice
        .orgs().upgrade(&org, typed(json!({ "package": "com.test.kv", "version": "0.2.0", "team": eng, "cascade": true })), Some(&mut pk))
        .await.map(wire)
        .unwrap();
    let run_id = run["runId"].as_str().unwrap().to_owned();
    assert_eq!(run["apps"].as_array().unwrap().len(), 2, "{run}");
    let done = finished_run(&alice, &org, &run_id).await;
    assert_eq!(done["done"].as_array().unwrap().len(), 2, "{done}");
    assert_eq!(done["failed"], json!({}));
    for name in ["top", "below"] {
        let a = alice.apps().get(&apps[name]).await.map(wire).unwrap();
        assert_eq!(a["lastUpgrade"]["ok"], json!(true), "{name}: {a}");
        assert_eq!(a["lastUpgrade"]["run"], json!(run_id));
    }
    let untouched = alice
        .apps()
        .get(&apps["elsewhere"])
        .await
        .map(wire)
        .unwrap();
    assert!(untouched.get("lastUpgrade").is_none(), "{untouched}");
    // Still serving after the run.
    let _ = alice
        .apps()
        .call(
            &apps["below"],
            "set",
            Some(json!({ "key": "k", "value": "v" })),
            None,
            None,
        )
        .await
        .unwrap();

    // A failed upgrade is recorded with its reason, and a retry runs it again: a
    // component's upgrade that names a migration is refused (D38).
    let _ = alice
        // A bundle of its own: an app's package is the first release naming its bundle.
        .packages().upload(mpk(
            "com.test.checked",
            "1.0.0",
            &variant(&kv, "checked-v1"),
            &publisher,
        ))
        .await.map(wire)
        .unwrap();
    let checked = alice
        .apps()
        .create(typed(
            json!({ "name": "checked", "package": "com.test.checked", "org": org }),
        ))
        .await
        .map(wire)
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    for k in ["x", "y"] {
        let _ = alice
            .apps()
            .call(
                &checked,
                "set",
                Some(json!({ "key": k, "value": k })),
                None,
                None,
            )
            .await
            .unwrap();
    }
    let _ = alice
        .packages()
        .upload(mpk(
            "com.test.checked",
            "2.0.0",
            &variant(&kv, "checked-v2"),
            &publisher,
        ))
        .await
        .map(wire)
        .unwrap();
    let run = alice
        .orgs().upgrade(&org, typed(json!({ "package": "com.test.checked", "version": "2.0.0", "migrate": "migrate_v1" })), Some(&mut pk))
        .await.map(wire)
        .unwrap();
    let failed = finished_run(&alice, &org, run["runId"].as_str().unwrap()).await;
    assert!(failed["failed"].get(&checked).is_some(), "{failed}");
    let a = alice.apps().get(&checked).await.map(wire).unwrap();
    assert_eq!(a["lastUpgrade"]["ok"], json!(false), "{a}");
    assert_ne!(a["lastUpgrade"]["error"], Value::Null, "{a}");
    let retry = alice
        .orgs()
        .upgrade_retry(&org, run["runId"].as_str().unwrap(), Some(&mut pk), None)
        .await
        .map(wire)
        .unwrap();
    assert_eq!(retry["apps"], json!([checked]), "{retry}");
    let again = finished_run(&alice, &org, retry["runId"].as_str().unwrap()).await;
    // A run keeps no migration name, so its retry is the code-only upgrade, which a
    // component takes.
    assert_eq!(again["done"], json!([checked]), "{again}");

    // Aliases: this account's names for apps.
    let _ = alice
        .auth()
        .alias_set("main", &apps["top"])
        .await
        .map(wire)
        .unwrap();
    assert_eq!(
        alice.auth().alias_get("main").await.map(wire).unwrap()["appId"],
        json!(apps["top"])
    );
    assert_eq!(
        alice.auth().alias_list().await.map(wire).unwrap()["aliases"]["main"],
        json!(apps["top"])
    );
    let (bob, _bpk) = signed_up(&c, 1, "bob").await;
    assert!(
        bob.auth().alias_get("main").await.is_err(),
        "aliases are per account"
    );
    assert!(
        bob.auth().alias_set("x", &apps["top"]).await.is_err(),
        "an app bob cannot reach"
    );
    let _ = alice.auth().alias_delete("main").await.unwrap();
    assert!(alice.auth().alias_get("main").await.is_err());
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
        .apps()
        .presence_publish(&app, 1, Some(Some(hex::encode(b"cursor:3")).as_deref()))
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
        if carol.apps().presence_get(&app).await.map(wire).unwrap()["present"]
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
        .apps()
        .presence_publish(&app, 0, Some(Some(hex::encode(b"stale")).as_deref()))
        .await
        .map(wire)
        .unwrap();
    let _ = alice
        .apps()
        .presence_publish(&app, 2, Some(None))
        .await
        .unwrap();
    let (_, _, d) = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d["state"], Value::Null, "{d}");
    assert_eq!(
        bob.apps().presence_get(&app).await.map(wire).unwrap()["present"],
        json!([])
    );

    // Heartbeats keep an entry alive past the TTL on every node; silence ends it.
    let _ = alice
        .apps()
        .presence_publish(&app, 3, Some(Some(hex::encode(b"here")).as_deref()))
        .await
        .map(wire)
        .unwrap();
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let _ = alice
            .apps()
            .presence_publish(&app, 3, Some(Some(hex::encode(b"here")).as_deref()))
            .await
            .unwrap();
    }
    for cl in [&bob, &carol] {
        assert_eq!(
            cl.apps().presence_get(&app).await.map(wire).unwrap()["present"]
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
        assert_eq!(
            cl.apps().presence_get(&app).await.map(wire).unwrap()["present"],
            json!([])
        );
    }
}

// ---------------------------------------------------------------- app frontends

/// App frontends: each package's frontend on its own origin (and on its
/// cluster's host for it, `cluster_label`), served from the
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
        .packages()
        .upload(mpk_full(
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
        .packages()
        .upload(mpk(
            "com.test.other",
            "0.1.0",
            &variant(&kv_wasm(), "o"),
            &publisher,
        ))
        .await
        .unwrap();
    let other = alice
        .apps()
        .create(typed(
            json!({ "name": "other", "package": "com.test.other", "access": { "kind": "public" } }),
        ))
        .await
        .map(wire)
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
    assert!(
        csp.contains("https://cloud.calimero.network"),
        "the attestation mirror is reachable, or no page verifies a TDX node: {csp}"
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
        .apps()
        .create(typed(json!({ "name": "mine", "package": "com.test.site" })))
        .await
        .map(wire)
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ = carol
        .apps()
        .call(
            &mine,
            "set",
            Some(json!({ "key": "k", "value": "v" })),
            None,
            None,
        )
        .await
        .unwrap();
    // Not another package's app, even a public one; not the account's keys.
    assert!(carol
        .apps()
        .call(
            &other,
            "set",
            Some(json!({ "key": "k", "value": "v" })),
            None,
            None
        )
        .await
        .is_err());
    assert!(carol.apps().get(&other).await.is_err());
    assert!(carol
        .apps()
        .create(typed(json!({ "name": "x", "package": "com.test.other" })))
        .await
        .is_err());
    assert!(carol
        .auth()
        .apikey_create(typed(json!({ "label": "k" })), Some(&mut carol_key))
        .await
        .map(|k| (k.key_id, k.key))
        .is_err());
    let listed = carol.apps().list(None, None).await.map(wire).unwrap();
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
    assert!(carol
        .auth()
        .apikey_create(typed(json!({ "label": "k" })), Some(&mut carol_key))
        .await
        .map(|k| (k.key_id, k.key))
        .is_err());
    assert!(carol
        .apps()
        .call(
            &other,
            "set",
            Some(json!({ "key": "k", "value": "v" })),
            None,
            None
        )
        .await
        .is_err());

    // The package's host on this cluster, `cluster_label`, is the same site
    // and the same origin of the package; one made for another cluster is not.
    let cluster_host = format!(
        "{}.localhost",
        calimero_node::app_host::cluster_label(&c.name, "com.test.site")
    );
    let cluster_origin = format!("http://{cluster_host}:{port}");
    let on_cluster = reqwest::Client::builder()
        .resolve(&cluster_host, ([127, 0, 0, 1], port).into())
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let root = on_cluster
        .get(format!("{cluster_origin}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(root.headers()["location"], "/0.1.0/");
    let page = on_cluster
        .get(format!("{cluster_origin}/0.1.0/"))
        .send()
        .await
        .unwrap();
    assert_eq!(page.bytes().await.unwrap().as_ref(), html);
    let known: Value = on_cluster
        .get(format!("{cluster_origin}/.well-known/calimero.json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(known["package"], json!("com.test.site"));
    let elsewhere = format!(
        "{}.localhost",
        calimero_node::app_host::cluster_label("another", "com.test.site")
    );
    let r = reqwest::Client::builder()
        .resolve(&elsewhere, ([127, 0, 0, 1], port).into())
        .build()
        .unwrap()
        .get(format!("http://{elsewhere}:{port}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404, "another cluster's host is not served here");
    let carol_there = c.client(0);
    let mut there_key = SoftAuthenticator::with_key(&carol_key, "localhost", &cluster_origin);
    let _s = carol_there.sign_in(&mut there_key).await.unwrap();
    let _ = carol_there
        .apps()
        .call(
            &mine,
            "set",
            Some(json!({ "key": "k", "value": "w" })),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(carol_there
        .apps()
        .call(
            &other,
            "set",
            Some(json!({ "key": "k", "value": "v" })),
            None,
            None
        )
        .await
        .is_err());
    assert!(carol_there
        .auth()
        .apikey_create(typed(json!({ "label": "k" })), Some(&mut there_key))
        .await
        .map(|k| (k.key_id, k.key))
        .is_err());

    // The same person on the cluster's own origin is not bound.
    let carol_home = c.client(2);
    let mut home_key = SoftAuthenticator::with_key(&there_key, "localhost", ORIGIN);
    let _s = carol_home.sign_in(&mut home_key).await.unwrap();
    let _ = carol_home
        .apps()
        .call(
            &other,
            "set",
            Some(json!({ "key": "k", "value": "v" })),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(carol_home
        .auth()
        .apikey_create(typed(json!({ "label": "k" })), Some(&mut home_key))
        .await
        .map(|k| (k.key_id, k.key))
        .is_ok());
}

/// A session opened on a package's frontend manages nothing of the account:
/// no organization, team, member or invite, and not even its own package's
/// apps' lifetime or access. The same person on the cluster's origin can.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_frontend_session_manages_no_organization_and_no_app() {
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, mut home_key) = signed_up(&c, 0, "alice").await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[6; 32]);
    let html: &[u8] = b"<!doctype html><html><body></body></html>";
    let _ = alice
        .packages()
        .upload(mpk_full(
            "com.test.site",
            "0.1.0",
            &kv_wasm(),
            &publisher,
            &[],
            &[("frontend/index.html", html)],
            None,
        ))
        .await
        .unwrap();
    let app = alice
        .apps()
        .create(typed(json!({ "name": "mine", "package": "com.test.site" })))
        .await
        .map(wire)
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    let org = alice.orgs().create("Acme", None).await.map(wire).unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let token = alice
        .orgs()
        .invite_create(
            &org,
            typed(json!({ "role": "member" })),
            Some(&mut home_key),
        )
        .await
        .map(wire)
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let bob_id = bob.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();

    // Alice signed in on the package's frontend origin: a bound session.
    let label = calimero_node::app_host::label("com.test.site");
    let port = url::Url::parse(&c.urls[0]).unwrap().port().unwrap();
    let origin = format!("http://{label}.localhost:{port}");
    let site = c.client(0);
    let mut site_key = SoftAuthenticator::with_key(&home_key, "localhost", &origin);
    let _s = site.sign_in(&mut site_key).await.unwrap();
    let _ = site
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

    let forbidden = |what: &str, r: Result<Value, Error>| {
        let e = r.expect_err(what);
        assert!(matches!(e, Error::Api { status: 403, .. }), "{what}: {e}");
    };
    forbidden(
        "create an organization",
        site.orgs().create("Evil", None).await.map(wire),
    );
    forbidden(
        "add a member",
        site.orgs()
            .set_member(&org, &bob_id, OrgRole::Admin, None)
            .await
            .map(wire),
    );
    forbidden(
        "remove a member",
        site.orgs()
            .remove_member(&org, &bob_id, None)
            .await
            .map(wire),
    );
    forbidden(
        "create a team",
        site.orgs()
            .team_create(&org, "t", TeamJoin::Invite, None)
            .await
            .map(wire),
    );
    forbidden(
        "set capabilities",
        site.orgs()
            .capabilities(&org, 0, None, None, None)
            .await
            .map(wire),
    );
    forbidden(
        "set a member's rows in several teams",
        site.orgs()
            .member_set_teams(&org, &bob_id, typed(json!([{ "team": "x" }])), None)
            .await
            .map(wire),
    );
    forbidden(
        "rename the organization",
        site.orgs().rename(&org, "Evil", None).await.map(wire),
    );
    forbidden(
        "give a member every app added later",
        site.orgs()
            .all_apps(&org, &bob_id, true, None)
            .await
            .map(wire),
    );
    forbidden(
        "mint an invite",
        site.orgs()
            .invite_create(&org, typed(json!({ "role": "member" })), None)
            .await
            .map(wire),
    );
    forbidden(
        "redeem an invite",
        site.orgs().invite_redeem(&token).await.map(wire),
    );
    forbidden(
        "change an app's access",
        site.apps()
            .access(&app, typed(json!({ "kind": "public" })))
            .await
            .map(wire),
    );
    forbidden("delete an app", site.apps().delete(&app).await.map(wire));
    let still = alice.apps().get(&app).await.map(wire).unwrap();
    assert_ne!(still["access"]["kind"], json!("public"), "{still}");

    // On the cluster's own origin the same account still can.
    let _ = alice
        .apps()
        .access(&app, typed(json!({ "kind": "public" })))
        .await
        .map(wire)
        .unwrap();
    let _ = alice.apps().delete(&app).await.unwrap();
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
    let new = c.join().await;
    let new_id = new as u64 + 1;
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    let _ = ops
        .ops()
        .register_node(
            new_id,
            &c.urls[new],
            Some("b"),
            None,
            None,
            Some(&harness::operator_sign),
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
    let replicas: Vec<u64> = alice.apps().get(&app).await.map(wire).unwrap()["replicas"]
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
        .ops()
        .set_replicas(&app, typed(json!(new_set)), Some(&harness::operator_sign))
        .await
        .unwrap();
    let session = alice.session().await.unwrap();
    let cl = c.client(new);
    cl.use_session(session).await;
    let mut served = Value::Null;
    for _ in 0..100 {
        if let Ok(v) = cl
            .apps()
            .view(
                &app,
                "get",
                Some(json!({ "key": "a" })),
                Some(ViewsRequestConsistency::Stale),
                Some(0),
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
        .ops()
        .register_node(
            new_id,
            &c.urls[new],
            Some("b"),
            None,
            None,
            Some(&harness::operator_sign),
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
    let before = alice.apps().get(&app).await.map(wire).unwrap()["replicas"].clone();
    let outside = c.client(new);
    outside.use_api_key("ops-secret").await;
    let e = outside
        .ops()
        .set_replicas(
            &app,
            typed(json!([2, 3, new_id])),
            Some(&harness::operator_sign),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NOT_LEADER"), "{e}");
    assert!(e.details().unwrap()["leaderUrl"].is_string(), "{e}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let after = alice.apps().get(&app).await.map(wire).unwrap()["replicas"].clone();
    assert_eq!(before, after, "nothing was written");
    assert!(joined.hosted(&app).is_none(), "no empty group was started");
}

/// Every node advertises the newest cluster version it supports
/// (`/health`), the system leader logs it once all of them do (`/ops/placement`,
/// and every replica's system state), and a node that joins later on the same
/// build is admitted at it (`system::cluster_version`).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_cluster_version_rises_to_what_every_node_supports() {
    let mut c = Cluster::start(3).await;
    let supported = calimero_node::system::cluster_version::supported();
    for url in &c.urls {
        let health: Value = reqwest::get(format!("{url}health"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(health["clusterVersion"], supported, "{health}");
    }
    let ops = ops_at(&c, 0).await;
    let logged_everywhere = |c: &Cluster| {
        c.nodes.iter().flatten().all(|n| {
            n.node
                .sys(|s| s.cluster_version.as_deref() == Some(supported))
        })
    };
    let deadline = tokio::time::Instant::now() + EVENTUALLY_DEADLINE;
    let mut placement = Value::Null;
    while tokio::time::Instant::now() < deadline {
        placement = ops.ops().placement().await.map(wire).unwrap();
        if placement["clusterVersion"] == supported && logged_everywhere(&c) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    if placement["clusterVersion"] != supported || !logged_everywhere(&c) {
        // What the system leader would judge by, to tell a node that never
        // answered from an entry that was refused.
        let mut seen = Vec::new();
        for (i, n) in c.nodes.iter().enumerate() {
            if let Some(n) = n {
                let logged = n.node.sys(|s| s.cluster_version.clone());
                let advertised = n.node.advertised_versions().await;
                // One raise from here, to say where it stops.
                let raised = n.node.raise_cluster_version_once().await;
                seen.push(format!(
                    "node {i}: logged {logged:?}, advertised {advertised:?}, a raise from it: {raised:?}"
                ));
            }
        }
        panic!(
            "the cluster version was not raised: {placement}\n{}",
            seen.join("\n")
        );
    }

    // A node on this build supports it, so it joins and learns it.
    let new = c.join().await;
    let joined = &c.nodes[new].as_ref().unwrap().node;
    let deadline = tokio::time::Instant::now() + EVENTUALLY_DEADLINE;
    while tokio::time::Instant::now() < deadline
        && joined.sys(|s| s.cluster_version.as_deref() != Some(supported))
    {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        joined.sys(|s| s.cluster_version.clone()).as_deref(),
        Some(supported),
        "the new node never joined the system group"
    );
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
        .ops()
        .register_node(
            new_id,
            &c.urls[new],
            Some("b"),
            None,
            None,
            Some(&harness::operator_sign),
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
        .ops()
        .remove_node(&gone_id.to_string(), Some(&harness::operator_sign))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("replica"), "{e}");
    let e = ops(gone)
        .await
        .ops()
        .remove_node(&gone_id.to_string(), Some(&harness::operator_sign))
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("421"), "{e}");

    // Move the app off it, then remove it.
    let keep: Vec<u64> = (1..=new_id).filter(|id| *id != gone_id).collect();
    let app_leader = c.app_leader(&app).unwrap();
    let _ = ops(app_leader)
        .await
        .ops()
        .set_replicas(&app, typed(json!(keep)), Some(&harness::operator_sign))
        .await
        .unwrap();
    let _ = ops(leader)
        .await
        .ops()
        .remove_node(&gone_id.to_string(), Some(&harness::operator_sign))
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

/// A client of node `i` holding the operator's token.
async fn ops_at(c: &Cluster, i: usize) -> Client {
    let cl = c.client(i);
    cl.use_api_key("ops-secret").await;
    cl
}

/// The node ids `cl` sees in `ops/placement` for `app`, and each node's app count.
async fn placement(cl: &Client, app: &str) -> (Vec<u64>, BTreeMap<u64, u64>) {
    let p = cl.ops().placement().await.map(wire).unwrap();
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
    // A blob, held by node 1 alone for now.
    let blob = alice
        .apps()
        .blob_upload(&app, b"held by one node".to_vec(), None)
        .await
        .map(wire)
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
        .ops()
        .register_node(
            2,
            &c.urls[two],
            Some("b"),
            None,
            None,
            Some(&harness::operator_sign),
        )
        .await
        .map(wire)
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
        .ops()
        .register_node(
            3,
            &c.urls[three],
            Some("c"),
            None,
            None,
            Some(&harness::operator_sign),
        )
        .await
        .map(wire)
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
        .ops()
        .set_replicas(&app, typed(json!([1, 2, 3])), Some(&harness::operator_sign))
        .await
        .map(wire)
        .unwrap();
    assert_eq!(moved["status"]["voters"], json!([1, 2, 3]), "{moved}");
    let again = ops_at(&c, leader)
        .await
        .ops()
        .set_replicas(&app, typed(json!([3, 2, 1])), Some(&harness::operator_sign))
        .await
        .map(wire)
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
            .apps()
            .call(
                &app,
                "set",
                Some(json!({ "key": "b", "value": "2" })),
                None,
                None,
            )
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
        alice2
            .apps()
            .blob_download(&app, &blob, None)
            .await
            .unwrap(),
        b"held by one node".to_vec()
    );
    // Through a node that did not take the write (the kv fixture's `get` is not
    // `#[app::public]`, so the reader signs in).
    let (reader, _r) = signed_up(&c, 2, "reader").await;
    let v = reader
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
    assert_eq!(v.result, json!("2"), "a read through another node");

    // Node 4 joins: voters stay 3, and it shows in the placement with no apps.
    let four = c.join().await;
    let _ = ops_at(&c, 1)
        .await
        .ops()
        .register_node(
            4,
            &c.urls[four],
            Some("b"),
            None,
            None,
            Some(&harness::operator_sign),
        )
        .await
        .map(wire)
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
    let p = ops_at(&c, 2)
        .await
        .ops()
        .placement()
        .await
        .map(wire)
        .unwrap();
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
            .ops()
            .set_replicas(&app, typed(json!([2, 3, 4])), Some(&harness::operator_sign))
            .await
            .map(wire)
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
    // The kv fixture's `get` is not `#[app::public]`: read as the reader.
    let on_four = c.client(four);
    on_four.use_session(reader.session().await.unwrap()).await;
    let mut served = Value::Null;
    for _ in 0..100 {
        if let Ok(v) = on_four
            .apps()
            .view(
                &app,
                "get",
                Some(json!({ "key": "a" })),
                Some(ViewsRequestConsistency::Stale),
                Some(0),
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
        .ops()
        .register_node(
            4,
            &c.urls[new],
            None,
            None,
            None,
            Some(&harness::operator_sign),
        )
        .await
        .map(wire)
        .unwrap();
    let leader = c.app_leader(&app).unwrap();
    let lead = ops_at(&c, leader).await;

    let same = lead
        .ops()
        .set_replicas(&app, typed(json!([1, 2, 3])), Some(&harness::operator_sign))
        .await
        .map(wire)
        .unwrap();
    assert_eq!(same["unchanged"], json!(true), "{same}");

    // Two moves at once, both keeping the leader: one runs, one is refused.
    let me = leader as u64 + 1;
    let others: Vec<u64> = (1..=3).filter(|id| *id != me).collect();
    let second = ops_at(&c, leader).await;
    let (ra, rb) = tokio::join!(
        lead.ops()
            .set_replicas(&app, vec![me, others[0], 4], Some(&harness::operator_sign)),
        second
            .ops()
            .set_replicas(&app, vec![me, others[1], 4], Some(&harness::operator_sign))
    );
    let (ra, rb) = (ra.map(wire), rb.map(wire));
    let (won, lost) = match (ra, rb) {
        (Ok(w), Err(l)) | (Err(l), Ok(w)) => (w, l),
        other => panic!("exactly one move runs: {other:?}"),
    };
    assert_eq!(lost.code(), Some("MOVE_IN_PROGRESS"), "{lost}");
    let voters = won["status"]["voters"].clone();
    let recorded = alice.apps().get(&app).await.map(wire).unwrap()["replicas"].clone();
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
/// root is its own, so their K_r is not the founders'. The app's state and a
/// backup taken before the upgrade must still open on the new nodes.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn nodes_on_the_next_release_keep_the_data_of_the_ones_before() {
    let mut c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let (kv, _) = deploy(&alice, "kv", kv_wasm(), json!({})).await;
    let _ = alice
        .apps()
        .call(
            &kv,
            "set",
            Some(json!({ "key": "a", "value": "1" })),
            None,
            None,
        )
        .await
        .unwrap();
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
                .sys(|s| s.apps.contains_key(&kv))
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
    for app in [&kv] {
        let mut moved = None;
        for _ in 0..100 {
            if let Some(leader) = c.app_leader(app) {
                match ops_at(&c, leader)
                    .await
                    .ops()
                    .set_replicas(app, typed(json!(next_ids)), Some(&harness::operator_sign))
                    .await
                    .map(wire)
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
                    .ops()
                    .remove_node(&(gone + 1).to_string(), Some(&harness::operator_sign))
                    .await
                    .map(wire)
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
        .transport()
        .json(
            calimero_client::Request::new(Method::POST, "internal/cluster-key").json(ask(&name, 2)),
        )
        .await
        .unwrap_err();
    assert!(e.to_string().contains("could not be delivered"), "{e}");
    assert!(!e.to_string().contains(&hex::encode(
        c.nodes[0].as_ref().unwrap().node.cluster_key()
    )));
    let e = cl
        .transport()
        .json(
            calimero_client::Request::new(Method::POST, "internal/cluster-key").json(ask(&name, 9)),
        )
        .await
        .unwrap_err();
    assert!(e.to_string().contains("not a node of this cluster"), "{e}");
    let e = cl
        .transport()
        .json(
            calimero_client::Request::new(Method::POST, "internal/cluster-key")
                .json(ask("elsewhere", 2)),
        )
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
        .ops()
        .remove_node(&new_id.to_string(), Some(&harness::operator_sign))
        .await
        .map(wire)
        .unwrap();
    let e = c
        .client(leader)
        .transport()
        .json(
            calimero_client::Request::new(Method::POST, "internal/cluster-key")
                .json(ask(&name, new_id)),
        )
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

/// Once the cluster is founded, its directory says which nodes hold K_c: an id
/// that only a node's own `--peers` lists (not registered, not a member) is
/// not one, whatever address that list gives for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_cluster_key_goes_only_to_registered_nodes_after_genesis() {
    use calimero_node::cluster_key::Request;

    let elsewhere = format!("http://127.0.0.1:{}/", free_port());
    let c = Cluster::start_with(3, |config| {
        if config.node_id == 3 {
            let _prev = config.peers.insert(9, elsewhere.clone());
        }
    })
    .await;
    let ask = serde_json::to_value(Request {
        cluster: c.name.clone(),
        node: 9,
        token: hex::encode([1_u8; 32]),
    })
    .unwrap();
    let e = c
        .client(2)
        .transport()
        .json(calimero_client::Request::new(Method::POST, "internal/cluster-key").json(ask))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("not a node of this cluster"), "{e}");
}

/// Ownership transfer: the owner hands an app to another account
/// with a passkey assertion; the new owner then decides, the old one no longer.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_owner_transfers_an_app_with_their_passkey() {
    let c = Cluster::start(3).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let bob_id = bob.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let (app, _) = deploy(&alice, "kv", kv_wasm(), json!({})).await;
    let e = alice
        .apps()
        .transfer(&app, &bob_id, None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    let e = bob.apps().transfer(&app, &bob_id, None).await.unwrap_err();
    assert!(!e.to_string().contains("CONFIRM_REQUIRED"), "{e}");
    alice
        .apps()
        .transfer(&app, &bob_id, Some(&mut alice_pk))
        .await
        .unwrap();
    let a = bob.apps().get(&app).await.map(wire).unwrap();
    assert_eq!(a["owner"], json!(bob_id), "{a}");
    assert!(alice.apps().delete(&app).await.is_err());
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
    assert!(ops.ops().status().await.is_ok());
    let wrong = c.client(0);
    wrong.use_api_key(&hash).await;
    assert!(
        wrong.ops().status().await.is_err(),
        "the hash is not the credential"
    );
    wrong.use_api_key("ops-secret").await;
    assert!(wrong.ops().status().await.is_err());

    let metrics = ops.ops().metrics().await.unwrap();
    assert!(metrics.contains("calimero_apps 1"), "{metrics}");
    assert!(metrics.contains("calimero_accounts 1"), "{metrics}");
    assert!(metrics.contains("calimero_system_voters 3"), "{metrics}");
    assert!(
        metrics.contains("# TYPE calimero_hosted_groups gauge"),
        "{metrics}"
    );
    assert!(alice.ops().metrics().await.is_err(), "operators only");
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
/// with the app's id, its payer (the organization, or `account:{owner}` for a
/// personal app) and its developer, the payer for an app made from a release
/// directly (`tests/deployments.rs` has an instance of a deployment). State
/// bytes are a gauge every replica reports alike.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn app_usage_is_counted_on_the_node_that_ran_the_wasm() {
    let c = Cluster::start_with(3, |cfg| {
        cfg.metrics_listen = Some("127.0.0.1:0".parse().unwrap());
    })
    .await;
    let (alice, _pk) = signed_up(&c, 0, "alice-the-billed").await;
    let alice_id = alice.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let org = alice.orgs().create("Acme", None).await.map(wire).unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let (app, _) = deploy(&alice, "kv", kv_wasm(), json!({ "org": org })).await;
    let (personal, _) = deploy(&alice, "mine", kv_wasm(), json!({})).await;
    for i in 0..3 {
        let _ = alice
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
    for _ in 0..2 {
        let v = alice
            .apps()
            .view(
                &app,
                "get",
                Some(json!({ "key": "k0" })),
                Some(ViewsRequestConsistency::Strong),
                None,
            )
            .await
            .unwrap();
        assert_eq!(v.result, json!("v"));
    }
    let _ = alice
        .apps()
        .call(
            &personal,
            "set",
            Some(json!({ "key": "a", "value": "b" })),
            None,
            None,
        )
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
    let in_org = format!("app=\"{app}\",org=\"{org}\",developer=\"{org}\"");
    let mine =
        format!("app=\"{personal}\",org=\"account:{alice_id}\",developer=\"account:{alice_id}\"");
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
        .packages()
        .upload(mpk("com.test.client", "1.0.0", &kv_wasm(), &publisher))
        .await
        .unwrap();
    let app = alice
        .apps()
        .create(typed(
            json!({ "name": "kv", "package": "com.test.client", "version": "1.0.0" }),
        ))
        .await
        .map(|a| a.app_id)
        .unwrap();
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

    let org = alice.orgs().create("doomed", None).await.map(wire).unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let refused = alice.orgs().delete(&org, None).await.unwrap_err();
    assert_eq!(refused.code(), Some("CONFIRM_REQUIRED"));
    let _ = alice.orgs().delete(&org, Some(&mut pk)).await.unwrap();
    let status = |e: calimero_client::Error| match e {
        calimero_client::Error::Api { status, .. } => status,
        calimero_client::Error::WrongNode { .. } => 421,
        calimero_client::Error::Transport(e) => panic!("{e}"),
    };
    assert_eq!(status(alice.orgs().get(&org).await.unwrap_err()), 404);

    let laptop = c.client(1);
    let _s = laptop.sign_in(&mut pk).await.unwrap();
    let _ = alice.auth().logout_all().await.unwrap();
    assert_eq!(status(laptop.auth().me().await.unwrap_err()), 401);
}

/// D70: the client gives every call a key of its own (resent unchanged on a
/// 503), so two calls made without one are two calls, never one replayed.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn calls_without_a_key_each_run_once() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 1, "alice").await;
    let (app, _) = deploy(&alice, "kv", kv_wasm(), json!({})).await;
    let a = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "k", "value": "1" })),
            None,
            None,
        )
        .await
        .unwrap();
    let b = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "k", "value": "1" })),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(
        b.index > a.index,
        "the second call ran: {} then {}",
        a.index,
        b.index
    );
    let again = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "k", "value": "2" })),
            Some("mine"),
            None,
        )
        .await
        .unwrap();
    let replay = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "k", "value": "3" })),
            Some("mine"),
            None,
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
    let bob_id = bob.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
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
    for round in 0..20 {
        let _ok = alice
            .apps()
            .access(&app, typed(json!({ "kind": "owner" })))
            .await
            .unwrap();
        let refused = bob
            .apps()
            .call(
                &app,
                "set",
                Some(json!({ "key": "k", "value": "no" })),
                None,
                None,
            )
            .await
            .expect_err("a revoked account is refused at once");
        assert_eq!(
            refused.code(),
            Some("NOT_FOUND"),
            "round {round}: {refused}"
        );
        let _ok = alice
            .apps()
            .access(
                &app,
                typed(json!({ "kind": "accounts", "accounts": [bob_id] })),
            )
            .await
            .unwrap();
        let _r = bob
            .apps()
            .call(
                &app,
                "set",
                Some(json!({ "key": "k", "value": "yes" })),
                None,
                None,
            )
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
    let alice_id = alice.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let (app, _) = deploy(&alice, "kv", kv_wasm(), json!({})).await;
    for i in 0..3 {
        let _ = alice
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
    let refused = alice.apps().get("no-such-app").await.expect_err("404");
    assert_eq!(refused.code(), Some("NOT_FOUND"), "{refused}");

    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    let metrics = ops.ops().metrics().await.unwrap();
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

/// How long WASM runs take, by kind, and how a logged call's time splits
/// between its run and its commit, on `/metrics`. Labels are kinds and phases,
/// never the app or the method.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wasm_runs_and_a_calls_run_and_commit_are_timed() {
    let c = Cluster::start_with(3, |cfg| {
        cfg.metrics_listen = Some("127.0.0.1:0".parse().unwrap());
    })
    .await;
    let (alice, _pk) = signed_up(&c, 0, "alice-the-timed").await;
    let (app, _) = deploy(&alice, "kv", kv_wasm(), json!({})).await;
    let _ = alice
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
    let v = alice
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
    assert_eq!(v.result, json!("v"));

    // The registry is the process's: any node's text holds every node's runs.
    let addr = c.nodes[0].as_ref().unwrap().metrics_addr.unwrap();
    let text = reqwest::Client::new()
        .get(format!("http://{addr}/metrics"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(text.contains("# TYPE calimero_wasm_run_duration_seconds histogram"));
    assert!(text.contains("# TYPE calimero_call_phase_duration_seconds histogram"));
    for kind in ["init", "call", "view"] {
        let labels = format!("kind=\"{kind}\"");
        let n = sample(&text, "calimero_wasm_run_duration_seconds_count", &labels);
        assert!(n.is_some_and(|n| n >= 1.0), "{kind}: {n:?} in\n{text}");
        let all = sample(
            &text,
            "calimero_wasm_run_duration_seconds_bucket",
            &format!("{labels},le=\"+Inf\""),
        );
        assert_eq!(all, n, "{kind}");
        let sum = sample(&text, "calimero_wasm_run_duration_seconds_sum", &labels);
        assert!(sum.is_some_and(|s| s > 0.0), "{kind}: {sum:?}");
    }
    for phase in ["run", "commit"] {
        let labels = format!("phase=\"{phase}\"");
        let n = sample(&text, "calimero_call_phase_duration_seconds_count", &labels);
        assert!(n.is_some_and(|n| n >= 1.0), "{phase}: {n:?} in\n{text}");
        let sum = sample(&text, "calimero_call_phase_duration_seconds_sum", &labels);
        assert!(sum.is_some_and(|s| s > 0.0), "{phase}: {sum:?}");
    }
    for line in text
        .lines()
        .filter(|l| l.starts_with("calimero_wasm_run_") || l.starts_with("calimero_call_phase_"))
    {
        for secret in [app.as_str(), "set", "get"] {
            assert!(!line.contains(secret), "{secret} in {line}");
        }
    }
}

// ---------------------------------------------------------------- release governance

const APPROVE_RELEASES: u32 = 1 << 10;

/// An account's id, from `/auth/me`.
async fn id_of(cl: &Client) -> String {
    cl.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// `who` joins `org` at `role` through an invite `inviter` mints with its
/// passkey `pk`.
async fn join(
    (inviter, pk): (&Client, &mut SoftAuthenticator),
    who: &Client,
    org: &str,
    role: &str,
) {
    let token = inviter
        .orgs()
        .invite_create(org, typed(json!({ "role": role })), Some(&mut *pk))
        .await
        .map(wire)
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = who.orgs().invite_redeem(&token).await.unwrap();
}

async fn new_org(cl: &Client, name: &str) -> String {
    cl.orgs().create(name, None).await.map(wire).unwrap()["orgId"]
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
    join((&alice, &mut alice_pk), &bob, &org, "member").await;
    join((&alice, &mut alice_pk), &carol, &org, "admin").await;
    let bob_id = id_of(&bob).await;

    let e = bob
        .orgs()
        .release_approval(&org, "9.0.0", None, Some(&mut bob_pk))
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("403"), "{e}");
    assert!(e.to_string().contains("APPROVE_RELEASES"), "{e}");

    let _ok = alice
        .orgs()
        .capabilities(
            &org,
            u64::from(APPROVE_RELEASES),
            Some(&bob_id),
            None,
            Some(&mut alice_pk),
        )
        .await
        .unwrap();
    let _ok = bob
        .orgs()
        .release_approval(&org, "9.0.0", None, Some(&mut bob_pk))
        .await
        .unwrap();
    let _ok = carol
        .orgs()
        .release_approval(&org, "9.1.0", None, Some(&mut carol_pk))
        .await
        .unwrap();
    let _ok = alice
        .orgs()
        .release_approval(&org, "9.2.0", None, Some(&mut alice_pk))
        .await
        .unwrap();
    let view = alice.orgs().get(&org).await.map(wire).unwrap();
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
        .orgs()
        .capabilities(&org, 0, Some(&bob_id), None, Some(&mut alice_pk))
        .await
        .map(wire)
        .unwrap();
    let e = bob
        .orgs()
        .release_approval(&org, "9.3.0", None, Some(&mut bob_pk))
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("403"), "revoked: {e}");

    let e = dan
        .orgs()
        .release_approval(&org, "9.3.0", None, Some(&mut dan_pk))
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("404"), "an outsider: {e}");

    let e = ops(&c)
        .await
        .ops()
        .release("9.4.0", None, None)
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
    set(&alice, &app, "v1").await.unwrap();

    // Announced for later: nothing changes yet, and anyone can read it.
    let _ok = set_deadline(&c, Some("2999-01-01T00:00:00Z"))
        .await
        .unwrap();
    let _early = alice
        .apps()
        .create(typed(json!({ "name": "early", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let public = c
        .client(1)
        .public()
        .releases_public()
        .await
        .map(wire)
        .unwrap();
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
        .ops()
        .release_deadline("2.0.0", Some("soon"), None)
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("400"), "{e}");

    // Bob approves 2.0.0 in time; then the deadline passes.
    let _ok = bob
        .auth()
        .account_release_approval("2.0.0", None, Some(&mut bob_pk))
        .await
        .map(wire)
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
            .apps()
            .create(typed(json!({ "name": "late", "bundle": &bundle })))
            .await
            .map(|a| a.app_id)
            .unwrap_err(),
    );
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[21; 32]);
    stopped(
        alice
            .packages()
            .upload(mpk("com.test.late", "0.1.0", &kv_wasm(), &publisher))
            .await
            .unwrap_err(),
    );
    let next = alice
        .packages()
        .bundle_put(variant(&kv_wasm(), "v2"))
        .await
        .map(|b| b.hash)
        .unwrap();
    stopped(
        alice
            .apps()
            .upgrade(&app, typed(json!({ "bundle": next })), None)
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
    let bobs = bob
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let created = bob
        .apps()
        .create(typed(json!({ "name": "b", "bundle": bobs })))
        .await
        .map(wire)
        .unwrap();
    assert_eq!(created["replicas"], json!([3]), "{created}");

    // Approving lifts it for Alice at once.
    let _ok = alice
        .auth()
        .account_release_approval("2.0.0", None, Some(&mut alice_pk))
        .await
        .map(wire)
        .unwrap();
    let created = alice
        .apps()
        .create(typed(json!({ "name": "late", "bundle": bundle })))
        .await
        .map(wire)
        .unwrap();
    assert_eq!(created["replicas"], json!([3]), "{created}");

    // Lifted by the operator: gone from the public rules.
    // Unsigned, it is not lifted; the operator's signature lifts it.
    let e = ops(&c)
        .await
        .ops()
        .release_deadline_lift("2.0.0", None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("OPERATOR_SIGNATURE_REQUIRED"), "{e}");
    let _ok = set_deadline(&c, None).await.unwrap();
    let public = c
        .client(1)
        .public()
        .releases_public()
        .await
        .map(wire)
        .unwrap();
    assert_eq!(public["deadlines"], json!([]), "{public}");
}

/// The binding of `key` to `org`: the key's signature over
/// `calimero-publisher-binding/v1\n{cluster}\n{org}\n{signer}\n{generation}`,
/// with the cluster's name and the key's generation as the node names them.
async fn publisher_binding(cl: &Client, key: &ed25519_dalek::SigningKey, org: &str) -> Value {
    use ed25519_dalek::Signer as _;
    let signer = calimero_bundle::derive_signer_id_did_key(key.verifying_key().as_bytes());
    let next = cl
        .public()
        .publisher_binding(&signer)
        .await
        .map(wire)
        .unwrap();
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
    let _ok = pam.packages().upload(first.clone()).await.unwrap();
    let _ok = cora.packages().upload(first).await.unwrap();
    let inst = cora
        .apps()
        .create(typed(
            json!({ "name": "i", "package": "com.test.pub", "org": customer }),
        ))
        .await
        .map(|a| a.app_id)
        .unwrap();

    // Until the key is bound, nobody upgrades a `publisher` package.
    let e = cora
        .apps()
        .upgrade(&inst, typed(json!({ "version": "1.0.0" })), None)
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
        .transport()
        .confirmed(
            Request::new(Method::POST, format!("orgs/{customer}/publishers"))
                .json(publisher_binding(&cora, &key, &publisher).await),
            &mut cora_pk,
        )
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("403"), "{e}");
    let binding = publisher_binding(&pam, &key, &publisher).await;
    let _ok = pam
        .transport()
        .confirmed(
            Request::new(Method::POST, format!("orgs/{publisher}/publishers"))
                .json(binding.clone()),
            &mut pam_pk,
        )
        .await
        .unwrap();
    let view = pam.orgs().get(&publisher).await.map(wire).unwrap();
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
        .transport()
        .confirmed(
            Request::new(Method::POST, format!("orgs/{publisher}/publishers")).json(binding),
            &mut pam_pk,
        )
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("403"), "{e}");

    let _ok = pam
        .packages()
        .upload(release("1.1.0", &variant(&kv, "v2"), Publisher))
        .await
        .unwrap();
    // The customer no longer upgrades it.
    let e = cora
        .apps()
        .upgrade(
            &inst,
            typed(json!({ "version": "1.1.0" })),
            Some(&mut cora_pk),
        )
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("403"), "{e}");
    // The publisher does, for every instance.
    let run = pam
        .orgs()
        .publisher_upgrade(&publisher, "com.test.pub", "1.1.0", None, Some(&mut pam_pk))
        .await
        .map(wire)
        .unwrap();
    let run = finished_run(&pam, &publisher, run["runId"].as_str().unwrap()).await;
    assert_eq!(run["done"], json!([inst]), "{run}");

    // Upgrades go to owners in a later release, never back.
    let _ok = pam
        .packages()
        .upload(release("1.2.0", &variant(&kv, "v3"), Owner))
        .await
        .map(wire)
        .unwrap();
    let e = pam
        .packages()
        .upload(release("1.3.0", &variant(&kv, "v4"), Publisher))
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
            .transport()
            .json(Request::new(
                Method::GET,
                format!("{route}{sep}limit=1{after}"),
            ))
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
        .orgs()
        .create("Acme Secret Org", None)
        .await
        .map(wire)
        .unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ = alice
        .orgs()
        .set_member(&org, &bob_id, OrgRole::Member, Some(&mut apk))
        .await
        .unwrap();
    let kv = kv_wasm();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let _ = alice
        .packages()
        .upload(mpk("com.test.track", "1.0.0", &kv, &publisher))
        .await
        .unwrap();
    let _ = alice
        .packages()
        .upload(mpk(
            "com.test.track",
            "1.1.0",
            &variant(&kv, "next"),
            &publisher,
        ))
        .await
        .unwrap();
    let created = alice
        .apps().create(typed(json!({ "name": "tracked-secret-name", "package": "com.test.track", "version": "1.0.0", "org": org, "access": { "kind": "org" } })))
        .await.map(wire)
        .unwrap();
    let app = created["appId"].as_str().unwrap().to_owned();
    let _ = alice
        .apps()
        .upgrade(&app, typed(json!({ "version": "1.1.0" })), Some(&mut apk))
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
    let report = ops.ops().apps(None, None).await.map(wire).unwrap();
    assert_eq!(
        report["governance"]["appUpgrades"],
        json!("owner"),
        "{report}"
    );
    let orgs = ops.ops().orgs(None, None).await.map(wire).unwrap();
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
    let p = ops.ops().placement().await.map(wire).unwrap();
    assert!(p["apps"][0].get("replicas").is_some(), "{p}");
    assert!(p["nodes"][0].get("release").is_some(), "{p}");

    // A member: the package, who upgraded it, and the release not approved yet.
    let route = format!("orgs/{org}/apps");
    let m = bob.orgs().apps(&org, None, None).await.map(wire).unwrap();
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
    let e = carol.orgs().apps(&org, None, None).await.unwrap_err();
    assert!(e.to_string().starts_with("404"), "{e}");
    let e = ops.orgs().apps(&org, None, None).await.unwrap_err();
    assert!(
        e.to_string().starts_with("401") || e.to_string().starts_with("404"),
        "{e}"
    );

    // The owner approves 2.0.0: both views show it.
    let _ = alice
        .orgs()
        .release_approval(&org, "2.0.0", None, Some(&mut apk))
        .await
        .map(wire)
        .unwrap();
    let m = bob.orgs().apps(&org, None, None).await.map(wire).unwrap();
    assert_eq!(m["approvedReleases"], json!(["1.0.0", "2.0.0"]), "{m}");
    let orgs = ops.ops().orgs(None, None).await.map(wire).unwrap();
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
        .apps()
        .create(typed(
            json!({ "name": "mine", "package": "com.test.track", "version": "1.0.0" }),
        ))
        .await
        .map(wire)
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    let own = alice
        .auth()
        .account_apps(None, None)
        .await
        .map(wire)
        .unwrap();
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
    let theirs = carol
        .auth()
        .account_apps(None, None)
        .await
        .map(wire)
        .unwrap();
    assert_eq!(theirs["apps"], json!([]), "{theirs}");
    // The page size is bounded.
    let e = alice
        .auth()
        .account_apps(None, Some(100_000))
        .await
        .unwrap_err();
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
        .ops()
        .package_upload(mpk("com.test.opsup", "1.0.0", &kv, &publisher))
        .await
        .unwrap();
    let app = alice
        .apps()
        .create(typed(json!({ "name": "kv", "package": "com.test.opsup" })))
        .await
        .map(wire)
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ = ops
        .ops()
        .package_upload(mpk(
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
    let own = alice
        .auth()
        .account_apps(None, None)
        .await
        .map(wire)
        .unwrap();
    assert_eq!(own["governance"]["appUpgrades"], json!("operator"), "{own}");
}

/// A release that changes only the frontend (the same WASM) is a release of
/// its own: an instance runs the release it was created at or upgraded to,
/// the reports name that release and the newer ones (same WASM included),
/// and moving to one is an upgrade like any other: approved with a passkey
/// by whoever holds the app's upgrades, one app at a time or in an org run.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_frontend_only_release_is_an_upgrade_of_its_own() {
    let c = Cluster::start(3).await;
    let (alice, mut apk) = signed_up(&c, 0, "alice").await;
    let (bob, _bpk) = signed_up(&c, 1, "bob").await;
    let bob_id = account_id(&bob).await;
    let org = alice.orgs().create("Acme", None).await.map(wire).unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ = alice
        .orgs()
        .set_member(&org, &bob_id, OrgRole::Member, Some(&mut apk))
        .await
        .unwrap();
    // One WASM, three releases whose frontends differ.
    let wasm = variant(&kv_wasm(), "fe");
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
    for version in ["1.0.0", "1.1.0", "1.2.0"] {
        let page = format!("<p>{version}");
        let _ = alice
            .packages()
            .upload(mpk_full(
                "com.test.fe",
                version,
                &wasm,
                &publisher,
                &[],
                &[("frontend/index.html", page.as_bytes())],
                None,
            ))
            .await
            .unwrap();
    }

    // Created at 1.1.0: it runs 1.1.0, not the first release shipping its WASM.
    let mine = alice
        .apps()
        .create(typed(
            json!({ "name": "mine", "package": "com.test.fe", "version": "1.1.0" }),
        ))
        .await
        .map(wire)
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    let a = reported_app(&alice, "auth/apps", &mine).await;
    assert_eq!(a["package"]["version"], json!("1.1.0"), "{a}");
    assert_eq!(a["availableVersions"], json!(["1.2.0"]), "{a}");
    let record = alice.apps().get(&mine).await.map(wire).unwrap();
    assert_eq!(
        record["release"],
        json!({ "package": "com.test.fe", "version": "1.1.0" }),
        "{record}"
    );

    // Moving to 1.2.0 (same WASM) needs the owner's passkey, as any upgrade.
    let e = alice
        .apps()
        .upgrade(&mine, typed(json!({ "version": "1.2.0" })), None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    let _ = alice
        .apps()
        .upgrade(&mine, typed(json!({ "version": "1.2.0" })), Some(&mut apk))
        .await
        .unwrap();
    let a = reported_app(&alice, "auth/apps", &mine).await;
    assert_eq!(a["package"]["version"], json!("1.2.0"), "{a}");
    assert_eq!(a["availableVersions"], json!([]), "{a}");
    assert_eq!(a["lastUpgrade"]["ok"], json!(true), "{a}");
    assert_eq!(a["lastUpgrade"]["from"]["version"], json!("1.1.0"), "{a}");
    assert_eq!(a["lastUpgrade"]["to"]["version"], json!("1.2.0"), "{a}");
    assert_eq!(
        a["lastUpgrade"]["from"]["bundle"], a["lastUpgrade"]["to"]["bundle"],
        "{a}"
    );
    // Still serving.
    let _ = alice
        .apps()
        .call(
            &mine,
            "set",
            Some(json!({ "key": "k", "value": "v" })),
            None,
            None,
        )
        .await
        .unwrap();

    // An org app: a member without MANAGE_APPLICATION may not move it, even
    // to a release with the same WASM; the owner's org run does.
    let shared = alice
        .apps().create(typed(json!({ "name": "shared", "package": "com.test.fe", "version": "1.0.0", "org": org, "access": { "kind": "org" } })))
        .await.map(wire)
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    let route = format!("orgs/{org}/apps");
    let a = reported_app(&bob, &route, &shared).await;
    assert_eq!(a["package"]["version"], json!("1.0.0"), "{a}");
    let e = bob
        .apps()
        .upgrade(&shared, typed(json!({ "version": "1.1.0" })), None)
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("403"), "{e}");
    let run = alice
        .orgs()
        .upgrade(
            &org,
            typed(json!({ "package": "com.test.fe", "version": "1.1.0" })),
            Some(&mut apk),
        )
        .await
        .map(wire)
        .unwrap();
    let run_id = run["runId"].as_str().unwrap().to_owned();
    let done = finished_run(&alice, &org, &run_id).await;
    assert_eq!(done["done"], json!([shared]), "{done}");
    let a = reported_app(&bob, &route, &shared).await;
    assert_eq!(a["package"]["version"], json!("1.1.0"), "{a}");
    assert_eq!(a["lastUpgrade"]["run"], json!(run_id), "{a}");
    assert_eq!(a["lastUpgrade"]["from"]["version"], json!("1.0.0"), "{a}");
    assert_eq!(a["lastUpgrade"]["to"]["version"], json!("1.1.0"), "{a}");
}

// ---------------------------------------------------------------- proofs at apply

/// Credentials, memberships and invites are a person's decisions: each
/// route answers `412 CONFIRM_REQUIRED` first and applies with the person's
/// passkey assertion (an existing passkey for a new one); the operator's
/// fleet and quota routes answer `428 OPERATOR_SIGNATURE_REQUIRED` first and
/// apply with the operator's signature.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn credentials_memberships_and_the_fleet_are_approved_where_they_are_made() {
    let c = Cluster::start(3).await;
    let (alice, mut apk) = signed_up(&c, 0, "alice").await;
    let (bob, _bpk) = signed_up(&c, 1, "bob").await;
    let bob_id = bob.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();

    // An API key: 412, then the passkey; the key works.
    let e = alice
        .auth()
        .apikey_create(typed(json!({ "label": "ci" })), None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    let (_id, key) = alice
        .auth()
        .apikey_create(typed(json!({ "label": "ci" })), Some(&mut apk))
        .await
        .map(|k| (k.key_id, k.key))
        .unwrap();
    let agent = c.client(2);
    agent.use_api_key(&key).await;
    assert_eq!(
        agent.auth().me().await.map(wire).unwrap()["via"],
        json!("apiKey")
    );
    // An API key cannot make another, nor approve a step-up.
    let e = agent
        .auth()
        .apikey_create(typed(json!({ "label": "more" })), Some(&mut apk))
        .await
        .map(|k| (k.key_id, k.key))
        .unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"), "{e}");

    // A passkey: approved by one the account has, not by itself.
    let mut phone = SoftAuthenticator::new("localhost", ORIGIN);
    let mut also_phone = SoftAuthenticator::with_key(&phone, "localhost", ORIGIN);
    let e = alice
        .add_passkey(&mut phone, &mut also_phone)
        .await
        .unwrap_err();
    assert!(e.code().is_some(), "{e}");
    let mut phone = SoftAuthenticator::new("localhost", ORIGIN);
    let phone_id = alice.add_passkey(&mut phone, &mut apk).await.unwrap();
    assert_eq!(
        alice.auth().me().await.map(wire).unwrap()["passkeys"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let e = alice
        .auth()
        .passkey_remove(&phone_id, None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    let _ok = alice
        .auth()
        .passkey_remove(&phone_id, Some(&mut apk))
        .await
        .unwrap();
    assert_eq!(
        alice.auth().me().await.map(wire).unwrap()["passkeys"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // An invite and a role: 412 first.
    let org = alice.orgs().create("Acme", None).await.map(wire).unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let e = alice
        .orgs()
        .invite_create(&org, typed(json!({ "role": "member" })), None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    let token = alice
        .orgs()
        .invite_create(&org, typed(json!({ "role": "member" })), Some(&mut apk))
        .await
        .map(wire)
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _joined = bob.orgs().invite_redeem(&token).await.unwrap();
    let promote = json!({ "account": bob_id, "role": "admin" });
    let e = alice
        .transport()
        .json(Request::new(Method::POST, format!("orgs/{org}/members")).json(promote.clone()))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    let _ok = alice
        .orgs()
        .set_member(&org, &bob_id, OrgRole::Admin, Some(&mut apk))
        .await
        .unwrap();
    let o = alice.orgs().get(&org).await.map(wire).unwrap();
    assert_eq!(o["members"][&bob_id], json!("admin"), "{o}");

    // The operator: 428 first, then signed.
    let ops = ops_at(&c, 0).await;
    let e = ops
        .ops()
        .quotas_set(typed(json!({ "maxAccounts": 9 })), None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("OPERATOR_SIGNATURE_REQUIRED"), "{e}");
    let d = e.details().unwrap();
    assert_eq!(d["method"], json!("__set_quotas"), "{d}");
    let _ok = ops
        .ops()
        .quotas_set(
            typed(json!({ "maxAccounts": 9 })),
            Some(&harness::operator_sign),
        )
        .await
        .unwrap();
    assert_eq!(
        ops.ops().quotas_get().await.map(wire).unwrap()["maxAccounts"],
        json!(9)
    );
    let e = ops
        .ops()
        .register_node(9, "http://127.0.0.1:9/", None, None, None, None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("OPERATOR_SIGNATURE_REQUIRED"), "{e}");
    let e = ops.ops().remove_node("3", None).await.unwrap_err();
    assert!(
        matches!(e.code(), Some("OPERATOR_SIGNATURE_REQUIRED" | "NOT_LEADER")),
        "{e}"
    );
}

/// A Raft message the sender meant for one node, reaching another (an address
/// now answered by another node of the cluster: a reused port, a node moved or
/// replaced), is refused there, never handled as that node's own. It comes over
/// the real peer path (`peer_post`: an identified channel, `api::peer`) from
/// the genuine leader, so the sender and membership checks (F4) pass: only the
/// addressee is wrong. A message without an addressee (a peer on the previous
/// release) is still handled, and both are counted.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_raft_message_for_another_node_is_refused_and_the_group_stays_healthy() {
    let c = Cluster::start(3).await;
    let node = |i: usize| &c.nodes[i].as_ref().unwrap().node;
    let leader = (0..3)
        .find(|i| node(*i).system.status().is_leader)
        .expect("a system leader");
    let followers: Vec<usize> = (0..3).filter(|i| *i != leader).collect();
    let (meant, reached) = (followers[0], followers[1]);
    let leader_id = node(leader).config.node_id;
    let meant_id = node(meant).config.node_id;
    let reached_id = node(reached).config.node_id;
    let before = node(leader).system.status();
    // `calimero_raft_misaddressed_total{reason}` on the node the messages reach.
    let ops = c.client(reached);
    ops.use_api_key("ops-secret").await;
    let misaddressed = || async {
        let text = ops.ops().metrics().await.unwrap();
        let count = |reason: &str| -> u64 {
            let prefix = format!("calimero_raft_misaddressed_total{{reason=\"{reason}\"}} ");
            text.lines()
                .find_map(|l| l.strip_prefix(prefix.as_str()))
                .unwrap_or_else(|| panic!("no {prefix} in the metrics"))
                .parse()
                .unwrap()
        };
        (count("not_addressee"), count("no_target"))
    };
    let counted_before = misaddressed().await;

    // The leader's heartbeat at a newer term, with nothing to append, meant
    // for `meant`. Had the reached node taken it as its own, it would move to
    // that term and refuse the leader's appends at the old one, and the group
    // would hold an election.
    let term = before.term + 10;
    let heartbeat = json!({
        "group": 1,
        "target": meant_id,
        "rpc": { "Append": {
            "vote": { "leader_id": { "term": term, "node_id": leader_id }, "committed": true },
            "prev_log_id": null,
            "leader_commit": null,
            "entries": [],
        } },
    });
    let refused = node(leader)
        .peer_post(&c.urls[reached], "internal/raft", &heartbeat)
        .await
        .expect_err("a message for another node must be refused");
    let refused = format!("{refused:#}");
    assert!(refused.contains("NOT_ADDRESSEE"), "{refused}");
    assert!(
        refused.contains(&format!("for node {meant_id}; this is node {reached_id}")),
        "{refused}"
    );

    // No addressee (a peer on the previous release): handled as before. The
    // leader's vote for a term long past changes nothing and is answered as a
    // vote.
    let stale_vote = json!({
        "group": 1,
        "rpc": { "Vote": {
            "vote": { "leader_id": { "term": 0, "node_id": leader_id }, "committed": false },
            "last_log_id": null,
        } },
    });
    let answered = node(leader)
        .peer_post(&c.urls[reached], "internal/raft", &stale_vote)
        .await
        .expect("a message without an addressee is handled");
    assert!(answered.get("Vote").is_some(), "{answered}");

    // Both are counted where they arrived: one refused, one accepted without an
    // addressee. Nothing else in this process sends either.
    let counted = misaddressed().await;
    assert_eq!(
        (counted.0 - counted_before.0, counted.1 - counted_before.1),
        (1, 1),
        "not_addressee and no_target deltas"
    );

    // The group is unharmed: same leader, same term on every node, and a write
    // through the node the message reached commits.
    tokio::time::sleep(Duration::from_secs(1)).await;
    for i in 0..3 {
        let s = node(i).system.status();
        assert_eq!(s.term, before.term, "node {} term moved", i + 1);
        assert_eq!(s.leader, before.leader, "node {} leader moved", i + 1);
    }
    let (_client, _a) = signed_up(&c, reached, "bob-after-a-misaddressed-heartbeat").await;
}

/// eu-shared-2 (and `bababa`) never kept a system leader: a TDX node attests a
/// peer in about a second (Intel's collateral is fetched each time), and
/// openraft gives an append one heartbeat (200 ms there). Each RPC began the
/// attestation, was dropped at its deadline, and the next began it again, so
/// the channel to the peer never opened and the peer never learned who called.
/// Attestation here takes ten heartbeats; the cluster still forms and writes.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn peers_that_attest_slower_than_a_heartbeat_still_form_a_cluster() {
    let c = tokio::time::timeout(
        Duration::from_secs(60),
        Cluster::start_with(3, |config| {
            calimero_node::testing::slow_attestation(&config.public_url, Duration::from_secs(1));
        }),
    )
    .await
    .expect("the cluster formed");
    let (_alice, _auth) = tokio::time::timeout(Duration::from_secs(30), signed_up(&c, 1, "alice"))
        .await
        .expect("a system write committed");
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

/// Unix seconds now, as the test's own clock reads it.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// `app`'s `lastWriteAt` in `route`'s report as `cl` reads it, once it names
/// one (a follower may apply the write a moment after it commits).
async fn reported_last_write(cl: &Client, route: &str, app: &str) -> u64 {
    for _ in 0..100 {
        let a = reported_app(cl, route, app).await;
        if let Some(at) = a["lastWriteAt"].as_u64() {
            return at;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{route} never named {app}'s last write");
}

/// When an app last committed a write (mdma's "last used"): `lastWriteAt` in
/// the operator's `GET /ops/apps`, the owner's `GET /auth/apps` and
/// `GET /apps/{app}/storage`. Nothing before the first write. The leader
/// stamps the time into the entry, so every node names the same second: a
/// replica from its own state, the node that holds no replica by asking one.
/// A restarted replica reads it back from its state.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_apps_last_write_is_the_same_on_every_node_and_survives_a_restart() {
    let mut c = Cluster::start(4).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let (app, _ctx) = deploy(&alice, "kv", kv_wasm(), json!({})).await;
    let operator = ops(&c).await;
    let a = reported_app(&operator, "ops/apps", &app).await;
    assert_eq!(a["lastWriteAt"], Value::Null, "no write yet: {a}");

    let before = unix_now();
    set_through(&alice, &app, "k", "v").await.unwrap();
    let after = unix_now();
    let at = reported_last_write(&operator, "ops/apps", &app).await;
    assert!(before <= at && at <= after, "{before} <= {at} <= {after}");

    // Three of the four nodes hold a replica; the fourth asks one.
    let outside: Vec<usize> = (0..4)
        .filter(|i| {
            c.nodes[*i]
                .as_ref()
                .is_some_and(|r| r.node.hosted(&app).is_none())
        })
        .collect();
    assert_eq!(outside.len(), 1, "one node holds no replica");
    for i in 0..4 {
        let cl = c.client(i);
        cl.use_api_key("ops-secret").await;
        assert_eq!(
            reported_last_write(&cl, "ops/apps", &app).await,
            at,
            "node {}",
            i + 1
        );
    }
    assert_eq!(reported_last_write(&alice, "auth/apps", &app).await, at);
    let storage = alice.apps().storage(&app).await.map(wire).unwrap();
    assert_eq!(storage["lastWriteAt"], json!(at), "{storage}");

    // A replica restarted on its data directory reads the same time back
    // from its own state.
    let replica = (0..4).find(|i| !outside.contains(i)).unwrap();
    c.restart(replica).await;
    let mut kept = None;
    for _ in 0..100 {
        kept = c.nodes[replica]
            .as_ref()
            .and_then(|r| r.node.hosted(&app))
            .and_then(|g| g.read_local(|m, _| m.last_write_ms()));
        if kept.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(kept.map(|ms| ms / 1000), Some(at), "node {}", replica + 1);
    let cl = c.client(replica);
    cl.use_api_key("ops-secret").await;
    assert_eq!(reported_last_write(&cl, "ops/apps", &app).await, at);
}
