//! The host core's policy, overlay and determinism rules (spec sections 3-5, D19, D46, D47).

use std::collections::BTreeMap;
use std::time::Duration;
use std::time::Instant;

use calimero_app_host::{
    app_key, Abi, Audience, Caller, Directory, Event, HostCore, HostDenied, Invocation,
    InvocationKind, MethodKind, OrgRole, Status, Usage, GENERATION, MAX_WRITES,
};
use serde_json::json;

struct NoDirectory;

impl Directory for NoDirectory {
    fn org(&self) -> Option<String> {
        None
    }
    fn app_owner(&self) -> String {
        "owner".to_owned()
    }
    fn org_role(&self, _account: &str) -> Option<OrgRole> {
        None
    }
    fn in_team(&self, _account: &str, _team: &str) -> bool {
        false
    }
    fn orgs_of(&self, _account: &str) -> Vec<(String, OrgRole)> {
        Vec::new()
    }
}

fn inv(kind: InvocationKind) -> Invocation {
    Invocation {
        kind,
        method: "m".to_owned(),
        args: Vec::new(),
        caller: Caller::Account("ada".to_owned()),
        entry_ts_ms: 1_000,
        entry_index: 42,
        seed: [7; 32],
        timer: None,
    }
}

fn base(entries: &[(&[u8], &[u8])]) -> BTreeMap<Vec<u8>, Vec<u8>> {
    entries
        .iter()
        .map(|(k, v)| (app_key(GENERATION, k), v.to_vec()))
        .collect()
}

fn cbor(n: i64) -> Vec<u8> {
    let mut out = Vec::new();
    ciborium::into_writer(&n, &mut out).unwrap();
    out
}

fn event() -> Event {
    Event {
        name: "e".to_owned(),
        data: Vec::new(),
        audience: Audience::All,
    }
}

#[test]
fn view_writes_are_denied() {
    let state = base(&[]);
    let inv = inv(InvocationKind::View);
    let mut host = HostCore::new("app", &state, 0, &inv, &NoDirectory);
    assert!(matches!(host.storage_put(b"k", b"v"), Err(HostDenied(_))));
    assert!(matches!(host.storage_delete(b"k"), Err(HostDenied(_))));
    assert!(matches!(host.storage_add(b"k", 1), Err(HostDenied(_))));
    assert!(matches!(host.emit(event()), Err(HostDenied(_))));
    assert!(matches!(host.start_action("a", b""), Err(HostDenied(_))));
    assert_eq!(host.storage_get(b"k"), Ok(None));
}

#[test]
fn eleven_thousand_writes_trap_after_ten_thousand() {
    let state = base(&[]);
    let inv = inv(InvocationKind::Call);
    let mut host = HostCore::new("app", &state, 0, &inv, &NoDirectory);
    for i in 0..MAX_WRITES {
        host.storage_put(&i.to_be_bytes(), b"v").unwrap();
    }
    assert_eq!(MAX_WRITES, 10_000);
    assert!(matches!(
        host.storage_put(b"one-more", b"v"),
        Err(HostDenied(_))
    ));
    assert!(matches!(
        host.storage_delete(b"one-more"),
        Err(HostDenied(_))
    ));
    assert!(matches!(
        host.storage_add(b"one-more", 1),
        Err(HostDenied(_))
    ));
}

#[test]
fn failed_status_drops_diff_and_events() {
    let state = base(&[]);
    let inv = inv(InvocationKind::Call);
    let mut host = HostCore::new("app", &state, 0, &inv, &NoDirectory);
    host.storage_put(b"k", b"v").unwrap();
    host.emit(event()).unwrap();
    host.log("kept".to_owned());
    let out = host.finish(
        Status::AppError {
            code: "Nope".to_owned(),
            message: String::new(),
        },
        b"\"err\"".to_vec(),
        Usage::default(),
    );
    assert!(out.diff.is_empty());
    assert!(out.events.is_empty());
    assert_eq!(out.result, b"\"err\"");
    assert_eq!(out.logs, vec!["kept".to_owned()]);

    let mut host = HostCore::new("app", &state, 0, &inv, &NoDirectory);
    host.storage_put(b"k", b"v").unwrap();
    host.emit(event()).unwrap();
    let out = host.finish(Status::Ok, Vec::new(), Usage::default());
    assert_eq!(out.diff.len(), 1);
    assert_eq!(out.events.len(), 1);
}

#[test]
fn range_sees_overlay_writes_and_deletes() {
    let state = base(&[(b"a", b"1"), (b"b", b"2"), (b"c", b"3")]);
    let inv = inv(InvocationKind::Call);
    let mut host = HostCore::new("app", &state, 0, &inv, &NoDirectory);
    host.storage_put(b"bb", b"9").unwrap();
    host.storage_delete(b"c").unwrap();

    let cursor = host.range_open(b"a", b"d", false).unwrap();
    let first = host.range_next(cursor, 2).unwrap();
    assert_eq!(
        first,
        vec![
            (b"a".to_vec(), b"1".to_vec()),
            (b"b".to_vec(), b"2".to_vec())
        ]
    );
    let second = host.range_next(cursor, 2).unwrap();
    assert_eq!(second, vec![(b"bb".to_vec(), b"9".to_vec())]);
    assert!(host.range_next(cursor, 2).unwrap().is_empty());

    let back = host.range_open(b"", b"", true).unwrap();
    let keys: Vec<_> = host
        .range_next(back, 10)
        .unwrap()
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(keys, vec![b"bb".to_vec(), b"b".to_vec(), b"a".to_vec()]);
}

