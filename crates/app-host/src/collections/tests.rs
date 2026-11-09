//! Host collections against the real SDK: the same calls on a `MemStore` and on a
//! [`HostCore`] must leave the same keys and bytes and answer the same pages.

use std::collections::BTreeMap;
use std::sync::Arc;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use calimero_sdk::app::StateField;
use calimero_sdk::collections::{Tally, UnorderedMap};
use calimero_sdk::encoding::{key as sdk_key, value as sdk_value};
use calimero_sdk::{FindOpts, MemStore, Record, Store, Table, TableError};
use ciborium::Value;
use serde_json::json;

use super::{
    canonical_value, collection_fuel, decode_key, encode_key, CollError, CollectionSchema, KeyPart,
    TableSchema, MAX_COLLECTION_LIMIT,
};
use crate::abi::Abi;
use crate::core::HostCore;
use crate::state::{KeyValue, GENERATION};
use crate::types::{Caller, Directory, HostDenied, Invocation, InvocationKind, OrgRole};

#[derive(calimero_sdk::Record, Clone, Debug, PartialEq)]
#[table(
    key = ["org", "id"],
    index(by_owner = ["owner"]),
    unique(by_email = ["email"]),
    count(per_owner = ["owner"])
)]
struct Member {
    org: String,
    id: u64,
    owner: String,
    email: Option<String>,
    score: i64,
    tags: Vec<String>,
    avatar: Vec<u8>,
}

fn member(org: &str, id: u64, owner: &str, email: Option<&str>) -> Member {
    Member {
        org: org.to_owned(),
        id,
        owner: owner.to_owned(),
        email: email.map(str::to_owned),
        score: -3,
        tags: vec!["x".to_owned()],
        avatar: vec![0, 1, 2],
    }
}

type Members = Table<Member, MemStore>;
type Votes = UnorderedMap<(String, u64), String, MemStore>;
type Counts = Tally<String, MemStore>;

const TABLE: &str = "members";
const MAP: &str = "votes";
const TALLY: &str = "tally";

fn abi() -> Abi {
    let table: serde_json::Value = serde_json::from_str(<Member as Record>::SCHEMA_JSON).unwrap();
    Abi::from_json(&json!({
        "abiVersion": 1,
        "methods": {},
        "state": {
            "collections": {
                TABLE: table,
                MAP: { "kind": <Votes as StateField>::KIND },
                TALLY: { "kind": <Counts as StateField>::KIND },
            },
        },
    }))
    .unwrap()
}

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

fn host(kind: InvocationKind) -> HostCore {
    let inv = Invocation {
        kind,
        method: "m".to_owned(),
        args: Vec::new(),
        caller: Caller::Account("ada".to_owned()),
        entry_ts_ms: 1_000,
        id_term: 1,
        id_seq: 1,
        seed: [7; 32],
        timer: None,
        compute: None,
    };
    HostCore::new(
        "app",
        Arc::new(BTreeMap::<Vec<u8>, Vec<u8>>::new()),
        0,
        inv,
        Arc::new(NoDirectory),
    )
    .unwrap()
    .with_collections(&abi())
}

fn call() -> HostCore {
    host(InvocationKind::Call)
}

/// Every key and value the run sees, keys as the guest wrote them.
fn dump(host: &mut HostCore) -> Vec<KeyValue> {
    let cursor = host.range_open(b"", b"", false).unwrap();
    let mut out = Vec::new();
    loop {
        let batch = host.range_next(cursor, 1_000).unwrap();
        if batch.is_empty() {
            break;
        }
        out.extend(batch);
    }
    host.range_close(cursor);
    out
}

fn sdk_dump(store: &MemStore) -> Vec<KeyValue> {
    let mut cursor = store.range(&[], &[0xFF; 8], false);
    let mut out = Vec::new();
    loop {
        let batch = calimero_sdk::RangeCursor::next(&mut cursor, 1_000);
        if batch.is_empty() {
            break;
        }
        out.extend(batch);
    }
    out
}

fn sdk_part(part: &KeyPart) -> sdk_key::KeyPart {
    match part {
        KeyPart::None => sdk_key::KeyPart::None,
        KeyPart::Bool(b) => sdk_key::KeyPart::Bool(*b),
        KeyPart::Int(i) => sdk_key::KeyPart::Int(*i),
        KeyPart::Str(s) => sdk_key::KeyPart::Str(s.clone()),
        KeyPart::Bytes(b) => sdk_key::KeyPart::Bytes(b.clone()),
    }
}

