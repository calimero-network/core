//! `env` and `guards` against the mock host.

use calimero_app_sdk::env::{self, mock, Audience, Caller, OrgRole};
use calimero_app_sdk::guards;

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
