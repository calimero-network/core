//! The size of the log entry a node writes for a run, for an embedder that runs apps without a
//! log (the SDK's test harness): the node refuses an entry over `calimero_log::MAX_ENTRY_BYTES`
//! and logs the run as a `HostDenied` trap with detail [`crate::ENTRY_TOO_LARGE`] instead (D50), whatever [`crate::MAX_RUN_BYTES`]
//! let through, because the entry is JSON with keys and values in base64.
//!
//! [`entry_bytes`] is the length of `calimero-node`'s `entry_json` for the same run, field by
//! field (pinned there by `the_host_cores_entry_size_is_the_logs`). It leaves out what only a
//! node knows: an xcall delivery's source fields, and the marks of files, published outputs
//! and started compute runs (their keys and segment hashes are the node's).

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde_json::{json, Map, Value};

use crate::action::HttpExchange;
use crate::types::{Audience, Caller, Outcome, Status, TrapKind};

/// The cap a node puts on one log entry (`calimero_log::MAX_ENTRY_BYTES`).
pub const MAX_ENTRY_BYTES: usize = 1024 * 1024;

/// What a logged run's entry records besides its outcome.
#[derive(Clone, Copy, Debug)]
pub enum EntryOf<'a> {
    /// A call or an xcall delivery (`componentCall`): the method, its JSON args and caller.
    Call {
        method: &'a str,
        args: &'a [u8],
        caller: &'a Caller,
    },
    /// A timer tick (`componentTick`).
    Tick {
        timer: &'a str,
        scheduled_ms: u64,
        previous_ms: Option<u64>,
    },
    /// `init` (`componentInit`): the bundle's hex hash.
    Init { bundle: &'a str },
}

/// The ids and times every run's entry carries.
#[derive(Clone, Copy, Debug)]
pub struct EntryIds {
    pub ts_ms: u64,
    pub base_index: u64,
    pub id_term: u64,
    pub id_seq: u64,
}

/// Bytes of the JSON log entry a node writes for a run of `of` that ended in `out`.
#[must_use]
pub fn entry_bytes(of: EntryOf<'_>, ids: EntryIds, out: &Outcome) -> usize {
    let mut entry = Map::new();
    match of {
        EntryOf::Call {
            method,
            args,
            caller,
        } => {
            let _ = entry.insert("op".into(), json!("componentCall"));
            let _ = entry.insert("method".into(), json!(method));
            let _ = entry.insert(
                "args".into(),
                serde_json::from_slice(args).unwrap_or(Value::Null),
            );
            let _ = entry.insert("caller".into(), caller_json(caller));
            let _ = entry.insert("idempotencyKey".into(), Value::Null);
            let _ = entry.insert("consent".into(), Value::Null);
        }
        EntryOf::Tick {
            timer,
            scheduled_ms,
            previous_ms,
        } => {
            let _ = entry.insert("op".into(), json!("componentTick"));
            let _ = entry.insert("timer".into(), json!(timer));
            let _ = entry.insert("scheduledMs".into(), json!(scheduled_ms));
            let _ = entry.insert("previousMs".into(), json!(previous_ms));
        }
        EntryOf::Init { bundle } => {
            let _ = entry.insert("op".into(), json!("componentInit"));
            let _ = entry.insert("bundle".into(), json!(bundle));
            let _ = entry.insert("computeBundle".into(), Value::Null);
        }
    }
    let _ = entry.insert("tsMs".into(), json!(ids.ts_ms));
    let _ = entry.insert("baseIndex".into(), json!(ids.base_index));
    let _ = entry.insert("idTerm".into(), json!(ids.id_term));
    let _ = entry.insert("idSeq".into(), json!(ids.id_seq));
    let _ = entry.insert("outcome".into(), outcome_json(out));
    serde_json::to_vec(&Value::Object(entry)).map_or(usize::MAX, |v| v.len())
}

fn caller_json(caller: &Caller) -> Value {
    match caller {
        Caller::Account(account) => json!({ "kind": "account", "account": account }),
        Caller::ApiKey { key_id, account } => {
            json!({ "kind": "apiKey", "key": key_id, "account": account })
        }
        Caller::Public => json!({ "kind": "public" }),
        Caller::System => json!({ "kind": "system" }),
        Caller::App(app) => json!({ "kind": "app", "app": app }),
        Caller::Visitor { link, key } => json!({ "kind": "visitor", "link": link, "key": key }),
    }
}

fn audience_json(audience: &Audience) -> Value {
    match audience {
        Audience::All => json!({ "kind": "all" }),
        Audience::Caller => json!({ "kind": "caller" }),
        Audience::Accounts(accounts) => json!({ "kind": "accounts", "accounts": accounts }),
        Audience::Team(team) => json!({ "kind": "team", "team": team }),
        Audience::OrgMembers(org) => json!({ "kind": "orgMembers", "org": org }),
        Audience::OrgAdmins(org) => json!({ "kind": "orgAdmins", "org": org }),
    }
}

