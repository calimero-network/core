//! One organization, many apps, separate access: app teams (a top-level team
//! whose `metadata.app.package` names the app's package, one per package),
//! `POST /orgs/{org}/app-teams` (`ScopeApps`) to give apps their own access
//! lists in one step, `GET /orgs/{org}/app-teams`, the invite list and revoke
//! (`GET /orgs/{org}/invites`, `DELETE /orgs/{org}/invites/{key}`), team
//! default capabilities, who changes an app's access, and what a member and a
//! guest see of the organization's people.
//!
//! Builds the component kv fixture `apps/v2-fixtures/kv` from this checkout
//! (see `harness/cluster.rs`).

// Tracking: these tests call the route methods `calimero-client` wrote by hand,
// deprecated for its generated layer (`client.<group>().<method>()`); they move
// over, and the hand-written methods go, in a follow-up.
#![allow(
    deprecated,
    reason = "the hand-written route methods, until these tests use the generated ones"
)]
#![allow(
    clippy::result_large_err,
    reason = "the helpers pass on calimero_client::Error as the client returns it"
)]

use calimero_client::{Client, Error};
use calimero_identity::testing::SoftAuthenticator;
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
const NOTES: &str = "com.test.notes";
const INVITE_MEMBERS: u32 = 1 << 1;
const CREATE_APP: u32 = 1 << 0;
const CREATE_CHILD_TEAM: u32 = 1 << 11;

/// A signed single-service `.mpk` of `package@1.0.0`, its WASM tagged so each
/// package ships its own.
fn release(package: &str) -> Vec<u8> {
    let mut wasm = kv_wasm();
    let tag = package.as_bytes();
    wasm.extend([
        0,
        u8::try_from(1 + tag.len()).unwrap(),
        u8::try_from(tag.len()).unwrap(),
    ]);
    wasm.extend_from_slice(tag);
    release_shipping(package, wasm)
}

