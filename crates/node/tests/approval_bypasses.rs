//! Phase 0 of gating the system group (`docs/specs/gate-system-group.md`):
//! what a node on a release an owner has not approved could do before, tried
//! the way such a node would: with the cluster secret, over its own sealed
//! channel, by proposing system commands, by minting sessions.
//!
//! - F2: the internal routes judge which node calls and its release.
//! - F3: a decision carries its maker's proof, checked where it applies.
//! - F4: a Raft message's sender is the node that sent it, a group member.
//! - F1: sessions are signed by the node that issued them, not by a key
//!   every node holds.
//!
//! Needs `kv_store.wasm`: `cargo build -p kv-store --target wasm32-unknown-unknown --profile app-release`,
//! or set `KV_STORE_WASM`.

use std::sync::Arc;
use std::time::Duration;

use calimero_client::attest::HttpAttestor;
use calimero_client::{Client, Policy};
use calimero_identity::testing::SoftAuthenticator;
use calimero_node::node::{ViewRequest, SYSTEM_GROUP};
use calimero_node::types::Caller;
use calimero_node::Running;
use calimero_sealed::client::SealedTransport as SealedClient;
use serde_json::{json, Value};

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{kv_wasm, signed_up, Cluster};

fn running(c: &Cluster, i: usize) -> &Running {
    c.nodes[i].as_ref().unwrap()
}

fn mock_policy() -> Policy {
    Policy {
        allow_mock: true,
        ..Policy::default()
    }
}

/// A sealed channel to node `to` opened as node `from` would open it: Noise IK
/// with `from`'s own transport key. What a node's image can do whatever its
/// code: it holds its key and the cluster secret.
fn as_node(c: &Cluster, from: usize, to: usize) -> SealedClient {
    SealedClient::identified_as(
        c.urls[to].parse().unwrap(),
        reqwest::Client::new(),
        Arc::new(HttpAttestor {
            policy: mock_policy(),
            seen_release: Arc::default(),
        }),
        running(c, from).node.transport.static_key(),
    )
}

/// An anonymous sealed channel to node `to` (Noise NK): what nodes from
/// before identified sessions open.
fn anonymous(c: &Cluster, to: usize) -> SealedClient {
    SealedClient::attested(
        c.urls[to].parse().unwrap(),
        reqwest::Client::new(),
        Arc::new(HttpAttestor {
            policy: mock_policy(),
            seen_release: Arc::default(),
        }),
    )
}

/// Send a peer request with the cluster secret, naming node `claimed`.
async fn peer_send(
    c: &Cluster,
    channel: &SealedClient,
    to: usize,
    get: bool,
    path: &str,
    body: &Value,
    claimed: u64,
) -> (u16, Value) {
    let url = format!("{}{path}", c.urls[to]);
    let http = reqwest::Client::new();
    let builder = if get {
        http.get(url)
    } else {
        http.post(url).json(body)
    };
    let req = builder
        .header("x-cluster-auth", running(c, 0).node.cluster_secret())
        .header("x-calimero-node", claimed.to_string())
        .build()
        .unwrap();
    let resp = channel.execute(req).await.unwrap();
    let status = resp.status().as_u16();
    let v = if get {
        Value::Null
    } else {
        resp.json().await.unwrap_or(Value::Null)
    };
    (status, v)
}

/// Node 3 runs a release (2.0.0) nobody approved; nodes 1 and 2 run 1.0.0,
/// which accounts that sign up on them approve.
async fn split_cluster() -> Cluster {
    Cluster::start_with(3, |cfg| {
        cfg.tee_release = Some(if cfg.node_id == 3 { "2.0.0" } else { "1.0.0" }.into());
    })
    .await
}

/// Alice's kv app on nodes 1 and 2, holding `k = v1`.
async fn alices_app(c: &Cluster) -> (Client, SoftAuthenticator, String, String, String) {
    let (alice, key) = signed_up(c, 0, "alice").await;
    let account = alice.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
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
    (alice, key, account, app, bundle)
}

// ---------------------------------------------------------------- F2, F4

