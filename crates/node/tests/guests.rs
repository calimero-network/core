//! Guests (`Feature::Guests`, 1.0.0-rc.33): accounts named on one instance (an `account`
//! grant) that are not members of its organization reach it where both the
//! release's signed manifest (`guests: allowed`) and the organization (`allowGuests`)
//! allow; they see nothing else of the organization and manage nothing. An instance's access
//! is a list of grants (`Feature::AccessGrants`): teams, named members and guests together,
//! a guest's grant carrying a scope the app defines. Instance invites
//! (`POST /apps/{app}/invites`), on any instance, name the redeemer on it.
//!
//! Builds the component kv fixture `apps/fixture-kv` from this checkout
//! (see `harness/cluster.rs`).

#![allow(
    clippy::result_large_err,
    reason = "the helpers pass on calimero_client::Error as the client returns it"
)]

use calimero_client::types::ViewsRequestConsistency;
use calimero_client::{Client, Error};
use calimero_identity::testing::SoftAuthenticator;
use serde_json::{json, Value};

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{kv_wasm, signed_up, typed, wire, Cluster};

const ALLOWS: &str = "com.test.guests-allowed";
const FORBIDS: &str = "com.test.guests-forbidden";

/// A signed single-service `.mpk` of `package@1.0.0` declaring `guests`, its WASM tagged
/// so each package ships its own.
fn release(package: &str, guests: Option<calimero_bundle::GuestPolicy>) -> Vec<u8> {
    use sha2::Digest;
    let artifact = |path: &str, bytes: &[u8]| calimero_bundle::BundleArtifact {
        path: path.to_owned(),
        hash: hex::encode(sha2::Sha256::digest(bytes)),
        size: bytes.len() as u64,
    };
    let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
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
        instance_org: None,
        guests,
        outbound: Vec::new(),
        secrets: None,
        uses: None,
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
    s(&c.auth().me().await.map(wire).unwrap()["accountId"])
}

/// An instance of `package` in `org` naming `accounts`, with `k = v` set by its owner.
async fn instance(owner: &Client, org: &str, package: &str, accounts: &[&str]) -> String {
    let app = owner
        .apps()
        .create(typed(json!({
            "name": package, "package": package, "org": org,
            "grants": accounts
                .iter()
                .map(|a| json!({ "kind": "account", "account": a }))
                .collect::<Vec<_>>(),
        })))
        .await
        .unwrap()
        .app_id;
    let _set = owner
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

async fn reads(c: &Client, app: &str) -> Result<Value, Error> {
    c.apps()
        .view(
            app,
            "get",
            Some(json!({ "key": "k" })),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .map(|v| v.result)
}

fn refused(e: &Error, status: &str) {
    let e = e.to_string();
    assert!(e.starts_with(status), "{e}");
}

async fn guests_switch(c: &Client, pk: &mut SoftAuthenticator, org: &str, allow: bool) {
    let _v = c
        .orgs()
        .settings(org, Some(allow), Some(pk))
        .await
        .map(wire)
        .unwrap();
}

/// A listed account outside the organization reaches an instance whose release allows
/// guests, while its organization allows them, and nothing else: not an instance whose
/// release forbids them, not the organization, its members, other apps or settings, and it
/// manages nothing. The organization's switch and taking a guest off the list hold from
/// their next request.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_guest_reaches_one_instance_only_while_both_switches_allow() {
    use calimero_bundle::GuestPolicy;
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let (carol, _c) = signed_up(&c, 2, "carol").await;
    let carol_id = id(&carol).await;
    let bob_id = id(&bob).await;
    let _ok = alice
        .packages()
        .upload(release(ALLOWS, Some(GuestPolicy::Allowed)))
        .await
        .unwrap();
    let _ok = alice
        .packages()
        .upload(release(FORBIDS, None))
        .await
        .unwrap();
    let acme = s(&alice
        .orgs()
        .create("Acme", None, None)
        .await
        .map(wire)
        .unwrap()["orgId"]);
    let open = instance(&alice, &acme, ALLOWS, &[&carol_id]).await;
    let closed = instance(&alice, &acme, FORBIDS, &[&carol_id]).await;
    let other = instance(&alice, &acme, ALLOWS, &[]).await;

    // The release allows guests: carol reaches it, and sees only herself listed.
    assert_eq!(reads(&carol, &open).await.unwrap(), json!("v"));
    let record = carol.apps().get(&open).await.map(wire).unwrap();
    assert_eq!(
        record["grants"],
        json!([{ "kind": "account", "account": carol_id }]),
        "{record}"
    );
    // The release forbids them (absent in its manifest): not even listed.
    refused(&reads(&carol, &closed).await.unwrap_err(), "404");
    // Nothing else of the organization, and she manages nothing.
    refused(&reads(&carol, &other).await.unwrap_err(), "404");
    refused(&carol.orgs().get(&acme).await.unwrap_err(), "404");
    refused(
        &carol.orgs().apps(&acme, None, None).await.unwrap_err(),
        "404",
    );
    let listed: Vec<String> = carol.apps().list(None, None).await.map(wire).unwrap()["apps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| s(&a["appId"]))
        .collect();
    assert_eq!(listed, std::slice::from_ref(&open));
    let e = carol
        .apps()
        .access(&open, vec![typed(json!({ "kind": "anyone" }))])
        .await
        .unwrap_err();
    assert!(
        e.to_string().starts_with("400") || e.to_string().starts_with("403"),
        "{e}"
    );
    let e = carol
        .apps()
        .transfer(&open, &carol_id, None)
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("403"), "{e}");
    // An invite of her own to the instance is a manager's.
    let e = carol
        .apps()
        .invite_create(&open, None, None, None, None)
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("403"), "{e}");

    // The organization turns guests off: refused at her next request; on again: back.
    guests_switch(&alice, &mut a, &acme, false).await;
    assert_eq!(
        alice.orgs().get(&acme).await.map(wire).unwrap()["allowGuests"],
        json!(false)
    );
    refused(&reads(&carol, &open).await.unwrap_err(), "404");
    guests_switch(&alice, &mut a, &acme, true).await;
    assert_eq!(reads(&carol, &open).await.unwrap(), json!("v"));

    // Bob, a member of no organization of alice's and not listed, never reaches it;
    // taken off the list, carol is out too.
    refused(&reads(&bob, &open).await.unwrap_err(), "404");
    let _ok = alice
        .apps()
        .access(
            &open,
            vec![typed(json!({ "kind": "account", "account": bob_id }))],
        )
        .await
        .unwrap();
    refused(&reads(&carol, &open).await.unwrap_err(), "404");
    assert_eq!(reads(&bob, &open).await.unwrap(), json!("v"));
}