fn trap_name(kind: TrapKind) -> &'static str {
    match kind {
        TrapKind::OutOfFuel => "OutOfFuel",
        TrapKind::Timeout => "Timeout",
        TrapKind::Cancelled => "Cancelled",
        TrapKind::Unavailable => "Unavailable",
        TrapKind::OutOfMemory => "OutOfMemory",
        TrapKind::StackOverflow => "StackOverflow",
        TrapKind::Fault => "Fault",
        TrapKind::HostDenied => "HostDenied",
    }
}

/// JSON of `bytes`, `null` when empty (the node's `args` of an xcall or action without any).
fn json_or_null(bytes: &[u8]) -> Value {
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(bytes).unwrap_or(Value::Null)
}

fn http_json(x: &HttpExchange) -> Value {
    let mut record = Map::new();
    let _ = record.insert("method".into(), json!(x.request.method));
    let _ = record.insert("url".into(), json!(x.request.url));
    let _ = record.insert("headers".into(), json!(x.request.headers));
    let _ = record.insert("body".into(), json!(STANDARD.encode(&x.request.body)));
    let _ = record.insert("bodyLen".into(), json!(x.request_body_len));
    let (headers, body) = match &x.response {
        Ok(r) => {
            let _ = record.insert("status".into(), json!(r.status));
            (json!(r.headers), STANDARD.encode(&r.body))
        }
        Err(e) => {
            let _ = record.insert("error".into(), json!(e));
            (json!([]), String::new())
        }
    };
    let _ = record.insert("responseHeaders".into(), headers);
    let _ = record.insert("responseBody".into(), json!(body));
    let _ = record.insert("responseBodyLen".into(), json!(x.response_body_len));
    Value::Object(record)
}

fn outcome_json(out: &Outcome) -> Value {
    let mut o = Map::new();
    let (status, error) = match &out.status {
        Status::Ok => ("ok", Value::Null),
        Status::AppError { code, message } => (
            "appError",
            json!({
                "code": code,
                "fields": serde_json::from_str::<Value>(message)
                    .unwrap_or_else(|_| Value::String(message.clone())),
            }),
        ),
        Status::Trap { kind, detail } => (
            "trap",
            json!({ "kind": trap_name(*kind), "detail": detail }),
        ),
    };
    let ok = out.status == Status::Ok;
    let _ = o.insert("status".into(), json!(status));
    let _ = o.insert(
        "result".into(),
        if ok {
            json_or_null(&out.result)
        } else {
            Value::Null
        },
    );
    let _ = o.insert("error".into(), error);
    let diff: Vec<Value> = if ok {
        out.diff
            .iter()
            .map(|w| {
                json!({
                    "key": STANDARD.encode(&w.key),
                    "value": w.value.as_ref().map(|v| STANDARD.encode(v)),
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let bytes_written: u64 = if ok {
        out.diff
            .iter()
            .map(|w| (w.key.len() + w.value.as_ref().map_or(0, Vec::len)) as u64)
            .sum()
    } else {
        0
    };
    let _ = o.insert("diff".into(), json!(diff));
    let events: Vec<Value> = if ok {
        out.events
            .iter()
            .map(|e| {
                json!({
                    "kind": e.name,
                    "data": json_or_null(&e.data),
                    "audience": audience_json(&e.audience),
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let n_events = events.len();
    let _ = o.insert("events".into(), json!(events));
    if ok && !out.xcalls.is_empty() {
        let xcalls: Vec<Value> = out
            .xcalls
            .iter()
            .map(|x| json!({ "app": x.app, "method": x.method, "args": json_or_null(&x.args) }))
            .collect();
        let _ = o.insert("xcalls".into(), json!(xcalls));
    }
    if ok && !out.actions.is_empty() {
        let actions: Vec<Value> = out
            .actions
            .iter()
            .map(|a| json!({ "id": a.id, "method": a.method, "args": json_or_null(&a.args) }))
            .collect();
        let _ = o.insert("actions".into(), json!(actions));
    }
    if !out.http.is_empty() {
        let http: Vec<Value> = out.http.iter().map(http_json).collect();
        let _ = o.insert("http".into(), json!(http));
    }
    let _ = o.insert(
        "usage".into(),
        json!({
            "fuel": out.usage.fuel,
            "wallMs": u64::try_from(out.usage.cpu.as_millis()).unwrap_or(u64::MAX),
            "writes": out.usage.writes,
            "bytesWritten": bytes_written,
            "events": n_events,
        }),
    );
    Value::Object(o)
}
