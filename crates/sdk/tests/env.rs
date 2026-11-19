//! `env` and `guards` against the mock host.

use calimero_sdk::env::{self, mock, Audience, Caller, OrgRole};
use calimero_sdk::guards;

#[test]
fn guards_follow_caller_and_role() {
    mock::reset();
    assert_eq!(guards::require_member().unwrap_err().code, "LoginRequired");
    mock::with_mut(|s| {
        s.caller = Caller::Account("ann".into());
        s.org = Some("org".into());
    });
    assert_eq!(guards::require_member().unwrap_err().code, "Forbidden");
    mock::with_mut(|s| {
        s.roles.insert("ann".into(), OrgRole::Member);
    });
    assert_eq!(guards::require_member().unwrap(), "ann");
    assert_eq!(guards::require_admin().unwrap_err().code, "Forbidden");
    mock::with_mut(|s| {
        s.roles.insert("ann".into(), OrgRole::Admin);
    });
    assert_eq!(guards::require_admin().unwrap(), "ann");
}

#[test]
fn unique_ids_and_events() {
    mock::reset();
    assert_eq!(env::unique_id(), "1.1.0");
    assert_eq!(env::unique_id(), "1.1.1");
    assert!(env::emit_raw("x", &1, Audience::Org).is_err());
    env::emit_raw("x", &1, Audience::All).unwrap();
    assert_eq!(mock::with_mut(|s| s.events.len()), 1);
    mock::with_mut(|s| s.now_ms = Some(42));
    assert_eq!(env::time_now(), 42);
    assert_eq!(env::random_bytes(8).len(), 8);
}

/// `env::unit_of`: none for an app on its own; the instance's id for a member of its unit,
/// which an app serving others takes as its admin (`Caller::App` of that id).
#[test]
fn unit_of_names_the_instance_whose_unit_the_app_is_in() {
    mock::reset();
    assert_eq!(env::unit_of(), None);
    mock::with_mut(|s| {
        s.unit_of = Some("notes-app".into());
        s.caller = Caller::App("notes-app".into());
    });
    assert_eq!(env::unit_of().as_deref(), Some("notes-app"));
    let admin = env::unit_of().is_some_and(|parent| env::caller() == Caller::App(parent));
    assert!(admin);
    mock::with_mut(|s| s.caller = Caller::App("other-app".into()));
    let admin = env::unit_of().is_some_and(|parent| env::caller() == Caller::App(parent));
    assert!(!admin);
}