/// An instance invite names its redeemer on the instance (a guest, not a member of its
/// organization); one already named spends no use; a revoked invite admits nobody; an
/// instance that takes no guests still takes an invite, which names a member of its
/// organization and refuses an outsider when redeemed, as does one redeemed while the
/// organization's switch is off; an organization-wide instance takes one, whose scope its
/// guest's grant carries.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_instance_invite_makes_its_redeemer_a_guest() {
    use calimero_bundle::GuestPolicy;
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (dave, _d) = signed_up(&c, 1, "dave").await;
    let (erin, _e) = signed_up(&c, 2, "erin").await;
    let _ok = alice
        .packages()
        .upload(release(ALLOWS, Some(GuestPolicy::Allowed)))
        .await
        .unwrap();
    let _ok = alice
        .packages()
        .upload(release(FORBIDS, Some(GuestPolicy::Forbidden)))
        .await
        .unwrap();
    let acme = s(&alice
        .orgs()
        .create("Acme", None, None)
        .await
        .map(wire)
        .unwrap()["orgId"]);
    let open = instance(&alice, &acme, ALLOWS, &[]).await;
    let closed = instance(&alice, &acme, FORBIDS, &[]).await;
    let org_wide = alice
        .apps()
        .create(typed(
            json!({ "name": "o", "package": ALLOWS, "org": acme }),
        ))
        .await
        .unwrap()
        .app_id;

    let token = s(&alice
        .apps()
        .invite_create(&open, None, Some(2), None, Some(&mut a))
        .await
        .map(wire)
        .unwrap()["token"]);
    let listed = alice.apps().invite_list(&open).await.map(wire).unwrap();
    assert_eq!(listed["invites"][0]["usesLeft"], 2, "{listed}");
    assert!(
        !listed.to_string().contains(&token),
        "no token in a listing"
    );
    refused(&reads(&dave, &open).await.unwrap_err(), "404");
    let joined = dave.orgs().invite_redeem(&token).await.map(wire).unwrap();
    assert_eq!(
        (&joined["app"], &joined["org"]),
        (&json!(open), &json!(acme))
    );
    assert_eq!(reads(&dave, &open).await.unwrap(), json!("v"));
    assert!(
        dave.auth().me().await.map(wire).unwrap()["orgs"]
            .as_array()
            .unwrap()
            .is_empty(),
        "a guest is in no organization"
    );
    // One use spent; again, listed already, no use spent.
    let listed = alice.apps().invite_list(&open).await.map(wire).unwrap();
    assert_eq!(listed["invites"][0]["usesLeft"], 1, "{listed}");
    // A redemption is signed over the second it is made in, and each is spent once: the same
    // one again within that second is a replay, so the second try comes a second later.
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    let _again = dave.orgs().invite_redeem(&token).await.unwrap();
    let listed = alice.apps().invite_list(&open).await.map(wire).unwrap();
    assert_eq!(listed["invites"][0]["usesLeft"], 1, "{listed}");

    // Revoked: erin is refused.
    let key = s(&listed["invites"][0]["inviteKey"]);
    let _ok = alice
        .apps()
        .invite_revoke(&open, &key, Some(&mut a))
        .await
        .unwrap();
    assert!(erin.orgs().invite_redeem(&token).await.is_err());
    refused(&reads(&erin, &open).await.unwrap_err(), "404");

    // A release that forbids guests still takes an invite: whether guests may come in is
    // judged when someone redeems it. A member of its organization is named on it; an
    // outsider is refused.
    let token = s(&alice
        .apps()
        .invite_create(&closed, None, Some(2), None, Some(&mut a))
        .await
        .map(wire)
        .unwrap()["token"]);
    let e = erin.orgs().invite_redeem(&token).await.unwrap_err();
    refused(&e, "409");
    assert!(e.to_string().contains("GUESTS_NOT_ALLOWED"), "{e}");
    refused(&reads(&erin, &closed).await.unwrap_err(), "404");
    let (frank, _f) = signed_up(&c, 0, "frank").await;
    let org_token = s(&alice
        .orgs()
        .invite_create(&acme, typed(json!({ "role": "member" })), Some(&mut a))
        .await
        .map(wire)
        .unwrap()["token"]);
    let _joined = frank.orgs().invite_redeem(&org_token).await.unwrap();
    let joined = frank.orgs().invite_redeem(&token).await.map(wire).unwrap();
    assert_eq!(joined["member"], json!(true), "{joined}");
    assert_eq!(reads(&frank, &closed).await.unwrap(), json!("v"));

    // The organization turns guests off after an invite was made: an outsider redeeming it
    // is refused on the state it is judged on, and in once they are on again.
    let token = s(&alice
        .apps()
        .invite_create(&open, None, None, None, Some(&mut a))
        .await
        .map(wire)
        .unwrap()["token"]);
    guests_switch(&alice, &mut a, &acme, false).await;
    let e = erin.orgs().invite_redeem(&token).await.unwrap_err();
    assert!(e.to_string().contains("GUESTS_NOT_ALLOWED"), "{e}");
    refused(&reads(&erin, &open).await.unwrap_err(), "404");
    guests_switch(&alice, &mut a, &acme, true).await;
    let joined = erin.orgs().invite_redeem(&token).await.map(wire).unwrap();
    assert_eq!(joined["member"], json!(false), "{joined}");
    assert_eq!(reads(&erin, &open).await.unwrap(), json!("v"));
    let general = json!({ "channel": "general" });
    let token = s(&alice
        .apps()
        .invite_create(&org_wide, None, None, Some(general.clone()), Some(&mut a))
        .await
        .map(wire)
        .unwrap()["token"]);
    let joined = erin.orgs().invite_redeem(&token).await.map(wire).unwrap();
    assert_eq!(joined["member"], json!(false), "{joined}");
    let record = erin.apps().get(&org_wide).await.map(wire).unwrap();
    assert_eq!(
        record["grants"],
        json!([{ "kind": "account", "account": id(&erin).await, "scope": general }]),
        "a guest sees only their own grant: {record}"
    );
    let all = alice.apps().get(&org_wide).await.map(wire).unwrap();
    assert_eq!(all["grants"][0], json!({ "kind": "org" }), "{all}");
}