/// A signed single-service `.mpk` of `package@1.0.0` shipping `wasm`.
fn release_shipping(package: &str, wasm: Vec<u8>) -> Vec<u8> {
    use sha2::Digest;
    let artifact = |path: &str, bytes: &[u8]| calimero_bundle::BundleArtifact {
        path: path.to_owned(),
        hash: hex::encode(sha2::Sha256::digest(bytes)),
        size: bytes.len() as u64,
    };
    let key = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
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

async fn new_org(owner: &Client) -> String {
    s(&owner.post("orgs", json!({ "name": "Acme" })).await.unwrap()["orgId"])
}

/// An invite into `org` at `role`, naming `teams`, made with `pk`: its token.
async fn invite(
    by: &Client,
    pk: &mut SoftAuthenticator,
    org: &str,
    role: &str,
    teams: &[&str],
) -> Result<String, Error> {
    let v = by
        .post_confirmed(
            &format!("orgs/{org}/invites"),
            json!({ "role": role, "teams": teams }),
            pk,
        )
        .await?;
    Ok(s(&v["token"]))
}

async fn join(c: &Client, token: &str) {
    let _r = c
        .post(&format!("invites/{token}/redeem"), json!({}))
        .await
        .unwrap();
}

/// An instance of `package` in `org` (`access` as `POST /apps` takes it).
async fn instance(c: &Client, org: &str, package: &str, access: Value) -> Result<String, Error> {
    let mut opts = json!({ "org": org });
    if !access.is_null() {
        opts["access"] = access;
    }
    c.create_app_from_package(package, package, None, None, opts)
        .await
}

/// The apps of `org` that `c` reaches, as `GET /orgs/{org}/apps` lists them.
async fn reaches(c: &Client, org: &str) -> Vec<String> {
    let v = c.get(&format!("orgs/{org}/apps")).await.unwrap();
    v["apps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| s(&a["app"]))
        .collect()
}

/// The accounts `c` sees in `org`'s member directory (`GET /orgs/{org}`).
async fn directory(c: &Client, org: &str) -> Vec<String> {
    let v = c.get(&format!("orgs/{org}")).await.unwrap();
    v["members"].as_object().unwrap().keys().cloned().collect()
}

async fn new_team(c: &Client, org: &str, name: &str) -> String {
    s(&c.post(
        &format!("orgs/{org}/teams"),
        json!({ "name": name, "join": "invite" }),
    )
    .await
    .unwrap()["teamId"])
}

/// The invites of `org` that `c` sees (`GET /orgs/{org}/invites`).
async fn invites_of(c: &Client, org: &str) -> Vec<Value> {
    c.get(&format!("orgs/{org}/invites")).await.unwrap()["invites"]
        .as_array()
        .unwrap()
        .clone()
}

async fn scope(
    c: &Client,
    pk: &mut SoftAuthenticator,
    org: &str,
    body: Value,
) -> Result<Value, Error> {
    c.post_confirmed(&format!("orgs/{org}/app-teams"), body, pk)
        .await
}

/// The app team of `package` in `org`, as `GET /orgs/{org}/app-teams` lists it.
async fn app_team(c: &Client, org: &str, package: &str) -> Value {
    let v = c.get(&format!("orgs/{org}/app-teams")).await.unwrap();
    v["appTeams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["package"] == package)
        .cloned()
        .unwrap_or_else(|| panic!("no app team of {package} in {v}"))
}

/// G1: an app's access is changed by its owner, an organization admin, or
/// whoever manages its team's apps (`MANAGE_APPLICATION` there, a lead) and,
/// for another team, that team's too; organization-wide access needs it
/// organization-wide. A plain member never.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_admin_and_the_apps_team_manager_change_its_access_not_only_its_owner() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let (carol, _c) = signed_up(&c, 2, "carol").await;
    let org = new_org(&alice).await;
    for p in [&bob, &carol] {
        join(
            p,
            &invite(&alice, &mut a, &org, "member", &[]).await.unwrap(),
        )
        .await;
    }
    let (bob_id, carol_id) = (id(&bob).await, id(&carol).await);
    let _ok = alice
        .post_confirmed(
            &format!("orgs/{org}/capabilities"),
            json!({ "account": bob_id, "capabilities": CREATE_APP }),
            &mut a,
        )
        .await
        .unwrap();
    let bundle = bob.upload_bundle(kv_wasm()).await.unwrap();
    // Bob's app, reached by the organization.
    let app = bob
        .create_app("b", &bundle, json!({ "org": org }))
        .await
        .unwrap();
    let (t1, t2) = (
        new_team(&alice, &org, "one").await,
        new_team(&alice, &org, "two").await,
    );
    let access = |t: &str| json!({ "access": { "kind": "team", "team": t } });

    // A plain member is refused; an admin (not the app's owner) is not.
    refused(
        &carol
            .post(&format!("apps/{app}/access"), access(&t1))
            .await
            .unwrap_err(),
        "MANAGE_APPLICATION",
    );
    let _ok = alice
        .post(&format!("apps/{app}/access"), access(&t1))
        .await
        .unwrap();

    // Carol leads team one: she moves its app only to a team she manages too,
    // and never to organization-wide access.
    let _ok = alice
        .post_confirmed(
            &format!("orgs/{org}/teams/{t1}/members"),
            json!({ "account": carol_id, "lead": true }),
            &mut a,
        )
        .await
        .unwrap();
    refused(
        &carol
            .post(&format!("apps/{app}/access"), access(&t2))
            .await
            .unwrap_err(),
        "MANAGE_APPLICATION",
    );
    refused(
        &carol
            .post(
                &format!("apps/{app}/access"),
                json!({ "access": { "kind": "org" } }),
            )
            .await
            .unwrap_err(),
        "MANAGE_APPLICATION",
    );
    let _ok = alice
        .post_confirmed(
            &format!("orgs/{org}/teams/{t2}/members"),
            json!({ "account": carol_id, "lead": true }),
            &mut a,
        )
        .await
        .unwrap();
    let _ok = carol
        .post(&format!("apps/{app}/access"), access(&t2))
        .await
        .unwrap();
    // The owner, not in team two, holds no owner powers over an app on it;
    // once in it, they may change its access anywhere in the organization.
    let to_org = json!({ "access": { "kind": "org" } });
    refused(
        &bob.post(&format!("apps/{app}/access"), to_org.clone())
            .await
            .unwrap_err(),
        "MANAGE_APPLICATION",
    );
    let _ok = alice
        .post_confirmed(
            &format!("orgs/{org}/teams/{t2}/members"),
            json!({ "account": bob_id, "role": "member" }),
            &mut a,
        )
        .await
        .unwrap();
    let _ok = bob
        .post(&format!("apps/{app}/access"), to_org)
        .await
        .unwrap();
}

/// G2: pending invites are listed (key, terms, creator, creation time, never
/// a token) to whoever may revoke them: an admin all, a team's lead those
/// naming only their teams, a creator their own; a revoked invite redeems no
/// more. Revoking takes the actor's passkey.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn invites_are_listed_and_revoked_by_whoever_may_see_them() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, mut b) = signed_up(&c, 1, "bob").await;
    let (carol, mut cpk) = signed_up(&c, 2, "carol").await;
    let (dave, _d) = signed_up(&c, 0, "dave").await;
    let org = new_org(&alice).await;
    for p in [&bob, &carol] {
        join(
            p,
            &invite(&alice, &mut a, &org, "member", &[]).await.unwrap(),
        )
        .await;
    }
    let (alice_id, bob_id) = (id(&alice).await, id(&bob).await);
    let team = s(&alice
        .post(
            &format!("orgs/{org}/teams"),
            json!({ "name": "design", "join": "invite" }),
        )
        .await
        .unwrap()["teamId"]);
    let _ok = alice
        .post_confirmed(
            &format!("orgs/{org}/teams/{team}/members"),
            json!({ "account": bob_id, "lead": true }),
            &mut a,
        )
        .await
        .unwrap();
    let alices = invite(&alice, &mut a, &org, "member", &[]).await.unwrap();
    let bobs = invite(&bob, &mut b, &org, "member", &[&team])
        .await
        .unwrap();

    let all = invites_of(&alice, &org).await;
    assert_eq!(all.len(), 2, "{all:?}");
    let of_bob = all.iter().find(|i| i["createdBy"] == bob_id).unwrap();
    assert_eq!(of_bob["teams"], json!([team]), "{of_bob}");
    assert_eq!(of_bob["role"], "member", "{of_bob}");
    assert_eq!(of_bob["usesLeft"], 1, "{of_bob}");
    assert!(of_bob["createdAt"].as_u64().unwrap() > 0, "{of_bob}");
    assert!(of_bob["inviteKey"].is_string(), "{of_bob}");
    assert!(
        !serde_json::to_string(&all).unwrap().contains(&bobs),
        "a token is never listed"
    );
    assert!(all.iter().any(|i| i["createdBy"] == alice_id));
    let bob_sees = invites_of(&bob, &org).await;
    assert_eq!(bob_sees.len(), 1, "{bob_sees:?}");
    assert_eq!(bob_sees[0]["createdBy"], bob_id);
    assert!(invites_of(&carol, &org).await.is_empty());
    let outside = dave.get(&format!("orgs/{org}/invites")).await.unwrap_err();
    assert!(outside.to_string().starts_with("404"), "{outside}");

    // Carol may not revoke Bob's (it reads as no invite); Bob may, with his
    // passkey, and its token then redeems nothing.
    let bob_key = s(&of_bob["inviteKey"]);
    let gone = carol
        .delete_confirmed(&format!("orgs/{org}/invites/{bob_key}"), &mut cpk)
        .await
        .unwrap_err();
    assert!(gone.to_string().starts_with("404"), "{gone}");
    let _ok = bob
        .delete_confirmed(&format!("orgs/{org}/invites/{bob_key}"), &mut b)
        .await
        .unwrap();
    refused(
        &dave
            .post(&format!("invites/{bobs}/redeem"), json!({}))
            .await
            .unwrap_err(),
        "used up, revoked or never existed",
    );
    assert!(invites_of(&bob, &org).await.is_empty());
    // The admin revokes any, and the one left still works until then.
    let alice_key = s(&invites_of(&alice, &org).await[0]["inviteKey"]);
    let _ok = alice
        .delete_confirmed(&format!("orgs/{org}/invites/{alice_key}"), &mut a)
        .await
        .unwrap();
    assert!(invites_of(&alice, &org).await.is_empty());
    refused(
        &dave
            .post(&format!("invites/{alices}/redeem"), json!({}))
            .await
            .unwrap_err(),
        "used up, revoked or never existed",
    );
}

/// G3, G9 and who installs: an admin converts an organization whose apps
/// use organization access into app teams in one step; everyone who reached
/// an app keeps it, two apps make the organization shared (every app team
/// closed), a package has one app team, and only an admin installs a new
/// app or changes app teams. A member of a shared organization sees the
/// people of the apps they are in (and its admins); an admin everyone.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn scoping_apps_keeps_everyone_in_and_separates_the_apps() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, mut b) = signed_up(&c, 1, "bob").await;
    let (carol, _c) = signed_up(&c, 2, "carol").await;
    let (dave, _d) = signed_up(&c, 0, "dave").await;
    for p in [CHAT, BLOCKS, NOTES] {
        let _ok = alice.upload_package(release(p)).await.unwrap();
    }
    let org = new_org(&alice).await;
    for p in [&bob, &carol] {
        join(
            p,
            &invite(&alice, &mut a, &org, "member", &[]).await.unwrap(),
        )
        .await;
    }
    let (alice_id, bob_id, carol_id) = (id(&alice).await, id(&bob).await, id(&carol).await);
    // Legacy: organization access, every member reaches both apps.
    let chat = instance(&alice, &org, CHAT, Value::Null).await.unwrap();
    let blocks = instance(&alice, &org, BLOCKS, Value::Null).await.unwrap();
    assert_eq!(
        alice.get(&format!("orgs/{org}")).await.unwrap()["mode"],
        "legacy"
    );
    for p in [&bob, &carol] {
        let r = reaches(p, &org).await;
        assert!(r.contains(&chat) && r.contains(&blocks), "{r:?}");
    }

    // Only an admin scopes, with a passkey.
    refused(
        &scope(&bob, &mut b, &org, json!({})).await.unwrap_err(),
        "only the organization's owner and admins",
    );
    let done = scope(&alice, &mut a, &org, json!({})).await.unwrap();
    assert_eq!(done["mode"], "shared", "{done}");
    assert_eq!(done["teams"].as_array().unwrap().len(), 2, "{done}");
    let chat_team = app_team(&bob, &org, CHAT).await;
    let blocks_team = app_team(&bob, &org, BLOCKS).await;
    assert_eq!(chat_team["join"], "invite", "{chat_team}");
    assert_eq!(chat_team["memberCount"], 3, "{chat_team}");
    assert_eq!(chat_team["member"], true, "{chat_team}");
    assert_eq!(chat_team["manages"], false, "{chat_team}");
    let (chat_team, blocks_team) = (s(&chat_team["teamId"]), s(&blocks_team["teamId"]));
    let app = alice.get(&format!("apps/{chat}")).await.unwrap();
    assert_eq!(
        app["access"],
        json!({ "kind": "team", "team": chat_team }),
        "{app}"
    );
    // Nobody lost an app.
    for p in [&bob, &carol] {
        let r = reaches(p, &org).await;
        assert!(r.contains(&chat) && r.contains(&blocks), "{r:?}");
    }
    // Scoping again changes nothing: the packages' teams are found.
    let again = scope(
        &alice,
        &mut a,
        &org,
        json!({ "apps": [{ "package": CHAT }] }),
    )
    .await
    .unwrap();
    assert_eq!(again["teams"][0]["teamId"], chat_team, "{again}");
    assert_eq!(again["teams"][0]["created"], false, "{again}");

    // Carol leaves Blocks: she keeps Chat only. Dave joins with an invite to
    // Blocks only: he gets Blocks only, as the org's new member.
    let _ok = alice
        .delete_confirmed(
            &format!("orgs/{org}/teams/{blocks_team}/members/{carol_id}"),
            &mut a,
        )
        .await
        .unwrap();
    assert_eq!(reaches(&carol, &org).await, vec![chat.clone()]);
    join(
        &dave,
        &invite(&alice, &mut a, &org, "member", &[&blocks_team])
            .await
            .unwrap(),
    )
    .await;
    let dave_id = id(&dave).await;
    assert_eq!(reaches(&dave, &org).await, vec![blocks.clone()]);

    // Directory: Dave (Blocks) sees Bob (Blocks) and Alice (the owner), not
    // Carol (Chat only); Carol sees no Dave; the owner sees everyone.
    let seen = directory(&dave, &org).await;
    assert!(
        seen.contains(&bob_id) && seen.contains(&alice_id),
        "{seen:?}"
    );
    assert!(!seen.contains(&carol_id), "{seen:?}");
    assert!(!directory(&carol, &org).await.contains(&dave_id));
    assert_eq!(directory(&alice, &org).await.len(), 4);
    let full = dave.get(&format!("orgs/{org}")).await.unwrap();
    assert!(
        !serde_json::to_string(&full).unwrap().contains(&carol_id),
        "{full}"
    );

    // One app team per package; only an admin marks or unmarks one.
    let other = s(&alice
        .post(
            &format!("orgs/{org}/teams"),
            json!({ "name": "chat 2", "join": "invite" }),
        )
        .await
        .unwrap()["teamId"]);
    refused(
        &alice
            .post(
                &format!("orgs/{org}/teams/{other}"),
                json!({ "metadata": { "app": { "package": CHAT } } }),
            )
            .await
            .unwrap_err(),
        "has an app team in this organization already",
    );
    let _ok = alice
        .post_confirmed(
            &format!("orgs/{org}/teams/{chat_team}/members"),
            json!({ "account": bob_id, "lead": true }),
            &mut a,
        )
        .await
        .unwrap();
    refused(
        &bob.post(
            &format!("orgs/{org}/teams/{chat_team}"),
            json!({ "metadata": { "color": "red" } }),
        )
        .await
        .unwrap_err(),
        "only the organization's owner and admins",
    );
    // Bob, Chat's lead with CREATE_APP everywhere, makes Chat instances, but
    // installing Notes, a package with no app team here, is an admin's.
    let _ok = alice
        .post_confirmed(
            &format!("orgs/{org}/capabilities"),
            json!({ "account": bob_id, "capabilities": CREATE_APP }),
            &mut a,
        )
        .await
        .unwrap();
    let _second = instance(
        &bob,
        &org,
        CHAT,
        json!({ "kind": "team", "team": chat_team }),
    )
    .await
    .unwrap();
    refused(
        &instance(&bob, &org, NOTES, Value::Null).await.unwrap_err(),
        "only the organization's owner and admins",
    );
    let _notes = instance(&alice, &org, NOTES, Value::Null).await.unwrap();
}