#[test]
fn range_sees_writes_made_between_batches() {
    let state = base(&[(b"a", b"1"), (b"c", b"3"), (b"e", b"5")]);
    let inv = inv(InvocationKind::Call);
    let mut host = HostCore::new("app", &state, 0, &inv, &NoDirectory);
    let cursor = host.range_open(b"", b"", false).unwrap();
    assert_eq!(host.range_next(cursor, 1).unwrap().len(), 1);
    host.storage_put(b"b", b"2").unwrap();
    host.storage_delete(b"c").unwrap();
    let keys: Vec<_> = host
        .range_next(cursor, 10)
        .unwrap()
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(keys, vec![b"b".to_vec(), b"e".to_vec()]);
}

#[test]
fn range_is_lazy() {
    let state: BTreeMap<Vec<u8>, Vec<u8>> = (0u32..100_000)
        .map(|i| (app_key(GENERATION, &i.to_be_bytes()), b"v".to_vec()))
        .collect();
    let inv = inv(InvocationKind::Call);
    let mut host = HostCore::new("app", &state, 0, &inv, &NoDirectory);
    let cursor = host.range_open(b"", b"", false).unwrap();
    assert_eq!(host.range_next(cursor, 10).unwrap().len(), 10);
    assert!(host.peak_buffered() <= 10, "{}", host.peak_buffered());
    let big = host.range_next(cursor, 5_000).unwrap();
    assert_eq!(big.len(), 1_000, "max is capped at 1,000");
}

#[test]
fn add_counts_as_one_write_and_sums() {
    let state = base(&[(b"k", &cbor(5)), (b"s", b"\x61x")]);
    let inv = inv(InvocationKind::Call);
    let mut host = HostCore::new("app", &state, 0, &inv, &NoDirectory);
    host.storage_add(b"k", 3).unwrap();
    host.storage_add(b"k", 3).unwrap();
    host.storage_add(b"fresh", -2).unwrap();
    assert!(matches!(host.storage_add(b"s", 1), Err(HostDenied(_))));
    let out = host.finish(Status::Ok, Vec::new(), Usage::default());
    assert_eq!(out.usage.writes, 3);
    let diff: BTreeMap<_, _> = out.diff.into_iter().map(|w| (w.key, w.value)).collect();
    assert_eq!(diff[&app_key(GENERATION, b"k")], Some(cbor(11)));
    assert_eq!(diff[&app_key(GENERATION, b"fresh")], Some(cbor(-2)));
}

#[test]
fn add_refuses_a_non_shortest_integer() {
    // 5 encoded with a one-byte argument (0x18 0x05) is valid CBOR but not deterministic.
    let state = base(&[(b"k", b"\x18\x05")]);
    let inv = inv(InvocationKind::Call);
    let mut host = HostCore::new("app", &state, 0, &inv, &NoDirectory);
    assert!(matches!(host.storage_add(b"k", 1), Err(HostDenied(_))));
}

#[test]
fn unique_ids_are_index_and_position() {
    let state = base(&[]);
    let call = inv(InvocationKind::Call);
    let mut host = HostCore::new("app", &state, 0, &call, &NoDirectory);
    assert_eq!(host.unique_id().unwrap(), "42.0");
    assert_eq!(host.unique_id().unwrap(), "42.1");
    assert_eq!(host.unique_id().unwrap(), "42.2");

    for kind in [
        InvocationKind::View,
        InvocationKind::Action,
        InvocationKind::Migrate,
    ] {
        let inv = inv(kind);
        let mut host = HostCore::new("app", &state, 0, &inv, &NoDirectory);
        assert!(matches!(host.unique_id(), Err(HostDenied(_))), "{kind:?}");
    }
}

#[test]
fn app_keys_carry_the_generation() {
    let mut state = base(&[]);
    state.insert(app_key(1, b"old"), b"x".to_vec());
    let inv = inv(InvocationKind::Call);
    let mut host = HostCore::new("app", &state, 0, &inv, &NoDirectory);
    assert_eq!(host.storage_get(b"old"), Ok(None));
    let all = host.range_open(b"", b"", false).unwrap();
    assert!(host.range_next(all, 10).unwrap().is_empty());
    host.storage_put(b"k", b"v").unwrap();
    let out = host.finish(Status::Ok, Vec::new(), Usage::default());
    let mut expected = 0u64.to_be_bytes().to_vec();
    expected.extend_from_slice(b"k");
    assert_eq!(out.diff[0].key, expected);
    assert_eq!(out.diff[0].value, Some(b"v".to_vec()));
}

