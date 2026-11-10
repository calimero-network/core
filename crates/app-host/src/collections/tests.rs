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
    canonical, collection_fuel, decode_key, encode_key, encoded_len, value_in, write_value,
    CollError, CollectionSchema, KeyPart, TableSchema, ValueType, MAX_COLLECTION_LIMIT,
    MAX_JSON_DEPTH, MAX_JSON_ITEMS,
};
use crate::abi::Abi;
use crate::core::HostCore;
use crate::core::{MAX_RUN_BYTES, MAX_WRITES};
use crate::read_set::ReadSet;
use crate::state::{stored_key, KeyValue, GENERATION};
use crate::types::{
    Caller, Directory, HostDenied, Invocation, InvocationKind, OrgRole, Status, Usage, Write,
};
use crate::COLLECTION_BASE_FUEL;

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
    ratio: f32,
    /// A type the ABI does not describe (`Value`): converted by the JSON's own shape.
    meta: serde_json::Value,
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
        ratio: 0.1,
        meta: json!({ "n": 7, "big": u64::MAX, "neg": -2, "f": 1.5, "list": [true, null, "s"] }),
    }
}

type Members = Table<Member, MemStore>;
type Votes = UnorderedMap<(String, u64), String, MemStore>;
type Bigs = UnorderedMap<String, u64, MemStore>;
type Counts = Tally<String, MemStore>;

const TABLE: &str = "members";
const MAP: &str = "votes";
const BIG: &str = "bigs";
const TALLY: &str = "tally";