/// Two packages may ship a byte-identical component: each instance belongs to the
/// package its release names, so scoping gives each package's team its own instances
/// (it used to judge by the component, whose record names the first package installed,
/// and put both instances in that package's team).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn scoping_two_packages_that_ship_one_component_keeps_their_instances_apart() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    for p in [CHAT, NOTES] {
        let _ok = alice
            .upload_package(release_shipping(p, kv_wasm()))
            .await
            .unwrap();
    }
    // Naming both packages, and naming none (every package with an instance).
    let both = json!({ "apps": [{ "package": CHAT }, { "package": NOTES }] });
    for body in [both, json!({})] {
        let org = new_org(&alice).await;
        let chat = instance(&alice, &org, CHAT, Value::Null).await.unwrap();
        let notes = instance(&alice, &org, NOTES, Value::Null).await.unwrap();
        let done = scope(&alice, &mut a, &org, body).await.unwrap();
        assert_eq!(done["teams"].as_array().unwrap().len(), 2, "{done}");
        for (app, package) in [(&chat, CHAT), (&notes, NOTES)] {
            let team = s(&app_team(&alice, &org, package).await["teamId"]);
            let record = alice.get(&format!("apps/{app}")).await.unwrap();
            assert_eq!(
                record["access"],
                json!({ "kind": "team", "team": team }),
                "{package}: {record}"
            );
        }
    }
}

