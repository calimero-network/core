//! The cluster's sign-up policy (`Governance::sign_up`), as a person and an
//! operator meet it through the API: `open` takes anyone; `invite` takes
//! someone holding an organization invite or a cluster sign-up invite;
//! `closed` only a cluster sign-up invite (a pre-authorization). A refusal is
//! `403 SIGN_UP_REFUSED` before the passkey ceremony, and `apply` refuses
//! again; the policy changes only with the operator's signature.
//!
//! Passkeys are each cluster's own while every cluster shares one rp-id: the
//! browser offers a passkey from one cluster on another, so the name it is
//! made under says which cluster, and a sign-in with one this cluster does
//! not hold says so (`401 PASSKEY_UNKNOWN_HERE`).

use calimero_client::{Client, Passkey, Request};
use calimero_identity::testing::SoftAuthenticator;
use calimero_node::system::{SignUp, SystemCmd};
use serde_json::{json, Value};

use crate::harness;

use harness::{ops_signed, typed, wire, Cluster, ORIGIN};
use reqwest::Method;

/// A client holding the ops token.
async fn ops(c: &Cluster) -> Client {
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    ops
}

/// Sign up on node `i` as `name`, with `invite` if given.
async fn sign_up(
    c: &Cluster,
    i: usize,
    name: &str,
    invite: Option<&str>,
) -> Result<(Client, SoftAuthenticator), Box<calimero_client::Error>> {
    let client = c.client(i);
    let mut pk = SoftAuthenticator::new("localhost", ORIGIN);
    let _s = client
        .sign_up_invited(&mut pk, name, invite)
        .await
        .map_err(Box::new)?;
    Ok((client, pk))
}

fn refused(r: Result<(Client, SoftAuthenticator), Box<calimero_client::Error>>, why: &str) {
    let Err(e) = r else {
        panic!("{why}: signed up");
    };
    assert_eq!(e.code(), Some("SIGN_UP_REFUSED"), "{why}: {e}");
}

/// A sign-up invite the operator issues: `uses` accounts, an hour.
async fn operator_invite(ops: &Client, token: &str, uses: u32) -> Value {
    let expires = (time::OffsetDateTime::now_utc() + time::Duration::hours(1))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    ops_signed(
        ops,
        "ops/sign-up/invites",
        json!({ "token": token, "uses": uses, "expiresAt": expires }),
    )
    .await
    .unwrap()
}

/// An organization of `owner`'s, and an invite to it for one member.
async fn org_invite(owner: &Client, pk: &mut SoftAuthenticator) -> (String, String) {
    let org = owner
        .orgs()
        .create("Docs", None, None)
        .await
        .map(wire)
        .unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let token = owner
        .orgs()
        .invite_create(&org, typed(json!({ "role": "member" })), Some(&mut *pk))
        .await
        .map(wire)
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    (org, token)
}

async fn account_of(client: &Client) -> String {
    client.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// A default cluster is open, says so before sign-up, and takes anyone; an
/// invite presented there is still checked, and an organization's joins it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_open_cluster_takes_anyone() {
    let c = Cluster::start(3).await;
    let info = c
        .client(1)
        .deployments()
        .sign_up_public()
        .await
        .map(wire)
        .unwrap();
    assert_eq!(info["policy"], "open", "{info}");
    assert_eq!(info["inviteRequired"], false, "{info}");
    let (alice, mut a) = sign_up(&c, 0, "alice", None).await.unwrap();
    refused(
        sign_up(&c, 1, "eve", Some("inv_made_up")).await,
        "an unknown invite",
    );
    let (org, token) = org_invite(&alice, &mut a).await;
    let (bob, _b) = sign_up(&c, 2, "bob", Some(&token)).await.unwrap();
    let orgs = bob
        .orgs()
        .get(&org, None, None, None)
        .await
        .map(wire)
        .unwrap();
    assert_eq!(orgs["members"][account_of(&bob).await], "member", "{orgs}");
}