#[test]
fn same_seed_same_randomness() {
    let state = base(&[]);
    let inv = inv(InvocationKind::Call);
    let mut one = HostCore::new("app", &state, 0, &inv, &NoDirectory);
    let mut two = HostCore::new("app", &state, 0, &inv, &NoDirectory);
    let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
    one.random_fill(&mut a);
    two.random_fill(&mut b);
    assert_eq!(a, b);
    assert_ne!(a, [0u8; 32]);

    let view = self::inv(InvocationKind::View);
    let mut v1 = HostCore::new("app", &state, 0, &view, &NoDirectory);
    let mut v2 = HostCore::new("app", &state, 0, &view, &NoDirectory);
    v1.random_fill(&mut a);
    v2.random_fill(&mut b);
    assert_ne!(a, b, "views draw from the node's CSPRNG, not the seed");
}

#[test]
fn clocks_are_the_entry_timestamp() {
    let state = base(&[]);
    let inv = inv(InvocationKind::Call);
    let host = HostCore::new("app", &state, 0, &inv, &NoDirectory);
    assert_eq!(host.wall_ms(), 1_000);
    assert_eq!(host.monotonic_ns(), 1_000_000_000);
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(20) {}
    assert_eq!(host.wall_ms(), 1_000);
    assert_eq!(host.monotonic_ns(), 1_000_000_000);
}

#[test]
fn disabled_interfaces_refuse() {
    let state = base(&[]);
    let call = inv(InvocationKind::Call);
    let mut host = HostCore::new("app", &state, 0, &call, &NoDirectory);
    assert_eq!(
        host.start_action("enrich", b"{}"),
        Ok(Err("actions are not enabled".to_owned()))
    );
    assert!(matches!(
        host.xcall_send("other", "m", b""),
        Err(HostDenied(_))
    ));
    assert!(matches!(host.blobs_info("b"), Err(HostDenied(_))));
    assert!(matches!(host.blobs_grant("b", 60), Err(HostDenied(_))));
    assert!(matches!(host.action_key(), Err(HostDenied(_))));
    assert!(matches!(host.action_view("m", b""), Err(HostDenied(_))));
    assert!(matches!(host.action_call("m", b""), Err(HostDenied(_))));
    assert!(matches!(host.rebuild_get(b"k"), Err(HostDenied(_))));
    assert!(matches!(host.rebuild_put(b"k", b"v"), Err(HostDenied(_))));

    let action = inv(InvocationKind::Action);
    let mut host = HostCore::new("app", &state, 0, &action, &NoDirectory);
    assert!(matches!(host.storage_get(b"k"), Err(HostDenied(_))));
    assert!(matches!(
        host.range_open(b"", b"", false),
        Err(HostDenied(_))
    ));

    let migrate = inv(InvocationKind::Migrate);
    let mut host = HostCore::new("app", &state, 0, &migrate, &NoDirectory);
    assert_eq!(host.storage_get(b"k"), Ok(None));
    assert!(matches!(host.storage_put(b"k", b"v"), Err(HostDenied(_))));
    assert!(matches!(host.org_role_strict("ada"), Err(HostDenied(_))));
}

#[test]
fn context_comes_from_the_invocation_and_directory() {
    let state = base(&[]);
    let inv = inv(InvocationKind::Call);
    let host = HostCore::new("app-1", &state, 9, &inv, &NoDirectory);
    assert_eq!(host.app_id(), "app-1");
    assert_eq!(host.app_owner(), "owner");
    assert_eq!(host.applied_index(), 9);
    assert_eq!(host.caller(), &Caller::Account("ada".to_owned()));
    assert_eq!(host.invocation().method, "m");
}

#[test]
fn abi_reads_method_attributes() {
    let abi = Abi::from_json(&json!({
        "world": "calimero:app@0.5.0",
        "methods": {
            "init": { "kind": "init", "params": [], "returns": null },
            "book": { "kind": "call", "params": [["roomId", "string"]], "returns": "string" },
            "availability": { "kind": "view" },
            "adminRelease": { "kind": "call", "confirm": true },
            "releaseNoShows": { "kind": "timer", "every": "1m", "allowSystem": true }
        }
    }))
    .unwrap();
    assert_eq!(abi.methods["init"].kind, MethodKind::Init);
    assert_eq!(abi.methods["book"].kind, MethodKind::Call);
    assert_eq!(abi.methods["availability"].kind, MethodKind::View);
    assert!(abi.methods["adminRelease"].confirm);
    let tick = &abi.methods["releaseNoShows"];
    assert_eq!(tick.every, Some(Duration::from_secs(60)));
    assert!(tick.allow_system);

    assert!(
        Abi::from_json(&json!({ "methods": { "x": { "kind": "call", "bogus": 1 } } })).is_err()
    );
    assert!(Abi::from_json(&json!({ "methods": { "x": { "kind": "timer" } } })).is_err());
    assert!(
        Abi::from_json(&json!({ "methods": { "x": { "kind": "call", "every": "1m" } } })).is_err()
    );
    assert!(Abi::from_json(&json!({})).is_err());
}
