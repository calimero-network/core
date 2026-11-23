//! Updates that apply on their own (C9): a green update of an app that sends no data out
//! applies after its hold under its organization's policy, with a restore point an owner
//! or admin may undo for a week. Time is moved with `testing::shift_update_clock`: every
//! rule judges the logged times, so a hold or the undo window passes at once.
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
    release_using(PACKAGE, version, wasm, key, Value::Null)
}

/// [`release`] of `package`, its signed manifest also declaring `uses` (`null`: none).
fn release_using(
    package: &str,
    version: &str,
    wasm: &[u8],
    key: &ed25519_dalek::SigningKey,
    uses: Value,
) -> Vec<u8> {
    let sha = |b: &[u8]| hex::encode(calimero_identity::sha256(b));
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
    if !uses.is_null() {
        manifest["uses"] = uses;
    }
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

/// `wasm` with its embedded ABI (`calimero:abi`) declaring state version `version`: a
/// release an update to which is red (`stateVersionChanged`) though it sends no data out.
fn state_version(wasm: &[u8], version: u64) -> Vec<u8> {
    let data = wasmparser::Parser::new(0)
        .parse_all(wasm)
        .find_map(|p| match p {
            Ok(wasmparser::Payload::CustomSection(s)) if s.name() == "calimero:abi" => {
                let at = usize::try_from(s.data_offset()).unwrap();
                Some(at..at + s.data().len())
            }
            _ => None,
        })
        .expect("the fixture embeds its ABI");
    let abi: Value = serde_json::from_slice(&wasm[data.clone()]).unwrap();
    let was = abi["state"]["version"].as_u64().expect("a state version");
    let (from, to) = (
        format!("\"version\":{was}"),
        format!("\"version\":{version}"),
    );
    assert_eq!(
        from.len(),
        to.len(),
        "a same-length edit keeps the section's size"
    );
    // The one occurrence whose change moves the state's version and nothing else.
    let section = &wasm[data.clone()];
    for at in (0..section.len()).filter(|i| section[*i..].starts_with(from.as_bytes())) {
        let mut edited = section.to_vec();
        edited[at..at + to.len()].copy_from_slice(to.as_bytes());
        let Ok(mut new) = serde_json::from_slice::<Value>(&edited) else {
            continue;
        };
        if new["state"]["version"] != json!(version) {
            continue;
        }
        new["state"]["version"] = json!(was);
        if new == abi {
            let mut out = wasm.to_vec();
            out[data].copy_from_slice(&edited);
            return out;
        }
    }
    panic!("no state version to change in the fixture's ABI");
}

/// `wasm` whose embedded ABI no longer lists method `from` and lists `to` (same length)
/// instead: to the node and the classifier, a release that removed `from`.
fn rename_method(wasm: &[u8], from: &str, to: &str) -> Vec<u8> {
    assert_eq!(
        from.len(),
        to.len(),
        "a same-length edit keeps the section's size"
    );
    let data = wasmparser::Parser::new(0)
        .parse_all(wasm)
        .find_map(|p| match p {
            Ok(wasmparser::Payload::CustomSection(s)) if s.name() == "calimero:abi" => {
                let at = usize::try_from(s.data_offset()).unwrap();
                Some(at..at + s.data().len())
            }
            _ => None,
        })
        .expect("the fixture embeds its ABI");
    let (from, to) = (format!("\"{from}\""), format!("\"{to}\""));
    let edited = String::from_utf8(wasm[data.clone()].to_vec())
        .unwrap()
        .replace(&from, &to);
    let abi: Value = serde_json::from_str(&edited).unwrap();
    assert!(
        abi["methods"].get(from.trim_matches('"')).is_none(),
        "{abi}"
    );
    assert!(abi["methods"].get(to.trim_matches('"')).is_some(), "{abi}");
    let mut out = wasm.to_vec();
    out[data].copy_from_slice(edited.as_bytes());
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

/// One of the system leader's passes, run now on the leader rather than waited for (one
/// comes every [`calimero_node::node::AUTO_UPDATE_EVERY`]): it is the same decision, so a
/// pass that applies nothing here is one the leader's own would. Answers how many applied.
async fn pass(c: &Cluster) -> usize {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(leader) = c
            .nodes
            .iter()
            .flatten()
            .find(|r| r.node.system.status().is_leader)
        {
            return leader.node.auto_update_once().await;
        }
        assert!(tokio::time::Instant::now() < deadline, "no system leader");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// At most this many passes for something the leader's passes should bring about.
const PASSES: usize = 20;

/// Run the leader's passes until `done` holds, at most [`PASSES`] of them.
async fn passes_until<F, Fut>(c: &Cluster, done: F, what: &str)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..PASSES {
        if done().await {
            return;
        }
        let _ = pass(c).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(done().await, "{what}");
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
    assert_eq!(pass(&c).await, 0);
    assert_eq!(running(&alice, &app).await, "1.0.0");
    let nothing = get(&alice, &format!("apps/{app}/undo")).await.unwrap();
    assert_eq!(nothing["available"], false, "{nothing}");
    assert_eq!(nothing["reason"]["code"], "NO_RESTORE_POINT", "{nothing}");

    // Past it, it applies on its own, as the policy's, after a restore point.
    shift_update_clock(DAY + 600);
    passes_until(
        &c,
        || async { running(&alice, &app).await == "1.1.0" },
        "it never applied",
    )
    .await;
    let moved = get_until(&alice, &format!("apps/{app}"), 15, |a| {
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
    assert_eq!(pass(&c).await, 0);
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

/// An app on a `noDestinations` policy (`holdHours` 24), at 1.0.0 with key `k` set, and
/// its organization: what the tests below start from.
async fn auto_app(
    alice: &calimero_client::Client,
    alice_pk: &mut calimero_identity::testing::SoftAuthenticator,
    publisher: &ed25519_dalek::SigningKey,
) -> String {
    let _ = alice
        .packages()
        .upload(release("1.0.0", &kv_wasm(), publisher))
        .await
        .unwrap();
    let org = home_org(alice).await;
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
    let _set = alice
        .transport()
        .confirmed(
            Request::new(Method::POST, format!("orgs/{org}/upgrade-policy").as_str())
                .json(json!({ "policy": { "autoApply": "noDestinations", "holdHours": 24 } })),
            alice_pk,
        )
        .await
        .unwrap();
    app
}

/// A failure before an automatic update runs anything (here its restore point, as when
/// the replica that would take it is briefly down) is not the release's fault: it leaves
/// no failed upgrade on the app, and once the problem is gone the same release applies
/// on its own after the leader's backoff, with nobody stepping in. The backoff holds the
/// retry for a while, so a lasting problem is not tried on every pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_passing_failure_before_an_automatic_update_is_retried_not_held_against_it() {
    shift_update_clock(0);
    calimero_node::testing::fail_restore_points(false);
    let c = Cluster::start(3).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[43; 32]);
    let app = auto_app(&alice, &mut alice_pk, &publisher).await;
    let _ = alice
        .packages()
        .upload(release("1.1.0", &variant(&kv_wasm(), "v11"), &publisher))
        .await
        .unwrap();
    let _ = get_until(&alice, &format!("apps/{app}/update"), 15, |u| {
        u["available"]["class"] == "green"
    })
    .await;

    // Past the hold, no node can take a restore point: nothing applies, nothing is held
    // against the release.
    calimero_node::testing::fail_restore_points(true);
    shift_update_clock(DAY + 600);
    assert_eq!(pass(&c).await, 0);
    let after = get(&alice, &format!("apps/{app}")).await.unwrap();
    assert_eq!(after["release"]["version"], "1.0.0", "{after}");
    assert!(
        after["lastUpgrade"].is_null(),
        "a passing failure is no failed upgrade: {after}"
    );

    // The problem is gone; the leader waits out its backoff, then the update applies.
    calimero_node::testing::fail_restore_points(false);
    assert_eq!(pass(&c).await, 0, "retried before the backoff passed");
    assert_eq!(running(&alice, &app).await, "1.0.0");
    // Past the longest backoff, whatever the leader counted.
    shift_update_clock(DAY + 600 + calimero_node::node::AUTO_UPDATE_RETRY_MAX_SECS);
    passes_until(
        &c,
        || async { running(&alice, &app).await == "1.1.0" },
        "the update never applied once restore points could be taken again",
    )
    .await;
    let moved = get(&alice, &format!("apps/{app}")).await.unwrap();
    assert_eq!(moved["lastUpgrade"]["auto"], true, "{moved}");
    assert_eq!(moved["lastUpgrade"]["ok"], true, "{moved}");
    shift_update_clock(0);
}

/// The newest release may be red for its code alone (its state version changed) while
/// sending no data out: the leader's classification finds it red, and the app moves on
/// its own to the newest release below it that is green, never left where it is.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_red_newest_release_does_not_hold_back_a_green_one_below_it() {
    shift_update_clock(0);
    let c = Cluster::start(3).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[44; 32]);
    let app = auto_app(&alice, &mut alice_pk, &publisher).await;
    for (version, wasm) in [
        ("1.1.0", variant(&kv_wasm(), "v11")),
        ("1.2.0", state_version(&variant(&kv_wasm(), "v12"), 2)),
    ] {
        let _ = alice
            .packages()
            .upload(release(version, &wasm, &publisher))
            .await
            .unwrap();
    }
    let update = get_until(&alice, &format!("apps/{app}/update"), 15, |u| {
        u["available"]["version"] == "1.2.0"
    })
    .await;
    assert_eq!(update["available"]["class"], "red", "{update}");

    shift_update_clock(DAY + 600);
    passes_until(
        &c,
        || async { running(&alice, &app).await == "1.1.0" },
        "the green release below a red newest one never applied",
    )
    .await;
    // And the red one never does on its own.
    assert_eq!(pass(&c).await, 0);
    assert_eq!(running(&alice, &app).await, "1.1.0");
    shift_update_clock(0);
}

/// The node that took a restore point may no longer host the app when an undo is asked
/// for (a replica move took the app off it): what the app wrote since is then read from a
/// replica it is on now, and the undo never says it loses no writes when they are there.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_undo_counts_lost_writes_after_the_restore_points_node_lost_the_app() {
    shift_update_clock(0);
    let c = Cluster::start(4).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[45; 32]);
    let app = auto_app(&alice, &mut alice_pk, &publisher).await;
    let _ = alice
        .packages()
        .upload(release("1.1.0", &variant(&kv_wasm(), "v11"), &publisher))
        .await
        .unwrap();
    let info = get(&alice, &format!("apps/{app}")).await.unwrap();
    let replicas: Vec<u64> = serde_json::from_value(info["replicas"].clone()).unwrap();
    let leader = c.app_leader(&app).expect("the app's leader") as u64 + 1;
    let follower = *replicas.iter().find(|r| **r != leader).unwrap();
    let spare = (1..=4).find(|n| !replicas.contains(n)).unwrap();

    // Upgraded through the follower, which takes the restore point itself.
    let at_follower = c.client(follower as usize - 1);
    let _s = at_follower.sign_in(&mut alice_pk).await.unwrap();
    let _up = at_follower
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
    assert_eq!(
        moved["restorePoint"]["file"]["node"],
        json!(follower),
        "{moved}"
    );
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

    // The app moves off the follower; its restore point stays there.
    let ops = c.client(leader as usize - 1);
    ops.use_api_key("ops-secret").await;
    let kept: Vec<u64> = replicas
        .iter()
        .copied()
        .filter(|r| *r != follower)
        .chain([spare])
        .collect();
    let _ = ops
        .ops()
        .set_replicas(&app, typed(json!(kept)), Some(&harness::operator_sign))
        .await
        .unwrap();
    let gone = &c.nodes[follower as usize - 1].as_ref().unwrap().node;
    for _ in 0..100 {
        if gone.hosted(&app).is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        gone.hosted(&app).is_none(),
        "the follower still hosts the app"
    );

    let plan = get(&alice, &format!("apps/{app}/undo")).await.unwrap();
    assert_eq!(plan["available"], true, "{plan}");
    let lost = &plan["apps"][0]["lostWrites"];
    assert_eq!(
        lost["any"], true,
        "the write since the upgrade is lost: {plan}"
    );
    assert!(
        lost["entriesSince"].as_u64().is_some_and(|n| n >= 1),
        "{plan}"
    );
}