/// An instance admits a team and two partners from outside its organization at once (one
/// with a scope the app defines); each grant is judged on its own, so a member outside the
/// team is refused. An invite on it names a member as a named member, and grants set anew
/// replace the list.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_team_and_two_partners_reach_one_instance_by_their_grants() {
    use calimero_bundle::GuestPolicy;
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let (dan, _d) = signed_up(&c, 2, "dan").await;
    let (carol, _c) = signed_up(&c, 0, "carol").await;
    let (erin, _e) = signed_up(&c, 1, "erin").await;
    let (carol_id, erin_id, dan_id) = (id(&carol).await, id(&erin).await, id(&dan).await);
    let _ok = alice
        .packages()
        .upload(release(ALLOWS, Some(GuestPolicy::Allowed)))
        .await
        .unwrap();
    let acme = s(&alice
        .orgs()
        .create("Acme", None, None)
        .await
        .map(wire)
        .unwrap()["orgId"]);
    let team = s(&alice
        .orgs()
        .team_create(&acme, "Deal", typed(json!("invite")), None)
        .await
        .map(wire)
        .unwrap()["teamId"]);
    for (who, teams) in [(&bob, vec![team.as_str()]), (&dan, vec![])] {
        let token = s(&alice
            .orgs()
            .invite_create(
                &acme,
                typed(json!({ "role": "member", "teams": teams })),
                Some(&mut a),
            )
            .await
            .map(wire)
            .unwrap()["token"]);
        let _joined = who.orgs().invite_redeem(&token).await.unwrap();
    }
    let partners = json!({ "channel": "partners" });
    let app = alice
        .apps()
        .create(typed(json!({
            "name": "deal", "package": ALLOWS, "org": acme,
            "grants": [
                { "kind": "team", "team": team },
                { "kind": "account", "account": carol_id },
                { "kind": "account", "account": erin_id, "scope": partners },
            ],
        })))
        .await
        .unwrap()
        .app_id;
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
    for (who, reached) in [(&bob, true), (&carol, true), (&erin, true), (&dan, false)] {
        match reads(who, &app).await {
            Ok(v) => assert!(reached && v == json!("v"), "{v}"),
            Err(e) => {
                assert!(!reached, "{e}");
                refused(&e, "404");
            }
        }
    }
    let seen = erin.apps().get(&app).await.map(wire).unwrap();
    assert_eq!(
        seen["grants"],
        json!([{ "kind": "account", "account": erin_id, "scope": partners }]),
        "{seen}"
    );
    assert_eq!(
        alice.apps().get(&app).await.map(wire).unwrap()["grants"]
            .as_array()
            .map(Vec::len),
        Some(3)
    );

    // Dan, a member outside the team, is named by an invite: a member grant, not a guest.
    let token = s(&alice
        .apps()
        .invite_create(&app, None, None, None, Some(&mut a))
        .await
        .map(wire)
        .unwrap()["token"]);
    let joined = dan.orgs().invite_redeem(&token).await.map(wire).unwrap();
    assert_eq!(joined["member"], json!(true), "{joined}");
    assert_eq!(reads(&dan, &app).await.unwrap(), json!("v"));
    let grants = alice.apps().get(&app).await.map(wire).unwrap()["grants"].clone();
    assert_eq!(
        grants[3],
        json!({ "kind": "account", "account": dan_id }),
        "{grants}"
    );

    // Grants set anew replace the list.
    let _ok = alice
        .apps()
        .access(
            &app,
            vec![typed(json!({ "kind": "account", "account": carol_id }))],
        )
        .await
        .unwrap();
    assert_eq!(reads(&carol, &app).await.unwrap(), json!("v"));
    refused(&reads(&bob, &app).await.unwrap_err(), "404");
}

/// A client from before grants that still sends the single `access` mode is refused, not
/// ignored: absent grants would admit the whole organization, so an instance it meant to keep
/// to a few accounts (a private channel) would open to everyone in it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_old_clients_access_field_is_refused_not_read_as_no_grants() {
    let c = Cluster::start(3).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let bob_id = id(&bob).await;
    let _ok = alice
        .packages()
        .upload(release(ALLOWS, None))
        .await
        .unwrap();
    let acme = s(&alice
        .orgs()
        .create("Acme", None, None)
        .await
        .map(wire)
        .unwrap()["orgId"]);
    let refused_create = alice
        .apps()
        .create(typed(json!({
            "name": "private", "package": ALLOWS, "org": acme,
            "access": { "kind": "accounts", "accounts": [bob_id] },
        })))
        .await
        .map(|r| r.app_id)
        .unwrap_err();
    refused(&refused_create, "400");
    assert!(refused_create.to_string().contains("ACCESS_REMOVED"), "{refused_create}");
    let apps = alice.apps().list(None, None).await.map(wire).unwrap();
    assert_eq!(apps["apps"].as_array().map(Vec::len), Some(0), "{apps}");
}