/// G6 and the guest role: in a standalone organization (one open app team)
/// its members invite others to the app; once the organization is shared
/// only leads and admins do, until an admin turns on the team's default
/// capability. Where apps have app teams, a guest reaches only the apps of
/// the teams they are in; a guest sees only those teams' people.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn members_invite_to_a_standalone_app_and_guests_get_only_their_apps() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, mut b) = signed_up(&c, 1, "bob").await;
    let (eve, _e) = signed_up(&c, 2, "eve").await;
    let (gail, _g) = signed_up(&c, 0, "gail").await;
    for p in [CHAT, BLOCKS] {
        let _ok = alice.upload_package(release(p)).await.unwrap();
    }
    let org = new_org(&alice).await;
    join(
        &bob,
        &invite(&alice, &mut a, &org, "member", &[]).await.unwrap(),
    )
    .await;
    let bob_id = id(&bob).await;
    let chat = instance(&alice, &org, CHAT, Value::Null).await.unwrap();

    // Standalone: one open app team, whose members may invite to it.
    let done = scope(&alice, &mut a, &org, json!({})).await.unwrap();
    assert_eq!(done["mode"], "standalone", "{done}");
    let chat_team = s(&done["teams"][0]["teamId"]);
    let t = app_team(&bob, &org, CHAT).await;
    assert_eq!(t["join"], "everyone", "{t}");
    assert_eq!(
        t["defaultCapabilities"],
        INVITE_MEMBERS | CREATE_CHILD_TEAM,
        "{t}"
    );
    assert!(reaches(&bob, &org).await.contains(&chat));
    // A guest is not in an open team by inheritance, reaches no
    // organization-access app here, and sees no one but themselves.
    let _legacy = instance(&alice, &org, CHAT, json!({ "kind": "org" }))
        .await
        .unwrap();
    join(
        &eve,
        &invite(&alice, &mut a, &org, "guest", &[]).await.unwrap(),
    )
    .await;
    let eve_id = id(&eve).await;
    assert!(reaches(&eve, &org).await.is_empty());
    assert_eq!(directory(&eve, &org).await, vec![eve_id.clone()]);
    let to_chat = invite(&bob, &mut b, &org, "member", &[&chat_team])
        .await
        .unwrap();
    join(&gail, &to_chat).await;
    assert!(reaches(&gail, &org).await.contains(&chat));
    // Never organization-wide, never above member.
    refused(
        &invite(&bob, &mut b, &org, "member", &[]).await.unwrap_err(),
        "INVITE_MEMBERS",
    );

    // Shared: Blocks connects; Chat closes, everyone kept, and members
    // invite no more until an admin says so for Chat.
    let _blocks = instance(&alice, &org, BLOCKS, Value::Null).await.unwrap();
    let done = scope(
        &alice,
        &mut a,
        &org,
        json!({ "apps": [{ "package": BLOCKS }] }),
    )
    .await
    .unwrap();
    assert_eq!(done["mode"], "shared", "{done}");
    assert!(reaches(&bob, &org).await.contains(&chat));
    assert!(reaches(&gail, &org).await.contains(&chat));
    refused(
        &invite(&bob, &mut b, &org, "member", &[&chat_team])
            .await
            .unwrap_err(),
        "INVITE_MEMBERS",
    );
    refused(
        &bob.post_confirmed(
            &format!("orgs/{org}/capabilities"),
            json!({ "team": chat_team, "capabilities": INVITE_MEMBERS }),
            &mut b,
        )
        .await
        .unwrap_err(),
        "not allowed to manage this team",
    );
    let _ok = alice
        .post_confirmed(
            &format!("orgs/{org}/capabilities"),
            json!({ "team": chat_team, "capabilities": INVITE_MEMBERS }),
            &mut a,
        )
        .await
        .unwrap();
    let _again = invite(&bob, &mut b, &org, "member", &[&chat_team])
        .await
        .unwrap();
    let team = alice
        .get(&format!("orgs/{org}/teams/{chat_team}"))
        .await
        .unwrap();
    assert_eq!(team["package"], CHAT, "{team}");
    assert_eq!(team["defaultCapabilities"], INVITE_MEMBERS, "{team}");

    // The guest, invited to Chat, has Chat and its people only.
    let eve_chat = invite(&alice, &mut a, &org, "guest", &[&chat_team])
        .await
        .unwrap();
    join(&eve, &eve_chat).await;
    assert_eq!(reaches(&eve, &org).await, vec![chat.clone()]);
    assert!(directory(&eve, &org).await.contains(&bob_id));
    let teams = eve.get(&format!("orgs/{org}/app-teams")).await.unwrap();
    assert_eq!(teams["appTeams"].as_array().unwrap().len(), 1, "{teams}");
}

/// `c` writes `app` (the kv fixture's `set`): `Ok` when it reaches it.
async fn writes(c: &Client, app: &str) -> Result<(), Error> {
    c.call(app, "set", json!({ "key": "k", "value": "v" }), None)
        .await
        .map(drop)
}