/// A node on an unapproved release holds the cluster secret and opens sealed
/// channels like any node. Before, `/internal/view`, `call`, `events`,
/// `storage`, `blob/held`, `app-ready`, `blob/get` and `raft` answered it
/// for any app (they checked only the secret). Now each judges the caller:
/// its key, the node whose address attests that key, and that node's release.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_node_on_an_unapproved_release_reaches_no_app_through_internal_routes() {
    let c = split_cluster().await;
    let (alice, _key, account, app, _bundle) = alices_app(&c).await;
    let caller = Caller::Account {
        account: account.clone(),
    };
    let view = serde_json::to_value(ViewRequest {
        app_id: app.clone(),
        method: "get".into(),
        args: json!({ "key": "k" }),
        caller: caller.clone(),
        strong: false,
        min_index: 0,
    })
    .unwrap();
    let call = json!({
        "appId": app, "method": "set", "args": { "key": "k", "value": "x" },
        "caller": serde_json::to_value(&caller).unwrap(),
    });
    let group = running(&c, 0).node.sys(|s| s.apps[&app].group);

    // Node 2 runs an approved release: it is answered.
    let approved = as_node(&c, 1, 0);
    let (status, v) = peer_send(&c, &approved, 0, false, "internal/view", &view, 2).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["result"], json!("v1"));

    // Node 3 is refused on every route that reaches the app's data.
    let node3 = as_node(&c, 2, 0);
    let routes: Vec<(String, bool, Value)> = vec![
        ("internal/view".into(), false, view.clone()),
        ("internal/call".into(), false, call.clone()),
        ("internal/storage".into(), false, json!({ "app": app })),
        (
            "internal/blob/held".into(),
            false,
            json!({ "app": app, "id": hex::encode([7; 32]) }),
        ),
        ("internal/app-ready".into(), false, json!({ "appId": app })),
        (
            "internal/blob/get".into(),
            false,
            json!({ "hash": hex::encode([7; 32]) }),
        ),
        (
            format!("internal/events?app={app}&from=0"),
            true,
            Value::Null,
        ),
    ];
    for (path, get, body) in &routes {
        let (status, v) = peer_send(&c, &node3, 0, *get, path, body, 3).await;
        assert_eq!(status, 403, "{path}: {v}");
    }
    // Claiming to be node 2 does not help: node 2's address attests another key.
    let (status, v) = peer_send(&c, &node3, 0, false, "internal/view", &view, 2).await;
    assert_eq!(status, 403, "{v}");

    // F4: a Raft append for the app's group, from node 3, is refused, whether
    // it names node 3 or node 1 (the app's leader) as its sender.
    for sender in [3, 1] {
        let rpc = json!({
            "group": group,
            "rpc": { "Vote": {
                "vote": { "leader_id": { "term": 1_000, "node_id": sender }, "committed": false },
                "last_log_id": null,
            } },
        });
        let (status, v) = peer_send(&c, &node3, 0, false, "internal/raft", &rpc, 3).await;
        assert_eq!(status, 403, "sender {sender}: {v}");
    }
    // The system group still takes node 3's messages: it is a registered node
    // (decision 7: any release may be a system voter).
    let rpc = json!({
        "group": SYSTEM_GROUP,
        "rpc": { "Vote": {
            "vote": { "leader_id": { "term": 0, "node_id": 3 }, "committed": false },
            "last_log_id": null,
        } },
    });
    let (status, v) = peer_send(&c, &node3, 0, false, "internal/raft", &rpc, 3).await;
    assert_eq!(status, 200, "{v}");

    // An anonymous channel with the cluster secret: refused.
    let anon = anonymous(&c, 0);
    let (status, v) = peer_send(&c, &anon, 0, false, "internal/view", &view, 3).await;
    assert_eq!(status, 403, "{v}");

    // Nor through its own API: its forward to the app's leader is refused.
    let on3 = c.client(2);
    on3.use_session(alice.session().await.unwrap()).await;
    let read = on3
        .view(
            &app,
            "get",
            json!({ "key": "k" }),
            calimero_client::Consistency::Strong,
        )
        .await;
    assert!(read.is_err(), "{read:?}");
}