fn s(text: &str) -> KeyPart {
    KeyPart::Str(text.to_owned())
}

fn ok<T>(answer: Result<Result<T, CollError>, HostDenied>) -> T {
    answer.unwrap().unwrap()
}

fn cbor(record: &Member) -> Vec<u8> {
    sdk_value::to_vec(record).unwrap()
}

fn decode(bytes: &[u8]) -> Member {
    sdk_value::from_slice(bytes).unwrap()
}

/// A CBOR map written by hand, so keys may be out of order and integers non-shortest.
fn raw_map(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut out = vec![0xA0 + u8::try_from(entries.len()).unwrap()];
    for (k, v) in entries {
        out.push(0x60 + u8::try_from(k.len()).unwrap());
        out.extend_from_slice(k.as_bytes());
        out.extend_from_slice(v);
    }
    out
}

fn text(t: &str) -> Vec<u8> {
    let mut out = vec![0x60 + u8::try_from(t.len()).unwrap()];
    out.extend_from_slice(t.as_bytes());
    out
}

#[test]
fn key_encoding_matches_the_sdk() {
    let parts = vec![
        KeyPart::None,
        KeyPart::Bool(false),
        KeyPart::Bool(true),
        KeyPart::Int(0),
        KeyPart::Int(-1),
        KeyPart::Int(i128::from(i64::MIN)),
        KeyPart::Int(i128::from(u64::MAX)),
        KeyPart::Int(i128::MAX),
        KeyPart::Int(i128::MIN),
        s(""),
        s("a\0b"),
        s("héllo"),
        KeyPart::Bytes(Vec::new()),
        KeyPart::Bytes(vec![0, 0, 0xFF, 1]),
    ];
    for part in &parts {
        let one = std::slice::from_ref(part);
        assert_eq!(
            encode_key(one),
            sdk_key::encode(&[sdk_part(part)]),
            "{part:?}"
        );
        assert_eq!(decode_key(&encode_key(one)).as_deref(), Some(one));
    }
    let all: Vec<sdk_key::KeyPart> = parts.iter().map(sdk_part).collect();
    assert_eq!(encode_key(&parts), sdk_key::encode(&all));
    assert_eq!(decode_key(&encode_key(&parts)), Some(parts));
    assert_eq!(decode_key(&[0x77]), None);
}

#[test]
fn value_canonicalization_matches_the_sdk() {
    let values = vec![
        Value::Integer(5.into()),
        Value::Integer((-70_000).into()),
        Value::Float(1.5),
        Value::Float(1.0e300),
        Value::Text("t".into()),
        Value::Bytes(vec![1, 2]),
        Value::Null,
        Value::Bool(true),
        Value::Tag(0xCA1B, Box::new(Value::Bytes(vec![9; 32]))),
        Value::Array(vec![Value::Map(vec![
            (Value::Text("zz".into()), Value::Integer(1.into())),
            (Value::Text("a".into()), Value::Integer(2.into())),
            (Value::Integer(10.into()), Value::Null),
        ])]),
    ];
    for v in values {
        let mut raw = Vec::new();
        ciborium::into_writer(&v, &mut raw).unwrap();
        assert_eq!(
            canonical_value(&raw).unwrap(),
            sdk_value::to_vec(&v).unwrap(),
            "{v:?}"
        );
    }
    assert!(canonical_value(&[0xF9, 0x7E, 0x00]).is_err(), "NaN");
    assert!(canonical_value(&[0x01, 0x02]).is_err(), "trailing bytes");
    assert!(canonical_value(&[]).is_err(), "empty");
}

