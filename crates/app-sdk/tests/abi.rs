//! The `app` macros end to end: the sample guest built into a component, its embedded ABI
//! read by the host, and its generated `handle` run on the wasmtime engine.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use calimero_app_build::{build_component, Built};
use calimero_app_engine_wasmtime::WasmtimeEngine;
use calimero_app_host::{
    Abi, Budget, Caller, Directory, Engine, HostCore, Invocation, InvocationKind, MethodKind,
    OrgRole, Outcome, Prepared, Status, TimerCtx,
};
use serde_json::{json, Value};

const ENTRY_TS_MS: u64 = 1_700_000_000_000;

fn built() -> &'static Built {
    static BUILT: OnceLock<Built> = OnceLock::new();
    BUILT.get_or_init(|| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/guests/sample");
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("guests");
        build_component(&dir, &target).unwrap_or_else(|e| panic!("build sample: {e}"))
    })
}

struct Dir;

impl Directory for Dir {
    fn org(&self) -> Option<String> {
        Some("acme".to_owned())
    }
    fn app_owner(&self) -> String {
        "owner".to_owned()
    }
    fn org_role(&self, account: &str) -> Option<OrgRole> {
        (account == "alice").then_some(OrgRole::Member)
    }
    fn in_team(&self, _account: &str, _team: &str) -> bool {
        false
    }
    fn team_exists(&self, _team: &str) -> bool {
        false
    }
    fn orgs_of(&self, _account: &str) -> Vec<(String, OrgRole)> {
        Vec::new()
    }
}

/// The sample app on a fresh engine, with state kept across invocations.
struct App {
    engine: WasmtimeEngine,
    prepared: Prepared,
    state: BTreeMap<Vec<u8>, Vec<u8>>,
    seq: u64,
    _cache: tempfile::TempDir,
}

impl App {
    fn new() -> Self {
        let cache = tempfile::tempdir().expect("tempdir");
        let engine = WasmtimeEngine::new(cache.path().to_path_buf()).expect("engine");
        let prepared = engine
            .prepare("sample-hash", &built().component)
            .unwrap_or_else(|e| panic!("prepare sample: {e}"));
        Self {
            engine,
            prepared,
            state: BTreeMap::new(),
            seq: 0,
            _cache: cache,
        }
    }

    fn run(
        &mut self,
        kind: InvocationKind,
        caller: Caller,
        method: &str,
        args: &[u8],
        timer: Option<TimerCtx>,
    ) -> Outcome {
        self.seq += 1;
        let inv = Invocation {
            kind,
            method: method.to_owned(),
            args: args.to_vec(),
            caller,
            entry_ts_ms: ENTRY_TS_MS,
            id_term: 1,
            id_seq: self.seq,
            seed: [7; 32],
            timer,
        };
        let host = HostCore::new(
            "app-1",
            Arc::new(self.state.clone()),
            self.seq,
            inv,
            Arc::new(Dir),
        )
        .expect("valid invocation");
        let out = self.engine.invoke(&self.prepared, host, Budget::call());
        if out.status == Status::Ok {
            for w in &out.diff {
                match &w.value {
                    Some(v) => self.state.insert(w.key.clone(), v.clone()),
                    None => self.state.remove(&w.key),
                };
            }
        }
        out
    }

    fn call(&mut self, method: &str, args: &[u8]) -> Outcome {
        let alice = Caller::Account("alice".to_owned());
        self.run(InvocationKind::Call, alice, method, args, None)
    }

    fn view(&mut self, method: &str, args: &[u8]) -> Outcome {
        let alice = Caller::Account("alice".to_owned());
        self.run(InvocationKind::View, alice, method, args, None)
    }
}

fn ok_json(out: &Outcome) -> Value {
    assert_eq!(out.status, Status::Ok, "{:?}", out.logs);
    serde_json::from_slice(&out.result).expect("result is JSON")
}

fn app_error(out: &Outcome) -> (String, String) {
    match &out.status {
        Status::AppError { code, message } => (code.clone(), message.clone()),
        other => panic!("expected an AppError, got {other:?} ({:?})", out.logs),
    }
}

