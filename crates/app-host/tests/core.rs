//! The host core's policy, overlay and determinism rules (spec sections 3-5, D19, D46, D47).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use calimero_app_host::{
    app_key, Abi, Audience, Caller, Directory, Event, HostCore, HostDenied, InvalidInvocation,
    Invocation, InvocationKind, KeyValue, MethodKind, OrgRole, PrepareError, Status, TimerCtx,
    Usage, EMIT_BAD_AUDIENCE, EMIT_DATA_TOO_LARGE, EMIT_FOREIGN_ORG, EMIT_NAME_TOO_LONG,
    EMIT_TOO_MANY, EMIT_UNKNOWN_TEAM, GENERATION, MAX_EVENTS, MAX_EVENT_DATA_BYTES,
    MAX_EVENT_NAME_BYTES, MAX_KEY_BYTES, MAX_WRITES, SUPPORTED_ABI_VERSIONS,
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
    fn team_exists(&self, _team: &str) -> bool {
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
        id_term: 3,
        id_seq: 42,
        seed: [7; 32],
        timer: None,
    }
}

fn core_for(state: &BTreeMap<Vec<u8>, Vec<u8>>, inv: &Invocation) -> HostCore {
    HostCore::new(
        "app",
        Arc::new(state.clone()),
        0,
        inv.clone(),
        Arc::new(NoDirectory),
    )
    .unwrap()
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
    let mut host = core_for(&state, &inv);
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
    let mut host = core_for(&state, &inv);
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
    let mut host = core_for(&state, &inv);
    host.storage_put(b"k", b"v").unwrap();
    host.emit(event()).unwrap().unwrap();
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

    let mut host = core_for(&state, &inv);
    host.storage_put(b"k", b"v").unwrap();
    host.emit(event()).unwrap().unwrap();
    let out = host.finish(Status::Ok, Vec::new(), Usage::default());
    assert_eq!(out.diff.len(), 1);
    assert_eq!(out.events.len(), 1);
}

