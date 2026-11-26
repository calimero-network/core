//! Live views on an in-process cluster: three nodes, the app on two of them, so a watch
//! stream is served by the leader, by a follower, or proxied by the node that does not host
//! the app (`api::watches`, `watch.rs`).

use crate::common;

use std::time::Duration;

use calimero_client::types::ViewsRequestConsistency;
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

/// A notice's data is exactly `{ sub, index }` and its id `<index>:<seq>`.
fn notice_index(id: &str, data: &Value, sub: &str) -> u64 {
    let obj = data.as_object().expect("an object");
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["index", "sub"], "{data}");
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
            .add_watch(&app, &s.id, "get", json!({ "key": key }))
            .await
            .unwrap();

        // A write elsewhere is not noticed.
        let _i = set(&owner, &app, "other", "x").await;
        assert!(frames_for(&mut s, 2).await.is_empty(), "{name}");

        // Writes to the key within the interval are noticed once (at most twice, if one
        // interval boundary falls between them), naming the newest.
        let mut last = 0;
        for v in ["1", "2", "3"] {
            last = set(&owner, &app, &key, v).await;
        }
        let got = frames_for(&mut s, 3).await;
        assert!(!got.is_empty() && got.len() < 3, "{name}: {got:?}");
        let (id, kind, data) = got.last().unwrap();
        assert_eq!(kind, "view-changed");
        let index = notice_index(id, data, &w.sub);
        assert_eq!(index, last, "{name}");

        // The re-read is a plain view at that index or later.
        let v = alice
            .apps()
            .view(
                &app,
                "get",
                Some(json!({ "key": key })),
                Some(ViewsRequestConsistency::Stale),
                Some(index),
            )
            .await
            .unwrap();
        assert_eq!(v.result, json!("3"), "{name}");

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
        .add_watch(&app, &s.id, "entries", json!({}))
        .await
        .unwrap();
    // A new entry lands in the scanned table: noticed.
    let index = set(&alice, &app, "c", "1").await;
    let (id, kind, data) = frame(&mut s, 3).await.expect("a notice");
    assert_eq!(kind, "view-changed");
    assert_eq!(notice_index(&id, &data, &all.sub), index);
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
        .add_watch(&app, &s.id, "get", json!({ "key": "a" }))
        .await
        .unwrap_err();
    assert!(
        matches!(code(e).as_deref(), Some("NOT_FOUND" | "FORBIDDEN")),
        "an outsider's watch"
    );
    assert!(bob.watch_stream(&app, None).await.is_err());

    let e = alice
        .add_watch(&app, &s.id, "getResult", json!({ "key": "a" }))
        .await
        .unwrap_err();
    assert_eq!(code(e).as_deref(), Some("NOT_SUBSCRIBABLE"));
    let e = alice
        .add_watch(&app, &s.id, "set", json!({ "key": "a", "value": "1" }))
        .await
        .unwrap_err();
    assert_eq!(code(e).as_deref(), Some("NOT_A_VIEW"));
    // A view reading more keys than a read set holds is refused, not cut down.
    let e = alice
        .add_watch(&app, &s.id, "probe", json!({ "n": 5000 }))
        .await
        .unwrap_err();
    assert_eq!(code(e).as_deref(), Some("READ_SET_UNBOUNDED"));
    let _ok = alice
        .add_watch(&app, &s.id, "probe", json!({ "n": 10 }))
        .await
        .unwrap();
    // A stream this cluster never made.
    let e = alice
        .add_watch(&app, "1.nope", "get", json!({ "key": "a" }))
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
        .add_watch(&app, &stream.id, "get", json!({ "key": "a" }))
        .await
        .unwrap();
    let _i = set(&alice, &app, "a", "1").await;
    let (_id, kind, _data) = frame(&mut stream, 3)
        .await
        .expect("a notice while a member");
    assert_eq!(kind, "view-changed");

    let _r = alice
        .orgs()
        .remove_member(&org, &bob_id, Some(&mut a))
        .await
        .unwrap();
    let _i = set(&alice, &app, "a", "2").await;
    let got = frames_for(&mut stream, 4).await;
    assert_eq!(got.len(), 1, "{got:?}");
    let (_id, kind, data) = &got[0];
    assert_eq!(kind, "watch-ended");
    assert_eq!(data, &json!({ "sub": w.sub }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_resumed_stream_gets_what_it_missed_and_a_lost_one_says_so() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let app = kv_app(&alice, json!({})).await;

    let s = alice.watch_stream(&app, None).await.unwrap();
    let id = s.id.clone();
    let w = alice
        .add_watch(&app, &id, "get", json!({ "key": "a" }))
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
    let (nid, kind, data) = frame(&mut again, 3).await.expect("the missed notice");
    assert_eq!(kind, "view-changed");
    assert_eq!(notice_index(&nid, &data, &w.sub), index);
    // Resumed after that notice: nothing is sent again.
    drop(again);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut third = alice.watch_stream(&app, Some((&id, index))).await.unwrap();
    assert_eq!(third.id, id);
    assert!(frames_for(&mut third, 2).await.is_empty());

    // A stream the node no longer holds comes back as a new one: register again, re-read.
    let fresh = alice
        .watch_stream(&app, Some(("1.gone", index)))
        .await
        .unwrap();
    assert_ne!(fresh.id, "1.gone");

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
    assert_eq!(n, WatchNotice::Changed { index });
}