fn args(v: Value) -> Vec<u8> {
    serde_json::to_vec(&v).unwrap()
}

#[test]
fn abi_lists_every_method_with_its_attributes() {
    let abi_json = built().abi.clone().expect("component carries an ABI");
    let abi = Abi::from_json(&abi_json).expect("host reads the ABI");

    let m = |name: &str| {
        abi.methods
            .get(name)
            .unwrap_or_else(|| panic!("{name} missing from {abi_json}"))
    };
    assert_eq!(m("book").kind, MethodKind::Call);
    assert_eq!(m("availability").kind, MethodKind::View);
    assert_eq!(m("init").kind, MethodKind::Init);
    assert!(m("adminRelease").confirm);
    assert_eq!(m("releaseNoShows").kind, MethodKind::Timer);
    assert_eq!(m("releaseNoShows").every, Some(Duration::from_secs(60)));
    assert!(m("releaseNoShows").allow_system);
    assert!(m("roomCount").public);
    assert!(!m("book").public && !m("book").confirm && !m("book").allow_system);
    assert!(
        !abi.methods.contains_key("slotTaken"),
        "private helpers stay out"
    );

    assert_eq!(abi_json["abiVersion"], 1);
    assert_eq!(abi_json["world"], "calimero:app@0.5.0");
    assert_eq!(abi_json["state"], json!({ "version": 3 }));
    assert_eq!(
        abi_json["methods"]["book"]["params"],
        json!([["roomId", "string"], ["start", "u64"], ["title", "string"]])
    );
    assert_eq!(abi_json["methods"]["book"]["returns"], "string");
    assert_eq!(abi_json["methods"]["adminRelease"]["returns"], Value::Null);
    assert_eq!(abi_json["methods"]["releaseNoShows"]["every"], "1m");
    assert_eq!(
        abi_json["errors"],
        json!({ "NotFound": {}, "Taken": { "start": "u64" }, "Invalid": { "0": "string" } })
    );
    assert_eq!(
        abi_json["events"],
        json!({
            "Booked": { "id": "string", "roomId": "string", "start": "u64" },
            "Released": { "id": "string" },
            "Stored": { "id": "string", "data": "bytes" },
        })
    );
}

#[test]
fn unknown_method_is_an_app_error() {
    let mut app = App::new();
    let out = app.call("noSuchMethod", b"{}");
    assert_eq!(app_error(&out).0, "UnknownMethod");
    // The Rust name is not the ABI name.
    let out = app.call("admin_release", b"{}");
    assert_eq!(app_error(&out).0, "UnknownMethod");
}

#[test]
fn malformed_args_is_an_app_error() {
    let mut app = App::new();
    assert_eq!(app_error(&app.call("book", b"{")).0, "BadArgs");
    assert_eq!(app_error(&app.call("book", b"[1]")).0, "BadArgs");
    let missing = args(json!({ "roomId": "r1", "title": "t" }));
    assert_eq!(app_error(&app.call("book", &missing)).0, "BadArgs");
    let snake = args(json!({ "room_id": "r1", "start": "1", "title": "t" }));
    assert_eq!(app_error(&app.call("book", &snake)).0, "BadArgs");
}