// ---------------------------------------------------------------- F3

/// The system leader, by index.
async fn system_leader(c: &Cluster) -> usize {
    for _ in 0..100 {
        if let Some(i) = (0..c.nodes.len()).find(|i| {
            c.nodes[*i]
                .as_ref()
                .is_some_and(|r| r.node.system.status().is_leader)
        }) {
            return i;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no system leader");
}

/// Append `cmd` to the system log directly, as a system leader's own code
/// would (decision 7: an unapproved node may lead the system group), and read
/// what `apply` answered.
async fn append(c: &Cluster, cmd: &Value) -> Result<Value, String> {
    let leader = system_leader(c).await;
    let (_index, out) = running(c, leader)
        .node
        .system
        .write(serde_json::to_vec(cmd).unwrap())
        .await
        .unwrap();
    calimero_node::system::output(&out)
}

/// A consent as the API logs it: `pk`'s assertion over the statement.
fn consent(pk: &mut SoftAuthenticator, subject: &str, method: &str, args: &Value) -> Value {
    let statement = calimero_node::proof::Statement::new(subject, method, args);
    let challenge = statement.challenge().unwrap();
    serde_json::to_value(calimero_node::proof::Consent {
        statement,
        assertion: pk.assert(&challenge),
    })
    .unwrap()
}

/// Every node's view of alice's approvals and her app's owner.
async fn settled(c: &Cluster, account: &str, app: &str) -> Vec<(Value, String)> {
    tokio::time::sleep(Duration::from_millis(500)).await;
    c.nodes
        .iter()
        .flatten()
        .map(|r| {
            r.node.sys(|s| {
                (
                    json!(s.accounts[account].approved_releases),
                    s.apps[app].owner.clone(),
                )
            })
        })
        .collect()
}

/// Before, `apply` discarded the consent (`consent: _`) and trusted `actor`:
/// any node could log `SetReleaseApproval { actor: <the owner>, approved }`
/// for its own release. Now the decision applies only with the actor's own
/// passkey assertion over exactly that decision, once, on every node,
/// whoever proposed it: the leader's own log or a peer's `internal/system`.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn forged_decisions_are_refused_where_they_apply_on_every_node() {
    let c = split_cluster().await;
    let (_alice, mut alice_pk, alice, app, bundle) = alices_app(&c).await;
    let (bob_client, mut bob_pk) = signed_up(&c, 1, "bob").await;
    let bob = bob_client.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let before = settled(&c, &alice, &app).await;
    assert_eq!(before[0], (json!(["1.0.0"]), alice.clone()));

    let approve = |consent: Value| {
        json!({
            "op": "setReleaseApproval", "scope": { "kind": "account" },
            "version": "2.0.0", "approved": true, "actor": alice, "consent": consent,
        })
    };
    let subject = format!("account:{alice}");
    let args = json!({ "version": "2.0.0" });
    let refused = [
        ("no consent", approve(Value::Null)),
        (
            "bob's passkey",
            approve(consent(&mut bob_pk, &subject, "__approve_release", &args)),
        ),
        (
            "another release",
            approve(consent(
                &mut alice_pk,
                &subject,
                "__approve_release",
                &json!({ "version": "3.0.0" }),
            )),
        ),
        (
            "a withdrawal",
            approve(consent(
                &mut alice_pk,
                &subject,
                "__withdraw_release",
                &args,
            )),
        ),
        (
            "an upgrade without consent",
            json!({
                "op": "upgradeApp", "appId": app, "bundle": bundle, "byOperator": false,
                "actor": alice, "consent": null,
            }),
        ),
        (
            "a transfer without consent",
            json!({ "op": "transferApp", "appId": app, "to": bob, "actor": alice, "consent": null }),
        ),
        (
            "a transfer with bob's passkey",
            json!({
                "op": "transferApp", "appId": app, "to": bob, "actor": alice,
                "consent": consent(&mut bob_pk, &app, "__transfer", &json!({ "to": bob })),
            }),
        ),
    ];
    for (what, cmd) in &refused {
        let e = append(&c, cmd).await.unwrap_err();
        assert!(!e.is_empty(), "{what}");
    }
    // Proposed by node 3 through a peer's route: refused where it applies.
    let node3 = as_node(&c, 2, system_leader(&c).await);
    let (status, v) = peer_send(
        &c,
        &node3,
        system_leader(&c).await,
        false,
        "internal/system",
        &approve(Value::Null),
        3,
    )
    .await;
    assert_eq!(status, 400, "{v}");
    assert_eq!(
        settled(&c, &alice, &app).await,
        before,
        "nothing applied anywhere"
    );

    // Alice's own consent over exactly this decision applies, once.
    let real = approve(consent(&mut alice_pk, &subject, "__approve_release", &args));
    let _ok = append(&c, &real).await.unwrap();
    let e = append(&c, &real).await.unwrap_err();
    assert!(e.contains("used already"), "{e}");
    let after = settled(&c, &alice, &app).await;
    assert!(
        after.iter().all(|(r, _)| *r == json!(["1.0.0", "2.0.0"])),
        "{after:?}"
    );
}

// ---------------------------------------------------------------- operator

/// Where the operator holds release approvals, its decisions carry its
/// signature (Ed25519, the key recorded at genesis). Before, any node could
/// log `SetOperatorRelease` for its own release with no proof at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn operator_decisions_need_the_operator_signature() {
    use ed25519_dalek::Signer;
    let operator = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
    let public = hex::encode(operator.verifying_key().to_bytes());
    let c = Cluster::start_with(3, |cfg| {
        cfg.governance.releases = calimero_node::system::Authority::Operator;
        cfg.operator_key = Some(public.clone());
    })
    .await;
    let recorded = running(&c, 0).node.sys(|s| s.operator_key.clone()).unwrap();
    assert_eq!(recorded.key, public);
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;

    // The ops token alone is not enough: the node names what to sign.
    let e = ops
        .post("ops/releases", json!({ "version": "2.0.0" }))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("OPERATOR_SIGNATURE_REQUIRED"), "{e}");
    let d = e.details().unwrap().clone();
    let message = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        d["message"].as_str().unwrap(),
    )
    .unwrap();
    let statement = calimero_node::proof::Statement {
        subject: "cluster".into(),
        method: "__approve_release".into(),
        args: d["args"].as_str().unwrap().into(),
        nonce: d["nonce"].as_str().unwrap().into(),
    };
    assert_eq!(
        statement.operator_message(&c.name),
        message,
        "the documented format"
    );

    // A signature by another key: refused.
    let other = ed25519_dalek::SigningKey::from_bytes(&[8; 32]);
    let e = ops
        .post(
            "ops/releases",
            json!({ "version": "2.0.0", "operatorProof": {
                "nonce": d["nonce"], "signature": hex::encode(other.sign(&message).to_bytes()),
            } }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("BAD_OPERATOR_SIGNATURE"), "{e}");

    // Logged directly, without or with a wrong proof: refused at apply.
    let forged = |proof: Value| json!({ "op": "setOperatorRelease", "version": "2.0.0", "approved": true, "proof": proof });
    assert!(append(&c, &forged(Value::Null)).await.is_err());
    let wrong = json!({
        "subject": "cluster", "method": "__approve_release", "args": d["args"],
        "nonce": d["nonce"], "signature": hex::encode(other.sign(&message).to_bytes()),
    });
    assert!(append(&c, &forged(wrong)).await.is_err());
    assert!(running(&c, 0)
        .node
        .sys(|s| !s.operator_releases.contains("2.0.0")));

    // Signed by the operator: applied, once.
    let proof = json!({ "nonce": d["nonce"], "signature": hex::encode(operator.sign(&message).to_bytes()) });
    let _ok = ops
        .post(
            "ops/releases",
            json!({ "version": "2.0.0", "operatorProof": proof }),
        )
        .await
        .unwrap();
    let e = ops
        .post(
            "ops/releases",
            json!({ "version": "2.0.0", "operatorProof": proof }),
        )
        .await
        .unwrap_err();
    assert!(e.to_string().contains("used already"), "{e}");
    tokio::time::sleep(Duration::from_millis(500)).await;
    for r in c.nodes.iter().flatten() {
        assert!(r.node.sys(|s| s.operator_releases.contains("2.0.0")));
    }
}

