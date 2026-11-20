//! Updates that apply on their own (C9): a green update of an app that sends no data out
//! applies after its hold under its organization's policy, with a restore point an owner
//! or admin may undo for a week; a deployment's offer reaches such instances in waves its
//! managers pause and resume. Time is moved with `testing::shift_update_clock`: every
//! rule judges the logged times, so a hold, a wave or the undo window passes at once.
//!
//! Builds the component fixture `apps/fixture-kv` from this checkout (see
//! `harness/cluster.rs`). Run one at a time (`--test-threads 1`): the clock shift is the
//! process's.

use std::time::Duration;

use calimero_client::types::ViewsRequestConsistency;
use calimero_client::Request;
use calimero_node::testing::shift_update_clock;
use reqwest::Method;
use serde_json::{json, Value};

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{home_org, kv_wasm, signed_up, typed, wire, Cluster};

const PACKAGE: &str = "com.test.auto";
const DAY: u64 = 24 * 3600;

/// A signed single-service `.mpk` of `PACKAGE` at `version`, declaring no destination.
fn release(version: &str, wasm: &[u8], key: &ed25519_dalek::SigningKey) -> Vec<u8> {
    let sha = |b: &[u8]| hex::encode(calimero_identity::sha256(b));
    let manifest = calimero_bundle::BundleManifest {
        version: "1.0".to_owned(),
        package: PACKAGE.to_owned(),
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
            hash: sha(wasm),
            size: wasm.len() as u64,
        }),
        abi: None,
        compute: None,
        links: None,
        frontend: None,
        frontend_aliases: Vec::new(),
        outbound: Vec::new(),
        secrets: None,
        uses: None,
        upgrades: None,
        instance_org: None,
        guests: None,
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

/// `wasm` with a custom section named `tag`: another module that runs the same.
fn variant(wasm: &[u8], tag: &str) -> Vec<u8> {
    let mut out = wasm.to_vec();
    out.push(0);
    out.push(u8::try_from(1 + tag.len()).unwrap());
    out.push(u8::try_from(tag.len()).unwrap());
    out.extend_from_slice(tag.as_bytes());
    out
}

#[allow(
    clippy::result_large_err,
    reason = "the client's own error, passed through for the test to read its status"
)]
async fn get(c: &calimero_client::Client, path: &str) -> Result<Value, calimero_client::Error> {
    c.transport().json(Request::new(Method::GET, path)).await
}

