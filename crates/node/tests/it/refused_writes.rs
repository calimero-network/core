//! A system write the API can already tell `apply` would refuse is answered
//! without proposing it, so it costs no entry in the system log; a write to an
//! instance or organization the caller cannot see answers as one that does not
//! exist, as reads do; and an unknown instance id is answered without waiting.
//! [Judged before it is proposed](https://calimero-network.github.io/core/protocol/system-group/#judged-before-it-is-proposed).

use std::time::{Duration, Instant};

use calimero_client::types::{AppCreateRequest, TeamJoin};
use calimero_client::{Client, Error};
use serde_json::{json, Value};

use crate::harness;

use harness::{home_org, kv_wasm, signed_up, typed, wire, Cluster};

/// The highest system log index any node holds.
fn last_log(c: &Cluster) -> u64 {
    c.nodes
        .iter()
        .flatten()
        .map(|r| r.node.system.status().last_log.unwrap_or(0))
        .max()
        .unwrap_or(0)
}

/// The system log index once nothing has been appended for half a second.
async fn settled(c: &Cluster) -> u64 {
    let mut last = last_log(c);
    let mut since = Instant::now();
    while since.elapsed() < Duration::from_millis(500) {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let now = last_log(c);
        if now != last {
            (last, since) = (now, Instant::now());
        }
    }
    last
}

/// An instance of `kv` in `org`, made by `owner`.
async fn instance(owner: &Client, org: &str) -> String {
    let bundle = owner
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    owner
        .apps()
        .create(AppCreateRequest {
            name: Some("kv".to_owned()),
            bundle: Some(bundle),
            org: Some(org.to_owned()),
            ..typed(json!({}))
        })
        .await
        .unwrap()
        .app_id
}

/// What an answer says: its status, code and message, or `ok`.
fn said<T>(answer: Result<T, Error>) -> String {
    match answer {
        Ok(_) => "ok".to_owned(),
        Err(Error::Api {
            status,
            code,
            message,
            ..
        }) => format!("{status} {code}: {message}"),
        Err(e) => format!("{e}"),
    }
}

/// An account with no role anywhere is refused every system write it tries on
/// another account's organization and instance, and none of them is proposed:
/// the system log does not grow.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_refused_system_write_costs_no_log_entry() {
    let c = Cluster::start(3).await;
    let (alice, _apk) = signed_up(&c, 0, "alice").await;
    let (bob, _bpk) = signed_up(&c, 1, "bob").await;
    let org = home_org(&alice).await;
    let team = wire(
        alice
            .orgs()
            .team_create(&org, "eng", TeamJoin::Invite, None)
            .await
            .unwrap(),
    )["teamId"]
        .as_str()
        .unwrap()
        .to_owned();
    let alice_id = wire(alice.auth().me().await.unwrap())["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let app = instance(&alice, &org).await;

    let before = settled(&c).await;
    let mut answers: Vec<(&str, String)> = vec![
        (
            "org metadata",
            said(bob.orgs().metadata(&org, json!({ "x": 1 })).await),
        ),
        (
            "missing org metadata",
            said(bob.orgs().metadata("no-such-org", json!({ "x": 1 })).await),
        ),
        (
            "member metadata",
            said(
                bob.orgs()
                    .member_metadata(&org, &alice_id, json!({ "x": 1 }))
                    .await,
            ),
        ),
        (
            "team create",
            said(
                bob.orgs()
                    .team_create(&org, "t", TeamJoin::Invite, None)
                    .await,
            ),
        ),
        (
            "missing org team create",
            said(
                bob.orgs()
                    .team_create("no-such-org", "t", TeamJoin::Invite, None)
                    .await,
            ),
        ),
        (
            "team update",
            said(
                bob.orgs()
                    .team_update(&org, &team, Some("x"), None, None, None)
                    .await,
            ),
        ),
        (
            "empty team update",
            said(
                bob.orgs()
                    .team_update(&org, &team, None, None, None, None)
                    .await,
            ),
        ),
        (
            "app metadata",
            said(bob.apps().metadata(&app, json!({ "x": 1 })).await),
        ),
        (
            "app access",
            said(bob.apps().access(&app, Vec::new()).await),
        ),
        ("app delete", said(bob.apps().delete(&app).await)),
        (
            "invite redeem",
            said(bob.orgs().invite_redeem("not-a-token").await),
        ),
    ];
    // An empty update a member who sees the team sends changes nothing, and
    // is not proposed either.
    answers.push((
        "owner's empty team update",
        said(
            alice
                .orgs()
                .team_update(&org, &team, None, None, None, None)
                .await,
        ),
    ));
    for (what, answer) in &answers {
        let owner_noop = *what == "owner's empty team update";
        assert_eq!(answer == "ok", owner_noop, "{what}: {answer}");
    }
    assert_eq!(
        last_log(&c),
        before,
        "refused or empty writes reached the system log: {answers:#?}"
    );
}