#[test]
fn the_abi_lists_the_collections() {
    let abi = abi();
    let CollectionSchema::Table(table) = &abi.collections[TABLE] else {
        panic!("not a table: {:?}", abi.collections[TABLE]);
    };
    assert_eq!(
        *table,
        TableSchema {
            key: vec!["org".into(), "id".into()],
            indexes: [("by_owner".into(), vec!["owner".into()])].into(),
            unique: [("by_email".into(), vec!["email".into()])].into(),
            counts: [("per_owner".into(), vec!["owner".into()])].into(),
        }
    );
    assert_eq!(abi.collections[MAP], CollectionSchema::Map);
    assert_eq!(abi.collections[TALLY], CollectionSchema::Tally);
    let other = Abi::from_json(&json!({
        "abiVersion": 1, "methods": {},
        "state": { "collections": { "c": { "kind": "cell" } } },
    }))
    .unwrap();
    assert_eq!(
        other.collections["c"],
        CollectionSchema::Other("cell".into())
    );
    for bad in [
        json!({ "c": "table" }),
        json!({ "c": {} }),
        json!({ "c": { "kind": "table", "key": "id" } }),
        json!({ "c": { "kind": "table", "key": [] } }),
        json!({ "c": { "kind": "table", "key": ["id"], "indexes": { "i": [1] } } }),
    ] {
        let abi = json!({ "abiVersion": 1, "methods": {}, "state": { "collections": bad } });
        assert!(Abi::from_json(&abi).is_err(), "{abi}");
    }
}

/// Two set names of one table that camelCase alike would share their storage keys.
#[test]
fn set_names_that_camelcase_alike_are_a_bad_abi() {
    for set in ["indexes", "unique", "counts"] {
        let abi = json!({
            "abiVersion": 1, "methods": {},
            "state": { "collections": { "t": {
                "kind": "table", "key": ["id"],
                set: { "by_owner": ["owner"], "byOwner": ["owner"] },
            } } },
        });
        let err = Abi::from_json(&abi).expect_err(set).to_string();
        assert!(err.contains("byOwner"), "{set}: {err}");
    }
    // Across kinds the keys differ (`i`, `u`, `c`), so one name may serve each.
    let abi = json!({
        "abiVersion": 1, "methods": {},
        "state": { "collections": { "t": {
            "kind": "table", "key": ["id"],
            "indexes": { "by_owner": ["owner"] }, "counts": { "byOwner": ["owner"] },
        } } },
    });
    assert!(Abi::from_json(&abi).is_ok());
}

/// A count over a partial prefix is summed batch by batch, each entry read charged.
#[test]
fn table_count_over_a_partial_prefix_charges_per_entry() {
    let mut host = call();
    for (id, owner) in [(1, "ann"), (2, "bob"), (3, "cy"), (4, "ann")] {
        ok(host.table_insert(TABLE, &cbor(&member("o1", id, owner, None))));
    }
    let before = host.host_fuel();
    assert_eq!(ok(host.table_count(TABLE, "per_owner", &[])), 4);
    let empty_prefix = host.host_fuel() - before;
    let before = host.host_fuel();
    assert_eq!(
        ok(host.table_count(TABLE, "per_owner", &[KeyPart::Str("ann".into())])),
        2
    );
    let one_entry = host.host_fuel() - before;
    // Three count entries read against one.
    assert!(
        empty_prefix >= one_entry + 2 * super::COLLECTION_ENTRY_FUEL,
        "{empty_prefix} vs {one_entry}"
    );
}

