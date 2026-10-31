//! The app's own tests, on the real engine.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use calimero_app_sdk::testing::{Failure, Org, TestApp};
use serde_json::{json, Value};

fn call(t: &mut TestApp, who: &str, method: &str, team: &str) -> Value {
    t.as_(who)
        .call(method, json!({ "teamId": team }))
        .unwrap_or_else(|e| panic!("{method}: {e:?}"))
}

#[test]
fn records_accumulate_per_team() {
    let mut t = common::app(Org::new());
    assert_eq!(call(&mut t, "owner", "recordWin", "red"), json!("1"));
    assert_eq!(call(&mut t, "owner", "recordWin", "red"), json!("2"));
    call(&mut t, "owner", "recordLoss", "red");
    call(&mut t, "owner", "recordDraw", "red");
    assert_eq!(call(&mut t, "owner", "getWins", "red"), json!("2"));
    assert_eq!(call(&mut t, "owner", "getLosses", "red"), json!("1"));
    assert_eq!(call(&mut t, "owner", "getDraws", "red"), json!("1"));
    let (name, data) = t.events_for("owner").last().cloned().unwrap();
    assert_eq!(name, "DrawRecorded");
    assert_eq!(data, json!({ "teamId": "red", "total": "1" }));
}

#[test]
fn teams_are_independent() {
    let mut t = common::app(Org::new());
    call(&mut t, "owner", "recordWin", "red");
    call(&mut t, "owner", "recordWin", "blue");
    call(&mut t, "owner", "recordWin", "blue");
    assert_eq!(call(&mut t, "owner", "getWins", "red"), json!("1"));
    assert_eq!(call(&mut t, "owner", "getWins", "blue"), json!("2"));
}

/// v1's convergence test: wins recorded by three writers sum (one log orders them).
#[test]
fn wins_from_several_members_sum() {
    let mut t = common::app(Org::new().members(["a", "b", "c"]));
    for who in ["a", "b", "c"] {
        call(&mut t, who, "recordWin", "liverpool");
    }
    call(&mut t, "a", "recordWin", "arsenal");
    assert_eq!(call(&mut t, "b", "getWins", "liverpool"), json!("3"));
    assert_eq!(call(&mut t, "c", "getWins", "arsenal"), json!("1"));
}

#[test]
fn an_unknown_team_is_an_error() {
    let mut t = common::app(Org::new());
    let r = t
        .as_("owner")
        .view("getWins", json!({ "teamId": "nobody" }));
    match r {
        Err(Failure::App { code, fields }) => {
            assert_eq!(code, "TeamNotFound");
            assert_eq!(fields, json!({ "teamId": "nobody" }));
        }
        other => panic!("{other:?}"),
    }
}

/// v1 proved its merge rule with two nodes each awarding a different badge; one log orders
/// the two awards here, and the second reads the first's mask, so both bits survive.
#[test]
fn badges_from_two_members_accumulate() {
    let mut t = common::app(Org::new().members(["a", "b"]));
    let award = |t: &mut TestApp, who: &str, badge: u32| {
        t.as_(who)
            .call("awardBadge", json!({ "teamId": "alpha", "badge": badge }))
            .unwrap()
    };
    assert_eq!(award(&mut t, "a", 0), json!("1"));
    assert_eq!(award(&mut t, "b", 1), json!("3"));
    assert_eq!(award(&mut t, "b", 1), json!("3"));
    assert_eq!(call(&mut t, "a", "getBadgeCount", "alpha"), json!(2));
    assert_eq!(call(&mut t, "b", "getBadges", "alpha"), json!("3"));
    let r = t
        .as_("a")
        .call("awardBadge", json!({ "teamId": "alpha", "badge": 64 }));
    match r {
        Err(Failure::App { code, fields }) => {
            assert_eq!(code, "BadgeOutOfRange");
            assert_eq!(fields, json!({ "badge": 64 }));
        }
        other => panic!("{other:?}"),
    }
}