/// Where an app has an app team, its `accounts` instances (DMs, private
/// channels) are reached only by those in the app team, their owner too:
/// someone kicked from Chat's team, who left it, who lost it when it closed,
/// or who was removed from the organization is out of Chat's DMs, though
/// listed in them; back in the team they have them again. A package with no
/// app team keeps its lists alone, and nobody outside the organization
/// reaches any.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_dm_is_reached_only_by_those_in_its_apps_team() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, mut b) = signed_up(&c, 1, "bob").await;
    let (carol, _cpk) = signed_up(&c, 2, "carol").await;
    let (dave, _d) = signed_up(&c, 0, "dave").await;
    let (frank, _f) = signed_up(&c, 1, "frank").await;
    let (erin, _e) = signed_up(&c, 2, "erin").await;
    for p in [CHAT, NOTES] {
        let _ok = alice.upload_package(release(p)).await.unwrap();
    }
    let org = new_org(&alice).await;
    for p in [&bob, &carol, &frank] {
        join(
            p,
            &invite(&alice, &mut a, &org, "member", &[]).await.unwrap(),
        )
        .await;
    }
    let (bob_id, carol_id, frank_id) = (id(&bob).await, id(&carol).await, id(&frank).await);
    let _chat = instance(&alice, &org, CHAT, Value::Null).await.unwrap();
    // Chat's team, open: everyone here now gets a row.
    let done = scope(&alice, &mut a, &org, json!({ "shared": false }))
        .await
        .unwrap();
    let chat_team = s(&done["teams"][0]["teamId"]);
    // Dave joins after: in Chat by inheritance only.
    join(
        &dave,
        &invite(&alice, &mut a, &org, "member", &[]).await.unwrap(),
    )
    .await;
    let dave_id = id(&dave).await;
    let everyone = json!([bob_id, carol_id, dave_id, frank_id]);
    let listing = |ids: &Value| json!({ "kind": "accounts", "accounts": ids });
    let dm = instance(&alice, &org, CHAT, listing(&everyone))
        .await
        .unwrap();
    let notes = instance(&alice, &org, NOTES, listing(&everyone))
        .await
        .unwrap();
    // Carol's own DM with Bob.
    let _ok = alice
        .post_confirmed(
            &format!("orgs/{org}/capabilities"),
            json!({ "account": carol_id, "capabilities": CREATE_APP | 1 << 2 }),
            &mut a,
        )
        .await
        .unwrap();
    let carols = instance(&carol, &org, CHAT, listing(&json!([bob_id])))
        .await
        .unwrap();
    for (who, app) in [
        (&bob, &dm),
        (&carol, &dm),
        (&dave, &dm),
        (&frank, &dm),
        (&carol, &carols),
        (&bob, &carols),
    ] {
        writes(who, app).await.unwrap();
    }

    // Carol is kicked from Chat; Bob stays; Frank is removed from the
    // organization.
    let _ok = alice
        .delete_confirmed(
            &format!("orgs/{org}/teams/{chat_team}/members/{carol_id}"),
            &mut a,
        )
        .await
        .unwrap();
    let _ok = alice
        .delete_confirmed(&format!("orgs/{org}/members/{frank_id}"), &mut a)
        .await
        .unwrap();
    // Chat closes: Dave, with no row, is out.
    let _ok = alice
        .post(
            &format!("orgs/{org}/teams/{chat_team}"),
            json!({ "join": "invite" }),
        )
        .await
        .unwrap();
    for (who, app) in [
        (&carol, &dm),
        (&carol, &carols),
        (&dave, &dm),
        (&frank, &dm),
        (&erin, &dm),
    ] {
        let err = writes(who, app).await.unwrap_err();
        assert_eq!(err.code(), Some("NOT_FOUND"), "{app}: {err}");
    }
    writes(&bob, &dm).await.unwrap();
    writes(&bob, &carols).await.unwrap();
    // Notes has no app team: its list alone decides.
    writes(&carol, &notes).await.unwrap();
    writes(&dave, &notes).await.unwrap();

    // Bob leaves Chat himself.
    let _ok = bob
        .delete_confirmed(
            &format!("orgs/{org}/teams/{chat_team}/members/{bob_id}"),
            &mut b,
        )
        .await
        .unwrap();
    let err = writes(&bob, &dm).await.unwrap_err();
    assert_eq!(err.code(), Some("NOT_FOUND"), "{err}");

    // Back in Chat, Carol has both DMs again.
    let _ok = alice
        .post_confirmed(
            &format!("orgs/{org}/teams/{chat_team}/members"),
            json!({ "account": carol_id, "role": "member" }),
            &mut a,
        )
        .await
        .unwrap();
    writes(&carol, &dm).await.unwrap();
    writes(&carol, &carols).await.unwrap();
}

/// Any member of an app team makes a private channel below it (a restricted
/// child team, `CREATE_CHILD_TEAM` in the app team's default) and leads it;
/// never an open one. Someone out of the app team (kicked, never in it, of
/// another organization) makes none, and someone removed from the app team,
/// or who leaves it, is out of the channels below it too.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn members_of_an_app_team_make_private_channels_below_it() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let (carol, mut cpk) = signed_up(&c, 2, "carol").await;
    let (dave, _d) = signed_up(&c, 0, "dave").await;
    let (gail, _g) = signed_up(&c, 1, "gail").await;
    let (erin, _e) = signed_up(&c, 2, "erin").await;
    let _ok = alice.upload_package(release(CHAT)).await.unwrap();
    let org = new_org(&alice).await;
    for p in [&bob, &carol, &dave] {
        join(
            p,
            &invite(&alice, &mut a, &org, "member", &[]).await.unwrap(),
        )
        .await;
    }
    let (bob_id, carol_id, dave_id) = (id(&bob).await, id(&carol).await, id(&dave).await);
    let _chat = instance(&alice, &org, CHAT, Value::Null).await.unwrap();
    // Chat's team, closed, with everyone in it.
    let done = scope(&alice, &mut a, &org, json!({ "shared": true }))
        .await
        .unwrap();
    let chat_team = s(&done["teams"][0]["teamId"]);
    let t = app_team(&alice, &org, CHAT).await;
    assert_eq!(t["defaultCapabilities"], CREATE_CHILD_TEAM, "{t}");
    // Gail joins after: not in Chat.
    join(
        &gail,
        &invite(&alice, &mut a, &org, "member", &[]).await.unwrap(),
    )
    .await;
    let channel = |join: &str| json!({ "name": "design", "join": join, "parent": chat_team });
    let teams = format!("orgs/{org}/teams");

    // Carol makes a private channel and leads it: she adds Dave.
    let private = s(&carol.post(&teams, channel("invite")).await.unwrap()["teamId"]);
    let _ok = carol
        .post_confirmed(
            &format!("orgs/{org}/teams/{private}/members"),
            json!({ "account": dave_id, "role": "member" }),
            &mut cpk,
        )
        .await
        .unwrap();
    let view = alice
        .get(&format!("orgs/{org}/teams/{private}"))
        .await
        .unwrap();
    assert!(
        view.to_string().contains(&carol_id) && view.to_string().contains(&dave_id),
        "{view}"
    );
    // Never an open one; never by someone out of Chat.
    refused(
        &carol.post(&teams, channel("everyone")).await.unwrap_err(),
        "not allowed to create this team",
    );
    refused(
        &gail.post(&teams, channel("invite")).await.unwrap_err(),
        "not allowed to create this team",
    );
    let _outside = erin.post(&teams, channel("invite")).await.unwrap_err();
    let _ok = alice
        .delete_confirmed(
            &format!("orgs/{org}/teams/{chat_team}/members/{bob_id}"),
            &mut a,
        )
        .await
        .unwrap();
    refused(
        &bob.post(&teams, channel("invite")).await.unwrap_err(),
        "not allowed to create this team",
    );

    // Dave, removed from Chat, and Carol, who leaves it, are out of the
    // channel below it.
    let _ok = alice
        .delete_confirmed(
            &format!("orgs/{org}/teams/{chat_team}/members/{dave_id}"),
            &mut a,
        )
        .await
        .unwrap();
    let _ok = carol
        .delete_confirmed(
            &format!("orgs/{org}/teams/{chat_team}/members/{carol_id}"),
            &mut cpk,
        )
        .await
        .unwrap();
    let view = alice
        .get(&format!("orgs/{org}/teams/{private}"))
        .await
        .unwrap();
    assert!(
        !view.to_string().contains(&carol_id) && !view.to_string().contains(&dave_id),
        "a row is left below: {view}"
    );
    assert!(dave
        .get(&format!("orgs/{org}/teams/{private}"))
        .await
        .is_err());
}