/// Invite only: refused without an invite, with a made-up one and with a
/// used one, before any passkey is asked; taken with the operator's sign-up
/// invite and with an organization invite, which joins the organization.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_invite_only_cluster_takes_invited_people() {
    let c = Cluster::start_with(3, |cfg| cfg.governance.sign_up = SignUp::Invite).await;
    let ops = ops(&c).await;
    let info = c
        .client(1)
        .deployments()
        .sign_up_public()
        .await
        .map(wire)
        .unwrap();
    for (key, want) in [
        ("policy", json!("invite")),
        ("orgInvites", json!(true)),
        ("signUpInvites", json!(true)),
        ("inviteRequired", json!(true)),
    ] {
        assert_eq!(info[key], want, "{key} in {info}");
    }
    // The cluster's name and the tag its passkeys' names carry.
    assert!(info["cluster"].is_string(), "{info}");
    assert!(
        info["passkeyTag"].as_str().is_some_and(|t| t.len() == 4),
        "{info}"
    );
    // Refused at begin: the passkey is never asked.
    let e = c
        .client(0)
        .auth()
        .register_begin(Some("eve"), None, None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("SIGN_UP_REFUSED"), "{e}");
    assert!(e.to_string().contains("invite only"), "{e}");
    refused(sign_up(&c, 0, "eve", None).await, "no invite");
    refused(
        sign_up(&c, 1, "eve", Some("sup_made_up_000000")).await,
        "a made-up invite",
    );

    let _i = operator_invite(&ops, "sup_alice_000000000", 1).await;
    let (alice, mut a) = sign_up(&c, 1, "alice", Some("sup_alice_000000000"))
        .await
        .unwrap();
    refused(
        sign_up(&c, 2, "eve", Some("sup_alice_000000000")).await,
        "a used sign-up invite",
    );

    let (org, token) = org_invite(&alice, &mut a).await;
    let (bob, _b) = sign_up(&c, 2, "bob", Some(&token)).await.unwrap();
    let o = bob
        .orgs()
        .get(&org, None, None, None)
        .await
        .map(wire)
        .unwrap();
    assert_eq!(o["members"][account_of(&bob).await], "member", "{o}");
    refused(
        sign_up(&c, 0, "eve", Some(&token)).await,
        "a used org invite",
    );

    // A begin with an invite and a finish without one: refused at finish.
    let _i = operator_invite(&ops, "sup_carol_000000000", 1).await;
    let client = c.client(0);
    let begin = client
        .auth()
        .register_begin(Some("eve"), None, Some("sup_carol_000000000"))
        .await
        .map(wire)
        .unwrap();
    let mut pk = SoftAuthenticator::new("localhost", ORIGIN);
    let registration = Passkey::create(&mut pk, &begin["options"]);
    let e = client
        .auth()
        .register_finish(
            begin["challengeId"].as_str().unwrap(),
            registration,
            None,
            None,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("SIGN_UP_REFUSED"), "{e}");
}

/// Closed: only someone the operator pre-authorized gets in; an
/// organization invite does not open the door.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_closed_cluster_takes_only_preauthorized_people() {
    let c = Cluster::start_with(3, |cfg| cfg.governance.sign_up = SignUp::Closed).await;
    let ops = ops(&c).await;
    let info = c
        .client(0)
        .deployments()
        .sign_up_public()
        .await
        .map(wire)
        .unwrap();
    assert_eq!(info["policy"], "closed");
    assert_eq!(info["orgInvites"], false);
    refused(sign_up(&c, 0, "eve", None).await, "no pre-authorization");
    let _i = operator_invite(&ops, "sup_alice_000000000", 1).await;
    let (alice, mut a) = sign_up(&c, 0, "alice", Some("sup_alice_000000000"))
        .await
        .unwrap();
    let (_org, token) = org_invite(&alice, &mut a).await;
    refused(sign_up(&c, 1, "bob", Some(&token)).await, "an org invite");

    // The operator lists what it issued, never a token, and withdraws one.
    let _i = operator_invite(&ops, "sup_bob_00000000000", 1).await;
    let listed = ops.ops().sign_up_invite_list().await.map(wire).unwrap();
    assert_eq!(listed["invites"].as_array().unwrap().len(), 1, "{listed}");
    assert!(!listed.to_string().contains("sup_bob"), "{listed}");
    let key = listed["invites"][0]["inviteKey"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ok = ops
        .ops()
        .sign_up_invite_revoke_delete(&key, Some(&harness::operator_sign))
        .await
        .unwrap();
    refused(
        sign_up(&c, 1, "bob", Some("sup_bob_00000000000")).await,
        "a withdrawn invite",
    );
    let e = alice.ops().sign_up_invite_list().await.unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"), "{e}");
}

