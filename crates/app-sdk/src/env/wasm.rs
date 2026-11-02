//! The guest side of `env`: calls through the `calimero:app` and `wasi` imports.

use crate::bindings::calimero::app::action;
use crate::bindings::calimero::app::blobs;
use crate::bindings::calimero::app::context;
use crate::bindings::calimero::app::events;
use crate::bindings::calimero::app::search;
use crate::bindings::calimero::app::types;
use crate::bindings::calimero::app::xcall;
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
    random::get_random_bytes(len as u64)
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

pub fn blob_info(id: &str) -> Option<crate::BlobInfo> {
    blobs::info(id).map(|i| crate::BlobInfo {
        size: i.size,
        sha256: i.sha256,
        mime: i.mime,
    })
}

pub fn blob_grant(id: &str, ttl_seconds: u32) -> String {
    blobs::grant(id, ttl_seconds)
}

pub fn search_query(
    table: &str,
    text: &str,
    mode: crate::SearchMode,
    cursor: u32,
    limit: u32,
) -> std::result::Result<crate::search::RawPage, String> {
    let mode = match mode {
        crate::SearchMode::Words => search::SearchMode::Words,
        crate::SearchMode::Prefix => search::SearchMode::Prefix,
        crate::SearchMode::Substring => search::SearchMode::Substring,
        crate::SearchMode::Fuzzy => search::SearchMode::Fuzzy,
    };
    let page = search::query(table, text, mode, cursor, limit)?;
    Ok(crate::search::RawPage {
        hits: page
            .hits
            .into_iter()
            .map(|h| crate::search::RawHit {
                key: h.key,
                score: h.score,
                snippet: h.snippet,
            })
            .collect(),
        next: page.next,
        total: page.total,
    })
}

pub fn xcall(app: &str, method: &str, args: &[u8]) -> std::result::Result<(), String> {
    xcall::send(app, method, args)
}

pub fn start_action(method: &str, args: &[u8]) -> std::result::Result<String, String> {
    context::start_action(method, args)
}

pub fn action_key() -> String {
    action::key()
}

pub fn action_view(method: &str, args: &[u8]) -> std::result::Result<Vec<u8>, String> {
    action::view(method, args)
}

pub fn action_call(method: &str, args: &[u8]) -> std::result::Result<Vec<u8>, String> {
    action::call(method, args)
}

/// One request over `wasi:http`: the body written and finished before `handle`, the
/// response read whole.
pub fn fetch(
    request: &crate::action::Request,
) -> std::result::Result<crate::action::Response, String> {
    use crate::bindings::wasi::http::outgoing_handler;
    use crate::bindings::wasi::http::types::{
        Fields, IncomingBody, Method, OutgoingBody, OutgoingRequest, Scheme,
    };
    use crate::bindings::wasi::io::streams::StreamError;

    let (scheme, rest) = if let Some(rest) = request.url.strip_prefix("https://") {
        (Scheme::Https, rest)
    } else if let Some(rest) = request.url.strip_prefix("http://") {
        (Scheme::Http, rest)
    } else {
        return Err("a URL starts with https:// or http://".into());
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let headers = Fields::new();
    for (name, value) in &request.headers {
        headers
            .append(name, value.as_bytes())
            .map_err(|e| format!("header {name}: {e:?}"))?;
    }
    let out = OutgoingRequest::new(headers);
    let method = match request.method.to_ascii_uppercase().as_str() {
        "GET" => Method::Get,
        "HEAD" => Method::Head,
        "POST" => Method::Post,
        "PUT" => Method::Put,
        "DELETE" => Method::Delete,
        "PATCH" => Method::Patch,
        "OPTIONS" => Method::Options,
        other => Method::Other(other.to_owned()),
    };
    out.set_method(&method)
        .map_err(|()| "bad method".to_owned())?;
    out.set_scheme(Some(&scheme))
        .map_err(|()| "bad scheme".to_owned())?;
    out.set_authority(Some(authority))
        .map_err(|()| "bad authority".to_owned())?;
    out.set_path_with_query(Some(path))
        .map_err(|()| "bad path".to_owned())?;
    let body = out.body().map_err(|()| "no body".to_owned())?;
    {
        let stream = body.write().map_err(|()| "no body stream".to_owned())?;
        for chunk in request.body.chunks(4096) {
            stream
                .blocking_write_and_flush(chunk)
                .map_err(|e| format!("body: {e:?}"))?;
        }
    }
    OutgoingBody::finish(body, None).map_err(|e| format!("body: {e:?}"))?;
    let future = outgoing_handler::handle(out, None).map_err(|e| format!("{e:?}"))?;
    future.subscribe().block();
    let response = future
        .get()
        .ok_or_else(|| "no response".to_owned())?
        .map_err(|()| "response taken".to_owned())?
        .map_err(|e| format!("{e:?}"))?;
    let status = response.status();
    let headers = response
        .headers()
        .entries()
        .into_iter()
        .map(|(n, v)| (n, String::from_utf8_lossy(&v).into_owned()))
        .collect();
    let incoming = response.consume().map_err(|()| "no body".to_owned())?;
    let mut bytes = Vec::new();
    {
        let stream = incoming
            .stream()
            .map_err(|()| "no body stream".to_owned())?;
        loop {
            match stream.blocking_read(64 * 1024) {
                Ok(chunk) => bytes.extend_from_slice(&chunk),
                Err(StreamError::Closed) => break,
                Err(e) => return Err(format!("body: {e:?}")),
            }
        }
    }
    let _trailers = IncomingBody::finish(incoming);
    Ok(crate::action::Response {
        status,
        headers,
        body: bytes,
    })
}