fn abi() -> Abi {
    let table: serde_json::Value = serde_json::from_str(<Member as Record>::SCHEMA_JSON).unwrap();
    Abi::from_json(&json!({
        "abiVersion": 1,
        "methods": {},
        "state": {
            "collections": {
                TABLE: table,
                MAP: { "kind": <Votes as StateField>::KIND, "value": "string" },
                BIG: { "kind": <Bigs as StateField>::KIND, "value": "u64" },
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

/// The record as the Rust SDK's host path sends it: its JSON (64-bit integers as decimal
/// strings, bytes as base64url).
fn json_of(record: &Member) -> String {
    serde_json::to_string(record).unwrap()
}

fn decode(text: &str) -> Member {
    serde_json::from_str(text).unwrap()
}

/// The record's JSON as a mutable object.
fn object_of(record: &Member) -> serde_json::Map<String, serde_json::Value> {
    let serde_json::Value::Object(object) = serde_json::to_value(record).unwrap() else {
        panic!("a record is an object");
    };
    object
}

/// A JSON string.
fn text(t: &str) -> String {
    serde_json::Value::String(t.to_owned()).to_string()
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
        assert_eq!(
            write_value(&canonical(v.clone()).unwrap()).unwrap(),
            sdk_value::to_vec(&v).unwrap(),
            "{v:?}"
        );
    }
    assert!(canonical(Value::Float(f64::NAN)).is_err(), "NaN");
}

#[test]
fn the_abi_lists_the_collections() {
    let abi = abi();
    let CollectionSchema::Table(table) = &abi.collections[TABLE] else {
        panic!("not a table: {:?}", abi.collections[TABLE]);
    };
    let ty = |name: &str| ValueType::parse(name).unwrap();
    assert_eq!(
        *table,
        TableSchema {
            fields: [
                ("org", ty("string")),
                ("id", ty("u64")),
                ("owner", ty("string")),
                ("email", ty("option<string>")),
                ("score", ty("i64")),
                ("tags", ty("list<string>")),
                ("avatar", ty("bytes")),
                ("ratio", ty("f32")),
                ("meta", ValueType::Other("Value".into())),
            ]
            .into_iter()
            .map(|(f, t)| (f.to_owned(), t))
            .collect(),
            key: vec!["org".into(), "id".into()],
            indexes: [("by_owner".into(), vec!["owner".into()])].into(),
            unique: [("by_email".into(), vec!["email".into()])].into(),
            counts: [("per_owner".into(), vec!["owner".into()])].into(),
        }
    );
    assert_eq!(abi.collections[MAP], CollectionSchema::Map(ValueType::Text));
    assert_eq!(
        abi.collections[BIG],
        CollectionSchema::Map(ValueType::Int {
            signed: false,
            bits: 64
        })
    );
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
    let fields = json!({ "id": "u64", "owner": "string" });
    for bad in [
        json!({ "c": "table" }),
        json!({ "c": {} }),
        json!({ "c": { "kind": "table", "fields": fields, "key": "id" } }),
        json!({ "c": { "kind": "table", "fields": fields, "key": [] } }),
        json!({ "c": { "kind": "table", "fields": fields, "key": ["id"], "indexes": { "i": [1] } } }),
        // Every record field and map value has a declared type.
        json!({ "c": { "kind": "table", "key": ["id"] } }),
        json!({ "c": { "kind": "table", "fields": ["id"], "key": ["id"] } }),
        json!({ "c": { "kind": "table", "fields": { "id": 1 }, "key": ["id"] } }),
        json!({ "c": { "kind": "table", "fields": { "id": "" }, "key": ["id"] } }),
        json!({ "c": { "kind": "unorderedMap" } }),
        json!({ "c": { "kind": "unorderedMap", "value": 1 } }),
        // A key, index, unique or count names a declared field.
        json!({ "c": { "kind": "table", "fields": fields, "key": ["nope"] } }),
        json!({ "c": { "kind": "table", "fields": fields, "key": ["id"], "counts": { "n": ["nope"] } } }),
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
                "kind": "table", "fields": { "id": "u64", "owner": "string" }, "key": ["id"],
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
            "kind": "table", "fields": { "id": "u64", "owner": "string" }, "key": ["id"],
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
        ok(host.table_insert(TABLE, &json_of(&member("o1", id, owner, None))));
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
    ok(host.table_insert(TABLE, &json_of(&a)));
    ok(host.table_insert(TABLE, &json_of(&b)));
    assert_eq!(dump(&mut host), sdk_dump(&store), "after insert");

    // The owner (index and count) and the email (unique) change.
    let a2 = member("o1", 1, "bob", Some("a2@x"));
    table.upsert(a2.clone()).unwrap();
    ok(host.table_upsert(TABLE, &json_of(&a2)));
    assert_eq!(dump(&mut host), sdk_dump(&store), "after upsert");
    // An upsert of a new key inserts.
    let c = member("o2", 1, "bob", Some("c@x"));
    table.upsert(c.clone()).unwrap();
    ok(host.table_upsert(TABLE, &json_of(&c)));
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
        Ok(Err(CollError::UnknownSet(ref n))) if n == "nope"
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
        ok(host.table_insert(TABLE, &json_of(r)));
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
        Ok(Err(CollError::UnknownSet(ref n))) if n == "nope"
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
    let value = |v: &str| text(v);
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
                        serde_json::from_str::<String>(&e.value).unwrap(),
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

/// JSON in any field order, with whitespace, a 64-bit integer as a number or a decimal
/// string, stores the bytes the SDK's guest path stores for the same record.
#[test]
fn values_are_canonicalized() {
    let store = MemStore::new();
    let mut table: Members = Table::with_store(TABLE, store.clone());
    let mut votes: Votes = UnorderedMap::with_store(MAP, store.clone());
    let mut host = call();

    let m = member("o1", 5, "ann", Some("a@x"));
    table.insert(m.clone()).unwrap();
    let mut fields: Vec<_> = object_of(&m).into_iter().collect();
    fields.reverse();
    let mut record = String::from("{ ");
    for (i, (k, v)) in fields.iter().enumerate() {
        let v = if k == "id" { json!(5) } else { v.clone() };
        record.push_str(&format!(
            "{}\n  {}: {v}",
            if i == 0 { "" } else { "," },
            text(k)
        ));
    }
    record.push_str(" }");
    assert_ne!(record, json_of(&m));
    ok(host.table_insert(TABLE, &record));

    let _ = votes
        .insert(&("ann".to_owned(), 1), &"v".to_owned())
        .unwrap();
    let _ = ok(host.map_insert(MAP, &[s("ann"), KeyPart::Int(1)], " \"v\" "));
    assert_eq!(dump(&mut host), sdk_dump(&store));
}

/// The stored record is byte for byte the guest path's (`value::to_vec`), and reads back as
/// the JSON the record's own serde writes.
#[test]
fn a_json_record_is_stored_as_the_guest_paths_bytes() {
    let m = member("o1", u64::MAX, "ann", Some("a@x"));
    let mut host = call();
    ok(host.table_insert(TABLE, &json_of(&m)));
    let key = encode_key(&[s(TABLE), s("r"), s("o1"), KeyPart::Int(u64::MAX.into())]);
    let stored = host.storage_get(&key).unwrap().unwrap();
    assert_eq!(stored, sdk_value::to_vec(&m).unwrap());
    let back = ok(host.table_get(TABLE, &[s("o1"), KeyPart::Int(u64::MAX.into())])).unwrap();
    assert_eq!(decode(&back), m);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&back).unwrap(),
        serde_json::to_value(&m).unwrap()
    );
}

/// A `u64` goes out as a decimal string and comes in as one or as an exact integer number;
/// above 2^53 (where a JavaScript number is no longer exact) it round-trips exactly.
#[test]
fn a_u64_round_trips_as_a_decimal_string() {
    let store = MemStore::new();
    let mut bigs: Bigs = UnorderedMap::with_store(BIG, store.clone());
    let mut host = call();
    let above = (1u64 << 53) + 1;
    for (who, n, sent) in [
        ("small", 7, "7".to_owned()),
        ("as_string", 7, "\"7\"".to_owned()),
        ("above", above, format!("\"{above}\"")),
        ("above_number", above, above.to_string()),
        ("max", u64::MAX, format!("\"{}\"", u64::MAX)),
    ] {
        let _ = bigs.insert(&who.to_owned(), &n).unwrap();
        assert_eq!(ok(host.map_insert(BIG, &[s(who)], &sent)), None);
        assert_eq!(
            ok(host.map_get(BIG, &[s(who)])),
            Some(format!("\"{n}\"")),
            "{who}"
        );
    }
    assert_eq!(dump(&mut host), sdk_dump(&store));
    // The old value comes back as a string too.
    assert_eq!(
        ok(host.map_insert(BIG, &[s("max")], "1")),
        Some(format!("\"{}\"", u64::MAX))
    );
    // A record's `u64` and `i64` fields likewise.
    let m = member("o1", above, "ann", None);
    let mut record = object_of(&m);
    let _ = record.insert("id".into(), json!(above));
    ok(host.table_insert(TABLE, &serde_json::Value::Object(record).to_string()));
    let back = ok(host.table_get(TABLE, &[s("o1"), KeyPart::Int(above.into())])).unwrap();
    let back: serde_json::Value = serde_json::from_str(&back).unwrap();
    assert_eq!(back["id"], json!(above.to_string()));
    assert_eq!(back["score"], json!("-3"));
    // Not a canonical decimal, a fraction, out of range or another type: refused.
    for bad in [
        "\"+1\"",
        "\"01\"",
        "\"-0\"",
        "1.5",
        "-1",
        "\"18446744073709551616\"",
        "true",
        "1e3",
    ] {
        assert!(
            matches!(
                host.map_insert(BIG, &[s("bad")], bad),
                Ok(Err(CollError::BadRecord(_)))
            ),
            "{bad}"
        );
    }
    assert_eq!(ok(host.map_get(BIG, &[s("bad")])), None);
}

/// A `BlobRef` crosses as its hex id and is stored as the SDK stores it: CBOR tag 0xCA1B
/// around the 32 bytes.
#[test]
fn a_blob_ref_is_stored_as_the_sdks_tag() {
    let id = [0xAB; 32];
    let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
    let blob = calimero_sdk::BlobRef::parse(&hex).unwrap();
    let ty = ValueType::parse("option<BlobRef>").unwrap();
    for (sent, want) in [
        (text(&hex), sdk_value::to_vec(&Some(blob)).unwrap()),
        (
            "null".to_owned(),
            sdk_value::to_vec(&None::<calimero_sdk::BlobRef>).unwrap(),
        ),
    ] {
        let value = canonical(value_in(&ty, &sent).unwrap()).unwrap();
        assert_eq!(write_value(&value).unwrap(), want, "{sent}");
    }
    assert!(value_in(&ty, &text("ABAB")).is_err());
}

#[test]
fn a_duplicate_key_is_refused() {
    let store = MemStore::new();
    let mut table: Members = Table::with_store(TABLE, store.clone());
    let mut host = call();
    let a = member("o1", 1, "ann", Some("a@x"));
    let again = member("o1", 1, "bob", Some("b@x"));
    table.insert(a.clone()).unwrap();
    ok(host.table_insert(TABLE, &json_of(&a)));
    let TableError::Duplicate { index } = table.insert(again.clone()).unwrap_err() else {
        panic!("not a duplicate");
    };
    let writes = host.write_count();
    assert_eq!(
        host.table_insert(TABLE, &json_of(&again)),
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
    ok(host.table_insert(TABLE, &json_of(&a)));
    let TableError::Duplicate { index } = table.insert(clash.clone()).unwrap_err() else {
        panic!("not a duplicate");
    };
    assert_eq!(
        index, "byEmail",
        "the SDK names the unique entry in camelCase"
    );
    let writes = host.write_count();
    assert_eq!(
        host.table_insert(TABLE, &json_of(&clash)),
        Ok(Err(CollError::Duplicate(index.clone())))
    );
    assert_eq!(
        host.table_upsert(TABLE, &json_of(&clash)),
        Ok(Err(CollError::Duplicate(index)))
    );
    assert_eq!(host.write_count(), writes, "nothing written");
    assert_eq!(dump(&mut host), sdk_dump(&store));
    // The record's own unique entry is not a clash, and a None email is never one.
    let same = member("o1", 1, "cyd", Some("a@x"));
    ok(host.table_upsert(TABLE, &json_of(&same)));
    ok(host.table_insert(TABLE, &json_of(&member("o1", 3, "x", None))));
    ok(host.table_insert(TABLE, &json_of(&member("o1", 4, "x", None))));
}

fn refused_as_bad_record(record: &str) {
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

/// A record carries exactly its declared fields, each of its type; anything else is
/// `bad-record` and writes nothing, whichever field it is.
#[test]
fn a_record_not_of_its_declared_types_is_refused() {
    let full = object_of(&member("o1", 1, "ann", None));
    let with = |field: &str, value: serde_json::Value| {
        let mut record = full.clone();
        let _ = record.insert(field.to_owned(), value);
        serde_json::Value::Object(record).to_string()
    };
    // A missing field: a key field, a plain one, an optional one (`null` is how none is
    // sent, never omitted).
    for missing in ["id", "owner", "email", "meta"] {
        let mut record = full.clone();
        let _ = record.remove(missing);
        refused_as_bad_record(&serde_json::Value::Object(record).to_string());
    }
    // A field the record does not declare.
    refused_as_bad_record(&with("extra", json!(1)));
    // A field of another type.
    for (field, bad) in [
        ("id", json!(1.5)),
        ("id", json!(-1)),
        ("id", json!("x")),
        ("org", json!(1)),
        ("org", json!([])),
        ("owner", json!({})),
        ("email", json!(5)),
        ("score", json!("1.0")),
        ("tags", json!("x")),
        ("tags", json!([1])),
        ("avatar", json!([0, 1, 2])),
        ("avatar", json!("not base64!")),
        ("ratio", json!("0.1")),
        ("ratio", json!(1e300)),
    ] {
        refused_as_bad_record(&with(field, bad));
    }
    // Not an object, not JSON.
    refused_as_bad_record("[1, 2, 3]");
    refused_as_bad_record("{");
    refused_as_bad_record("");
}

#[test]
fn an_unknown_collection_is_refused() {
    let mut host = call();
    let unknown = |a| matches!(a, Ok(Err(CollError::UnknownCollection(ref n))) if n == "nope");
    assert!(unknown(
        host.table_insert("nope", &json_of(&member("o", 1, "a", None)))
            .map(|r| r.map(|()| 0))
    ));
    assert!(unknown(
        host.table_get("nope", &[s("k")]).map(|r| r.map(|_| 0))
    ));
    assert!(unknown(
        host.map_get("nope", &[s("k")]).map(|r| r.map(|_| 0))
    ));
    assert!(unknown(
        host.map_insert("nope", &[s("k")], "1")
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
        host.map_insert(TABLE, &[s("k")], "1"),
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
    let record = json_of(&member("o1", 1, "ann", None));
    let key = [s("o1"), KeyPart::Int(1)];
    assert!(host.table_insert(TABLE, &record).is_err());
    assert!(host.table_upsert(TABLE, &record).is_err());
    assert!(host
        .table_upsert_many(TABLE, std::slice::from_ref(&record))
        .is_err());
    assert!(host.table_upsert_many(TABLE, &[]).is_err());
    assert!(host.table_remove(TABLE, &key).is_err());
    assert!(host.map_insert(MAP, &key, "\"v\"").is_err());
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
    let record = json_of(&member("o1", 1, "ann", None));
    let key = [s("o1"), KeyPart::Int(1)];
    assert!(host.table_insert(TABLE, &record).is_err());
    assert!(host.table_upsert(TABLE, &record).is_err());
    assert!(host
        .table_upsert_many(TABLE, std::slice::from_ref(&record))
        .is_err());
    assert!(host.table_get(TABLE, &key).is_err());
    assert!(host.table_remove(TABLE, &key).is_err());
    assert!(host.table_find(TABLE, None, &[], None, 1, false).is_err());
    assert!(host.table_count(TABLE, "perOwner", &[]).is_err());
    assert!(host.map_get(MAP, &key).is_err());
    assert!(host.map_insert(MAP, &key, "\"v\"").is_err());
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
        let _ = ok(host.map_insert(MAP, &[s("k"), KeyPart::Int(i.into())], "\"\""));
    }
    for i in 0..1_001u64 {
        let m = member("o", i, "ann", None);
        table.insert(m.clone()).unwrap();
        ok(host.table_insert(TABLE, &json_of(&m)));
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
    // The input (name and key parts), then one entry touched: its key and the one-byte
    // integer written.
    let input = TALLY.len() + encode_key(&[s("yes")]).len();
    assert_eq!(host.host_fuel(), collection_fuel(input + key.len() + 1, 1));
    let before = host.host_fuel();
    assert!(matches!(host.map_get("nope", &[]), Ok(Err(_))));
    assert_eq!(
        host.host_fuel(),
        before + collection_fuel("nope".len(), 0),
        "a refusal costs the base and its input"
    );
    assert!(collection_fuel(10, 2) > collection_fuel(0, 0));
    assert!(
        collection_fuel(usize::MAX, usize::MAX) == u64::MAX,
        "saturates"
    );
    let _ = GENERATION;
}

#[test]
fn an_integer_key_part_outside_s64_and_u64_is_refused() {
    // An `id` below `i64::MIN`: outside its `u64` type, and no WIT key part holds it, so
    // storing it would poison every later find or page that returns its key.
    let mut record = object_of(&member("o1", 1, "ann", None));
    let _ = record.insert("id".into(), json!("-18446744073709551616"));
    refused_as_bad_record(&serde_json::Value::Object(record).to_string());

    let mut host = call();
    for out_of_range in [i128::from(i64::MIN) - 1, i128::from(u64::MAX) + 1] {
        let key = [KeyPart::Int(out_of_range)];
        let bad = |a: &Result<Result<_, CollError>, HostDenied>| {
            matches!(a, Ok(Err(CollError::BadRecord(_))))
        };
        assert!(bad(&host
            .map_insert(MAP, &key, &text("v"))
            .map(|r| r.map(|_| ()))));
        assert!(bad(&host.tally_add(TALLY, &key, 1)));
    }
    // The edges themselves fit.
    ok(host.tally_add(TALLY, &[KeyPart::Int(i64::MIN.into())], 1));
    ok(host.tally_add(TALLY, &[KeyPart::Int(u64::MAX.into())], 1));
    assert_eq!(host.write_count(), 2, "only the two edge adds were written");
}

/// A record whose text alone passes `MAX_RUN_BYTES` could never be stored: refused before
/// it is parsed, its bytes still charged.
#[test]
fn an_oversized_record_is_refused_before_parsing() {
    let record = format!("[{}0]", "0,".repeat(1024 * 1024));
    let mut host = call();
    for answer in [
        host.table_insert(TABLE, &record),
        host.table_upsert(TABLE, &record),
    ] {
        assert!(
            matches!(answer, Ok(Err(CollError::BadRecord(ref m))) if m == &format!("a value of {} bytes, over {MAX_RUN_BYTES}", record.len())),
            "{answer:?}"
        );
    }
    let fuel = collection_fuel(TABLE.len() + record.len(), 0);
    assert_eq!(host.host_fuel(), 2 * fuel, "every input byte charged");
    let before = host.host_fuel();
    let answer = host.map_insert(MAP, &[s("k")], &record);
    assert!(
        matches!(answer, Ok(Err(CollError::BadRecord(_)))),
        "{answer:?}"
    );
    let input = MAP.len() + encode_key(&[s("k")]).len() + record.len();
    assert_eq!(host.host_fuel() - before, collection_fuel(input, 0));
    assert_eq!(host.write_count(), 0, "nothing written");
    assert_eq!(dump(&mut host), []);
}

/// Nesting past `MAX_JSON_DEPTH` or more than `MAX_JSON_ITEMS` items is refused on the
/// text, before a tree is built.
#[test]
fn a_deeply_nested_record_is_refused() {
    let bad = |record: &str, why: &str| {
        let mut host = call();
        for answer in [
            host.table_insert(TABLE, record),
            host.table_upsert(TABLE, record),
            host.map_insert(MAP, &[s("k")], record)
                .map(|r| r.map(|_| ())),
        ] {
            assert!(
                matches!(answer, Ok(Err(CollError::BadRecord(ref m))) if m.contains(why)),
                "{why}: {answer:?}"
            );
        }
        assert_eq!(host.write_count(), 0, "{why}: nothing written");
    };
    // `meta` (no declared type) nested `levels` arrays deep inside the record's object.
    let nested = |levels: usize| {
        let mut record = object_of(&member("o1", 1, "ann", None));
        let _ = record.insert("meta".into(), json!(0));
        let text = serde_json::Value::Object(record).to_string();
        text.replace(
            "\"meta\":0",
            &format!("\"meta\":{}0{}", "[".repeat(levels), "]".repeat(levels)),
        )
    };
    bad(&nested(MAX_JSON_DEPTH), "deeper");
    bad(&"[".repeat(MAX_JSON_DEPTH + 1), "deeper");
    // Brackets inside strings do not count.
    let mut host = call();
    let mut record = object_of(&member("o1", 1, "ann", None));
    let _ = record.insert("org".into(), json!("[".repeat(10 * MAX_JSON_DEPTH)));
    ok(host.table_insert(TABLE, &serde_json::Value::Object(record).to_string()));
    // One level less is a well-formed record (its `meta` is not a key field).
    ok(call().table_insert(TABLE, &nested(MAX_JSON_DEPTH - 1)));
    // More items than the cap, in few bytes each.
    let many = format!("[{}0]", "0,".repeat(MAX_JSON_ITEMS));
    bad(&many, "JSON items");
}

/// Every guest-supplied byte (names, records, keys, prefixes, cursors) is charged, also
/// when the call is refused before it is parsed or encoded; an over-long key is refused
/// from its computed length, never encoded.
#[test]
fn input_bytes_are_charged_before_parsing() {
    let mut host = call();
    let garbage = "\u{FF}".repeat(2_000);
    let before = host.host_fuel();
    assert!(matches!(
        host.table_insert(TABLE, &garbage),
        Ok(Err(CollError::BadRecord(_)))
    ));
    assert_eq!(
        host.host_fuel() - before,
        collection_fuel(TABLE.len() + garbage.len(), 0)
    );
    // An unknown collection still pays for the key it was handed.
    let key = vec![KeyPart::Bytes(vec![1; 5_000]); 3];
    assert_eq!(encoded_len(&key), encode_key(&key).len());
    let before = host.host_fuel();
    assert!(matches!(
        host.map_get("nope", &key),
        Ok(Err(CollError::UnknownCollection(_)))
    ));
    assert_eq!(
        host.host_fuel() - before,
        collection_fuel("nope".len() + encoded_len(&key), 0)
    );
    // Too many parts, or too long: refused with the exact length the key would have had.
    for key in [
        vec![KeyPart::None; 100_000],
        vec![s(&"x".repeat(2_000))],
        vec![KeyPart::Bytes(vec![0; 600])],
    ] {
        let full = [vec![s(MAP), s("m")], key.clone()].concat();
        assert_eq!(encoded_len(&full), encode_key(&full).len());
        let before = host.host_fuel();
        let answer = host.map_get(MAP, &key);
        assert_eq!(
            answer,
            Ok(Err(CollError::BadRecord(format!(
                "a key of {} bytes, over 1023",
                encoded_len(&full)
            ))))
        );
        assert_eq!(
            host.host_fuel() - before,
            collection_fuel(MAP.len() + encoded_len(&key), 0)
        );
    }
    // A cursor longer than any key's is refused before decoding; its bytes are charged.
    let cursor = "A".repeat(100_000);
    let before = host.host_fuel();
    assert_eq!(
        host.map_page(MAP, &[], Some(&cursor), 10, false),
        Ok(Err(CollError::BadCursor))
    );
    assert_eq!(
        host.host_fuel() - before,
        collection_fuel(MAP.len() + cursor.len(), 0)
    );
    assert_eq!(host.write_count(), 0);
}

/// An index entry's value is a record key of the same table, or the find refuses it
/// rather than reading whatever key it names.
#[test]
fn an_index_entry_not_holding_a_record_key_is_refused() {
    let mut host = call();
    ok(host.map_insert(MAP, &[s("secret")], &text("v")));
    let index = encode_key(&[
        s(TABLE),
        s("i"),
        s("byOwner"),
        s("ann"),
        s("o1"),
        KeyPart::Int(1),
    ]);
    let elsewhere = encode_key(&[s(MAP), s("m"), s("secret")]);
    host.storage_put(&index, &elsewhere).unwrap();
    assert_eq!(
        host.table_find(TABLE, Some("byOwner"), &[], None, 10, false),
        Ok(Err(CollError::BadRecord(
            "an index entry not holding a record key of this table".into()
        )))
    );
}

fn texts(records: &[Member]) -> Vec<String> {
    records.iter().map(json_of).collect()
}

/// A batch of new keys, existing keys and a key repeated in the batch answers the new ones
/// and leaves what the single upserts (on the host and in the SDK) leave, charged as those
/// upserts but for one base fee.
#[test]
fn table_upsert_many_stores_what_single_upserts_would() {
    let store = MemStore::new();
    let mut table: Members = Table::with_store(TABLE, store.clone());
    let sdk_many_store = MemStore::new();
    let mut sdk_many: Members = Table::with_store(TABLE, sdk_many_store.clone());
    let mut many = call();
    let mut single = call();
    let first = [
        member("o1", 1, "ann", Some("a@x")),
        member("o1", 2, "ann", None),
    ];
    for record in &first {
        table.insert(record.clone()).unwrap();
        sdk_many.insert(record.clone()).unwrap();
        ok(many.table_insert(TABLE, &json_of(record)));
        ok(single.table_insert(TABLE, &json_of(record)));
    }
    let batch = [
        // Existing: the owner (index, count) and the email (unique) move.
        member("o1", 1, "bob", Some("b@x")),
        // New, taking the email the record before gave up.
        member("o1", 3, "cyd", Some("a@x")),
        // New, then again in the same batch: counted once, the last one stays.
        member("o2", 1, "dee", Some("d@x")),
        member("o2", 1, "eve", Some("e@x")),
        // Existing, unchanged.
        member("o1", 2, "ann", None),
        // New, taking the email the repeated record gave up within the batch.
        member("o2", 2, "fay", Some("d@x")),
    ];
    let (fuel_many, fuel_single) = (many.host_fuel(), single.host_fuel());
    assert_eq!(ok(many.table_upsert_many(TABLE, &texts(&batch))), 3);
    assert_eq!(sdk_many.upsert_many(batch.clone()).unwrap(), 3);
    for record in &batch {
        ok(single.table_upsert(TABLE, &json_of(record)));
        table.upsert(record.clone()).unwrap();
    }
    assert_eq!(dump(&mut many), dump(&mut single), "as the single upserts");
    assert_eq!(dump(&mut many), sdk_dump(&store), "as the SDK");
    assert_eq!(
        dump(&mut many),
        sdk_dump(&sdk_many_store),
        "as the SDK's upsert_many"
    );
    assert_eq!(many.write_count(), single.write_count());
    let (fuel_many, fuel_single) = (
        many.host_fuel() - fuel_many,
        single.host_fuel() - fuel_single,
    );
    let n = batch.len() as u64;
    assert_eq!(
        fuel_many,
        fuel_single - (n - 1) * COLLECTION_BASE_FUEL,
        "the single upserts' charge but one base fee"
    );
    // An empty batch writes nothing and answers 0, for its base fee and the table name.
    let (before, writes) = (many.host_fuel(), many.write_count());
    assert_eq!(ok(many.table_upsert_many(TABLE, &[])), 0);
    assert_eq!(many.write_count(), writes);
    assert_eq!(many.host_fuel() - before, collection_fuel(TABLE.len(), 0));
}

/// One record not of its declared types refuses the batch: nothing of it is written, the
/// records before it included.
#[test]
fn table_upsert_many_with_a_bad_record_writes_nothing() {
    let mut host = call();
    ok(host.table_insert(TABLE, &json_of(&member("o1", 1, "ann", Some("a@x")))));
    let before = dump(&mut host);
    let writes = host.write_count();
    let mut records = texts(&[
        member("o1", 1, "bob", None),
        member("o1", 2, "cyd", Some("c@x")),
    ]);
    records.push(r#"{"org":"o1"}"#.to_owned());
    records.push(json_of(&member("o1", 3, "dee", None)));
    assert!(matches!(
        host.table_upsert_many(TABLE, &records),
        Ok(Err(CollError::BadRecord(_)))
    ));
    assert_eq!(host.write_count(), writes, "nothing written");
    assert_eq!(dump(&mut host), before);
}

/// Two records of one batch taking the same unique entry clash with each other: `duplicate`,
/// and nothing of the batch is written. So does one taking an entry another record holds.
#[test]
fn table_upsert_many_refuses_a_unique_clash_within_the_batch() {
    let mut host = call();
    ok(host.table_insert(TABLE, &json_of(&member("o1", 1, "ann", Some("a@x")))));
    let before = dump(&mut host);
    let writes = host.write_count();
    for batch in [
        [
            member("o1", 2, "bob", Some("same@x")),
            member("o1", 3, "cyd", Some("same@x")),
        ],
        [
            member("o1", 2, "bob", Some("b@x")),
            member("o1", 3, "cyd", Some("a@x")),
        ],
    ] {
        assert_eq!(
            host.table_upsert_many(TABLE, &texts(&batch)),
            Ok(Err(CollError::Duplicate("byEmail".into())))
        );
        assert_eq!(host.write_count(), writes, "nothing written");
        assert_eq!(dump(&mut host), before);
    }
    // The same key twice is not a clash with itself.
    let twice = [
        member("o1", 2, "bob", Some("b@x")),
        member("o1", 2, "bob", Some("b@x")),
    ];
    assert_eq!(ok(host.table_upsert_many(TABLE, &texts(&twice))), 1);
}

/// One batch of N records costs more than one upsert and less than N of them.
#[test]
fn a_batch_costs_less_than_its_single_upserts() {
    const N: u64 = 20;
    let records: Vec<Member> = (0..N)
        .map(|i| member("o1", i, "ann", Some(&format!("{i}@x"))))
        .collect();
    let mut singles = call();
    for record in &records {
        ok(singles.table_upsert(TABLE, &json_of(record)));
    }
    let mut one = call();
    ok(one.table_upsert(TABLE, &json_of(&records[0])));
    let mut batch = call();
    assert_eq!(ok(batch.table_upsert_many(TABLE, &texts(&records))), 20);
    let (one, singles, batch) = (one.host_fuel(), singles.host_fuel(), batch.host_fuel());
    println!("one upsert {one}; {N} single upserts {singles}; one batch of {N} {batch}");
    assert!(
        batch > one && batch < singles,
        "{one} < {batch} < {singles}"
    );
    assert_eq!(batch, singles - (N - 1) * COLLECTION_BASE_FUEL);
}

/// A batch that could not fit the run's remaining writes (each record writes its key at
/// least) is refused before any record is parsed.
#[test]
fn a_batch_past_the_write_cap_is_denied() {
    let mut host = call();
    let records = vec![String::new(); MAX_WRITES as usize + 1];
    let denied = host.table_upsert_many(TABLE, &records).unwrap_err();
    assert!(denied.0.contains("10,000 writes"), "{denied:?}");
    assert_eq!(host.write_count(), 0);
}

// --- read sets (PoC, view subscriptions) ---------------------------------------------------

/// The committed state after a call inserted `records`.
fn committed(records: &[Member]) -> BTreeMap<Vec<u8>, Vec<u8>> {
    let mut host = call();
    for r in records {
        ok(host.table_insert(TABLE, &json_of(r)));
    }
    let out = host.finish(Status::Ok, Vec::new(), Usage::default());
    out.diff
        .into_iter()
        .filter_map(|w| w.value.map(|v| (w.key, v)))
        .collect()
}

fn on(state: &BTreeMap<Vec<u8>, Vec<u8>>, kind: InvocationKind) -> HostCore {
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
        Arc::new(state.clone()),
        0,
        inv,
        Arc::new(NoDirectory),
    )
    .unwrap()
    .with_collections(&abi())
}

/// What a view that runs `read` on `state` read.
fn read_set(state: &BTreeMap<Vec<u8>, Vec<u8>>, read: impl FnOnce(&mut HostCore)) -> ReadSet {
    let mut view = on(state, InvocationKind::View).with_read_set();
    read(&mut view);
    view.finish(Status::Ok, Vec::new(), Usage::default())
        .read_set
        .unwrap()
}

/// The diff of a call on `state` that upserts `records`.
fn diff_upserting(state: &BTreeMap<Vec<u8>, Vec<u8>>, records: &[Member]) -> Vec<Write> {
    let mut host = on(state, InvocationKind::Call);
    for r in records {
        ok(host.table_upsert(TABLE, &json_of(r)));
    }
    host.finish(Status::Ok, Vec::new(), Usage::default()).diff
}

fn five() -> BTreeMap<Vec<u8>, Vec<u8>> {
    let records: Vec<Member> = (1..=5).map(|id| member("o1", id, "ann", None)).collect();
    committed(&records)
}

#[test]
fn a_point_read_is_touched_by_its_own_key_only() {
    let state = five();
    let set = read_set(&state, |v| {
        let got = ok(v.table_get(TABLE, &[s("o1"), KeyPart::Int(2)]));
        assert!(got.is_some());
    });
    assert_eq!(set.keys().len(), 1);
    assert!(set.ranges().is_empty());
    let mut changed = member("o1", 2, "ann", None);
    changed.score = 9;
    assert!(set.touches(&diff_upserting(&state, &[changed])));
    let mut other = member("o1", 3, "ann", None);
    other.score = 9;
    assert!(!set.touches(&diff_upserting(&state, &[other])));
}

/// A page cut short by `limit` read up to the entry after its last (which decides `next`):
/// a write past it changes nothing the view saw; a write inside it does.
#[test]
fn a_page_cut_short_is_bounded_at_the_last_key_read() {
    let state = five();
    let set = read_set(&state, |v| {
        let page = ok(v.table_find(TABLE, None, &[], None, 2, false));
        assert_eq!(page.items.len(), 2);
        assert!(page.next.is_some());
    });
    assert_eq!(set.ranges().len(), 1);
    let mut past = member("o1", 5, "ann", None);
    past.score = 9;
    assert!(!set.touches(&diff_upserting(&state, &[past])));
    assert!(!set.touches(&diff_upserting(&state, &[member("o1", 9, "ann", None)])));
    let mut inside = member("o1", 2, "ann", None);
    inside.score = 9;
    assert!(set.touches(&diff_upserting(&state, &[inside])));
    // A new record between the first two keys lands inside the range read.
    assert!(set.touches(&diff_upserting(&state, &[member("o0", 1, "ann", None)])));
}

/// A page that reached the end of its prefix read all of it: a record added after the last
/// one changes the answer.
#[test]
fn an_exhausted_page_covers_its_whole_prefix() {
    let state = five();
    let set = read_set(&state, |v| {
        let page = ok(v.table_find(TABLE, None, &[s("o1")], None, 10, false));
        assert_eq!(page.items.len(), 5);
        assert!(page.next.is_none());
    });
    assert!(set.touches(&diff_upserting(&state, &[member("o1", 9, "ann", None)])));
    // Another org's records are outside the prefix: their index and count entries too.
    assert!(!set.touches(&diff_upserting(&state, &[member("o2", 1, "bob", None)])));
}

#[test]
fn a_prefix_count_reads_its_count_entries() {
    let state = five();
    let all = read_set(&state, |v| {
        assert_eq!(ok(v.table_count(TABLE, "per_owner", &[])), 5);
    });
    let ann = read_set(&state, |v| {
        assert_eq!(ok(v.table_count(TABLE, "per_owner", &[s("ann")])), 5);
    });
    let bobs = diff_upserting(&state, &[member("o2", 1, "bob", None)]);
    assert!(all.touches(&bobs));
    assert!(!ann.touches(&bobs));
    let anns = diff_upserting(&state, &[member("o2", 1, "ann", None)]);
    assert!(ann.touches(&anns));
}

#[test]
fn a_run_without_the_recorder_records_nothing() {
    let state = five();
    let mut view = on(&state, InvocationKind::View);
    let _ = ok(view.table_get(TABLE, &[s("o1"), KeyPart::Int(2)]));
    let out = view.finish(Status::Ok, Vec::new(), Usage::default());
    assert!(out.read_set.is_none());
}

#[test]
fn a_raw_cursor_stopped_early_is_cut_at_its_last_key() {
    let state = five();
    for reverse in [false, true] {
        let set = read_set(&state, |v| {
            let cursor = v.range_open(b"", b"", reverse).unwrap();
            assert_eq!(v.range_next(cursor, 1).unwrap().len(), 1);
            v.range_close(cursor);
        });
        let all = dump(&mut on(&state, InvocationKind::View));
        let (first, last) = (all.first().unwrap(), all.last().unwrap());
        let (seen, unseen) = if reverse {
            (last, first)
        } else {
            (first, last)
        };
        let write = |key: &[u8]| Write {
            key: stored_key(GENERATION, key),
            value: None,
        };
        assert!(set.touches(&[write(&seen.0)]), "reverse {reverse}");
        assert!(!set.touches(&[write(&unseen.0)]), "reverse {reverse}");
    }
}