#[test]
fn range_sees_overlay_writes_and_deletes() {
    let state = base(&[(b"a", b"1"), (b"b", b"2"), (b"c", b"3")]);
    let inv = inv(InvocationKind::Call);
    let mut host = core_for(&state, &inv);
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
fn range_ignores_writes_made_between_batches() {
    let state = base(&[(b"a", b"1"), (b"c", b"3"), (b"e", b"5")]);
    let inv = inv(InvocationKind::Call);
    let mut host = core_for(&state, &inv);
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
    // The cursor reads the state as of its opening.
    assert_eq!(keys, vec![b"c".to_vec(), b"e".to_vec()]);
    assert_eq!(host.storage_get(b"c"), Ok(None));
}

#[test]
fn range_is_lazy() {
    let state: BTreeMap<Vec<u8>, Vec<u8>> = (0u32..100_000)
        .map(|i| (app_key(GENERATION, &i.to_be_bytes()), b"v".to_vec()))
        .collect();
    let inv = inv(InvocationKind::Call);
    let mut host = core_for(&state, &inv);
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
    let mut host = core_for(&state, &inv);
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
    let mut host = core_for(&state, &inv);
    assert!(matches!(host.storage_add(b"k", 1), Err(HostDenied(_))));
}

#[test]
fn unique_ids_are_term_seq_and_position() {
    let state = base(&[]);
    let call = inv(InvocationKind::Call);
    let mut host = core_for(&state, &call);
    assert_eq!(host.unique_id().unwrap(), "3.42.0");
    assert_eq!(host.unique_id().unwrap(), "3.42.1");
    assert_eq!(host.unique_id().unwrap(), "3.42.2");

    for kind in [
        InvocationKind::View,
        InvocationKind::Action,
        InvocationKind::Migrate,
    ] {
        let inv = inv(kind);
        let mut host = core_for(&state, &inv);
        assert!(matches!(host.unique_id(), Err(HostDenied(_))), "{kind:?}");
    }
}

#[test]
fn app_keys_carry_the_generation() {
    let mut state = base(&[]);
    state.insert(app_key(1, b"old"), b"x".to_vec());
    let inv = inv(InvocationKind::Call);
    let mut host = core_for(&state, &inv);
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
    let mut one = core_for(&state, &inv);
    let mut two = core_for(&state, &inv);
    let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
    one.random_fill(&mut a);
    two.random_fill(&mut b);
    assert_eq!(a, b);
    assert_ne!(a, [0u8; 32]);

    let view = self::inv(InvocationKind::View);
    let mut v1 = core_for(&state, &view);
    let mut v2 = core_for(&state, &view);
    v1.random_fill(&mut a);
    v2.random_fill(&mut b);
    assert_ne!(a, b, "views draw from the node's CSPRNG, not the seed");
}

#[test]
fn clocks_are_the_entry_timestamp() {
    let state = base(&[]);
    let inv = inv(InvocationKind::Call);
    let host = core_for(&state, &inv);
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
    let mut host = core_for(&state, &call);
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
    let mut host = core_for(&state, &action);
    assert!(matches!(host.storage_get(b"k"), Err(HostDenied(_))));
    assert!(matches!(
        host.range_open(b"", b"", false),
        Err(HostDenied(_))
    ));

    let migrate = inv(InvocationKind::Migrate);
    let mut host = core_for(&state, &migrate);
    assert_eq!(host.storage_get(b"k"), Ok(None));
    assert!(matches!(host.storage_put(b"k", b"v"), Err(HostDenied(_))));
    assert!(matches!(host.org_role_strict("ada"), Err(HostDenied(_))));
}

#[test]
fn context_comes_from_the_invocation_and_directory() {
    let state = base(&[]);
    let inv = inv(InvocationKind::Call);
    let host = HostCore::new(
        "app-1",
        Arc::new(state.clone()),
        9,
        inv.clone(),
        Arc::new(NoDirectory),
    )
    .unwrap();
    assert_eq!(host.app_id(), "app-1");
    assert_eq!(host.app_owner(), "owner");
    assert_eq!(host.applied_index(), 9);
    assert_eq!(host.caller(), &Caller::Account("ada".to_owned()));
    assert_eq!(host.invocation().method, "m");
}

#[test]
fn abi_reads_method_attributes() {
    let abi = Abi::from_json(&json!({
        "abiVersion": 1,
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
        Abi::from_json(&json!({ "abiVersion": 1, "methods": { "x": { "kind": "timer" } } }))
            .is_err()
    );
    assert!(Abi::from_json(
        &json!({ "abiVersion": 1, "methods": { "x": { "kind": "call", "every": "1m" } } })
    )
    .is_err());
    assert!(Abi::from_json(&json!({ "abiVersion": 1 })).is_err());
}

#[test]
fn abi_ignores_unknown_keys_and_refuses_bad_known_ones() {
    let abi = Abi::from_json(&json!({
        "abiVersion": 1,
        "futureTopLevel": { "anything": true },
        "methods": { "x": { "kind": "call", "futureAttribute": 7 } }
    }))
    .unwrap();
    assert_eq!(abi.methods["x"].kind, MethodKind::Call);

    for bad in [
        json!({ "abiVersion": 1, "methods": { "x": { "kind": "call", "confirm": "yes" } } }),
        json!({ "abiVersion": 1, "methods": { "x": { "kind": 3 } } }),
        json!({ "abiVersion": 1, "methods": [] }),
        json!({ "abiVersion": "1", "methods": {} }),
        json!({ "abiVersion": 0, "methods": {} }),
        json!({ "abiVersion": 1, "methods": { "t": { "kind": "timer", "every": 5 } } }),
        json!({ "abiVersion": 1, "methods": { "x": { "kind": "action" } } }),
    ] {
        assert!(
            matches!(Abi::from_json(&bad), Err(PrepareError::BadAbi(_))),
            "{bad}"
        );
    }
}

#[test]
fn abi_version_is_required_and_checked() {
    assert_eq!(SUPPORTED_ABI_VERSIONS, &[1]);
    assert!(Abi::from_json(&json!({ "abiVersion": 1, "methods": {} })).is_ok());
    assert!(matches!(
        Abi::from_json(&json!({ "abiVersion": 2, "methods": {} })),
        Err(PrepareError::UnsupportedAbiVersion(2))
    ));
    assert!(matches!(
        Abi::from_json(&json!({ "methods": {} })),
        Err(PrepareError::MissingAbiVersion)
    ));
}

fn keys(batch: Vec<KeyValue>) -> Vec<Vec<u8>> {
    batch.into_iter().map(|(k, _)| k).collect()
}

fn assert_send_static<T: Send + 'static>() {}

#[test]
fn host_core_is_send_and_static() {
    // wasmtime 50 needs `Store<T: 'static>` and `Send` store data for wasi and async calls.
    assert_send_static::<HostCore>();
}

#[test]
fn timer_slot_must_match_the_kind() {
    let state = Arc::new(base(&[]));
    let dir = Arc::new(NoDirectory);
    let tick = inv(InvocationKind::Timer);
    assert_eq!(
        HostCore::new("app", state.clone(), 0, tick, dir.clone()).err(),
        Some(InvalidInvocation::TimerWithoutSlot)
    );
    let mut call = inv(InvocationKind::Call);
    call.timer = Some(TimerCtx {
        scheduled_ms: 60_000,
        previous_ms: None,
    });
    assert_eq!(
        HostCore::new("app", state.clone(), 0, call, dir.clone()).err(),
        Some(InvalidInvocation::SlotOutsideTimer)
    );
    let mut tick = inv(InvocationKind::Timer);
    tick.timer = Some(TimerCtx {
        scheduled_ms: 60_000,
        previous_ms: Some(0),
    });
    let mut host = HostCore::new("app", state, 0, tick, dir).unwrap();
    assert_eq!(host.emit(event()), Ok(Ok(())));
}

#[test]
fn reverse_range_ignores_writes_between_batches() {
    let state = base(&[(b"a", b"a"), (b"c", b"c"), (b"e", b"e"), (b"g", b"g")]);
    let inv = inv(InvocationKind::Call);
    let mut host = core_for(&state, &inv);
    let cursor = host.range_open(b"", b"", true).unwrap();
    assert_eq!(
        keys(host.range_next(cursor, 1).unwrap()),
        vec![b"g".to_vec()]
    );
    host.storage_delete(b"e").unwrap();
    host.storage_put(b"f", b"F").unwrap();
    host.storage_put(b"d", b"D").unwrap();
    host.storage_put(b"h", b"H").unwrap();
    assert_eq!(
        keys(host.range_next(cursor, 2).unwrap()),
        vec![b"e".to_vec(), b"c".to_vec()]
    );
    host.storage_put(b"a", b"A2").unwrap();
    assert_eq!(
        host.range_next(cursor, 10).unwrap(),
        vec![(b"a".to_vec(), b"a".to_vec())]
    );
}

#[test]
fn forward_range_keeps_a_key_deleted_after_opening() {
    let state = base(&[(b"a", b"a"), (b"b", b"b"), (b"c", b"c")]);
    let inv = inv(InvocationKind::Call);
    let mut host = core_for(&state, &inv);
    let cursor = host.range_open(b"", b"", false).unwrap();
    assert_eq!(
        keys(host.range_next(cursor, 1).unwrap()),
        vec![b"a".to_vec()]
    );
    host.storage_delete(b"b").unwrap();
    assert_eq!(
        keys(host.range_next(cursor, 1).unwrap()),
        vec![b"b".to_vec()]
    );
    assert_eq!(
        keys(host.range_next(cursor, 1).unwrap()),
        vec![b"c".to_vec()]
    );
    assert!(host.range_next(cursor, 1).unwrap().is_empty());
}

#[test]
fn tombstones_empty_ranges_and_max_zero() {
    let state = base(&[(b"a", b"a"), (b"b", b"b"), (b"c", b"c"), (b"d", b"d")]);
    let inv = inv(InvocationKind::Call);
    let mut host = core_for(&state, &inv);
    for key in [b"a", b"b", b"c"] {
        host.storage_delete(key).unwrap();
    }
    let cursor = host.range_open(b"", b"", false).unwrap();
    assert!(host.range_next(cursor, 0).unwrap().is_empty());
    assert_eq!(
        keys(host.range_next(cursor, 1).unwrap()),
        vec![b"d".to_vec()]
    );
    let inverted = host.range_open(b"z", b"a", false).unwrap();
    assert!(host.range_next(inverted, 10).unwrap().is_empty());
    let empty = host.range_open(b"a", b"a", true).unwrap();
    assert!(host.range_next(empty, 10).unwrap().is_empty());
}

#[test]
fn range_stops_at_the_next_generation() {
    let mut state = base(&[(b"\xff\xff\xff\xff\xff\xff\xff\xff\xff", b"v")]);
    state.insert(app_key(1, b""), b"g1".to_vec());
    let inv = inv(InvocationKind::Call);
    let mut host = core_for(&state, &inv);
    let forward = host.range_open(b"", b"", false).unwrap();
    assert_eq!(host.range_next(forward, 10).unwrap().len(), 1);
    let reverse = host.range_open(b"", b"", true).unwrap();
    assert_eq!(host.range_next(reverse, 10).unwrap().len(), 1);
}

/// The RFC 8949 shortest encoding of `n`, written out by hand as the reference.
fn shortest(n: i128) -> Vec<u8> {
    let (major, magnitude) = if n >= 0 {
        (0u8, u64::try_from(n).unwrap())
    } else {
        (1u8, u64::try_from(-1 - n).unwrap())
    };
    let m = major << 5;
    let mut out = Vec::new();
    if magnitude < 24 {
        out.push(m | u8::try_from(magnitude).unwrap());
    } else if let Ok(b) = u8::try_from(magnitude) {
        out.extend([m | 24, b]);
    } else if let Ok(h) = u16::try_from(magnitude) {
        out.push(m | 25);
        out.extend(h.to_be_bytes());
    } else if let Ok(w) = u32::try_from(magnitude) {
        out.push(m | 26);
        out.extend(w.to_be_bytes());
    } else {
        out.push(m | 27);
        out.extend(magnitude.to_be_bytes());
    }
    out
}

fn added(start: Option<Vec<u8>>, delta: i64) -> Result<Vec<u8>, HostDenied> {
    let state = match start {
        Some(value) => base(&[(b"k", &value)]),
        None => base(&[]),
    };
    let inv = inv(InvocationKind::Call);
    let mut host = core_for(&state, &inv);
    host.storage_add(b"k", delta)?;
    let out = host.finish(Status::Ok, Vec::new(), Usage::default());
    Ok(out.diff[0].value.clone().unwrap())
}

#[test]
fn add_writes_the_shortest_encoding() {
    for delta in [
        0,
        23,
        24,
        255,
        256,
        65_535,
        65_536,
        i64::MAX,
        -1,
        -24,
        -25,
        -256,
        -257,
        i64::MIN,
    ] {
        assert_eq!(
            added(None, delta).unwrap(),
            shortest(i128::from(delta)),
            "{delta}"
        );
    }
    let near_max = shortest(i128::from(u64::MAX) - 5);
    assert_eq!(
        added(Some(near_max), 5).unwrap(),
        shortest(i128::from(u64::MAX))
    );
}

#[test]
fn add_refuses_overflow_and_non_integers() {
    assert!(
        added(Some(shortest(i128::from(u64::MAX))), 1).is_err(),
        "past u64::MAX"
    );
    let most_negative = -1 - i128::from(u64::MAX);
    assert!(
        added(Some(shortest(most_negative)), -1).is_err(),
        "past -2^64"
    );
    assert!(
        added(Some(vec![0xc2, 0x41, 0x05]), 1).is_err(),
        "bignum tag"
    );
    assert!(added(Some(vec![0x05, 0x00]), 1).is_err(), "trailing bytes");
}

/// The index key of record `id` at position `pos`: `"i" ‖ pos (u32 BE)`; the value is `id`.
fn index_key(pos: u32) -> Vec<u8> {
    let mut key = b"i".to_vec();
    key.extend(pos.to_be_bytes());
    key
}

fn position(key: &[u8]) -> u32 {
    u32::from_be_bytes(key[1..5].try_into().unwrap())
}

/// Scans the index in batches of 7, moving every yielded record by `shift` positions (an
/// update that moves its index key) and returns how often each record id was yielded.
fn scan_and_move(host: &mut HostCore, reverse: bool, shift: i64) -> BTreeMap<Vec<u8>, u32> {
    let cursor = host.range_open(b"i", b"j", reverse).unwrap();
    let mut seen = BTreeMap::new();
    for _ in 0..1_000 {
        let batch = host.range_next(cursor, 7).unwrap();
        if batch.is_empty() {
            return seen;
        }
        for (key, id) in batch {
            *seen.entry(id.clone()).or_insert(0) += 1;
            let moved = u32::try_from(i64::from(position(&key)) + shift).unwrap();
            host.storage_delete(&key).unwrap();
            host.storage_put(&index_key(moved), &id).unwrap();
        }
    }
    panic!("the scan did not end");
}

fn index(positions: std::ops::Range<u32>) -> BTreeMap<Vec<u8>, Vec<u8>> {
    positions
        .map(|pos| {
            (
                app_key(GENERATION, &index_key(pos)),
                pos.to_be_bytes().to_vec(),
            )
        })
        .collect()
}

#[test]
fn scan_does_not_see_its_own_forward_moves() {
    let state = index(0..50);
    let inv = inv(InvocationKind::Call);
    let mut host = core_for(&state, &inv);
    let seen = scan_and_move(&mut host, false, 1_000);
    assert_eq!(seen.len(), 50);
    assert!(seen.values().all(|&n| n == 1), "{seen:?}");
}

#[test]
fn reverse_scan_does_not_see_its_own_backward_moves() {
    let state = index(1_000..1_050);
    let inv = inv(InvocationKind::Call);
    let mut host = core_for(&state, &inv);
    let seen = scan_and_move(&mut host, true, -1_000);
    assert_eq!(seen.len(), 50);
    assert!(seen.values().all(|&n| n == 1), "{seen:?}");
}

#[test]
fn reads_after_a_scan_see_its_writes() {
    let state = index(0..50);
    let inv = inv(InvocationKind::Call);
    let mut host = core_for(&state, &inv);
    let _ = scan_and_move(&mut host, false, 1_000);
    assert_eq!(host.storage_get(&index_key(0)), Ok(None));
    assert_eq!(
        host.storage_get(&index_key(1_000)),
        Ok(Some(0u32.to_be_bytes().to_vec()))
    );
    let after = host.range_open(b"i", b"j", false).unwrap();
    let positions: Vec<u32> = keys(host.range_next(after, 100).unwrap())
        .iter()
        .map(|key| position(key))
        .collect();
    assert_eq!(positions, (1_000..1_050).collect::<Vec<_>>());
}

#[test]
fn a_cursor_sees_writes_made_before_it_opened() {
    let state = base(&[(b"b", b"1")]);
    let inv = inv(InvocationKind::Call);
    let mut host = core_for(&state, &inv);
    host.storage_put(b"a", b"0").unwrap();
    host.storage_delete(b"b").unwrap();
    let cursor = host.range_open(b"", b"", false).unwrap();
    host.storage_put(b"c", b"2").unwrap();
    host.storage_put(b"a", b"9").unwrap();
    assert_eq!(
        host.range_next(cursor, 10).unwrap(),
        vec![(b"a".to_vec(), b"0".to_vec())]
    );
}

struct OrgDirectory;

impl Directory for OrgDirectory {
    fn org(&self) -> Option<String> {
        Some("acme".to_owned())
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
    fn team_exists(&self, team: &str) -> bool {
        team == "ops"
    }
    fn orgs_of(&self, _account: &str) -> Vec<(String, OrgRole)> {
        Vec::new()
    }
}

fn org_core() -> HostCore {
    HostCore::new(
        "app",
        Arc::new(base(&[])),
        0,
        inv(InvocationKind::Call),
        Arc::new(OrgDirectory),
    )
    .unwrap()
}

fn emitted(audience: Audience) -> Result<(), String> {
    let mut host = org_core();
    let result = host
        .emit(Event {
            name: "e".to_owned(),
            data: Vec::new(),
            audience,
        })
        .unwrap();
    let out = host.finish(Status::Ok, Vec::new(), Usage::default());
    assert_eq!(
        out.events.len(),
        usize::from(result.is_ok()),
        "a refused event is not recorded"
    );
    result
}

#[test]
fn emit_checks_the_audience() {
    let refused = |reason: &str| Err(reason.to_owned());
    assert_eq!(emitted(Audience::All), Ok(()));
    assert_eq!(emitted(Audience::Caller), Ok(()));
    assert_eq!(emitted(Audience::Accounts(vec!["ada".to_owned()])), Ok(()));
    assert_eq!(emitted(Audience::Team("ops".to_owned())), Ok(()));
    assert_eq!(emitted(Audience::OrgMembers("acme".to_owned())), Ok(()));
    assert_eq!(emitted(Audience::OrgAdmins("acme".to_owned())), Ok(()));

    assert_eq!(
        emitted(Audience::Accounts(Vec::new())),
        refused(EMIT_BAD_AUDIENCE)
    );
    assert_eq!(
        emitted(Audience::Accounts(vec![String::new()])),
        refused(EMIT_BAD_AUDIENCE)
    );
    assert_eq!(
        emitted(Audience::Team(String::new())),
        refused(EMIT_BAD_AUDIENCE)
    );
    assert_eq!(
        emitted(Audience::OrgMembers(String::new())),
        refused(EMIT_BAD_AUDIENCE)
    );
    assert_eq!(
        emitted(Audience::OrgAdmins(String::new())),
        refused(EMIT_BAD_AUDIENCE)
    );
    assert_eq!(
        emitted(Audience::Team("devs".to_owned())),
        refused(EMIT_UNKNOWN_TEAM)
    );
    assert_eq!(
        emitted(Audience::OrgMembers("other".to_owned())),
        refused(EMIT_FOREIGN_ORG)
    );
    assert_eq!(
        emitted(Audience::OrgAdmins("other".to_owned())),
        refused(EMIT_FOREIGN_ORG)
    );
}

#[test]
fn emit_bounds_names_data_and_count() {
    let mut host = org_core();
    let sized = |name: usize, data: usize| Event {
        name: "n".repeat(name),
        data: vec![0; data],
        audience: Audience::All,
    };
    assert_eq!(host.emit(sized(MAX_EVENT_NAME_BYTES, 0)), Ok(Ok(())));
    assert_eq!(
        host.emit(sized(MAX_EVENT_NAME_BYTES + 1, 0)),
        Ok(Err(EMIT_NAME_TOO_LONG.to_owned()))
    );
    assert_eq!(host.emit(sized(1, MAX_EVENT_DATA_BYTES)), Ok(Ok(())));
    assert_eq!(
        host.emit(sized(1, MAX_EVENT_DATA_BYTES + 1)),
        Ok(Err(EMIT_DATA_TOO_LARGE.to_owned()))
    );
    for _ in 2..MAX_EVENTS {
        assert_eq!(host.emit(sized(1, 0)), Ok(Ok(())));
    }
    assert_eq!(MAX_EVENTS, 1_000);
    assert_eq!(
        host.emit(sized(1, 0)),
        Ok(Err(EMIT_TOO_MANY.to_owned())),
        "the 1,001st event"
    );
    let out = host.finish(Status::Ok, Vec::new(), Usage::default());
    assert_eq!(out.events.len(), MAX_EVENTS);
}

#[test]
fn cursor_sees_value_as_of_open_after_overwrite() {
    let state = base(&[]);
    let inv = inv(InvocationKind::Call);
    let mut host = core_for(&state, &inv);
    host.storage_put(b"k", b"1").unwrap();
    let cursor = host.range_open(b"", b"", false).unwrap();
    host.storage_put(b"k", b"2").unwrap();
    assert_eq!(
        host.range_next(cursor, 10).unwrap(),
        vec![(b"k".to_vec(), b"1".to_vec())]
    );
    assert_eq!(host.storage_get(b"k"), Ok(Some(b"2".to_vec())));
}

#[test]
fn cursor_sees_key_as_of_open_after_delete() {
    let state = base(&[]);
    let inv = inv(InvocationKind::Call);
    let mut host = core_for(&state, &inv);
    host.storage_put(b"k", b"1").unwrap();
    let cursor = host.range_open(b"", b"", false).unwrap();
    host.storage_delete(b"k").unwrap();
    assert_eq!(
        host.range_next(cursor, 10).unwrap(),
        vec![(b"k".to_vec(), b"1".to_vec())]
    );
    assert_eq!(host.storage_get(b"k"), Ok(None));
}

#[test]
fn cursor_sees_committed_value_overwritten_after_open() {
    let state = base(&[(b"k", b"committed")]);
    let inv = inv(InvocationKind::Call);
    let mut host = core_for(&state, &inv);
    let cursor = host.range_open(b"", b"", false).unwrap();
    host.storage_put(b"k", b"new").unwrap();
    host.storage_put(b"k", b"newer").unwrap();
    assert_eq!(
        host.range_next(cursor, 10).unwrap(),
        vec![(b"k".to_vec(), b"committed".to_vec())]
    );
}

fn ids(id_term: u64, id_seq: u64) -> Vec<String> {
    let state = base(&[(b"k", b"v")]);
    let mut inv = inv(InvocationKind::Call);
    inv.id_term = id_term;
    inv.id_seq = id_seq;
    let mut host = core_for(&state, &inv);
    (0..3).map(|_| host.unique_id().unwrap()).collect()
}

#[test]
fn unique_ids_differ_across_runs_on_the_same_base() {
    // Two calls run against the same base and applied index, as two runs before either
    // commits would: the executor gives them different sequence numbers.
    let first = ids(3, 42);
    let second = ids(3, 43);
    assert!(
        first.iter().all(|id| !second.contains(id)),
        "{first:?} {second:?}"
    );
    assert_eq!(
        ids(3, 42),
        first,
        "the same term, seq and position are stable"
    );
    let next_term = ids(4, 42);
    assert!(first.iter().all(|id| !next_term.contains(id)));
}

#[test]
fn keys_are_capped_at_one_kib() {
    let state = base(&[]);
    let inv = inv(InvocationKind::Call);
    let mut host = core_for(&state, &inv);
    let fits = vec![b'k'; MAX_KEY_BYTES];
    let over = vec![b'k'; MAX_KEY_BYTES + 1];
    assert_eq!(MAX_KEY_BYTES, 1_024);
    assert_eq!(host.storage_put(&fits, b"v"), Ok(()));
    assert_eq!(host.storage_get(&fits), Ok(Some(b"v".to_vec())));
    assert!(matches!(host.storage_put(&over, b"v"), Err(HostDenied(_))));
    assert!(matches!(host.storage_get(&over), Err(HostDenied(_))));
    assert!(matches!(host.storage_delete(&over), Err(HostDenied(_))));
    assert!(matches!(host.storage_add(&over, 1), Err(HostDenied(_))));
    assert!(matches!(
        host.range_open(&over, b"", false),
        Err(HostDenied(_))
    ));
    assert!(matches!(
        host.range_open(b"", &over, false),
        Err(HostDenied(_))
    ));
}

/// Drains a cursor one entry at a time.
fn drain_one_by_one(host: &mut HostCore, reverse: bool) -> Vec<KeyValue> {
    let cursor = host.range_open(b"", b"", reverse).unwrap();
    let mut all = Vec::new();
    loop {
        let batch = host.range_next(cursor, 1).unwrap();
        assert!(batch.len() <= 1);
        let Some(entry) = batch.into_iter().next() else {
            return all;
        };
        all.push(entry);
    }
}

#[test]
fn single_entry_batches_merge_puts_and_tombstones() {
    let state = base(&[(b"b", b"B"), (b"d", b"D"), (b"f", b"F"), (b"h", b"H")]);
    let inv = inv(InvocationKind::Call);
    let mut host = core_for(&state, &inv);
    host.storage_put(b"a", b"a").unwrap(); // before every committed key
    host.storage_delete(b"d").unwrap(); // hides a committed key
    host.storage_put(b"e", b"e").unwrap(); // between committed keys
    host.storage_put(b"f", b"F2").unwrap(); // overwrites a committed key
    host.storage_delete(b"g").unwrap(); // tombstone with no committed key
    host.storage_put(b"z", b"z").unwrap(); // after every committed key
    let expected: Vec<KeyValue> = [
        (b"a", &b"a"[..]),
        (b"b", b"B"),
        (b"e", b"e"),
        (b"f", b"F2"),
        (b"h", b"H"),
        (b"z", b"z"),
    ]
    .iter()
    .map(|(k, v)| (k.to_vec(), v.to_vec()))
    .collect();
    assert_eq!(drain_one_by_one(&mut host, false), expected);
    let mut reversed = expected;
    reversed.reverse();
    assert_eq!(drain_one_by_one(&mut host, true), reversed);
}