#[test]
fn table_insert_writes_the_sdk_layout() {
    let store = MemStore::new();
    let mut table: Members = Table::with_store(TABLE, store.clone());
    let mut host = call();

    let a = member("o1", 1, "ann", Some("a@x"));
    let b = member("o1", 2, "ann", None);
    table.insert(a.clone()).unwrap();
    table.insert(b.clone()).unwrap();
    ok(host.table_insert(TABLE, &cbor(&a)));
    ok(host.table_insert(TABLE, &cbor(&b)));
    assert_eq!(dump(&mut host), sdk_dump(&store), "after insert");

    // The owner (index and count) and the email (unique) change.
    let a2 = member("o1", 1, "bob", Some("a2@x"));
    table.upsert(a2.clone()).unwrap();
    ok(host.table_upsert(TABLE, &cbor(&a2)));
    assert_eq!(dump(&mut host), sdk_dump(&store), "after upsert");
    // An upsert of a new key inserts.
    let c = member("o2", 1, "bob", Some("c@x"));
    table.upsert(c.clone()).unwrap();
    ok(host.table_upsert(TABLE, &cbor(&c)));
    assert_eq!(
        dump(&mut host),
        sdk_dump(&store),
        "after upsert of a new key"
    );

    let key = [s("o1"), KeyPart::Int(2)];
    assert!(table.remove(&("o1".to_owned(), 2u64)).unwrap());
    assert!(ok(host.table_remove(TABLE, &key)));
    assert!(!ok(host.table_remove(TABLE, &key)));
    assert_eq!(dump(&mut host), sdk_dump(&store), "after remove");

    let got = ok(host.table_get(TABLE, &[s("o1"), KeyPart::Int(1)])).unwrap();
    assert_eq!(decode(&got), a2);
    assert_eq!(ok(host.table_get(TABLE, &key)), None);
    for (name, prefix) in [
        ("per_owner", vec![s("bob")]),
        ("perOwner", vec![s("bob")]),
        ("perOwner", vec![s("ann")]),
        ("perOwner", Vec::new()),
    ] {
        let sdk_prefix: Vec<_> = prefix.iter().map(sdk_part).collect();
        assert_eq!(
            ok(host.table_count(TABLE, name, &prefix)),
            table.count("perOwner", &sdk_prefix).unwrap(),
            "{name} {prefix:?}"
        );
    }
    assert!(matches!(
        host.table_count(TABLE, "nope", &[]),
        Ok(Err(CollError::UnknownCollection(_)))
    ));
    assert!(matches!(
        host.table_count(TABLE, "perOwner", &[s("a"), s("b")]),
        Ok(Err(CollError::BadRecord(_)))
    ));
}

#[test]
fn table_find_pages_like_the_sdk() {
    let store = MemStore::new();
    let mut table: Members = Table::with_store(TABLE, store.clone());
    let mut host = call();
    let records = [
        member("o1", 3, "ann", None),
        member("o2", 1, "ann", None),
        member("o1", 1, "bob", None),
        member("o1", 2, "ann", None),
        member("o1", 4, "ann", None),
        member("o3", 1, "cyd", None),
    ];
    for r in &records {
        table.insert(r.clone()).unwrap();
        ok(host.table_insert(TABLE, &cbor(r)));
    }

    for reverse in [false, true] {
        // By index, with a prefix, two at a time, to the end.
        let mut opts = FindOpts::default().limit(2);
        if reverse {
            opts = opts.reverse();
        }
        let mut cursor: Option<String> = None;
        loop {
            let want = table
                .find(
                    "byOwner",
                    &[sdk_key::KeyPart::Str("ann".into())],
                    opts.clone(),
                )
                .unwrap();
            let got = ok(host.table_find(
                TABLE,
                Some("byOwner"),
                &[s("ann")],
                cursor.as_deref(),
                2,
                reverse,
            ));
            let items: Vec<Member> = got.items.iter().map(|e| decode(&e.value)).collect();
            assert_eq!(items, want.items, "reverse {reverse}");
            for (entry, record) in got.items.iter().zip(&items) {
                assert_eq!(entry.key, [s(&record.org), KeyPart::Int(record.id.into())]);
            }
            assert_eq!(got.next, want.next, "reverse {reverse}");
            match want.next {
                Some(next) => {
                    opts = opts.cursor(next.clone());
                    cursor = Some(next);
                }
                None => break,
            }
        }

        // By key, no prefix: the SDK's scan.
        let mut opts = FindOpts::default().limit(2);
        if reverse {
            opts = opts.reverse();
        }
        let want = table.scan(opts).unwrap();
        let got = ok(host.table_find(TABLE, None, &[], None, 2, reverse));
        let items: Vec<Member> = got.items.iter().map(|e| decode(&e.value)).collect();
        assert_eq!(items, want.items);
        assert_eq!(got.next, want.next);
    }

    // By key with a prefix: org "o1", in key order, the cursor the base64url of the last
    // record key and bound to the prefix.
    let got = ok(host.table_find(TABLE, None, &[s("o1")], None, 2, false));
    let ids: Vec<u64> = got.items.iter().map(|e| decode(&e.value).id).collect();
    assert_eq!(ids, [1, 2]);
    let last = sdk_key::encode(&[
        sdk_key::KeyPart::Str(TABLE.into()),
        sdk_key::KeyPart::Str("r".into()),
        sdk_key::KeyPart::Str("o1".into()),
        sdk_key::KeyPart::Int(2),
    ]);
    assert_eq!(got.next, Some(URL_SAFE_NO_PAD.encode(&last)));
    let rest = ok(host.table_find(TABLE, None, &[s("o1")], got.next.as_deref(), 2, false));
    let ids: Vec<u64> = rest.items.iter().map(|e| decode(&e.value).id).collect();
    assert_eq!((ids, rest.next), (vec![3, 4], None));
    // The same cursor under another prefix or index is refused.
    assert_eq!(
        host.table_find(TABLE, None, &[s("o2")], got.next.as_deref(), 2, false),
        Ok(Err(CollError::BadCursor))
    );
    assert_eq!(
        host.table_find(TABLE, Some("byOwner"), &[], got.next.as_deref(), 2, false),
        Ok(Err(CollError::BadCursor))
    );
    assert_eq!(
        host.table_find(TABLE, None, &[], Some("not base64!"), 2, false),
        Ok(Err(CollError::BadCursor))
    );
    // The index as written in `#[table]` is accepted too; an unknown one is not.
    let as_written = ok(host.table_find(TABLE, Some("by_owner"), &[], None, 10, false));
    assert_eq!(as_written.items.len(), 6);
    assert!(matches!(
        host.table_find(TABLE, Some("nope"), &[], None, 2, false),
        Ok(Err(CollError::UnknownCollection(_)))
    ));
    assert!(matches!(
        host.table_find(
            TABLE,
            None,
            &[s("o1"), KeyPart::Int(1), s("x")],
            None,
            2,
            false
        ),
        Ok(Err(CollError::BadRecord(_)))
    ));
    assert_eq!(
        ok(host.table_find(TABLE, None, &[], None, 0, false)).items,
        []
    );
}