// ---------------------------------------------------------------- F1

/// Before, every node held the session key K_s and could mint a session for
/// any account. Now
/// a node signs the sessions it issues with its attested receipt key, and
/// other nodes accept one only where the issuer's release may serve the
/// account and, for an app, hold the app's data.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn sessions_signed_by_an_unapproved_node_reach_nothing() {
    let c = split_cluster().await;
    let (alice, mut key, account, app, _bundle) = alices_app(&c).await;
    let token = alice.session().await.unwrap().session_token;
    assert!(token.starts_with("v2."), "{token}");

    // Node 3 does not issue her a session: no other node would take it.
    let on3 = c.client(2);
    let e = on3.sign_in(&mut key).await.unwrap_err();
    assert!(matches!(e, calimero_client::Error::WrongNode { .. }), "{e}");
    // Its image can still sign whatever it likes with its own key: a token
    // for alice, as node 3 would mint it.
    // Without alice's refresh token, which a client would otherwise use on
    // the 401 and get a session an approved node signs.
    let mut minted = alice.session().await.unwrap();
    minted.refresh_token = String::new();
    minted.session_token = running(&c, 2)
        .node
        .mint_session(calimero_identity::SessionClaims {
            account: account.clone(),
            session: "forged".into(),
            expires_at: calimero_node::types::now_secs() + 600,
            passkey: None,
            app: None,
            node: 0,
            key: String::new(),
        });
    let on1 = c.client(0);
    on1.use_session(minted.clone()).await;
    let e = on1.me().await.unwrap_err();
    assert!(e.to_string().starts_with("401"), "{e}");
    let e = on1
        .call(&app, "set", json!({ "key": "k", "value": "x" }), None)
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("401"), "{e}");
    on3.use_session(minted.clone()).await;
    let e = on3.me().await.unwrap_err();
    assert_eq!(e.code(), Some("WRONG_NODE"), "{e}");

    // Her session from node 1 works on every approved node.
    let on2 = c.client(1);
    on2.use_session(alice.session().await.unwrap()).await;
    assert_eq!(on2.me().await.unwrap()["accountId"], json!(account));

    // A refresh node 3 may not serve is refused before the refresh token is
    // spent: the same token then works on an approved node (spent, it would
    // look like a stolen copy there and revoke the session).
    on3.use_session(alice.session().await.unwrap()).await;
    let e = on3.refresh().await.unwrap_err();
    assert!(matches!(e, calimero_client::Error::WrongNode { .. }), "{e}");
    // The same token, through an approved node: a session that node signed.
    let fresh = alice.refresh().await.unwrap();
    // A sign-in node 3 may not serve is refused before its challenge is spent:
    // the same ceremony then finishes on an approved node.
    let begin = on3
        .post("auth/passkey/login/begin", json!({}))
        .await
        .unwrap();
    let challenge =
        calimero_identity::from_b64url(begin["options"]["challenge"].as_str().unwrap()).unwrap();
    let finish = json!({
        "challengeId": begin["challengeId"],
        "assertion": calimero_client::Passkey::get(&mut key, &challenge),
    });
    let e = on3
        .post("auth/passkey/login/finish", finish.clone())
        .await
        .unwrap_err();
    assert!(matches!(e, calimero_client::Error::WrongNode { .. }), "{e}");
    let signed_in = c
        .client(0)
        .post("auth/passkey/login/finish", finish)
        .await
        .unwrap();
    assert_eq!(signed_in["accountId"], json!(account));
    assert!(
        fresh.session_token.starts_with("v2."),
        "{}",
        fresh.session_token
    );
    on2.use_session(fresh).await;
    assert_eq!(on2.me().await.unwrap()["accountId"], json!(account));
}
