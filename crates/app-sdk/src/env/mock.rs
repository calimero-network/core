//! A thread-local stand-in for the host, for tests on the host target.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use rand::Rng;
use serde_json::json;

use crate::env::{Audience, Caller, OrgRole};
use crate::error::{AppError, Result};

/// What the mock host says and records.
pub struct MockState {
    /// The caller of the run.
    pub caller: Caller,
    /// The app id.
    pub app_id: String,
    /// The app owner's account.
    pub owner: String,
    /// The app's org.
    pub org: Option<String>,
    /// Roles by account.
    pub roles: HashMap<String, OrgRole>,
    /// `(account, team)` pairs that are in a team.
    pub teams: HashSet<(String, String)>,
    /// The applied log index.
    pub applied_index: u64,
    /// The clock, when pinned; the system clock otherwise.
    pub now_ms: Option<u64>,
    /// Next `unique_id` position.
    pub unique_n: u64,
    /// The `<term>.<seq>` prefix of unique ids.
    pub id_prefix: String,
    /// Events emitted: name, JSON data, audience.
    pub events: Vec<(String, Vec<u8>, Audience)>,
    /// The app's blobs by id (hex), as the node would record them at upload.
    pub blobs: HashMap<String, crate::BlobInfo>,
    /// xcalls sent: target app, method, JSON arguments.
    pub xcalls: Vec<(String, String, Vec<u8>)>,
    /// What `search_query` answers, by table; a table not listed is not searchable.
    pub search: HashMap<String, crate::search::RawPage>,
}

impl Default for MockState {
    fn default() -> Self {
        Self {
            caller: Caller::System,
            app_id: "app".into(),
            owner: "owner".into(),
            org: None,
            roles: HashMap::new(),
            teams: HashSet::new(),
            applied_index: 0,
            now_ms: None,
            unique_n: 0,
            id_prefix: "1.1".into(),
            events: Vec::new(),
            blobs: HashMap::new(),
            search: HashMap::new(),
            xcalls: Vec::new(),
        }
    }
}

thread_local! {
    static STATE: RefCell<MockState> = RefCell::new(MockState::default());
}

/// Change the mock host's state.
pub fn with_mut<R>(f: impl FnOnce(&mut MockState) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

/// Back to the default state.
pub fn reset() {
    with_mut(|s| *s = MockState::default());
}

fn read<R>(f: impl FnOnce(&MockState) -> R) -> R {
    STATE.with(|s| f(&s.borrow()))
}

pub fn caller() -> Caller {
    read(|s| s.caller.clone())
}

pub fn app_id() -> String {
    read(|s| s.app_id.clone())
}

pub fn app_owner() -> String {
    read(|s| s.owner.clone())
}

pub fn org() -> Option<String> {
    read(|s| s.org.clone())
}

pub fn org_role(account: &str) -> Option<OrgRole> {
    read(|s| s.roles.get(account).copied())
}

pub fn in_team(account: &str, team: &str) -> bool {
    read(|s| s.teams.contains(&(account.to_owned(), team.to_owned())))
}

pub fn applied_index() -> u64 {
    read(|s| s.applied_index)
}

pub fn unique_id() -> String {
    with_mut(|s| {
        let id = format!("{}.{}", s.id_prefix, s.unique_n);
        s.unique_n += 1;
        id
    })
}

pub fn time_now_ms() -> Option<u64> {
    read(|s| s.now_ms)
}

pub fn random_bytes(len: usize) -> Vec<u8> {
    let mut out = vec![0; len];
    rand::rng().fill_bytes(&mut out);
    out
}

pub fn emit(name: &str, data: Vec<u8>, audience: Audience) -> Result<()> {
    if matches!(audience, Audience::Org | Audience::OrgAdmins) && org().is_none() {
        return Err(AppError::with_fields(
            "Emit",
            json!({ "message": "the app has no org" }),
        ));
    }
    with_mut(|s| s.events.push((name.to_owned(), data, audience)));
    Ok(())
}

pub fn blob_info(id: &str) -> Option<crate::BlobInfo> {
    read(|s| s.blobs.get(id).cloned())
}

/// `"grant:<id>:<ttl>"`; panics, as the host traps, for an unknown blob or a TTL out of range.
pub fn blob_grant(id: &str, ttl_seconds: u32) -> String {
    // SAFETY: mirrors the host's trap (`HostDenied`), which ends the run.
    assert!(
        (1..=86_400).contains(&ttl_seconds) && blob_info(id).is_some(),
        "host denied: unknown blob or grant ttl out of range"
    );
    format!("grant:{id}:{ttl_seconds}")
}

pub fn search_query(
    table: &str,
    _text: &str,
    _mode: crate::SearchMode,
    _cursor: u32,
    _limit: u32,
) -> std::result::Result<crate::search::RawPage, String> {
    read(|s| s.search.get(table).cloned())
        .ok_or_else(|| format!("table {table:?} declares no search fields"))
}

pub fn xcall(app: &str, method: &str, args: &[u8]) -> std::result::Result<(), String> {
    with_mut(|s| {
        s.xcalls
            .push((app.to_owned(), method.to_owned(), args.to_vec()))
    });
    Ok(())
}