/// The policy changes only with the operator's signature: the token alone
/// gets the bytes to sign, another key's signature and a replay are refused,
/// and a change logged without one is refused where it applies. The new
/// policy is what clients read and what receipt keys attest.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_policy_change_needs_the_operators_signature() {
    let c = Cluster::start(3).await;
    let ops = ops(&c).await;
    let body = json!({ "policy": "invite" });
    let e = ops
        .transport()
        .json(Request::new(Method::POST, "ops/sign-up").json(body.clone()))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("OPERATOR_SIGNATURE_REQUIRED"), "{e}");
    let mut other = body.clone();
    other["operatorProof"] = {
        use ed25519_dalek::Signer;
        let st = calimero_node::proof::Statement::new("cluster", "__sign_up", &body);
        let sig =
            ed25519_dalek::SigningKey::from_bytes(&[8; 32]).sign(&st.operator_message(&c.name));
        json!({ "nonce": st.nonce, "signature": hex::encode(sig.to_bytes()) })
    };
    let e = ops
        .transport()
        .json(Request::new(Method::POST, "ops/sign-up").json(other))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("BAD_OPERATOR_SIGNATURE"), "{e}");
    // A person cannot change it.
    let (alice, _a) = sign_up(&c, 0, "alice", None).await.unwrap();
    let e = alice
        .transport()
        .json(Request::new(Method::POST, "ops/sign-up").json(body.clone()))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"), "{e}");
    let e = alice
        .transport()
        .json(Request::new(Method::POST, "sign-up").json(body.clone()))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"), "{e}");
    // Logged without a signature (a node's own code): refused at apply.
    let forged = serde_json::to_value(SystemCmd::SetSignUp {
        policy: SignUp::Closed,
        actor: calimero_node::system::OPERATOR.into(),
        proof: None,
        consent: None,
    })
    .unwrap();
    let leader = c
        .nodes
        .iter()
        .flatten()
        .find(|r| r.node.system.status().is_leader)
        .expect("a system leader");
    let (_i, out) = leader
        .node
        .system
        .write(serde_json::to_vec(&forged).unwrap())
        .await
        .unwrap();
    assert!(calimero_node::system::output(&out).is_err());
    assert_eq!(
        c.client(1)
            .deployments()
            .sign_up_public()
            .await
            .map(wire)
            .unwrap()["policy"],
        "open"
    );

    let v = ops_signed(&ops, "ops/sign-up", body).await.unwrap();
    assert_eq!(v["governance"]["signUp"], "invite", "{v}");
    for i in 0..3 {
        assert_eq!(
            c.client(i)
                .deployments()
                .sign_up_public()
                .await
                .map(wire)
                .unwrap()["policy"],
            "invite"
        );
    }
    let public = c
        .client(2)
        .public()
        .releases_public()
        .await
        .map(wire)
        .unwrap();
    assert_eq!(public["governance"]["signUp"], "invite", "{public}");
    refused(sign_up(&c, 1, "bob", None).await, "invite only now");
}

/// Two clusters under one rp-id, as Calimero Cloud's are: a passkey is made
/// under a name that says which cluster it is for, and signing in on the
/// other cluster with it answers `401 PASSKEY_UNKNOWN_HERE`, which names
/// neither the cluster nor the account it belongs to.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_passkey_from_another_cluster_is_named_for_its_own_and_refused_here() {
    // Each test cluster has a name of its own (`test-{pid}-{n}`), and both
    // take `localhost` as their rp-id.
    let home = Cluster::start(1).await;
    let away = Cluster::start(1).await;
    let tag = passkey_tag(&home).await;
    // Test clusters share one storage key (`[42; 32]`), so they share a
    // cluster key and a tag; in production every cluster gets its own KMS
    // identity, so a cluster made again under its old name gets another tag
    // (`backup::tests`: another cluster key, another identity key).
    assert_eq!(tag, passkey_tag(&away).await);
    let shown = format!("{} · alice · {tag}", home.name);
    let begin = home
        .client(0)
        .auth()
        .register_begin(Some("alice"), None, None)
        .await
        .map(wire)
        .unwrap();
    let user = &begin["options"]["user"];
    assert_eq!(user["name"], shown, "{begin}");
    assert_eq!(user["displayName"], shown, "{begin}");

    let (_alice, mut pk) = sign_up(&home, 0, "alice", None).await.unwrap();
    let _s = home.client(0).sign_in(&mut pk).await.unwrap();
    let e = away.client(0).sign_in(&mut pk).await.unwrap_err();
    let calimero_client::Error::Api {
        status,
        code,
        message,
        details,
    } = &e
    else {
        panic!("not an API error: {e}");
    };
    assert_eq!(
        (*status, code.as_str()),
        (401, "PASSKEY_UNKNOWN_HERE"),
        "{e}"
    );
    assert!(message.contains("another Calimero cluster"), "{e}");
    assert!(
        !message.contains(&home.name) && !message.contains("alice"),
        "{e}"
    );
    assert!(details.is_null(), "{e}");
}

