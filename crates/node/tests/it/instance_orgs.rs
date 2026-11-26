//! Where an instance goes (`POST /apps`), from cluster version 1.0.0-rc.32
//! (`Feature::OrdinaryOrgs`): a sign-up makes no organization, and a release's
//! signed `instanceOrg` decides: `standalone` makes an organization with every
//! instance, `org` goes into one its creator is in, `either` (the default) into
//! the one named, the creator's only one, or a new one when they have none.
//! An organization's app is handed only to a member of it.
//!
//! Builds the component kv fixture `apps/fixture-kv` from this checkout
//! (see `harness/cluster.rs`).

#![allow(
    clippy::result_large_err,
    reason = "the helpers pass on calimero_client::Error as the client returns it"
)]
use calimero_client::{Client, Error};
use serde_json::{json, Value};

use crate::harness;

use harness::{kv_wasm, signed_up, typed, wire, Cluster};

const STANDALONE: &str = "com.test.standalone";
const IN_ORG: &str = "com.test.in-org";
const EITHER: &str = "com.test.either";

/// A signed single-service `.mpk` of `package@1.0.0` declaring `mode` (none:
/// the field left out), its WASM tagged so each package ships its own.
fn release(package: &str, mode: Option<calimero_bundle::InstanceOrg>) -> Vec<u8> {
    use sha2::Digest;
    let artifact = |path: &str, bytes: &[u8]| calimero_bundle::BundleArtifact {
        path: path.to_owned(),
        hash: hex::encode(sha2::Sha256::digest(bytes)),
        size: bytes.len() as u64,
    };
    let key = ed25519_dalek::SigningKey::from_bytes(&[6; 32]);
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
        instance_org: mode,
        guests: None,
        lineage: None,
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

/// The organizations `/auth/me` lists for `c`, by id.
async fn orgs(c: &Client) -> Vec<String> {
    let me = c.auth().me().await.map(wire).unwrap();
    let mut ids: Vec<String> = me["orgs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| s(&o["orgId"]))
        .collect();
    ids.sort();
    ids
}

/// `POST /apps` of `package` with the extra fields `with` (`org`, `newOrg`):
/// the new app's organization, or the refusal.
async fn create(c: &Client, package: &str, name: &str, with: Value) -> Result<String, Error> {
    let mut body = json!({ "name": name, "package": package });
    body.as_object_mut()
        .unwrap()
        .extend(with.as_object().unwrap().clone());
    let app = c.apps().create(typed(body)).await?.app_id;
    Ok(s(&c.apps().get(&app).await.map(wire).unwrap()["org"]))
}

fn refused(e: &Error, status: &str, code: &str) {
    let e = e.to_string();
    assert!(e.starts_with(status) && e.contains(code), "{e}");
}

async fn new_org(owner: &Client, name: &str) -> String {
    s(&owner
        .orgs()
        .create(name, None, None)
        .await
        .map(wire)
        .unwrap()["orgId"])
}

/// A sign-up makes no organization; each mode places an instance as its
/// release declares, for a creator in none, one or several organizations,
/// naming one or not, or asking for a new one.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_instance_goes_where_its_release_says() {
    use calimero_bundle::InstanceOrg;
    let c = Cluster::start(3).await;
    let (zed, _z) = signed_up(&c, 0, "zed").await;
    let (yan, _y) = signed_up(&c, 1, "yan").await;
    assert!(
        orgs(&zed).await.is_empty(),
        "a sign-up makes no organization"
    );
    // The operator's catalog: an account's install is for an organization it is in.
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    for (package, mode) in [
        (STANDALONE, Some(InstanceOrg::Standalone)),
        (IN_ORG, Some(InstanceOrg::Org)),
        (EITHER, None),
    ] {
        let _ok = ops
            .ops()
            .package_upload(release(package, mode))
            .await
            .unwrap();
    }
    let recorded = c.nodes[0].as_ref().unwrap().node.sys(|s| {
        [STANDALONE, IN_ORG, EITHER].map(|p| s.packages[p].versions["1.0.0"].instance_org)
    });
    assert_eq!(
        recorded,
        [
            Some(InstanceOrg::Standalone),
            Some(InstanceOrg::Org),
            Some(InstanceOrg::Either)
        ],
        "the mode is kept on the release, absent being either"
    );

    // In none: `org` needs one first, `standalone` names none, `either` makes one.
    refused(
        &create(&zed, IN_ORG, "kv", json!({})).await.unwrap_err(),
        "409",
        "NO_ORGANIZATION",
    );
    refused(
        &create(&zed, IN_ORG, "kv", json!({ "newOrg": { "name": "New" } }))
            .await
            .unwrap_err(),
        "400",
        "ORG_INSTANCE",
    );
    let elsewhere = new_org(&yan, "Yan's").await;
    refused(
        &create(&zed, STANDALONE, "kv", json!({ "org": elsewhere }))
            .await
            .unwrap_err(),
        "400",
        "STANDALONE_INSTANCE",
    );
    assert!(orgs(&zed).await.is_empty(), "no refusal made one");
    let home = create(&zed, EITHER, "Notes", json!({})).await.unwrap();
    assert_eq!(orgs(&zed).await, std::slice::from_ref(&home));
    let org = zed.orgs().get(&home).await.map(wire).unwrap();
    assert_eq!(org["name"], "Notes", "named after the instance: {org}");
    assert_eq!(
        org["members"][&s(&zed.auth().me().await.map(wire).unwrap()["accountId"])],
        "owner"
    );
    assert!(org.get("personal").is_none(), "{org}");

    // In one: `org` and `either` go there; `standalone` makes another.
    assert_eq!(create(&zed, IN_ORG, "kv", json!({})).await.unwrap(), home);
    assert_eq!(create(&zed, EITHER, "kv", json!({})).await.unwrap(), home);
    let own = create(&zed, STANDALONE, "  Board\n", json!({}))
        .await
        .unwrap();
    assert_ne!(own, home);
    assert_eq!(
        zed.orgs().get(&own).await.map(wire).unwrap()["name"],
        "Board",
        "the instance's name made to fit"
    );

    // In several: naming one is required, unless asking for a new one.
    for package in [IN_ORG, EITHER] {
        refused(
            &create(&zed, package, "kv", json!({})).await.unwrap_err(),
            "400",
            "ORG_REQUIRED",
        );
        assert_eq!(
            create(&zed, package, "kv", json!({ "org": home }))
                .await
                .unwrap(),
            home
        );
    }
    let fresh = create(&zed, EITHER, "kv", json!({ "newOrg": { "name": "Fresh" } }))
        .await
        .unwrap();
    assert_eq!(
        zed.orgs().get(&fresh).await.map(wire).unwrap()["name"],
        "Fresh"
    );
    let named = create(
        &zed,
        STANDALONE,
        "kv",
        json!({ "newOrg": { "name": "Named" } }),
    )
    .await
    .unwrap();
    assert_eq!(
        zed.orgs().get(&named).await.map(wire).unwrap()["name"],
        "Named"
    );
    refused(
        &create(
            &zed,
            EITHER,
            "kv",
            json!({ "org": home, "newOrg": { "name": "x" } }),
        )
        .await
        .unwrap_err(),
        "400",
        "BAD_REQUEST",
    );
    assert_eq!(orgs(&zed).await.len(), 4, "home, its own, fresh, named");

    // A standalone instance for an account in none, too.
    let yans_own = create(&yan, STANDALONE, "Team chat", json!({}))
        .await
        .unwrap();
    assert_ne!(yans_own, elsewhere);
}

/// An organization's app goes only to a member of it: a transfer to someone
/// outside is refused before any passkey is asked for, and nothing moves.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_app_is_transferred_only_to_a_member_of_its_organization() {
    let c = Cluster::start(3).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let _org = alice.orgs().create("alice org", None, None).await.unwrap();
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let _ok = alice
        .packages()
        .upload(release(EITHER, None), None)
        .await
        .unwrap();
    let app = alice
        .apps()
        .create(typed(json!({ "name": "kv", "package": EITHER })))
        .await
        .unwrap()
        .app_id;
    let bob_id = s(&bob.auth().me().await.map(wire).unwrap()["accountId"]);
    let e = alice
        .apps()
        .transfer(&app, &bob_id, None)
        .await
        .unwrap_err();
    refused(&e, "400", "not a member of the app's organization");
    let after = alice.apps().get(&app).await.map(wire).unwrap();
    assert_ne!(after["owner"], json!(bob_id), "{after}");
}
