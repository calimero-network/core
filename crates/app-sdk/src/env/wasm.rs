//! The guest side of `env`: calls through the `calimero:app` and `wasi` imports.

use crate::bindings::calimero::app::context;
use crate::bindings::calimero::app::events;
use crate::bindings::calimero::app::types;
use crate::bindings::wasi::random::random;
use crate::env::{Audience, Caller, OrgRole};
use crate::error::{AppError, Result};
use serde_json::json;

pub fn caller() -> Caller {
    match context::caller() {
        types::Caller::Account(a) => Caller::Account(a),
        types::Caller::ApiKey(k) => Caller::ApiKey {
            key_id: k.key_id,
            account: k.account,
        },
        types::Caller::Public => Caller::Public,
        types::Caller::System => Caller::System,
        types::Caller::App(a) => Caller::App(a),
    }
}

pub fn app_id() -> String {
    context::app_id()
}

pub fn app_owner() -> String {
    context::app_owner()
}

pub fn org() -> Option<String> {
    context::org()
}

pub fn org_role(account: &str) -> Option<OrgRole> {
    context::org_role(account).map(|r| match r {
        types::OrgRole::Owner => OrgRole::Owner,
        types::OrgRole::Admin => OrgRole::Admin,
        types::OrgRole::Member => OrgRole::Member,
        types::OrgRole::Guest => OrgRole::Guest,
    })
}

pub fn in_team(account: &str, team: &str) -> bool {
    context::in_team(account, team)
}

pub fn applied_index() -> u64 {
    context::applied_index()
}

pub fn unique_id() -> String {
    context::unique_id()
}

/// The clock is `std::time`, which reaches `wasi:clocks/wall-clock`: no override here.
pub fn time_now_ms() -> Option<u64> {
    None
}

pub fn random_bytes(len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        let piece = (len - out.len()).min(calimero_app_wit::MAX_RANDOM_BYTES_PER_CALL);
        out.extend(random::get_random_bytes(piece as u64));
    }
    out
}

pub fn emit(name: &str, data: Vec<u8>, audience: Audience) -> Result<()> {
    let audience = match audience {
        Audience::All => types::Audience::All,
        Audience::Caller => types::Audience::Caller,
        Audience::Accounts(a) => types::Audience::Accounts(a),
        Audience::Team(t) => types::Audience::Team(t),
        Audience::Org => types::Audience::OrgMembers(no_org()?),
        Audience::OrgAdmins => types::Audience::OrgAdmins(no_org()?),
    };
    events::emit(&types::Event {
        name: name.to_owned(),
        data,
        audience,
    })
    .map_err(|message| AppError::with_fields("Emit", json!({ "message": message })))
}

fn no_org() -> Result<String> {
    context::org()
        .ok_or_else(|| AppError::with_fields("Emit", json!({ "message": "the app has no org" })))
}
