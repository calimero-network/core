//! Moving an app instance to another organization (`POST /apps/{app}/move`,
//! `SystemCmd::MoveApp`): its data stays on its replicas; its organization,
//! payer, access and, with `bringMembers`, the people who reach it go to the
//! target. Who may move it is judged on the state of the moment: a member
//! removed from the source, one who left it, and an admin of the target
//! demoted are each refused, as are a mover who is not the target's admin and
//! a move past the target's quota. A personal app moves into an organization
//! its owner administers.
//!
//! Builds the component kv fixture `apps/v2-fixtures/kv` from this checkout
//! (see `harness/cluster.rs`).

#![allow(
    clippy::result_large_err,
    reason = "the helpers pass on calimero_client::Error as the client returns it"
)]

use calimero_client::{Client, Consistency, Error};
use calimero_identity::testing::SoftAuthenticator;
use calimero_node::system::SystemCmd;
use serde_json::{json, Value};

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{kv_wasm, signed_up, Cluster};

const CHAT: &str = "com.test.chat";
const BLOCKS: &str = "com.test.blocks";

/// A signed single-service `.mpk` of `package@1.0.0`, its WASM tagged so each
/// package ships its own.
fn release(package: &str) -> Vec<u8> {
    use sha2::Digest;
    let artifact = |path: &str, bytes: &[u8]| calimero_bundle::BundleArtifact {
        path: path.to_owned(),
        hash: hex::encode(sha2::Sha256::digest(bytes)),
        size: bytes.len() as u64,
    };
    let key = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
    let mut wasm = kv_wasm();
    let tag = package.as_bytes();
    wasm.extend([
        0,
        u8::try_from(1 + tag.len()).unwrap(),
        u8::try_from(tag.len()).unwrap(),
    ]);
    wasm.extend_from_slice(tag);
    let manifest = calimero_bundle::BundleManifest {
        version: "1.0".to_owned(),
        package: package.to_owned(),
        app_version: "1.0.0".to_owned(),
        signer_id: Some(calimero_bundle::derive_signer_id_did_key(
            key.verifying_key().as_bytes(),
        )),
        min_runtime_version: "0.1.0".to_owned(),
        metadata: None,
        handlers: None,
        build_info: None,
        interfaces: None,
        wasm: Some(artifact("app.wasm", wasm.as_slice())),
        compute: None,
        abi: None,
        links: None,
        frontend: None,
        frontend_aliases: Vec::new(),
        upgrades: None,
        outbound: Vec::new(),
        services: None,
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
        ("app.wasm", wasm),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_path(name).unwrap();
        header.set_size(bytes.len() as u64);
        header.set_cksum();
        tar.append(&header, bytes.as_slice()).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

fn s(v: &Value) -> String {
    v.as_str()
        .unwrap_or_else(|| panic!("not a string: {v}"))
        .to_owned()
}

async fn id(c: &Client) -> String {
    s(&c.me().await.unwrap()["accountId"])
}

fn refused(e: &Error, why: &str) {
    let e = e.to_string();
    assert!(e.contains(why), "{e}");
}

async fn new_org(owner: &Client, name: &str) -> String {
    s(&owner.post("orgs", json!({ "name": name })).await.unwrap()["orgId"])
}

/// `who` joins `org` at `role`, invited by `by` (its owner) with `pk`.
async fn join(by: &Client, pk: &mut SoftAuthenticator, org: &str, who: &Client, role: &str) {
    let token = invite(by, pk, org, role, &[]).await;
    let _r = who
        .post(&format!("invites/{token}/redeem"), json!({}))
        .await
        .unwrap();
}

async fn invite(
    by: &Client,
    pk: &mut SoftAuthenticator,
    org: &str,
    role: &str,
    teams: &[&str],
) -> String {
    let v = by
        .post_confirmed(
            &format!("orgs/{org}/invites"),
            json!({ "role": role, "teams": teams }),
            pk,
        )
        .await
        .unwrap();
    s(&v["token"])
}

async fn set_role(by: &Client, pk: &mut SoftAuthenticator, org: &str, account: &str, role: &str) {
    let _ok = by
        .post_confirmed(
            &format!("orgs/{org}/members"),
            json!({ "account": account, "role": role }),
            pk,
        )
        .await
        .unwrap();
}

/// An instance of `package` in `org` (organization access), or a personal one.
async fn instance(c: &Client, org: Option<&str>, package: &str) -> String {
    let opts = org.map_or_else(|| json!({}), |o| json!({ "org": o }));
    c.create_app_from_package(package, package, None, None, opts)
        .await
        .unwrap()
}

async fn move_app(
    c: &Client,
    pk: &mut SoftAuthenticator,
    app: &str,
    to: &str,
    bring: bool,
) -> Result<Value, Error> {
    c.post_confirmed(
        &format!("apps/{app}/move"),
        json!({ "toOrg": to, "bringMembers": bring }),
        pk,
    )
    .await
}

/// The apps of `org` that `c` reaches, as `GET /orgs/{org}/apps` lists them.
async fn reaches(c: &Client, org: &str) -> Vec<String> {
    let Ok(v) = c.get(&format!("orgs/{org}/apps")).await else {
        return Vec::new();
    };
    v["apps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| s(&a["app"]))
        .collect()
}

async fn invites_of(c: &Client, org: &str) -> Vec<Value> {
    c.get(&format!("orgs/{org}/invites")).await.unwrap()["invites"]
        .as_array()
        .unwrap()
        .clone()
}

/// An admin of both organizations moves an app with its people: they join
/// the target (capped at member) with rows on a new open app team there, the
/// app's data is where it was, and the invites into the team it left are
/// revoked while an organization-wide one stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_admin_moves_an_app_with_its_people_and_its_data() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let (carol, _c) = signed_up(&c, 2, "carol").await;
    for p in [CHAT, BLOCKS] {
        let _ok = alice.upload_package(release(p)).await.unwrap();
    }
    let src = new_org(&alice, "Source").await;
    join(&alice, &mut a, &src, &bob, "member").await;
    join(&alice, &mut a, &src, &carol, "admin").await;
    let (bob_id, carol_id) = (id(&bob).await, id(&carol).await);
    let app = instance(&alice, Some(&src), CHAT).await;
    let _set = alice
        .call(&app, "set", json!({ "key": "k", "value": "v" }), None)
        .await
        .unwrap();
    // The source gives its app an app team; invites name it, and one doesn't.
    let _scoped = alice
        .post_confirmed(&format!("orgs/{src}/app-teams"), json!({}), &mut a)
        .await
        .unwrap();
    let team = s(&alice.get(&format!("apps/{app}")).await.unwrap()["access"]["team"]);
    let _into_team = invite(&alice, &mut a, &src, "member", &[&team]).await;
    let _org_wide = invite(&alice, &mut a, &src, "member", &[]).await;
    // The target holds another app on an open app team: standalone.
    let dst = new_org(&alice, "Target").await;
    let _other = instance(&alice, Some(&dst), BLOCKS).await;
    let _scoped = alice
        .post_confirmed(&format!("orgs/{dst}/app-teams"), json!({}), &mut a)
        .await
        .unwrap();

    let out = move_app(&alice, &mut a, &app, &dst, true).await.unwrap();
    assert_eq!(out["org"], json!(dst), "{out}");
    assert_eq!(out["from"], json!(src), "{out}");
    assert_eq!(out["created"], true, "{out}");
    assert_eq!(out["revokedInvites"], 1, "{out}");
    let mut added: Vec<String> = out["added"].as_array().unwrap().iter().map(s).collect();
    added.sort();
    let mut expected = vec![bob_id.clone(), carol_id.clone()];
    expected.sort();
    assert_eq!(added, expected, "{out}");

    let record = alice.get(&format!("apps/{app}")).await.unwrap();
    assert_eq!(record["org"], json!(dst), "{record}");
    assert_eq!(record["access"]["kind"], "team", "{record}");
    let members = alice.get(&format!("orgs/{dst}")).await.unwrap()["members"].clone();
    assert_eq!(members[&bob_id], "member", "{members}");
    assert_eq!(
        members[&carol_id], "member",
        "an admin comes in as a member: {members}"
    );
    // Its people reach it in the target, not in the source, and its data stayed.
    for p in [&alice, &bob, &carol] {
        assert!(reaches(p, &dst).await.contains(&app));
        assert!(!reaches(p, &src).await.contains(&app));
    }
    let got = bob
        .view(&app, "get", json!({ "key": "k" }), Consistency::Strong)
        .await
        .unwrap()
        .result;
    assert_eq!(got, json!("v"));
    let left = invites_of(&alice, &src).await;
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0]["teams"], json!([]), "{left:?}");
}