#[test]
fn map_and_tally_write_the_sdk_layout() {
    let store = MemStore::new();
    let mut votes: Votes = UnorderedMap::with_store(MAP, store.clone());
    let mut counts: Counts = Tally::with_store(TALLY, store.clone());
    let mut host = call();

    let key = |who: &str, n: u64| [s(who), KeyPart::Int(n.into())];
    let value = |v: &str| sdk_value::to_vec(&v.to_owned()).unwrap();
    for (who, n, v) in [
        ("ann", 1, "a"),
        ("ann", 2, "b"),
        ("bob", 1, "c"),
        ("ann", 1, "d"),
    ] {
        let old = votes.insert(&(who.to_owned(), n), &v.to_owned()).unwrap();
        let got = ok(host.map_insert(MAP, &key(who, n), &value(v)));
        assert_eq!(got, old.map(|o| value(&o)));
    }
    assert_eq!(
        votes.remove(&("bob".to_owned(), 1)).unwrap(),
        Some("c".to_owned())
    );
    assert_eq!(ok(host.map_remove(MAP, &key("bob", 1))), Some(value("c")));
    assert_eq!(ok(host.map_remove(MAP, &key("bob", 1))), None);
    assert_eq!(ok(host.map_get(MAP, &key("ann", 1))), Some(value("d")));
    assert_eq!(ok(host.map_get(MAP, &key("zed", 1))), None);

    for (who, delta) in [("yes", 2), ("yes", 3), ("no", -1)] {
        counts.add(&who.to_owned(), delta).unwrap();
        ok(host.tally_add(TALLY, &[s(who)], delta));
    }
    assert_eq!(ok(host.tally_get(TALLY, &[s("yes")])), 5);
    assert_eq!(ok(host.tally_get(TALLY, &[s("no")])), -1);
    assert_eq!(ok(host.tally_get(TALLY, &[s("maybe")])), 0);
    assert_eq!(dump(&mut host), sdk_dump(&store));

    // Pages: the prefix `ann`, one at a time, both directions.
    for reverse in [false, true] {
        let mut opts = FindOpts::default().limit(1);
        if reverse {
            opts = opts.reverse();
        }
        let mut cursor: Option<String> = None;
        loop {
            let want = votes
                .prefix(&[sdk_key::KeyPart::Str("ann".into())], opts.clone())
                .unwrap();
            let got = ok(host.map_page(MAP, &[s("ann")], cursor.as_deref(), 1, reverse));
            let items: Vec<_> = got
                .items
                .iter()
                .map(|e| {
                    (
                        e.key.clone(),
                        sdk_value::from_slice::<String>(&e.value).unwrap(),
                    )
                })
                .collect();
            let want_items: Vec<_> = want
                .items
                .iter()
                .map(|((who, n), v)| (key(who, *n).to_vec(), v.clone()))
                .collect();
            assert_eq!(items, want_items);
            assert_eq!(got.next, want.next);
            match want.next {
                Some(next) => {
                    opts = opts.cursor(next.clone());
                    cursor = Some(next);
                }
                None => break,
            }
        }
    }
}