/// The tag this cluster's passkeys are named with (`GET /sign-up`, public).
async fn passkey_tag(c: &Cluster) -> String {
    let public = c
        .client(0)
        .deployments()
        .sign_up_public()
        .await
        .map(wire)
        .unwrap();
    let tag = public["passkeyTag"].as_str().unwrap_or_default().to_owned();
    assert_eq!(tag.len(), 4, "{public}");
    assert_eq!(public["cluster"], c.name.as_str(), "{public}");
    tag
}

/// A passkey added to an account (and one a recovery makes) gets a user
/// handle of its own, as a sign-up's does. A password manager keeps one
/// passkey per site and handle: with the account's handle every time, a
/// second passkey added in the same iCloud Keychain or Google Password
/// Manager silently replaced the first, which the account still listed.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn each_added_passkey_gets_a_user_handle_of_its_own() {
    let c = Cluster::start(1).await;
    let tag = passkey_tag(&c).await;
    let (alice, _pk) = sign_up(&c, 0, "alice", None).await.unwrap();
    let first = alice.auth().passkey_add_begin().await.map(wire).unwrap();
    let second = alice.auth().passkey_add_begin().await.map(wire).unwrap();
    let (a, b) = (&first["options"]["user"], &second["options"]["user"]);
    assert!(a["id"].as_str().is_some_and(|id| !id.is_empty()), "{first}");
    assert_ne!(a["id"], b["id"], "{first} {second}");
    assert_eq!(a["name"], format!("{} · alice · {tag}", c.name), "{first}");
}

/// A node creates a minute's accounts at once, then paces them (`SIGN_UPS_PER_MINUTE`, 30 at
/// once, then one per 2 s): past that a sign-up is refused `429 RATE_LIMITED`
/// before its ceremony and creates nothing, while another node of the cluster still takes
/// one; once the wait has passed the node takes one again. Invited sign-ups count.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_node_paces_the_accounts_it_creates() {
    const BURST: usize = calimero_node::api::SIGN_UPS_PER_MINUTE as usize;
    // The harness lifts the pace for other tests; this one runs on the default.
    let c = Cluster::start_with(3, |cfg| {
        cfg.sign_ups_per_minute = calimero_node::api::SIGN_UPS_PER_MINUTE;
    })
    .await;
    let ops = ops(&c).await;
    // Every sign-up on node 0 redeems this invite, so the ones counted are invited.
    let _invite = operator_invite(&ops, "inv_many_sign_ups", 2 * BURST as u32).await;
    let accounts = || c.nodes[0].as_ref().unwrap().node.sys(|s| s.accounts.len());
    let first = accounts();
    let started = std::time::Instant::now();
    let mut taken = 0;
    let refusal = loop {
        match sign_up(&c, 0, &format!("p{taken}"), Some("inv_many_sign_ups")).await {
            Ok(_) => taken += 1,
            Err(e) => break e,
        }
        assert!(taken <= 2 * BURST, "never refused");
    };
    assert_eq!(refusal.code(), Some("RATE_LIMITED"), "{refusal}");
    // The bucket refills while the burst signs up, one token per 2 s.
    let refilled = started.elapsed().as_secs() as usize / 2 + 1;
    assert!(
        (BURST..=BURST + refilled).contains(&taken),
        "{taken} sign-ups before the refusal"
    );
    // Another node's bucket is its own; the refused sign-up made no account.
    let _elsewhere = sign_up(&c, 1, "elsewhere", None).await.unwrap();
    harness::poll::until(harness::poll::EVENTUALLY, "the accounts made", || {
        accounts() == first + taken + 1
    })
    .await;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert_eq!(
        accounts(),
        first + taken + 1,
        "a refused sign-up made an account"
    );

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let _later = sign_up(&c, 0, "later", None).await.unwrap();
}