/// An admin closes one app's team and no other, in one step, keeping either
/// everyone who reaches it now or only the people named: whoever is not
/// kept loses the app and the private channels under its team, and nobody
/// else loses anything. Opening one team again is one step too. Only an
/// organization admin does either, and only for the organization's members.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_admin_closes_one_app_team_keeping_whom_they_name() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, mut b) = signed_up(&c, 1, "bob").await;
    let (carol, _c) = signed_up(&c, 2, "carol").await;
    let (dave, mut d) = signed_up(&c, 0, "dave").await;
    let (frank, _f) = signed_up(&c, 1, "frank").await;
    let (erin, mut e) = signed_up(&c, 2, "erin").await;
    for p in [CHAT, BLOCKS] {
        let _ok = alice.upload_package(release(p)).await.unwrap();
    }
    let org = new_org(&alice).await;
    for p in [&bob, &carol, &dave, &frank] {
        join(
            p,
            &invite(&alice, &mut a, &org, "member", &[]).await.unwrap(),
        )
        .await;
    }
    let (bob_id, carol_id, dave_id, frank_id) = (
        id(&bob).await,
        id(&carol).await,
        id(&dave).await,
        id(&frank).await,
    );
    // Erin is an admin of an organization of her own, never of this one.
    let _theirs = new_org(&erin).await;
    let erin_id = id(&erin).await;
    let chat = instance(&alice, &org, CHAT, Value::Null).await.unwrap();
    let blocks = instance(&alice, &org, BLOCKS, Value::Null).await.unwrap();
    // Both apps get open teams: everyone has both.
    let done = scope(&alice, &mut a, &org, json!({ "shared": false }))
        .await
        .unwrap();
    assert_eq!(done["mode"], "standalone", "{done}");
    let chat_team = s(&app_team(&alice, &org, CHAT).await["teamId"]);
    // Carol leads a private channel under Chat, with an instance of its own.
    let channel = s(&alice
        .post(
            &format!("orgs/{org}/teams"),
            json!({ "name": "carol's", "join": "invite", "parent": chat_team }),
        )
        .await
        .unwrap()["teamId"]);
    let _ok = alice
        .post_confirmed(
            &format!("orgs/{org}/teams/{channel}/members"),
            json!({ "account": carol_id, "lead": true }),
            &mut a,
        )
        .await
        .unwrap();
    let private = instance(
        &alice,
        &org,
        CHAT,
        json!({ "kind": "team", "team": channel }),
    )
    .await
    .unwrap();
    assert!(reaches(&carol, &org).await.contains(&private));
    // Dave left Chat; Frank is removed from the organization.
    let _ok = dave
        .delete_confirmed(
            &format!("orgs/{org}/teams/{chat_team}/members/{dave_id}"),
            &mut d,
        )
        .await
        .unwrap();
    let _ok = alice
        .delete_confirmed(&format!("orgs/{org}/members/{frank_id}"), &mut a)
        .await
        .unwrap();

    let close = |keep: Value| {
        let mut app = json!({ "package": CHAT, "join": "invite" });
        if !keep.is_null() {
            app["keep"] = keep;
        }
        json!({ "apps": [app] })
    };
    // Only an admin of this organization closes it, keeping only its members.
    refused(
        &scope(&bob, &mut b, &org, close(json!([bob_id])))
            .await
            .unwrap_err(),
        "only the organization's owner and admins",
    );
    let _outside = scope(&erin, &mut e, &org, close(json!([erin_id])))
        .await
        .unwrap_err();
    for gone in [&erin_id, &frank_id] {
        refused(
            &scope(&alice, &mut a, &org, close(json!([bob_id, gone])))
                .await
                .unwrap_err(),
            "only members of the organization keep an app",
        );
    }
    refused(
        &scope(
            &alice,
            &mut a,
            &org,
            json!({ "apps": [{ "package": CHAT, "keep": [bob_id] }] }),
        )
        .await
        .unwrap_err(),
        "only closing an app team",
    );
    refused(
        &scope(
            &alice,
            &mut a,
            &org,
            json!({ "apps": [{ "package": CHAT, "join": "everyone" }], "shared": true }),
        )
        .await
        .unwrap_err(),
        "cannot open while the step closes every app team",
    );
    assert_eq!(app_team(&alice, &org, CHAT).await["join"], "everyone");

    // Closed keeping Bob: Bob and Alice (its lead) have Chat; Carol loses it
    // and her channel under it; Dave, who left, is not brought back; Blocks
    // stays open for everyone.
    let done = scope(&alice, &mut a, &org, close(json!([bob_id])))
        .await
        .unwrap();
    assert_eq!(done["mode"], "shared", "{done}");
    assert_eq!(done["teams"][0]["join"], "invite", "{done}");
    assert_eq!(done["teams"][0]["dropped"], 1, "{done}");
    assert_eq!(app_team(&alice, &org, BLOCKS).await["join"], "everyone");
    assert!(reaches(&bob, &org).await.contains(&chat));
    assert!(reaches(&alice, &org).await.contains(&chat));
    for p in [&carol, &dave] {
        let r = reaches(p, &org).await;
        assert!(!r.contains(&chat) && !r.contains(&private), "{r:?}");
        assert!(r.contains(&blocks), "{r:?}");
    }
    let row = alice
        .get(&format!("orgs/{org}/teams/{channel}"))
        .await
        .unwrap();
    assert!(
        !serde_json::to_string(&row).unwrap().contains(&carol_id),
        "{row}"
    );

    // Opened again, alone: Carol has Chat back (not her channel, whose row
    // went with the close), Dave stays out, Blocks is untouched.
    let done = scope(
        &alice,
        &mut a,
        &org,
        json!({ "apps": [{ "package": CHAT, "join": "everyone" }] }),
    )
    .await
    .unwrap();
    assert_eq!(done["mode"], "standalone", "{done}");
    let r = reaches(&carol, &org).await;
    assert!(r.contains(&chat) && !r.contains(&private), "{r:?}");
    assert!(!reaches(&dave, &org).await.contains(&chat));

    // Closed keeping everyone who reaches it: nobody loses it.
    let done = scope(&alice, &mut a, &org, close(Value::Null))
        .await
        .unwrap();
    assert_eq!(done["teams"][0]["dropped"], 0, "{done}");
    assert!(reaches(&carol, &org).await.contains(&chat));
    assert!(reaches(&bob, &org).await.contains(&chat));
    assert!(!reaches(&dave, &org).await.contains(&chat));
    assert_eq!(app_team(&alice, &org, BLOCKS).await["join"], "everyone");

    // A single team also opens with the team update an admin always had.
    let _ok = alice
        .post(
            &format!("orgs/{org}/teams/{chat_team}"),
            json!({ "join": "everyone" }),
        )
        .await
        .unwrap();
    assert_eq!(app_team(&alice, &org, CHAT).await["join"], "everyone");
}