#[test]
fn values_are_canonicalized() {
    let store = MemStore::new();
    let mut table: Members = Table::with_store(TABLE, store.clone());
    let mut votes: Votes = UnorderedMap::with_store(MAP, store.clone());
    let mut host = call();

    let m = member("o1", 5, "ann", Some("a@x"));
    table.insert(m.clone()).unwrap();
    // Reverse key order, `id` as an 8-byte integer, `score` (-3) as a 1-byte-argument one.
    let record = raw_map(&[
        ("tags", [vec![0x81], text("x")].concat()),
        ("score", vec![0x38, 0x02]),
        ("owner", text("ann")),
        ("org", text("o1")),
        ("id", vec![0x1B, 0, 0, 0, 0, 0, 0, 0, 5]),
        ("email", text("a@x")),
        ("avatar", vec![0x43, 0, 1, 2]),
    ]);
    assert_ne!(record, cbor(&m));
    ok(host.table_insert(TABLE, &record));

    // A map value: a two-byte-argument integer string length.
    let _ = votes
        .insert(&("ann".to_owned(), 1), &"v".to_owned())
        .unwrap();
    let _ = ok(host.map_insert(MAP, &[s("ann"), KeyPart::Int(1)], &[0x79, 0, 1, b'v']));
    assert_eq!(dump(&mut host), sdk_dump(&store));
}

#[test]
fn a_duplicate_key_is_refused() {
    let store = MemStore::new();
    let mut table: Members = Table::with_store(TABLE, store.clone());
    let mut host = call();
    let a = member("o1", 1, "ann", Some("a@x"));
    let again = member("o1", 1, "bob", Some("b@x"));
    table.insert(a.clone()).unwrap();
    ok(host.table_insert(TABLE, &cbor(&a)));
    let TableError::Duplicate { index } = table.insert(again.clone()).unwrap_err() else {
        panic!("not a duplicate");
    };
    let writes = host.write_count();
    assert_eq!(
        host.table_insert(TABLE, &cbor(&again)),
        Ok(Err(CollError::Duplicate(index)))
    );
    assert_eq!(host.write_count(), writes, "nothing written");
    assert_eq!(dump(&mut host), sdk_dump(&store));
}

#[test]
fn a_unique_clash_is_refused() {
    let store = MemStore::new();
    let mut table: Members = Table::with_store(TABLE, store.clone());
    let mut host = call();
    let a = member("o1", 1, "ann", Some("a@x"));
    let clash = member("o1", 2, "bob", Some("a@x"));
    table.insert(a.clone()).unwrap();
    ok(host.table_insert(TABLE, &cbor(&a)));
    let TableError::Duplicate { index } = table.insert(clash.clone()).unwrap_err() else {
        panic!("not a duplicate");
    };
    assert_eq!(
        index, "byEmail",
        "the SDK names the unique entry in camelCase"
    );
    let writes = host.write_count();
    assert_eq!(
        host.table_insert(TABLE, &cbor(&clash)),
        Ok(Err(CollError::Duplicate(index.clone())))
    );
    assert_eq!(
        host.table_upsert(TABLE, &cbor(&clash)),
        Ok(Err(CollError::Duplicate(index)))
    );
    assert_eq!(host.write_count(), writes, "nothing written");
    assert_eq!(dump(&mut host), sdk_dump(&store));
    // The record's own unique entry is not a clash, and a None email is never one.
    let same = member("o1", 1, "cyd", Some("a@x"));
    ok(host.table_upsert(TABLE, &cbor(&same)));
    ok(host.table_insert(TABLE, &cbor(&member("o1", 3, "x", None))));
    ok(host.table_insert(TABLE, &cbor(&member("o1", 4, "x", None))));
}

