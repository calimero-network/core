//! The bytes the API answers, held to what it answered before its answers were typed
//! (review finding E7): each test builds an answer the way its handler does, from a fixed
//! state, and compares the serialized text, key order included, with the text the
//! `json!`-built answer produced. A typed answer that renames, drops, adds or reorders a
//! field, or writes a number as a string, turns one red.

use serde::Serialize;
use serde_json::json;

use super::{backup_keys, orgs, packages, session_answer, sign_up, NewSession};
use crate::system::{BackupHolder, PackageRecord, SignUp, SignUpInvite, SystemState};
use crate::types::{AppRecord, Org};

/// `answer` as the handler writes it.
fn text<T: Serialize>(answer: &T) -> String {
    serde_json::to_string(answer).expect("an answer serializes")
}

/// A state with one organization (`o1`: `alice` its owner, `bob` a member), a team `t1`
/// with both in it (`alice` its lead) and a child team `t2`, an app `a1` on `t1`, a
/// package `p1` with two releases, a backup key for `o1` and two release deadlines.
fn state() -> SystemState {
    let mut s = SystemState::default();
    let org: Org = serde_json::from_value(json!({
        "orgId": "o1",
        "name": "Org One",
        "members": { "alice": "owner", "bob": "member" },
        "teams": {
            "t1": {
                "teamId": "t1",
                "name": "Team One",
                "join": "invite",
                "members": { "alice": true, "bob": false },
                "metadata": { "app": { "package": "p1" } },
                "defaultCapabilities": 3
            },
            "t2": { "teamId": "t2", "name": "Team Two", "parent": "t1", "join": "everyone" }
        },
        "upgradePolicy": { "autoApply": "noDestinations", "holdHours": 48, "secondApprover": true },
        "appUpgradePolicies": { "a1": { "holdHours": 24 } }
    }))
    .expect("an org");
    let _prev = s.orgs.insert("o1".to_owned(), org);
    let app: AppRecord = serde_json::from_value(json!({
        "appId": "a1",
        "name": "App One",
        "group": 7,
        "bundle": "b1",
        "owner": "alice",
        "org": "o1",
        "grants": [{ "kind": "team", "team": "t1" }],
        "replicas": [1, 2, 3],
        "createdAt": 10
    }))
    .expect("an app");
    let _prev = s.apps.insert("a1".to_owned(), app);
    let _prev = s.packages.insert("p1".to_owned(), package());
    let _prev = s.backup_keys.insert(
        BackupHolder::Org("o1".to_owned()).key(),
        serde_json::from_value(json!({
            "holder": { "kind": "org", "id": "o1" },
            "publicKey": "ab".repeat(32),
            "setAt": 1_700_000_000_u64,
            "setBy": "alice"
        }))
        .expect("a backup key"),
    );
    let _prev = s
        .release_deadlines
        .insert("1.10.0".to_owned(), 1_800_000_000);
    let _prev = s
        .release_deadlines
        .insert("1.9.0".to_owned(), 1_700_000_000);
    s
}

fn package() -> PackageRecord {
    serde_json::from_value(json!({
        "name": "p1",
        "signerId": "did:key:z6Mk",
        "owner": "alice",
        "versions": {
            "1.0.0": {
                "version": "1.0.0", "mpk": "h1", "mpkSize": 100, "source": "upload",
                "services": { "app": "c1" }, "metadata": { "title": "One" },
                "outbound": ["https://api.example.com/"], "installedBy": "alice",
                "installedAt": 5
            },
            "1.2.0": {
                "version": "1.2.0", "mpk": "h2", "mpkSize": 200, "source": "registry",
                "services": {}, "components": "computeOnly", "installedBy": "bob",
                "installedAt": 6
            }
        }
    }))
    .expect("a package")
}

#[test]
fn a_session_answers_as_before() {
    let new = NewSession {
        refresh: "rt_r".to_owned(),
        session: "s".to_owned(),
        expires_at: 9,
    };
    assert_eq!(
        text(&session_answer("acc", &("tok".to_owned(), 1_700_000_900), &new).0),
        r#"{"accountId":"acc","sessionToken":"tok","expiresAt":1700000900,"refreshToken":"rt_r"}"#
    );
}

#[test]
fn a_backup_key_answers_as_before() {
    let s = state();
    assert_eq!(
        text(&backup_keys::backup_key_answer(
            &s,
            &BackupHolder::Org("o1".to_owned())
        )),
        r#"{"holder":{"kind":"org","id":"o1"},"key":{"publicKey":"abababababababababababababababababababababababababababababababab","setAt":1700000000,"setBy":"alice"},"apps":1}"#
    );
    assert_eq!(
        text(&backup_keys::backup_key_answer(
            &s,
            &BackupHolder::Account("bob".to_owned())
        )),
        r#"{"holder":{"kind":"account","id":"bob"},"key":null,"apps":0}"#
    );
}