/// A standing "every app added later" grant (`allApps`): an admin's invite
/// carries it, and an admin sets it on a member later, with a passkey. Each
/// app team made from then on gives them a row, so in an organization that
/// keeps its apps apart a new app's team holds them though nobody named
/// them; an app team already there does not. A member below admin neither
/// mints such an invite nor sets the grant, and removal from the
/// organization ends it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_all_apps_member_gets_every_app_added_later() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let (carol, _c) = signed_up(&c, 2, "carol").await;
    let (dave, mut d) = signed_up(&c, 0, "dave").await;
    for p in [CHAT, NOTES, BLOCKS] {
        let _ok = alice.upload_package(release(p)).await.unwrap();
    }
    let org = new_org(&alice).await;
    for p in [&carol, &dave] {
        join(
            p,
            &invite(&alice, &mut a, &org, "member", &[]).await.unwrap(),
        )
        .await;
    }
    let (carol_id, dave_id) = (id(&carol).await, id(&dave).await);
    // Chat is kept apart before Bob joins: Alice alone in it.
    let chat = instance(&alice, &org, CHAT, Value::Null).await.unwrap();
    let closed =
        |package: &str| json!({ "apps": [{ "package": package, "join": "invite", "keep": [] }] });
    let _ok = scope(&alice, &mut a, &org, closed(CHAT)).await.unwrap();

    // Dave may invite, but not for every app: that is an admin's.
    let _ok = alice
        .post_confirmed(
            &format!("orgs/{org}/capabilities"),
            json!({ "account": dave_id, "capabilities": INVITE_MEMBERS | 1 << 2 }),
            &mut a,
        )
        .await
        .unwrap();
    let all_apps_invite = |role: &str| json!({ "role": role, "allApps": true });
    let err = dave
        .post_confirmed(
            &format!("orgs/{org}/invites"),
            all_apps_invite("member"),
            &mut d,
        )
        .await
        .unwrap_err();
    refused(&err, "every app");
    let token = alice
        .post_confirmed(
            &format!("orgs/{org}/invites"),
            all_apps_invite("member"),
            &mut a,
        )
        .await
        .unwrap()["token"]
        .clone();
    let listed = invites_of(&alice, &org).await;
    assert!(listed.iter().any(|i| i["allApps"] == true), "{listed:?}");
    join(&bob, &s(&token)).await;
    let bob_id = id(&bob).await;
    let o = alice.get(&format!("orgs/{org}")).await.unwrap();
    assert_eq!(o["allApps"], json!([bob_id]), "{o}");
    // Chat was there before: Bob is not in it.
    assert!(!reaches(&bob, &org).await.contains(&chat));

    // Notes is added later and kept apart, naming nobody: Bob has it.
    let notes = instance(&alice, &org, NOTES, Value::Null).await.unwrap();
    let _ok = scope(&alice, &mut a, &org, closed(NOTES)).await.unwrap();
    assert!(reaches(&bob, &org).await.contains(&notes));
    for p in [&carol, &dave] {
        assert!(!reaches(p, &org).await.contains(&notes));
    }

    // An admin sets it on Carol later; a member below admin cannot.
    let path = format!("orgs/{org}/all-apps");
    let err = dave
        .post_confirmed(
            &path,
            json!({ "account": dave_id, "allApps": true }),
            &mut d,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("FORBIDDEN"), "{err}");
    let err = alice
        .post(&path, json!({ "account": carol_id, "allApps": true }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("CONFIRM_REQUIRED"), "{err}");
    let _ok = alice
        .post_confirmed(
            &path,
            json!({ "account": carol_id, "allApps": true }),
            &mut a,
        )
        .await
        .unwrap();
    let blocks = instance(&alice, &org, BLOCKS, Value::Null).await.unwrap();
    let _ok = scope(&alice, &mut a, &org, closed(BLOCKS)).await.unwrap();
    for p in [&bob, &carol] {
        assert!(reaches(p, &org).await.contains(&blocks));
    }
    assert!(!reaches(&dave, &org).await.contains(&blocks));
    assert!(!reaches(&carol, &org).await.contains(&notes));

    // Removed from the organization, Bob's grant goes with him.
    let _ok = alice
        .delete_confirmed(&format!("orgs/{org}/members/{bob_id}"), &mut a)
        .await
        .unwrap();
    let o = alice.get(&format!("orgs/{org}")).await.unwrap();
    assert_eq!(o["allApps"], json!([carol_id]), "{o}");
}

/// Closing an open app team with the plain team update (`POST
/// /orgs/{org}/teams/{team} { join: invite }`) takes, from whoever loses it
/// (in it by inheritance alone), their rows in the private channels below it,
/// as a removal from the team does: they no longer reach a channel's apps,
/// even one they made. Whoever keeps the team (a row of their own) keeps the
/// channels.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn closing_an_app_team_takes_the_channels_of_whoever_loses_it() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let (dave, mut d) = signed_up(&c, 2, "dave").await;
    let _ok = alice.upload_package(release(CHAT)).await.unwrap();
    let org = new_org(&alice).await;
    join(
        &bob,
        &invite(&alice, &mut a, &org, "member", &[]).await.unwrap(),
    )
    .await;
    let bob_id = id(&bob).await;
    let chat = instance(&alice, &org, CHAT, Value::Null).await.unwrap();
    // Chat's team, open: Alice and Bob get rows.
    let done = scope(&alice, &mut a, &org, json!({ "shared": false }))
        .await
        .unwrap();
    let chat_team = s(&done["teams"][0]["teamId"]);
    // Dave joins after: in Chat by inheritance alone.
    join(
        &dave,
        &invite(&alice, &mut a, &org, "member", &[]).await.unwrap(),
    )
    .await;
    let dave_id = id(&dave).await;
    // Dave makes a private channel below Chat, adds Bob, and puts an
    // instance in it.
    let channel = s(&dave
        .post(
            &format!("orgs/{org}/teams"),
            json!({ "name": "design", "join": "invite", "parent": chat_team }),
        )
        .await
        .unwrap()["teamId"]);
    let _ok = dave
        .post_confirmed(
            &format!("orgs/{org}/teams/{channel}/members"),
            json!({ "account": bob_id, "role": "member" }),
            &mut d,
        )
        .await
        .unwrap();
    let private = instance(
        &dave,
        &org,
        CHAT,
        json!({ "kind": "team", "team": channel }),
    )
    .await
    .unwrap();
    for p in [&dave, &bob] {
        let r = reaches(p, &org).await;
        assert!(r.contains(&chat) && r.contains(&private), "{r:?}");
    }

    // Alice closes Chat with the plain team update: Dave, with no row in
    // it, loses Chat and the channel below it; Bob keeps both.
    let _ok = alice
        .post(
            &format!("orgs/{org}/teams/{chat_team}"),
            json!({ "join": "invite" }),
        )
        .await
        .unwrap();
    let r = reaches(&dave, &org).await;
    assert!(!r.contains(&chat) && !r.contains(&private), "{r:?}");
    let err = writes(&dave, &private).await.unwrap_err();
    assert_eq!(err.code(), Some("NOT_FOUND"), "{err}");
    assert!(dave
        .get(&format!("orgs/{org}/teams/{channel}"))
        .await
        .is_err());
    let view = alice
        .get(&format!("orgs/{org}/teams/{channel}"))
        .await
        .unwrap();
    assert!(
        !view.to_string().contains(&dave_id),
        "a row is left below: {view}"
    );
    let r = reaches(&bob, &org).await;
    assert!(r.contains(&chat) && r.contains(&private), "{r:?}");
    writes(&bob, &private).await.unwrap();
}