/// Passes take turns: one called while the leader's own timed pass runs, or two called
/// together, never overlap, so a green update is moved once and no pass races another
/// on the same app (whose loser would note the winner's move as a failed upgrade).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn passes_take_turns_and_move_an_app_once() {
    shift_update_clock(0);
    let c = Cluster::start(3).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[46; 32]);
    let app = auto_app(&alice, &mut alice_pk, &publisher).await;
    let _ = alice
        .packages()
        .upload(release("1.1.0", &variant(&kv_wasm(), "v11"), &publisher))
        .await
        .unwrap();
    let _ = get_until(&alice, &format!("apps/{app}/update"), 15, |u| {
        u["available"]["class"] == "green"
    })
    .await;

    // The leader's own timed pass may run too (and apply it first): whichever pass moves
    // the app, the others find nothing left to do.
    shift_update_clock(DAY + 600);
    let (a, b) = tokio::join!(pass(&c), pass(&c));
    assert!(
        a + b <= 1,
        "the app moved once, not by both passes: {a} + {b}"
    );
    let moved = get_until(&alice, &format!("apps/{app}"), 15, |a| {
        a["release"]["version"] == "1.1.0"
    })
    .await;
    // Moved once: its last upgrade is that move, on its own, not a racing pass's failure.
    let last = &moved["lastUpgrade"];
    assert_eq!(last["ok"], true, "{moved}");
    assert_eq!(last["auto"], true, "{moved}");
    assert_eq!(last["toRelease"]["version"], "1.1.0", "{moved}");
    assert_eq!(
        calimero_node::testing::most_auto_update_passes_at_once(),
        1,
        "two passes ran at once"
    );
    shift_update_clock(0);
}