/// A move without members leaves the people outside the target out; the
/// wrong movers are refused on the state of the moment (a member of the
/// target, a demoted admin of it, a member removed from the source, one who
/// left it, and someone who manages no app), as is a move past the target's
/// file quota; a personal app moves into an organization its owner
/// administers, and no other.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn wrong_movers_and_quotas_are_refused_and_a_personal_app_moves_in() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let (carol, mut cpk) = signed_up(&c, 2, "carol").await;
    let (dave, mut d) = signed_up(&c, 0, "dave").await;
    let _ok = alice.upload_package(release(CHAT)).await.unwrap();
    let src = new_org(&alice, "Source").await;
    let dst = new_org(&alice, "Target").await;
    join(&alice, &mut a, &src, &bob, "admin").await;
    join(&alice, &mut a, &src, &carol, "admin").await;
    join(&alice, &mut a, &src, &dave, "member").await;
    join(&alice, &mut a, &dst, &bob, "member").await;
    join(&alice, &mut a, &dst, &carol, "admin").await;
    let (bob_id, carol_id) = (id(&bob).await, id(&carol).await);
    let app = instance(&alice, Some(&src), CHAT).await;
    let to = json!({ "toOrg": dst, "bringMembers": false });
    let path = format!("apps/{app}/move");

    // Bob administers the source but is a member of the target.
    refused(
        &bob.post(&path, to.clone()).await.unwrap_err(),
        "only an admin or the owner of the target",
    );
    // Made its admin, then demoted again.
    set_role(&alice, &mut a, &dst, &bob_id, "admin").await;
    set_role(&alice, &mut a, &dst, &bob_id, "member").await;
    refused(
        &bob.post(&path, to.clone()).await.unwrap_err(),
        "only an admin or the owner of the target",
    );
    // Its admin again, but removed from the source.
    set_role(&alice, &mut a, &dst, &bob_id, "admin").await;
    let _gone = alice
        .delete_confirmed(&format!("orgs/{src}/members/{bob_id}"), &mut a)
        .await
        .unwrap();
    refused(
        &bob.post(&path, to.clone()).await.unwrap_err(),
        "only the app's owner",
    );
    // Carol administers both, then leaves the source.
    let _left = carol
        .delete_confirmed(&format!("orgs/{src}/members/{carol_id}"), &mut cpk)
        .await
        .unwrap();
    refused(
        &carol.post(&path, to.clone()).await.unwrap_err(),
        "only the app's owner",
    );
    // Dave, a member who manages nothing.
    refused(
        &dave.post(&path, to.clone()).await.unwrap_err(),
        "only the app's owner",
    );
    assert_eq!(
        alice.get(&path.replace("/move", "")).await.unwrap()["org"],
        json!(src)
    );

    // Without members: dave, outside the target, stops reaching it.
    assert!(reaches(&dave, &src).await.contains(&app));
    let out = move_app(&alice, &mut a, &app, &dst, false).await.unwrap();
    assert_eq!(out["added"], json!([]), "{out}");
    assert_eq!(
        out["access"],
        json!({ "kind": "org" }),
        "a legacy target: {out}"
    );
    assert!(!reaches(&dave, &src).await.contains(&app));
    assert!(reaches(&carol, &dst).await.contains(&app));

    // Over the target's file quota: refused, and the app stays.
    let second = instance(&alice, Some(&src), CHAT).await;
    let node = &c.nodes[0].as_ref().unwrap().node;
    for (app, bytes) in [(&second, 100), (&app, 60)] {
        let _ok = node
            .sys_write(&SystemCmd::ChargeFileBytes {
                app_id: app.clone(),
                delta: bytes,
            })
            .await
            .unwrap();
    }
    let ops = c.client(1);
    ops.use_api_key("ops-secret").await;
    let _ok = ops
        .post_operator_signed(
            "ops/quotas",
            json!({ "maxFileBytesPerOrg": 150 }),
            &harness::operator_sign,
        )
        .await
        .unwrap();
    let e = move_app(&alice, &mut a, &second, &dst, false)
        .await
        .unwrap_err();
    refused(&e, "QUOTA_EXCEEDED");
    assert_eq!(
        alice.get(&format!("apps/{second}")).await.unwrap()["org"],
        json!(src)
    );

    // A personal app: into an organization its owner administers, and not
    // into one they are only a member of, or outside.
    let mine = instance(&alice, None, CHAT).await;
    let out = move_app(&alice, &mut a, &mine, &dst, false).await.unwrap();
    assert_eq!(out["org"], json!(dst), "{out}");
    assert_eq!(out["from"], Value::Null, "{out}");
    let daves = instance(&dave, None, CHAT).await;
    refused(
        &move_app(&dave, &mut d, &daves, &src, false)
            .await
            .unwrap_err(),
        "only an admin or the owner of the target",
    );
    refused(
        &move_app(&dave, &mut d, &daves, &dst, false)
            .await
            .unwrap_err(),
        "only an admin or the owner of the target",
    );
}