fn refused_as_bad_record(record: &[u8]) {
    let mut host = call();
    for answer in [
        host.table_insert(TABLE, record),
        host.table_upsert(TABLE, record),
    ] {
        assert!(
            matches!(answer, Ok(Err(CollError::BadRecord(_)))),
            "{answer:?}"
        );
    }
    assert_eq!(host.write_count(), 0, "nothing written");
    assert_eq!(dump(&mut host), []);
}

#[test]
fn a_record_missing_a_key_field_is_refused() {
    let full = cbor(&member("o1", 1, "ann", None));
    let Value::Map(entries) = ciborium::from_reader::<Value, _>(full.as_slice()).unwrap() else {
        panic!("a record is a map");
    };
    for missing in ["id", "owner"] {
        let kept: Vec<_> = entries
            .iter()
            .filter(|(k, _)| k.as_text() != Some(missing))
            .cloned()
            .collect();
        let mut record = Vec::new();
        ciborium::into_writer(&Value::Map(kept), &mut record).unwrap();
        refused_as_bad_record(&record);
    }
    refused_as_bad_record(&[0x83, 1, 2, 3]);
    refused_as_bad_record(b"\xFF");
}

#[test]
fn a_float_key_field_is_refused() {
    let full = cbor(&member("o1", 1, "ann", None));
    let Value::Map(entries) = ciborium::from_reader::<Value, _>(full.as_slice()).unwrap() else {
        panic!("a record is a map");
    };
    for (field, bad) in [
        ("id", Value::Float(1.5)),
        ("org", Value::Array(Vec::new())),
        ("owner", Value::Map(Vec::new())),
    ] {
        let changed: Vec<_> = entries
            .iter()
            .map(|(k, v)| {
                if k.as_text() == Some(field) {
                    (k.clone(), bad.clone())
                } else {
                    (k.clone(), v.clone())
                }
            })
            .collect();
        let mut record = Vec::new();
        ciborium::into_writer(&Value::Map(changed), &mut record).unwrap();
        refused_as_bad_record(&record);
    }
}

#[test]
fn an_unknown_collection_is_refused() {
    let mut host = call();
    let unknown = |a| matches!(a, Ok(Err(CollError::UnknownCollection(ref n))) if n == "nope");
    assert!(unknown(
        host.table_insert("nope", &cbor(&member("o", 1, "a", None)))
            .map(|r| r.map(|()| 0))
    ));
    assert!(unknown(
        host.table_get("nope", &[s("k")]).map(|r| r.map(|_| 0))
    ));
    assert!(unknown(
        host.map_get("nope", &[s("k")]).map(|r| r.map(|_| 0))
    ));
    assert!(unknown(
        host.map_insert("nope", &[s("k")], &[1])
            .map(|r| r.map(|_| 0))
    ));
    assert!(unknown(
        host.tally_add("nope", &[s("k")], 1).map(|r| r.map(|()| 0))
    ));
    assert!(unknown(
        host.tally_get("nope", &[s("k")]).map(|r| r.map(|_| 0))
    ));
    assert_eq!(host.write_count(), 0);
}

#[test]
fn a_map_call_on_a_table_is_wrong_kind() {
    let mut host = call();
    assert_eq!(
        host.map_get(TABLE, &[s("k")]),
        Ok(Err(CollError::WrongKind("table".into())))
    );
    assert_eq!(
        host.map_insert(TABLE, &[s("k")], &[1]),
        Ok(Err(CollError::WrongKind("table".into())))
    );
    assert_eq!(
        host.table_get(MAP, &[s("k")]),
        Ok(Err(CollError::WrongKind("unorderedMap".into())))
    );
    assert_eq!(
        host.tally_add(MAP, &[s("k")], 1),
        Ok(Err(CollError::WrongKind("unorderedMap".into())))
    );
    assert_eq!(
        host.map_page(TALLY, &[], None, 1, false),
        Ok(Err(CollError::WrongKind("tally".into())))
    );
    assert_eq!(host.write_count(), 0);
}