/// Only the system leader's pass moves anything: a pass on a node that does not lead the
/// system group (one that lost the leadership mid-pass, as after a failover) upgrades
/// nothing, so it never races the new leader's pass on the same apps.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_pass_off_the_system_leader_moves_nothing() {
    shift_update_clock(0);
    let c = Cluster::start(3).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[47; 32]);
    let app = auto_app(&alice, &mut alice_pk, &publisher).await;
    let _ = alice
        .packages()
        .upload(release("1.1.0", &variant(&kv_wasm(), "v11"), &publisher))
        .await
        .unwrap();
    let _ = get_until(&alice, &format!("apps/{app}/update"), 15, |u| {
        u["available"]["class"] == "green"
    })
    .await;
    shift_update_clock(DAY + 600);
    let follower = c
        .nodes
        .iter()
        .flatten()
        .find(|r| !r.node.system.status().is_leader)
        .expect("a follower");
    assert_eq!(
        follower.node.auto_update_once().await,
        0,
        "a node that does not lead the system group upgraded an app"
    );
    shift_update_clock(0);
}

/// A green update that drops a method waits for a person: another app may call it (here
/// the plugin's `askWho` calls the calendar's `whoView`), so it never applies on its own,
/// and a quiet release below it still does. A person may still approve it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_green_update_that_drops_a_method_waits_for_a_person() {
    shift_update_clock(0);
    let c = Cluster::start(3).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[47; 32]);
    let uses = json!({
        "cal": { "package": PACKAGE, "range": "^1", "mode": "attach", "methods": ["whoView"] },
    });
    for mpk in [
        release("1.0.0", &kv_wasm(), &publisher),
        release_using("com.test.plugin", "1.0.0", &kv_wasm(), &publisher, uses),
    ] {
        let _ = alice.packages().upload(mpk).await.unwrap();
    }
    let org = home_org(&alice).await;
    let create = |name: &str, package: &str| {
        typed(json!({ "name": name, "package": package, "version": "1.0.0", "org": org }))
    };
    let calendar = alice
        .apps()
        .create(create("calendar", PACKAGE))
        .await
        .unwrap()
        .app_id;
    let plugin = alice
        .apps()
        .create(create("plugin", "com.test.plugin"))
        .await
        .unwrap()
        .app_id;
    let ask = || {
        alice.apps().call(
            &plugin,
            "askWho",
            Some(json!({ "app": calendar })),
            None,
            None,
        )
    };
    assert_eq!(ask().await.unwrap().result, json!(format!("app:{plugin}")));

    let _set = alice
        .transport()
        .confirmed(
            Request::new(Method::POST, format!("orgs/{org}/upgrade-policy").as_str())
                .json(json!({ "policy": { "autoApply": "noDestinations", "holdHours": 24 } })),
            &mut alice_pk,
        )
        .await
        .unwrap();
    let dropped = rename_method(&kv_wasm(), "whoView", "whoVieX");
    for mpk in [
        release("1.0.1", &variant(&kv_wasm(), "v101"), &publisher),
        release("1.1.0", &dropped, &publisher),
    ] {
        let _ = alice.packages().upload(mpk).await.unwrap();
    }
    let update = get_until(&alice, &format!("apps/{calendar}/update"), 15, |u| {
        u["available"]["version"] == "1.1.0"
    })
    .await;
    assert_eq!(update["available"]["class"], "green", "{update}");
    assert_eq!(
        update["available"]["removedMethods"],
        json!(["whoView"]),
        "{update}"
    );

    // Past the hold, the quiet 1.0.1 applies on its own and 1.1.0 never does.
    shift_update_clock(DAY + 600);
    passes_until(
        &c,
        || async { running(&alice, &calendar).await == "1.0.1" },
        "the release below it never applied",
    )
    .await;
    for _ in 0..3 {
        assert_eq!(
            pass(&c).await,
            0,
            "a release that drops a method applied on its own"
        );
    }
    assert_eq!(running(&alice, &calendar).await, "1.0.1");
    assert_eq!(ask().await.unwrap().result, json!(format!("app:{plugin}")));
    let waiting = get(&alice, &format!("apps/{calendar}/update"))
        .await
        .unwrap();
    assert_eq!(waiting["available"]["version"], "1.1.0", "{waiting}");
    assert_eq!(waiting["mayApprove"], true, "{waiting}");
    shift_update_clock(0);
}