/// An app team whose default lacks `CREATE_CHILD_TEAM` (one made before it
/// was the default, or one an admin turned it off for) gets it back when an
/// admin names `childTeams: true` for its package, in one step that changes
/// nothing else: its members then make private channels below it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_admin_lets_an_app_teams_members_make_channels() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, mut b) = signed_up(&c, 1, "bob").await;
    for p in [CHAT, BLOCKS] {
        let _ok = alice.upload_package(release(p)).await.unwrap();
    }
    let org = new_org(&alice).await;
    join(
        &bob,
        &invite(&alice, &mut a, &org, "member", &[]).await.unwrap(),
    )
    .await;
    for p in [CHAT, BLOCKS] {
        let _app = instance(&alice, &org, p, Value::Null).await.unwrap();
    }
    let _done = scope(&alice, &mut a, &org, json!({ "shared": false }))
        .await
        .unwrap();
    let chat_team = s(&app_team(&alice, &org, CHAT).await["teamId"]);
    // Chat's default without it, as an app team made before it had.
    let _ok = alice
        .post_confirmed(
            &format!("orgs/{org}/capabilities"),
            json!({ "team": chat_team, "capabilities": INVITE_MEMBERS }),
            &mut a,
        )
        .await
        .unwrap();
    let teams = format!("orgs/{org}/teams");
    let channel = json!({ "name": "design", "join": "invite", "parent": chat_team });
    refused(
        &bob.post(&teams, channel.clone()).await.unwrap_err(),
        "not allowed to create this team",
    );
    // Only an admin turns it on.
    let on = json!({ "apps": [{ "package": CHAT, "instances": [], "childTeams": true }] });
    let _err = scope(&bob, &mut b, &org, on.clone()).await.unwrap_err();

    let done = scope(&alice, &mut a, &org, on).await.unwrap();
    assert_eq!(done["mode"], "standalone", "{done}");
    let t = app_team(&alice, &org, CHAT).await;
    assert_eq!(
        t["defaultCapabilities"],
        INVITE_MEMBERS | CREATE_CHILD_TEAM,
        "{t}"
    );
    assert_eq!(app_team(&alice, &org, BLOCKS).await["join"], "everyone");
    let _private = s(&bob.post(&teams, channel).await.unwrap()["teamId"]);
}

/// Calimero Home's "give all apps": an admin gives one member rows in every
/// app team at once, with one passkey approval (`POST
/// /orgs/{org}/members/{account}/teams`), and takes them out of several the
/// same way (`DELETE`, the private channels below included). Each team is
/// judged as its own team-member change would be: a batch naming one team
/// the caller may not change is refused whole, and nothing changes.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_admin_gives_a_member_every_app_in_one_step() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, mut b) = signed_up(&c, 1, "bob").await;
    let (carol, _c) = signed_up(&c, 2, "carol").await;
    for p in [CHAT, BLOCKS, NOTES] {
        let _ok = alice.upload_package(release(p)).await.unwrap();
    }
    let org = new_org(&alice).await;
    join(
        &bob,
        &invite(&alice, &mut a, &org, "member", &[]).await.unwrap(),
    )
    .await;
    let apps = [
        instance(&alice, &org, CHAT, Value::Null).await.unwrap(),
        instance(&alice, &org, BLOCKS, Value::Null).await.unwrap(),
        instance(&alice, &org, NOTES, Value::Null).await.unwrap(),
    ];
    // Three restricted app teams: Carol, who joins after, is in none.
    let _done = scope(&alice, &mut a, &org, json!({ "shared": true }))
        .await
        .unwrap();
    join(
        &carol,
        &invite(&alice, &mut a, &org, "member", &[]).await.unwrap(),
    )
    .await;
    let carol_id = id(&carol).await;
    let mut teams = Vec::new();
    for p in [CHAT, BLOCKS, NOTES] {
        teams.push(s(&app_team(&alice, &org, p).await["teamId"]));
    }
    assert!(reaches(&carol, &org).await.is_empty());
    let rows = |teams: &[String]| json!({ "teams": teams.iter().map(|t| json!({ "team": t })).collect::<Vec<_>>() });
    let path = format!("orgs/{org}/members/{carol_id}/teams");

    // Bob leads none of them: refused, nothing set.
    let err = bob
        .post_confirmed(&path, rows(&teams), &mut b)
        .await
        .unwrap_err();
    refused(&err, "not allowed to manage this team");
    assert!(reaches(&carol, &org).await.is_empty());
    // A batch naming one team twice, or none, is refused before a passkey.
    for body in [rows(&[teams[0].clone(), teams[0].clone()]), rows(&[])] {
        let err = alice.post(&path, body).await.unwrap_err();
        assert_eq!(err.code(), Some("BAD_TEAMS"), "{err}");
    }

    // Bob leads Chat now: a batch of Chat and Blocks is still refused whole.
    let bob_id = id(&bob).await;
    let _ok = alice
        .post_confirmed(
            &format!("orgs/{org}/teams/{}/members", teams[0]),
            json!({ "account": bob_id, "role": "lead" }),
            &mut a,
        )
        .await
        .unwrap();
    let err = bob
        .post_confirmed(&path, rows(&teams[..2]), &mut b)
        .await
        .unwrap_err();
    refused(&err, "not allowed to manage this team");
    assert!(reaches(&carol, &org).await.is_empty());

    // Alice gives Carol all three with one approval.
    let _ok = alice
        .post_confirmed(&path, rows(&teams), &mut a)
        .await
        .unwrap();
    let mut r = reaches(&carol, &org).await;
    r.sort();
    let mut want = apps.to_vec();
    want.sort();
    assert_eq!(r, want);
    for app in &apps {
        writes(&carol, app).await.unwrap();
    }

    // Out of Chat and Notes with one approval; Blocks stays.
    let _ok = alice
        .delete_confirmed_with(&path, json!({ "teams": [teams[0], teams[2]] }), &mut a)
        .await
        .unwrap();
    assert_eq!(reaches(&carol, &org).await, vec![apps[1].clone()]);
    let err = writes(&carol, &apps[0]).await.unwrap_err();
    assert_eq!(err.code(), Some("NOT_FOUND"), "{err}");
}