#[test]
fn writes_in_a_view_are_denied() {
    let mut host = host(InvocationKind::View);
    let record = cbor(&member("o1", 1, "ann", None));
    let key = [s("o1"), KeyPart::Int(1)];
    assert!(host.table_insert(TABLE, &record).is_err());
    assert!(host.table_upsert(TABLE, &record).is_err());
    assert!(host.table_remove(TABLE, &key).is_err());
    assert!(host.map_insert(MAP, &key, &[1]).is_err());
    assert!(host.map_remove(MAP, &key).is_err());
    assert!(host.tally_add(TALLY, &key, 1).is_err());
    // Even a write that would be refused softly is a trap in a view.
    assert!(host.table_insert("nope", &record).is_err());
    // Reads work.
    assert_eq!(host.table_get(TABLE, &key), Ok(Ok(None)));
    assert_eq!(host.map_get(MAP, &key), Ok(Ok(None)));
    assert_eq!(host.tally_get(TALLY, &key), Ok(Ok(0)));
    assert_eq!(host.table_count(TABLE, "perOwner", &[]), Ok(Ok(0)));
    assert!(ok(host.table_find(TABLE, None, &[], None, 10, false))
        .items
        .is_empty());
    assert!(ok(host.map_page(MAP, &[], None, 10, false))
        .items
        .is_empty());
}

#[test]
fn collections_in_migrate_are_denied() {
    let mut host = host(InvocationKind::Migrate);
    let record = cbor(&member("o1", 1, "ann", None));
    let key = [s("o1"), KeyPart::Int(1)];
    assert!(host.table_insert(TABLE, &record).is_err());
    assert!(host.table_upsert(TABLE, &record).is_err());
    assert!(host.table_get(TABLE, &key).is_err());
    assert!(host.table_remove(TABLE, &key).is_err());
    assert!(host.table_find(TABLE, None, &[], None, 1, false).is_err());
    assert!(host.table_count(TABLE, "perOwner", &[]).is_err());
    assert!(host.map_get(MAP, &key).is_err());
    assert!(host.map_insert(MAP, &key, &[1]).is_err());
    assert!(host.map_remove(MAP, &key).is_err());
    assert!(host.map_page(MAP, &[], None, 1, false).is_err());
    assert!(host.tally_add(TALLY, &key, 1).is_err());
    assert!(host.tally_get(TALLY, &key).is_err());
    // An unknown collection too: the interface is closed, not the name.
    assert!(host.map_get("nope", &key).is_err());
}

#[test]
fn find_limit_is_capped_at_1000() {
    assert_eq!(MAX_COLLECTION_LIMIT, 1_000);
    let store = MemStore::new();
    let mut votes: Votes = UnorderedMap::with_store(MAP, store.clone());
    let mut table: Members = Table::with_store(TABLE, store.clone());
    let mut host = call();
    for i in 0..1_100u64 {
        let _ = votes.insert(&("k".to_owned(), i), &String::new()).unwrap();
        let _ = ok(host.map_insert(MAP, &[s("k"), KeyPart::Int(i.into())], &[0x60]));
    }
    for i in 0..1_001u64 {
        let m = member("o", i, "ann", None);
        table.insert(m.clone()).unwrap();
        ok(host.table_insert(TABLE, &cbor(&m)));
    }
    let page = ok(host.map_page(MAP, &[], None, 5_000, false));
    let want = votes.page(FindOpts::default().limit(5_000)).unwrap();
    assert_eq!(page.items.len(), 1_000);
    assert_eq!(page.next, want.next);
    let page = ok(host.table_find(TABLE, Some("byOwner"), &[], None, u32::MAX, true));
    let want = table
        .find(
            "byOwner",
            &[],
            FindOpts::default().limit(u32::MAX).reverse(),
        )
        .unwrap();
    assert_eq!(page.items.len(), 1_000);
    assert_eq!(page.next, want.next);
}

#[test]
fn every_call_charges_collection_fuel() {
    let mut host = call();
    assert_eq!(host.host_fuel(), 0);
    ok(host.tally_add(TALLY, &[s("yes")], 1));
    let key = encode_key(&[s(TALLY), s("t"), s("yes")]);
    // One entry touched: its key and the one-byte integer written.
    assert_eq!(host.host_fuel(), collection_fuel(key.len() + 1, 1));
    let before = host.host_fuel();
    assert!(matches!(host.map_get("nope", &[]), Ok(Err(_))));
    assert_eq!(
        host.host_fuel(),
        before + collection_fuel(0, 0),
        "a refusal costs the base"
    );
    assert!(collection_fuel(10, 2) > collection_fuel(0, 0));
    assert!(
        collection_fuel(usize::MAX, usize::MAX) == u64::MAX,
        "saturates"
    );
    let _ = GENERATION;
}