/// `GET path` until `done` holds of the answer, for at most `secs`.
async fn get_until(
    c: &calimero_client::Client,
    path: &str,
    secs: u64,
    done: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let last = get(c, path).await;
        if let Ok(v) = &last {
            if done(v) {
                return v.clone();
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{path} never got there; last answer {last:?}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// The release `app` runs, as `GET /apps/{app}` answers it.
async fn running(c: &calimero_client::Client, app: &str) -> Value {
    get(c, &format!("apps/{app}")).await.unwrap()["release"]["version"].clone()
}

/// How many of `d`'s instances run its offered release.
async fn updated(dev: &calimero_client::Client, d: &str) -> Value {
    get(dev, &format!("deployments/{d}/rollout")).await.unwrap()["apps"][0]["updated"].clone()
}

/// A few of the system leader's passes ([`calimero_node::node::AUTO_UPDATE_EVERY`]).
async fn passes() {
    tokio::time::sleep(calimero_node::node::AUTO_UPDATE_EVERY * 3).await;
}

async fn read(c: &calimero_client::Client, app: &str, key: &str) -> Value {
    c.apps()
        .view(
            app,
            "get",
            Some(json!({ "key": key })),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap()
        .result
}

/// Under an organization's `noDestinations` policy, a green update applies on its own
/// once its hold has passed since the release was installed, never before; its restore
/// point is the app's one undo: within a week an owner undoes it with their passkey, the
/// app goes back to the release and the state it had (what it wrote since is lost, and
/// the route said so first), and that release never applies on its own again. Past the
/// week an undo is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_green_update_applies_after_its_hold_and_is_undone_within_a_week() {
    shift_update_clock(0);
    let c = Cluster::start(3).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[41; 32]);
    let _ = alice
        .packages()
        .upload(release("1.0.0", &kv_wasm(), &publisher))
        .await
        .unwrap();
    let org = home_org(&alice).await;
    let app = alice
        .apps()
        .create(typed(
            json!({ "name": "notes", "package": PACKAGE, "version": "1.0.0", "org": org }),
        ))
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
            Some(json!({ "key": "k", "value": "before" })),
            None,
            None,
        )
        .await
        .unwrap();
    let set = alice
        .transport()
        .confirmed(
            Request::new(Method::POST, format!("orgs/{org}/upgrade-policy").as_str())
                .json(json!({ "policy": { "autoApply": "noDestinations", "holdHours": 24 } })),
            &mut alice_pk,
        )
        .await
        .unwrap();
    assert_eq!(set["policy"]["autoApply"], "noDestinations", "{set}");
    let _ = alice
        .packages()
        .upload(release("1.1.0", &variant(&kv_wasm(), "v11"), &publisher))
        .await
        .unwrap();
    let update = get_until(&alice, &format!("apps/{app}/update"), 15, |u| {
        u["available"]["version"] == "1.1.0"
    })
    .await;
    assert_eq!(update["available"]["class"], "green", "{update}");

    // Nothing applies before the hold: the leader's passes leave it where it is.
    shift_update_clock(DAY - 600);
    passes().await;
    assert_eq!(running(&alice, &app).await, "1.0.0");
    let nothing = get(&alice, &format!("apps/{app}/undo")).await.unwrap();
    assert_eq!(nothing["available"], false, "{nothing}");
    assert_eq!(nothing["reason"]["code"], "NO_RESTORE_POINT", "{nothing}");

    // Past it, it applies on its own, as the policy's, after a restore point.
    shift_update_clock(DAY + 600);
    let moved = get_until(&alice, &format!("apps/{app}"), 60, |a| {
        a["release"]["version"] == "1.1.0"
    })
    .await;
    let last = &moved["lastUpgrade"];
    assert_eq!(last["auto"], true, "{moved}");
    assert_eq!(last["by"], "policy:auto-update", "{moved}");
    assert_eq!(last["class"], "green", "{moved}");
    assert_eq!(
        moved["restorePoint"]["fromRelease"]["version"], "1.0.0",
        "{moved}"
    );
    assert!(moved["restorePoint"]["file"]["node"].is_u64(), "{moved}");
    assert_eq!(read(&alice, &app, "k").await, json!("before"));

    // It writes on the new release; the undo says that write goes with it.
    let _r = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "k", "value": "after" })),
            None,
            None,
        )
        .await
        .unwrap();
    let plan = get(&alice, &format!("apps/{app}/undo")).await.unwrap();
    assert_eq!(plan["available"], true, "{plan}");
    let part = &plan["apps"][0];
    assert_eq!(part["from"]["version"], "1.1.0", "{plan}");
    assert_eq!(part["to"]["version"], "1.0.0", "{plan}");
    assert_eq!(part["auto"], true, "{plan}");
    assert_eq!(part["lostWrites"]["any"], true, "{plan}");
    assert!(
        part["lostWrites"]["entriesSince"].as_u64().unwrap() >= 2,
        "{plan}"
    );

    // Undone with the owner's passkey: the release and the state from before.
    let undone = alice
        .transport()
        .confirmed(
            Request::new(Method::POST, format!("apps/{app}/undo").as_str()).json(json!({})),
            &mut alice_pk,
        )
        .await
        .unwrap();
    assert_eq!(undone["restored"], true, "{undone}");
    let back = get_until(&alice, &format!("apps/{app}"), 15, |a| {
        a["release"]["version"] == "1.0.0" && a["restore"].is_null()
    })
    .await;
    assert_eq!(back["lastUpgrade"]["undo"], true, "{back}");
    assert!(back["restorePoint"].is_null(), "spent: {back}");
    assert_eq!(read(&alice, &app, "k").await, json!("before"));
    let _r = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "k", "value": "again" })),
            None,
            None,
        )
        .await
        .unwrap();
    // The release it undid never applies on its own again.
    passes().await;
    assert_eq!(running(&alice, &app).await, "1.0.0");

    // A manual upgrade takes a restore point too; past a week it is no longer undone.
    let _up = alice
        .apps()
        .upgrade(
            &app,
            typed(json!({ "version": "1.1.0" })),
            Some(&mut alice_pk),
        )
        .await
        .unwrap();
    let moved = get_until(&alice, &format!("apps/{app}"), 15, |a| {
        a["release"]["version"] == "1.1.0"
    })
    .await;
    assert!(moved["restorePoint"]["file"].is_object(), "{moved}");
    shift_update_clock(DAY + 600 + 8 * DAY);
    let late = get(&alice, &format!("apps/{app}/undo")).await.unwrap();
    assert_eq!(late["available"], false, "{late}");
    assert_eq!(late["reason"]["code"], "UNDO_WINDOW_PASSED", "{late}");
    let e = alice
        .transport()
        .confirmed(
            Request::new(Method::POST, format!("apps/{app}/undo").as_str()).json(json!({})),
            &mut alice_pk,
        )
        .await
        .unwrap_err();
    assert!(e.to_string().contains("UNDO_WINDOW_PASSED"), "{e}");
    assert_eq!(read(&alice, &app, "k").await, json!("again"));
    shift_update_clock(0);
}

