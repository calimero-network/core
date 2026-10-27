//! The host core's policy, overlay and determinism rules (spec sections 3-5, D19, D46, D47).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use calimero_app_host::{
    app_key, Abi, Audience, Caller, Directory, Event, HostCore, HostDenied, InvalidInvocation,
    Invocation, InvocationKind, KeyValue, MethodKind, OrgRole, Status, TimerCtx, Usage, GENERATION,
    MAX_WRITES,
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
fn range_sees_writes_made_between_batches() {
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
    assert_eq!(keys, vec![b"b".to_vec(), b"e".to_vec()]);
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
fn unique_ids_are_index_and_position() {
    let state = base(&[]);
    let call = inv(InvocationKind::Call);
    let mut host = core_for(&state, &call);
    assert_eq!(host.unique_id().unwrap(), "42.0");
    assert_eq!(host.unique_id().unwrap(), "42.1");
    assert_eq!(host.unique_id().unwrap(), "42.2");

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
fn reverse_range_sees_writes_between_batches() {
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
    host.storage_put(b"h", b"H").unwrap(); // behind the cursor: not returned
    assert_eq!(
        keys(host.range_next(cursor, 2).unwrap()),
        vec![b"f".to_vec(), b"d".to_vec()]
    );
    host.storage_put(b"c", b"C2").unwrap();
    assert_eq!(
        host.range_next(cursor, 10).unwrap(),
        vec![
            (b"c".to_vec(), b"C2".to_vec()),
            (b"a".to_vec(), b"a".to_vec())
        ]
    );
}

#[test]
fn forward_range_skips_a_key_deleted_between_batches() {
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