/// On every write route of an instance, an account that does not reach it gets
/// the very answer an id that does not exist gets.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_unseen_instance_answers_as_a_missing_one() {
    let c = Cluster::start(3).await;
    let (alice, _apk) = signed_up(&c, 0, "alice").await;
    let (bob, mut bpk) = signed_up(&c, 1, "bob").await;
    let org = home_org(&alice).await;
    let bobs = home_org(&bob).await;
    let app = instance(&alice, &org).await;
    let missing = "no-such-app";

    let mut differ = Vec::new();
    for (what, seen, unknown) in [
        (
            "metadata",
            said(bob.apps().metadata(&app, json!({})).await),
            said(bob.apps().metadata(missing, json!({})).await),
        ),
        (
            "access",
            said(bob.apps().access(&app, Vec::new()).await),
            said(bob.apps().access(missing, Vec::new()).await),
        ),
        (
            "delete",
            said(bob.apps().delete(&app).await),
            said(bob.apps().delete(missing).await),
        ),
        (
            "transfer",
            said(bob.apps().transfer(&app, "alice", Some(&mut bpk)).await),
            said(bob.apps().transfer(missing, "alice", Some(&mut bpk)).await),
        ),
        (
            "move",
            said(bob.apps().app_move(&app, &bobs, None, Some(&mut bpk)).await),
            said(
                bob.apps()
                    .app_move(missing, &bobs, None, Some(&mut bpk))
                    .await,
            ),
        ),
        (
            "delegations",
            said(bob.apps().delegations(&app).await),
            said(bob.apps().delegations(missing).await),
        ),
        (
            "unlink",
            said(bob.apps().unlink(&app, "ref", Some(&mut bpk)).await),
            said(bob.apps().unlink(missing, "ref", Some(&mut bpk)).await),
        ),
        (
            "secrets",
            said(bob.apps().secrets(&app).await),
            said(bob.apps().secrets(missing).await),
        ),
        (
            "upgrade",
            said(
                bob.apps()
                    .upgrade(&app, typed(json!({ "version": "9.9.9" })), Some(&mut bpk))
                    .await,
            ),
            said(
                bob.apps()
                    .upgrade(
                        missing,
                        typed(json!({ "version": "9.9.9" })),
                        Some(&mut bpk),
                    )
                    .await,
            ),
        ),
    ] {
        assert!(unknown.starts_with("404 "), "{what}: {unknown}");
        if seen != unknown {
            differ.push(json!({ "route": what, "unseen": seen, "missing": unknown }));
        }
    }
    assert!(differ.is_empty(), "{:#}", Value::from(differ));
}

/// An id no instance has is answered at once on a node that caught up with the
/// system leader, not after the wait for an instance made through another node.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_unknown_instance_id_is_answered_without_waiting() {
    let c = Cluster::start(3).await;
    let (alice, _apk) = signed_up(&c, 1, "alice").await;
    let started = Instant::now();
    let answer = said(alice.apps().get("no-such-app").await);
    let took = started.elapsed();
    assert!(answer.starts_with("404 "), "{answer}");
    assert!(took < Duration::from_secs(1), "a missing id took {took:?}");
}
