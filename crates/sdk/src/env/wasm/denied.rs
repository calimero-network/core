//! The calls of `env` a compute component cannot make: its world has no such import, so each
//! one traps.

use crate::env::{Audience, Caller, OrgRole};
use crate::error::Result;

fn denied() -> ! {
    core::arch::wasm32::unreachable()
}

pub fn caller() -> Caller {
    denied()
}

pub fn app_id() -> String {
    denied()
}

pub fn app_owner() -> String {
    denied()
}

pub fn org() -> Option<String> {
    denied()
}

pub fn org_role(_account: &str) -> Option<OrgRole> {
    denied()
}

pub fn in_team(_account: &str, _team: &str) -> bool {
    denied()
}

pub fn applied_index() -> u64 {
    denied()
}

pub fn unique_id() -> String {
    denied()
}

pub fn emit(_name: &str, _data: Vec<u8>, _audience: Audience) -> Result<()> {
    denied()
}

pub fn blob_info(_id: &str) -> Option<crate::BlobInfo> {
    denied()
}

pub fn blob_grant(_id: &str, _ttl_seconds: u32) -> String {
    denied()
}

pub fn search_query(
    _table: &str,
    _text: &str,
    _mode: crate::SearchMode,
    _cursor: u32,
    _limit: u32,
) -> std::result::Result<crate::search::RawPage, String> {
    denied()
}

pub fn xcall(_app: &str, _method: &str, _args: &[u8]) -> std::result::Result<(), String> {
    denied()
}

pub fn xcall_call(_app: &str, _method: &str, _args: &[u8]) -> std::result::Result<Vec<u8>, String> {
    denied()
}

pub fn xcall_view(_app: &str, _method: &str, _args: &[u8]) -> std::result::Result<Vec<u8>, String> {
    denied()
}

pub fn start_action(_method: &str, _args: &[u8]) -> std::result::Result<String, String> {
    denied()
}

pub fn action_key() -> String {
    denied()
}

pub fn action_view(_method: &str, _args: &[u8]) -> std::result::Result<Vec<u8>, String> {
    denied()
}

pub fn action_call(_method: &str, _args: &[u8]) -> std::result::Result<Vec<u8>, String> {
    denied()
}

pub fn fetch(
    _request: &crate::action::Request,
) -> std::result::Result<crate::action::Response, String> {
    denied()
}