/// A deployment's new offer reaches the instances on an automatic policy in waves, each
/// after its hold: the first wave's one instance, then nobody new while its managers
/// pause it, then everyone once they resume it and the later waves open.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_deployments_offer_rolls_out_in_waves_its_managers_pause_and_resume() {
    shift_update_clock(0);
    let c = Cluster::start(3).await;
    let (dev, mut dev_pk) = signed_up(&c, 0, "dev").await;
    let (carol, mut carol_pk) = signed_up(&c, 1, "carol").await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[42; 32]);
    for (version, tag) in [("1.0.0", "v1"), ("1.1.0", "v11")] {
        let _ = dev
            .packages()
            .upload(release(version, &variant(&kv_wasm(), tag), &publisher))
            .await
            .unwrap();
    }
    let dev_id = dev.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let d = dev
        .deployments()
        .create(
            "acme",
            typed(json!({ "account": dev_id })),
            Some(&mut dev_pk),
        )
        .await
        .map(wire)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ok = dev
        .deployments()
        .offer(&d, "notes", PACKAGE, "1.0.0", Some(&mut dev_pk))
        .await
        .unwrap();
    let org = home_org(&carol).await;
    let mut apps = Vec::new();
    for i in 0..3 {
        let id = carol
            .apps()
            .create(typed(json!({
                "name": format!("notes{i}"), "deployment": d, "package": PACKAGE, "org": org,
            })))
            .await
            .map(|a| a.app_id)
            .unwrap();
        apps.push(id);
    }
    let _set = carol
        .transport()
        .confirmed(
            Request::new(Method::POST, format!("orgs/{org}/upgrade-policy").as_str())
                .json(json!({ "policy": { "autoApply": "noDestinations" } })),
            &mut carol_pk,
        )
        .await
        .unwrap();
    let _ok = dev
        .deployments()
        .offer(&d, "notes", PACKAGE, "1.1.0", Some(&mut dev_pk))
        .await
        .unwrap();
    let rollout = get(&dev, &format!("deployments/{d}/rollout"))
        .await
        .unwrap();
    let notes = &rollout["apps"][0];
    assert_eq!(
        (notes["version"].clone(), notes["wave"].clone()),
        (json!("1.1.0"), json!(0)),
        "{rollout}"
    );
    assert_eq!(notes["instances"], 3, "{rollout}");
    assert!(
        get(&carol, &format!("deployments/{d}/rollout"))
            .await
            .is_err(),
        "a customer is no manager"
    );

    // A day on: the hold passed and wave 1 (10%, at least one) is open: one instance.
    shift_update_clock(DAY + 600);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while updated(&dev, &d).await != 1 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the first wave never moved"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    // Paused by a manager: nobody new, however long it waits.
    let paused = dev
        .transport()
        .confirmed(
            Request::new(
                Method::POST,
                format!("deployments/{d}/rollout/notes/pause").as_str(),
            )
            .json(json!({})),
            &mut dev_pk,
        )
        .await
        .unwrap();
    assert!(paused["apps"][0]["paused"]["by"].is_string(), "{paused}");
    shift_update_clock(30 * DAY);
    passes().await;
    assert_eq!(updated(&dev, &d).await, 1);
    // Resumed with a shorter pause between waves: the later waves open and reach all.
    let resumed = dev
        .transport()
        .confirmed(
            Request::new(
                Method::POST,
                format!("deployments/{d}/rollout/notes/resume").as_str(),
            )
            .json(json!({ "waveHours": 1 })),
            &mut dev_pk,
        )
        .await
        .unwrap();
    assert!(resumed["apps"][0]["paused"].is_null(), "{resumed}");
    assert_eq!(resumed["apps"][0]["waveHours"], 1, "{resumed}");
    shift_update_clock(31 * DAY);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    while updated(&dev, &d).await != 3 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the rollout never reached everyone"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    for app in &apps {
        assert_eq!(running(&carol, app).await, "1.1.0");
    }
    shift_update_clock(0);
}