#[test]
fn u64_args_are_decimal_strings() {
    let mut app = App::new();
    let out = app.view("echoStart", br#"{"start": "18446744073709551615"}"#);
    assert_eq!(ok_json(&out), json!("18446744073709551615"));
    let out = app.view("echoStart", br#"{"start": 1}"#);
    assert_eq!(app_error(&out).0, "BadArgs");
    let out = app.view("echoStart", br#"{"start": "1", "label": "x"}"#);
    assert_eq!(ok_json(&out), json!("1"));
}

#[test]
fn declared_error_carries_fields() {
    let mut app = App::new();
    let book = args(json!({ "roomId": "r1", "start": "7", "title": "standup" }));
    let id = ok_json(&app.call("book", &book));
    assert!(id.is_string(), "{id}");
    let out = app.call("book", &book);
    assert_eq!(
        app_error(&out),
        ("Taken".to_owned(), r#"{"start":"7"}"#.to_owned())
    );
    let empty = args(json!({ "roomId": "r1", "start": "8", "title": "" }));
    assert_eq!(
        app_error(&app.call("book", &empty)),
        ("Invalid".to_owned(), r#"{"0":"empty title"}"#.to_owned())
    );
    let out = app.call("adminRelease", &args(json!({ "id": "nope" })));
    assert_eq!(app_error(&out), ("NotFound".to_owned(), "{}".to_owned()));
}

#[test]
fn a_call_method_is_not_a_view() {
    let mut app = App::new();
    let book = args(json!({ "roomId": "r1", "start": "7", "title": "t" }));
    assert_eq!(app_error(&app.view("book", &book)).0, "NotAView");
    // A view may be invoked as a call.
    let free = args(json!({ "roomId": "r1", "start": "7" }));
    assert_eq!(ok_json(&app.call("availability", &free)), json!(true));
}

#[test]
fn events_and_records_use_camel_case_and_decimal_u64() {
    let mut app = App::new();
    let book = args(json!({ "roomId": "r1", "start": "42", "title": "t" }));
    let out = app.call("book", &book);
    let id = ok_json(&out);
    assert_eq!(out.events.len(), 1);
    assert_eq!(out.events[0].name, "Booked");
    let data: Value = serde_json::from_slice(&out.events[0].data).unwrap();
    assert_eq!(data, json!({ "id": id, "roomId": "r1", "start": "42" }));

    let rec = ok_json(&app.view("booking", &args(json!({ "id": id }))));
    assert_eq!(
        rec,
        json!({ "id": id, "roomId": "r1", "start": "42", "title": "t", "checkedIn": false })
    );
    let count = app.view("roomCount", &args(json!({ "roomId": "r1" })));
    assert_eq!(ok_json(&count), json!("1"));
    let free = args(json!({ "roomId": "r1", "start": "42" }));
    assert_eq!(ok_json(&app.view("availability", &free)), json!(false));
}

#[test]
fn callers_follow_d13() {
    let mut app = App::new();
    // A method without `allow(system)` refuses System.
    let out = app.run(InvocationKind::Call, Caller::System, "bump", b"{}", None);
    assert_eq!(app_error(&out).0, "Forbidden");
    // Public callers reach only public methods.
    let out = app.run(
        InvocationKind::View,
        Caller::Public,
        "availability",
        &args(json!({ "roomId": "r", "start": "1" })),
        None,
    );
    assert_eq!(app_error(&out).0, "LoginRequired");
    let out = app.run(
        InvocationKind::View,
        Caller::Public,
        "roomCount",
        &args(json!({ "roomId": "r" })),
        None,
    );
    assert_eq!(ok_json(&out), json!("0"));
    // Another app is refused by default.
    let out = app.run(
        InvocationKind::Call,
        Caller::App("com.x".into()),
        "bump",
        b"{}",
        None,
    );
    assert_eq!(app_error(&out).0, "Forbidden");
    // init runs as the owner.
    let owner = Caller::Account("owner".into());
    let out = app.run(InvocationKind::Init, owner, "init", b"{}", None);
    assert_eq!(ok_json(&out), Value::Null);
    // A timer method is not callable as a call, and init only as init.
    assert_eq!(app_error(&app.call("releaseNoShows", b"{}")).0, "WrongKind");
    assert_eq!(app_error(&app.call("init", b"{}")).0, "WrongKind");
}

#[test]
fn timer_sees_its_tick() {
    let mut app = App::new();
    let book = args(json!({ "roomId": "r1", "start": "100", "title": "t" }));
    ok_json(&app.call("book", &book));
    let tick = TimerCtx {
        scheduled_ms: 500,
        previous_ms: Some(440),
    };
    let out = app.run(
        InvocationKind::Timer,
        Caller::System,
        "releaseNoShows",
        b"",
        Some(tick),
    );
    assert_eq!(ok_json(&out), json!("500"));
    let free = args(json!({ "roomId": "r1", "start": "100" }));
    assert_eq!(ok_json(&app.view("availability", &free)), json!(true));
    let count = app.view("roomCount", &args(json!({ "roomId": "r1" })));
    assert_eq!(ok_json(&count), json!("0"));
}

#[test]
fn unit_and_counter_results() {
    let mut app = App::new();
    assert_eq!(ok_json(&app.call("bump", b"")), json!("1"));
    assert_eq!(ok_json(&app.call("bump", b"{}")), json!("2"));
}

/// The raw `calimero:abi` text of the built component.
fn raw_abi() -> &'static str {
    let bytes = &built().component;
    let needle = br#"{"abiVersion""#;
    let start = bytes
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("ABI in the component");
    let mut stream =
        serde_json::Deserializer::from_slice(&bytes[start..]).into_iter::<serde::de::IgnoredAny>();
    let _ = stream.next().expect("one JSON value").expect("valid JSON");
    std::str::from_utf8(&bytes[start..start + stream.byte_offset()]).unwrap()
}

/// `v` with every object's keys in sorted order, compact.
fn sorted(v: &Value) -> String {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", Value::String(k.clone()), sorted(&map[k])))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(sorted).collect();
            format!("[{}]", parts.join(","))
        }
        other => other.to_string(),
    }
}

#[test]
fn abi_bytes_have_sorted_keys() {
    let raw = raw_abi();
    let parsed: Value = serde_json::from_str(raw).unwrap();
    assert_eq!(raw, sorted(&parsed));
}

#[test]
fn every_implies_allow_system() {
    let abi = Abi::from_json(built().abi.as_ref().unwrap()).unwrap();
    assert!(abi.methods["heartbeat"].allow_system);
    let mut app = App::new();
    let tick = TimerCtx {
        scheduled_ms: 30_000,
        previous_ms: None,
    };
    let out = app.run(
        InvocationKind::Timer,
        Caller::System,
        "heartbeat",
        b"",
        Some(tick),
    );
    assert_eq!(ok_json(&out), Value::Null);
}

#[test]
fn bytes_are_base64url_in_json_and_cbor_bytes_in_storage() {
    let abi = built().abi.clone().unwrap();
    assert_eq!(
        abi["methods"]["putBlob"]["params"],
        json!([["id", "string"], ["data", "bytes"]])
    );
    assert_eq!(
        abi["events"]["Stored"],
        json!({ "id": "string", "data": "bytes" })
    );

    let mut app = App::new();
    let out = app.view("echoBytes", br#"{"data": "AQL_", "extra": "AA"}"#);
    assert_eq!(ok_json(&out), json!("AQL_"));
    for bad in [
        &br#"{"data": "!!"}"#[..],
        br#"{"data": [1, 2]}"#,
        br#"{"data": "AQL_", "extra": "A"}"#,
    ] {
        assert_eq!(app_error(&app.view("echoBytes", bad)).0, "BadArgs");
    }

    let out = app.call("putBlob", br#"{"id": "b1", "data": "AQL_"}"#);
    assert_eq!(ok_json(&out), Value::Null);
    let data: Value = serde_json::from_slice(&out.events[0].data).unwrap();
    assert_eq!(data, json!({ "id": "b1", "data": "AQL_" }));
    // The record: {"id": "b1", "data": h'0102ff'}, keys in encoded-byte order.
    let cbor: &[u8] = &[
        0xa2, 0x62, b'i', b'd', 0x62, b'b', b'1', 0x64, b'd', b'a', b't', b'a', 0x43, 0x01, 0x02,
        0xff,
    ];
    assert!(
        out.diff.iter().any(|w| w.value.as_deref() == Some(cbor)),
        "{:?}",
        out.diff
    );
    let rec = ok_json(&app.view("blob", br#"{"id": "b1"}"#));
    assert_eq!(rec, json!({ "id": "b1", "data": "AQL_" }));
}

#[test]
fn wide_integers_refuse_leading_zeros_and_plus() {
    let mut app = App::new();
    for bad in ["01", "+1", "-0", " 1", ""] {
        let args = args(json!({ "start": bad }));
        assert_eq!(
            app_error(&app.view("echoStart", &args)).0,
            "BadArgs",
            "{bad:?}"
        );
    }
    let out = app.view("echoStart", br#"{"start": "0"}"#);
    assert_eq!(ok_json(&out), json!("0"));
}