#[test]
fn a_package_answers_as_before() {
    let s = state();
    let p = package();
    assert_eq!(
        text(&packages::package_json(&s, &p, "alice", true)),
        r#"{"package":"p1","signerId":"did:key:z6Mk","owner":"alice","latest":"1.2.0","versions":[{"version":"1.0.0","mpk":"h1","size":100,"source":"upload","services":{"app":"c1"},"components":"app","metadata":{"title":"One"},"outbound":["https://api.example.com/"],"installedBy":"alice","installedAt":5},{"version":"1.2.0","mpk":"h2","size":200,"source":"registry","services":{},"components":"computeOnly","metadata":null,"outbound":[],"installedBy":"bob","installedAt":6}]}"#
    );
    assert!(packages::package_json(&s, &p, "carol", false).is_none());
}

#[test]
fn an_origin_answers_as_before() {
    let o: crate::origins::PackageOrigin = serde_json::from_value(json!({
        "package": "p1",
        "rpId": "example.com",
        "addedBy": "alice",
        "addedAt": 1_700_000_000_u64,
        "served": true
    }))
    .expect("an origin");
    assert_eq!(
        text(&crate::origins::origin_json("https://app.example.com", &o)),
        r#"{"package":"p1","origin":"https://app.example.com","rpId":"example.com","addedBy":"alice","addedAt":"2023-11-14T22:13:20Z","served":true}"#
    );
}

#[test]
fn a_policy_answers_as_before() {
    let s = state();
    assert_eq!(
        text(&s.policy_json("o1", None)),
        r#"{"org":"o1","policy":{"autoApply":"noDestinations","holdHours":48,"secondApprover":true},"source":"org","own":{"autoApply":"noDestinations","holdHours":48,"secondApprover":true}}"#
    );
    assert_eq!(
        text(&s.policy_json("o1", Some("a1"))),
        r#"{"org":"o1","app":"a1","policy":{"autoApply":"never","holdHours":24,"secondApprover":false},"source":"app","own":{"autoApply":"never","holdHours":24,"secondApprover":false}}"#
    );
    assert_eq!(
        text(&s.policy_json("o9", None)),
        r#"{"org":"o9","policy":{"autoApply":"never","holdHours":24,"secondApprover":false},"source":"default","own":null}"#
    );
}

#[test]
fn a_team_answers_as_before() {
    let s = state();
    let o = s.orgs.get("o1").expect("o1");
    assert_eq!(
        text(&orgs::team_view(&s, o, "t1", "bob")),
        r#"{"teamId":"t1","name":"Team One","parent":null,"join":"invite","metadata":{"app":{"package":"p1"}},"package":"p1","defaultCapabilities":3,"members":[{"account":"alice","role":"lead","capabilities":4294967295},{"account":"bob","role":"member","capabilities":7}],"children":["t2"],"apps":[{"appId":"a1","name":"App One","contextId":"f55ff16f66f43360266b95db6f8fec01d76031054306ae4a4b380598f6cfd114"}],"myRole":"member","member":true}"#
    );
    assert_eq!(
        text(&orgs::team_view(&s, o, "t2", "alice")),
        r#"{"teamId":"t2","name":"Team Two","parent":"t1","join":"everyone","metadata":null,"package":null,"defaultCapabilities":null,"members":[],"children":[],"apps":[],"myRole":null,"member":true}"#
    );
}

#[test]
fn deadlines_answer_as_before() {
    assert_eq!(
        text(&state().deadlines_json()),
        r#"[{"version":"1.9.0","after":"2023-11-14T22:13:20Z","afterSecs":1700000000},{"version":"1.10.0","after":"2027-01-15T08:00:00Z","afterSecs":1800000000}]"#
    );
}

#[test]
fn the_sign_up_policy_answers_as_before() {
    assert_eq!(
        text(&sign_up::sign_up_json(SignUp::Open)),
        r#"{"policy":"open","orgInvites":true,"signUpInvites":true,"inviteRequired":false}"#
    );
    assert_eq!(
        text(&sign_up::sign_up_json(SignUp::Invite)),
        r#"{"policy":"invite","orgInvites":true,"signUpInvites":true,"inviteRequired":true}"#
    );
    assert_eq!(
        text(&sign_up::sign_up_json(SignUp::Closed)),
        r#"{"policy":"closed","orgInvites":false,"signUpInvites":true,"inviteRequired":true}"#
    );
    let invite = SignUpInvite {
        expires_at: 1_800_000_000,
        uses_left: 3,
        created_by: "operator".to_owned(),
        created_at: 1_700_000_000,
    };
    assert_eq!(
        text(&sign_up::sign_up_invite_json("ik", &invite)),
        r#"{"inviteKey":"ik","uses":3,"expiresAt":"2027-01-15T08:00:00Z","expiresAtSecs":1800000000,"createdBy":"operator","createdAt":"2023-11-14T22:13:20Z"}"#
    );
}
