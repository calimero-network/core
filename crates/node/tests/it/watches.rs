//! Live views on an in-process cluster: three nodes, the app on two of them, so a watch
//! stream is served by the leader, by a follower, or proxied by the node that does not host
//! the app (`api::watches`, `watch.rs`).

use crate::common;

use std::time::Duration;

use calimero_client::{Client, WatchNotice, WatchStream};
use calimero_identity::testing::SoftAuthenticator;
use common::{signed_up, typed, wire, Cluster};
use futures_util::StreamExt;
use serde_json::{json, Value};

/// The next frame within `secs`, `None` if none came.
async fn frame(s: &mut WatchStream, secs: u64) -> Option<(String, String, Value)> {
    tokio::time::timeout(Duration::from_secs(secs), s.frames.next())
        .await
        .ok()
        .flatten()
}

/// Every frame that arrives within `secs`.
async fn frames_for(s: &mut WatchStream, secs: u64) -> Vec<(String, String, Value)> {
    let until = tokio::time::Instant::now() + Duration::from_secs(secs);
    let mut out = Vec::new();
    while let Ok(Some(f)) = tokio::time::timeout_at(until, s.frames.next()).await {
        out.push(f);
    }
    out
}

/// The app's nodes: (leader, follower, the node that does not host it), by index.
async fn roles(c: &Cluster, app: &str) -> (usize, usize, usize) {
    for _ in 0..100 {
        let hosts: Vec<usize> = (0..3)
            .filter(|i| {
                c.nodes[*i]
                    .as_ref()
                    .is_some_and(|r| r.node.hosted(app).is_some())
            })
            .collect();
        if let (2, Some(leader)) = (hosts.len(), c.app_leader(app)) {
            let follower = *hosts.iter().find(|i| **i != leader).unwrap();
            let outsider = (0..3).find(|i| !hosts.contains(i)).unwrap();
            return (leader, follower, outsider);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the app did not settle on two replicas");
}

/// A `view-result`'s data is exactly `{ sub, index, answer }` and its id `<index>:<seq>`:
/// its index and answer.
fn result_of(kind: &str, id: &str, data: &Value, sub: &str) -> (u64, Value) {
    assert_eq!(kind, "view-result", "{data}");
    (
        frame_index(id, data, sub, &["answer", "index", "sub"]),
        data["answer"].clone(),
    )
}

/// A frame naming `sub` with exactly `keys`, whose id is `<index>:<seq>`: its index.
fn frame_index(id: &str, data: &Value, sub: &str, keys: &[&str]) -> u64 {
    let obj = data.as_object().expect("an object");
    let mut got: Vec<&str> = obj.keys().map(String::as_str).collect();
    got.sort_unstable();
    assert_eq!(got, keys, "{data}");
    assert_eq!(data["sub"], sub);
    let index = data["index"].as_u64().unwrap();
    let (id_index, seq) = id.split_once(':').expect("<index>:<seq>");
    assert_eq!(id_index.parse::<u64>().unwrap(), index);
    assert!(seq.parse::<u64>().is_ok(), "{id}");
    index
}

async fn signed_in(c: &Cluster, i: usize, pk: &mut SoftAuthenticator) -> Client {
    let client = c.client(i);
    let _s = client.sign_in(pk).await.unwrap();
    client
}

async fn kv_app(owner: &Client, extra: Value) -> String {
    let bundle = owner
        .packages()
        .bundle_put(common::component("fixture-kv"))
        .await
        .map(|b| b.hash)
        .unwrap();
    let mut body = json!({ "name": "kv", "bundle": &bundle });
    if let (Some(body), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
        body.extend(extra.clone());
    }
    owner
        .apps()
        .create(typed(body))
        .await
        .map(|a| a.app_id)
        .unwrap()
}

async fn set(c: &Client, app: &str, key: &str, value: &str) -> u64 {
    c.apps()
        .call(
            app,
            "set",
            Some(json!({ "key": key, "value": value })),
            None,
            None,
        )
        .await
        .unwrap()
        .index
}

fn code(e: calimero_client::Error) -> Option<String> {
    e.code().map(str::to_owned)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_watched_view_is_noticed_on_any_node_only_when_what_it_read_changes() {
    let c = Cluster::start_with(3, |cfg| cfg.replicas = 2).await;
    let (owner, mut pk) = signed_up(&c, 0, "alice").await;
    let app = kv_app(&owner, json!({})).await;
    let (leader, follower, outsider) = roles(&c, &app).await;

    for (at, name) in [
        (leader, "leader"),
        (follower, "follower"),
        (outsider, "proxy"),
    ] {
        let alice = signed_in(&c, at, &mut pk).await;
        let mut s = alice.watch_stream(&app, None).await.unwrap();
        // The stream is held by a replica: this node, or the one it proxies to.
        let holder: u64 = s.id.split('.').next().unwrap().parse().unwrap();
        assert!(c.nodes[holder as usize - 1]
            .as_ref()
            .unwrap()
            .node
            .hosted(&app)
            .is_some());
        if at != outsider {
            assert_eq!(holder, at as u64 + 1, "{name}");
        }
        let key = format!("k-{name}");
        let w = alice
            .add_watch(&app, &s.id, "get", json!({ "key": key }), None)
            .await
            .unwrap();
        assert_eq!(w.answer, Value::Null, "{name}");

        // A write elsewhere is not noticed.
        let _i = set(&owner, &app, "other", "x").await;
        assert!(frames_for(&mut s, 2).await.is_empty(), "{name}");

        // The first write is pushed at once; the ones within the interval after it come
        // as one more frame, with the newest answer.
        let mut last = 0;
        for v in ["1", "2", "3"] {
            last = set(&owner, &app, &key, v).await;
        }
        let got = frames_for(&mut s, 3).await;
        assert!(!got.is_empty() && got.len() < 3, "{name}: {got:?}");
        let (id, kind, data) = got.last().unwrap();
        let (index, answer) = result_of(kind, id, data, &w.sub);
        assert_eq!(index, last, "{name}");
        assert_eq!(answer, json!("3"), "{name}");

        // The same answer again is not sent.
        let _i = set(&owner, &app, &key, "3").await;
        assert!(frames_for(&mut s, 2).await.is_empty(), "{name}");

        // Removed, it is noticed no more.
        alice.remove_watch(&app, &w.sub).await.unwrap();
        let _i = set(&owner, &app, &key, "4").await;
        assert!(frames_for(&mut s, 2).await.is_empty(), "{name}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_scan_is_noticed_for_writes_in_what_it_read_only() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let app = kv_app(&alice, json!({})).await;
    for k in ["a", "b"] {
        let _i = set(&alice, &app, k, "1").await;
    }
    let mut s = alice.watch_stream(&app, None).await.unwrap();
    let all = alice
        .add_watch(&app, &s.id, "entries", json!({}), None)
        .await
        .unwrap();
    assert_eq!(all.answer, json!({ "a": "1", "b": "1" }));
    // A new entry lands in the scanned table: noticed.
    let index = set(&alice, &app, "c", "1").await;
    let (id, kind, data) = frame(&mut s, 3).await.expect("a notice");
    let (at, answer) = result_of(&kind, &id, &data, &all.sub);
    assert_eq!(at, index);
    assert_eq!(answer, json!({ "a": "1", "b": "1", "c": "1" }));
    // A note is outside the entries table: not noticed.
    let _r = alice
        .apps()
        .call(
            &app,
            "note",
            Some(json!({ "id": "n1", "title": "t", "body": "b" })),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(frames_for(&mut s, 2).await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn watches_are_refused_unless_the_caller_may_call_a_subscribable_bounded_view() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let (bob, _bk) = signed_up(&c, 1, "bob").await;
    let app = kv_app(&alice, json!({})).await;
    let s = alice.watch_stream(&app, None).await.unwrap();

    // Someone who cannot reach the app cannot watch it, on any stream.
    let e = bob
        .add_watch(&app, &s.id, "get", json!({ "key": "a" }), None)
        .await
        .unwrap_err();
    assert!(
        matches!(code(e).as_deref(), Some("NOT_FOUND" | "FORBIDDEN")),
        "an outsider's watch"
    );
    assert!(bob.watch_stream(&app, None).await.is_err());

    let e = alice
        .add_watch(&app, &s.id, "getResult", json!({ "key": "a" }), None)
        .await
        .unwrap_err();
    assert_eq!(code(e).as_deref(), Some("NOT_SUBSCRIBABLE"));
    let e = alice
        .add_watch(
            &app,
            &s.id,
            "set",
            json!({ "key": "a", "value": "1" }),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(code(e).as_deref(), Some("NOT_A_VIEW"));
    // A view reading more keys than a read set holds is refused, not cut down.
    let e = alice
        .add_watch(&app, &s.id, "probe", json!({ "n": 5000 }), None)
        .await
        .unwrap_err();
    assert_eq!(code(e).as_deref(), Some("READ_SET_UNBOUNDED"));
    let _ok = alice
        .add_watch(&app, &s.id, "probe", json!({ "n": 10 }), None)
        .await
        .unwrap();
    // A stream this cluster never made.
    let e = alice
        .add_watch(&app, "1.nope", "get", json!({ "key": "a" }), None)
        .await
        .unwrap_err();
    assert_eq!(code(e).as_deref(), Some("UNKNOWN_STREAM"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_member_who_loses_access_gets_watch_ended_and_no_notice() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let s = |v: Value, k: &str| v[k].as_str().unwrap().to_owned();
    let org = s(
        alice
            .orgs()
            .create("Acme", None, None)
            .await
            .map(wire)
            .unwrap(),
        "orgId",
    );
    let token = s(
        alice
            .orgs()
            .invite_create(&org, typed(json!({ "role": "member" })), Some(&mut a))
            .await
            .map(wire)
            .unwrap(),
        "token",
    );
    let _r = bob.orgs().invite_redeem(&token).await.unwrap();
    let bob_id = s(bob.auth().me().await.map(wire).unwrap(), "accountId");
    let app = kv_app(&alice, json!({ "org": org })).await;

    let mut stream = bob.watch_stream(&app, None).await.unwrap();
    let w = bob
        .add_watch(&app, &stream.id, "get", json!({ "key": "a" }), None)
        .await
        .unwrap();
    let _i = set(&alice, &app, "a", "1").await;
    let (_id, kind, _data) = frame(&mut stream, 3)
        .await
        .expect("a notice while a member");
    assert_eq!(kind, "view-result");

    // The removal alone ends the watch: no write to what it read is needed.
    let _r = alice
        .orgs()
        .remove_member(&org, &bob_id, Some(&mut a))
        .await
        .unwrap();
    let got = frames_for(&mut stream, 4).await;
    assert_eq!(got.len(), 1, "{got:?}");
    let (_id, kind, data) = &got[0];
    assert_eq!(kind, "watch-ended");
    assert_eq!(data, &json!({ "sub": w.sub, "reason": "access-lost" }));
    let _i = set(&alice, &app, "a", "2").await;
    assert!(frames_for(&mut stream, 2).await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_resumed_stream_gets_what_it_missed_and_a_lost_one_says_so() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let app = kv_app(&alice, json!({})).await;

    let s = alice.watch_stream(&app, None).await.unwrap();
    let id = s.id.clone();
    let w = alice
        .add_watch(&app, &id, "get", json!({ "key": "a" }), None)
        .await
        .unwrap();
    // The client goes; a write lands while it is away.
    drop(s);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let index = set(&alice, &app, "a", "1").await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let mut again = alice
        .watch_stream(&app, Some((&id, w.index)))
        .await
        .unwrap();
    assert_eq!(again.id, id);
    let (nid, kind, data) = frame(&mut again, 3).await.expect("the missed answer");
    assert_eq!(result_of(&kind, &nid, &data, &w.sub), (index, json!("1")));
    // Resumed after that notice: nothing is sent again.
    drop(again);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut third = alice.watch_stream(&app, Some((&id, index))).await.unwrap();
    assert_eq!(third.id, id);
    assert!(frames_for(&mut third, 2).await.is_empty());

    // A stream the node no longer holds comes back as a new one that says so: register
    // again.
    let mut fresh = alice
        .watch_stream(&app, Some(("1.gone", index)))
        .await
        .unwrap();
    assert_ne!(fresh.id, "1.gone");
    let (_id, kind, data) = frame(&mut fresh, 3).await.expect("a reset");
    assert_eq!((kind.as_str(), data), ("watch-reset", json!({})));

    // `Client::watch` does that by itself.
    let mut watch = alice
        .watch(&app, "get", json!({ "key": "b" }))
        .await
        .unwrap();
    let index = set(&alice, &app, "b", "1").await;
    let n = tokio::time::timeout(Duration::from_secs(3), watch.notices.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        n,
        WatchNotice::Answer {
            index,
            answer: json!("1")
        }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_burst_is_one_frame_per_interval_and_a_large_answer_a_bare_notice() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let app = kv_app(&alice, json!({})).await;
    let mut s = alice.watch_stream(&app, None).await.unwrap();
    // Asked for less than the app's 250 ms (`get`'s `watchIntervalMs`): the app's rate.
    let w = alice
        .add_watch(&app, &s.id, "get", json!({ "key": "a" }), Some(1))
        .await
        .unwrap();

    // Twenty writes: the first at once, then at most one frame per 250 ms; the last frame
    // has the last answer.
    let started = tokio::time::Instant::now();
    let mut last = 0;
    for v in 0..20 {
        last = set(&alice, &app, "a", &v.to_string()).await;
    }
    let spent = started.elapsed();
    let got = frames_for(&mut s, 2).await;
    let most = 2 + usize::try_from(spent.as_millis() / 250).unwrap();
    assert!(!got.is_empty() && got.len() <= most, "{got:?} in {spent:?}");
    let (id, kind, data) = got.last().unwrap();
    assert_eq!(result_of(kind, id, data, &w.sub), (last, json!("19")));

    // An answer over 64 KiB is a bare `view-changed`; the client reads it.
    let big = "x".repeat(70 * 1024);
    let r = alice
        .apps()
        .call(
            &app,
            "put",
            Some(json!({ "key": "a", "value": big })),
            None,
            None,
        )
        .await
        .unwrap();
    let (id, kind, data) = frame(&mut s, 3).await.expect("a notice");
    assert_eq!(kind, "view-changed");
    assert_eq!(frame_index(&id, &data, &w.sub, &["index", "sub"]), r.index);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_view_that_reads_the_directory_follows_system_changes() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let s = |v: Value, k: &str| v[k].as_str().unwrap().to_owned();
    let org = s(
        alice
            .orgs()
            .create("Acme", None, None)
            .await
            .map(wire)
            .unwrap(),
        "orgId",
    );
    let token = s(
        alice
            .orgs()
            .invite_create(&org, typed(json!({ "role": "member" })), Some(&mut a))
            .await
            .map(wire)
            .unwrap(),
        "token",
    );
    let _r = bob.orgs().invite_redeem(&token).await.unwrap();
    let bob_id = s(bob.auth().me().await.map(wire).unwrap(), "accountId");
    let app = kv_app(&alice, json!({ "org": org })).await;

    let mut stream = bob.watch_stream(&app, None).await.unwrap();
    let w = bob
        .add_watch(&app, &stream.id, "myRole", json!({}), None)
        .await
        .unwrap();
    assert_eq!(w.answer, json!("member"));
    // No app entry changes: the promotion is a system change.
    let _r = alice
        .orgs()
        .set_member(&org, &bob_id, typed(json!("admin")), Some(&mut a))
        .await
        .unwrap();
    let (id, kind, data) = frame(&mut stream, 5).await.expect("the new role");
    assert_eq!(result_of(&kind, &id, &data, &w.sub).1, json!("admin"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_signed_out_session_ends_its_watches() {
    let c = Cluster::start(3).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let app = kv_app(&alice, json!({})).await;
    let laptop = signed_in(&c, 1, &mut pk).await;
    let mut stream = laptop.watch_stream(&app, None).await.unwrap();
    let w = laptop
        .add_watch(&app, &stream.id, "get", json!({ "key": "a" }), None)
        .await
        .unwrap();
    let _r = laptop.auth().logout().await.unwrap();
    let got = frames_for(&mut stream, 4).await;
    assert_eq!(
        got.last().map(|(_, k, d)| (k.clone(), d.clone())),
        Some((
            "watch-ended".to_owned(),
            json!({ "sub": w.sub, "reason": "access-lost" })
        )),
        "{got:?}"
    );
}

/// The app sets how often a watch is sent a new answer (`get`: 250 ms, `entries`: the
/// default 1 s); a client asking for faster still gets the app's rate.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_app_sets_the_watch_rate_and_a_client_can_only_slow_it() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let app = kv_app(&alice, json!({})).await;
    let mut s = alice.watch_stream(&app, None).await.unwrap();
    let fast = alice
        .add_watch(&app, &s.id, "get", json!({ "key": "a" }), None)
        .await
        .unwrap();
    let slow = alice
        .add_watch(&app, &s.id, "entries", json!({}), Some(1))
        .await
        .unwrap();
    let slower = alice
        .add_watch(&app, &s.id, "get", json!({ "key": "a" }), Some(5_000))
        .await
        .unwrap();

    // A write every 100 ms for about 3 s, then what is still due.
    let started = tokio::time::Instant::now();
    for v in 0..30 {
        let _ = set(&alice, &app, "a", &v.to_string()).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let spent = started.elapsed();
    let got = frames_for(&mut s, 2).await;
    let count = |sub: &str| got.iter().filter(|(_, _, d)| d["sub"] == sub).count();
    let (fast, slow, slower) = (count(&fast.sub), count(&slow.sub), count(&slower.sub));
    let secs = usize::try_from(spent.as_secs()).unwrap();
    assert!(slow <= 2 + secs, "entries: {slow} frames in {spent:?}");
    assert!(
        slower <= 2 + secs / 5,
        "slowed get: {slower} frames in {spent:?}"
    );
    assert!(
        fast > slow + 2,
        "get {fast} vs entries {slow} frames in {spent:?}"
    );
}

/// Every node's count of one [`calimero_node::watch::RunStats`] field.
fn runs(c: &Cluster, f: fn(&calimero_node::watch::RunStats) -> u64) -> u64 {
    c.nodes
        .iter()
        .flatten()
        .map(|r| f(&r.node.watches.runs.stats))
        .sum()
}

fn reruns(s: &calimero_node::watch::RunStats) -> u64 {
    s.reruns.load(std::sync::atomic::Ordering::Relaxed)
}

/// Alice's org with Bob and Carol in it as members, and a kv app in it.
async fn org_with_members(
    c: &Cluster,
) -> (Client, SoftAuthenticator, Client, Client, String, String) {
    let (alice, mut a) = signed_up(c, 0, "alice").await;
    let (bob, _b) = signed_up(c, 1, "bob").await;
    let (carol, _c) = signed_up(c, 2, "carol").await;
    let s = |v: Value, k: &str| v[k].as_str().unwrap().to_owned();
    let org = s(
        alice
            .orgs()
            .create("Acme", None, None)
            .await
            .map(wire)
            .unwrap(),
        "orgId",
    );
    for who in [&bob, &carol] {
        let token = s(
            alice
                .orgs()
                .invite_create(&org, typed(json!({ "role": "member" })), Some(&mut a))
                .await
                .map(wire)
                .unwrap(),
            "token",
        );
        let _r = who.orgs().invite_redeem(&token).await.unwrap();
    }
    let app = kv_app(&alice, json!({ "org": org })).await;
    (alice, a, bob, carol, org, app)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_sign_in_re_runs_no_view_and_a_promotion_only_the_promoted_ones() {
    let c = Cluster::start(3).await;
    let (alice, mut a, bob, carol, org, app) = org_with_members(&c).await;
    let mut bs = bob.watch_stream(&app, None).await.unwrap();
    let mut cs = carol.watch_stream(&app, None).await.unwrap();
    let wb = bob
        .add_watch(&app, &bs.id, "myRole", json!({}), None)
        .await
        .unwrap();
    let _wc = carol
        .add_watch(&app, &cs.id, "myRole", json!({}), None)
        .await
        .unwrap();
    assert!(frames_for(&mut bs, 1).await.is_empty());
    let before = runs(&c, reruns);
    // Another session of Alice's: sessions and tokens change no fact a view reads.
    let _laptop = signed_in(&c, 2, &mut a).await;
    assert!(frames_for(&mut bs, 2).await.is_empty());
    assert_eq!(runs(&c, reruns), before);
    // Bob promoted: Bob's view runs again, Carol's does not.
    let bob_id = bob.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = alice
        .orgs()
        .set_member(&org, &bob_id, typed(json!("admin")), Some(&mut a))
        .await
        .unwrap();
    let (id, kind, data) = frame(&mut bs, 5).await.expect("Bob's new role");
    assert_eq!(result_of(&kind, &id, &data, &wb.sub).1, json!("admin"));
    assert!(frames_for(&mut cs, 2).await.is_empty());
    assert_eq!(runs(&c, reruns), before + 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn fifty_watches_of_one_view_share_one_run_per_change() {
    let c = Cluster::start(3).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let app = kv_app(&alice, json!({})).await;
    let mut stream = alice.watch_stream(&app, None).await.unwrap();
    let mut subs = Vec::new();
    for _ in 0..50 {
        let w = alice
            .add_watch(&app, &stream.id, "get", json!({ "key": "a" }), None)
            .await
            .unwrap();
        subs.push(w.sub);
    }
    let before = runs(&c, reruns);
    let _i = set(&alice, &app, "a", "1").await;
    let got = frames_for(&mut stream, 3).await;
    assert_eq!(got.len(), 50, "{got:?}");
    assert!(got.iter().all(|(_, _, d)| d["answer"] == json!("1")));
    assert_eq!(runs(&c, reruns) - before, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn over_the_run_budget_frames_come_later_and_none_is_lost() {
    let c = Cluster::start(3).await;
    for r in c.nodes.iter().flatten() {
        r.node.watches.runs.set_per_sec(1);
    }
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let app = kv_app(&alice, json!({})).await;
    let mut stream = alice.watch_stream(&app, None).await.unwrap();
    let mut subs = Vec::new();
    for k in ["a", "b", "c", "d"] {
        let w = alice
            .add_watch(&app, &stream.id, "get", json!({ "key": k }), None)
            .await
            .unwrap();
        subs.push((w.sub, k));
    }
    for k in ["a", "b", "c", "d"] {
        let _i = set(&alice, &app, k, &format!("{k}1")).await;
    }
    let got = frames_for(&mut stream, 12).await;
    for (sub, k) in &subs {
        let last = got
            .iter()
            .rev()
            .find(|(_, _, d)| d["sub"] == json!(sub))
            .unwrap_or_else(|| panic!("no frame for {k}: {got:?}"));
        assert_eq!(last.2["answer"], json!(format!("{k}1")));
    }
    assert!(
        runs(&c, |s| s
            .deferred
            .load(std::sync::atomic::Ordering::Relaxed))
            > 0
    );
}
