//! `#[app::state]` lists every field's kind in the ABI's `"state"`, and every field's search
//! fields (empty but for a searchable table), keys sorted, as bytes the TypeScript SDK can
//! reproduce.

use calimero_sdk::prelude::*;

#[derive(Record)]
#[table(key = "id")]
pub struct Row {
    pub id: String,
}

#[allow(dead_code, reason = "a fixture: only its ABI is under test")]
#[app::state(version = 2)]
pub struct Every {
    rows: Table<Row>,
    zeta_map: UnorderedMap<String, u32>,
    count: Counter,
    title: Cell<String>,
    tags: Set<String>,
    items: Vector<u32>,
    blobs: Frozen<String>,
    prefs: PerUser<String>,
    body: Text,
    doc: RichText,
}

#[test]
fn the_state_abi_lists_each_field_kind_in_key_order() {
    assert_eq!(
        <Every as AppState>::ABI,
        concat!(
            r#"{"collections":{"blobs":{"kind":"frozen"},"body":{"kind":"text"},"count":{"kind":"counter"},"#,
            r#""doc":{"kind":"richText"},"items":{"kind":"vector"},"prefs":{"kind":"perUser"},"#,
            r#""rows":{"counts":{},"fields":{"id":"string"},"indexes":{},"key":["id"],"kind":"table","unique":{}},"#,
            r#""tags":{"kind":"set"},"title":{"kind":"cell"},"zetaMap":{"kind":"unorderedMap","value":"u32"}},"#,
            r#""fields":{"blobs":"frozen","body":"text","count":"counter","doc":"richText","#,
            r#""items":"vector","prefs":"perUser","rows":"table","tags":"set","title":"cell","#,
            r#""zetaMap":"unorderedMap"},"search":{"blobs":[],"body":[],"count":[],"doc":[],"#,
            r#""items":[],"prefs":[],"rows":[],"tags":[],"title":[],"zetaMap":[]},"version":2}"#
        )
    );
    assert_eq!(<Every as AppState>::VERSION, 2);
}

#[derive(Record)]
#[table(key = "id", search = ["title", "body_text"])]
pub struct Note {
    pub id: String,
    pub title: String,
    pub body_text: String,
}

#[allow(dead_code, reason = "a fixture: only its ABI is under test")]
#[app::state(version = 1)]
pub struct Searchable {
    notes: Table<Note>,
    rows: Table<Row>,
}

#[test]
fn a_searchable_table_lists_its_fields_in_camel_case() {
    assert_eq!(
        <Searchable as AppState>::ABI,
        r#"{"collections":{"notes":{"counts":{},"fields":{"bodyText":"string","id":"string","title":"string"},"indexes":{},"key":["id"],"kind":"table","unique":{}},"rows":{"counts":{},"fields":{"id":"string"},"indexes":{},"key":["id"],"kind":"table","unique":{}}},"fields":{"notes":"table","rows":"table"},"search":{"notes":["title","bodyText"],"rows":[]},"version":1}"#
    );
}

#[derive(Record)]
#[table(key = ["poll_id", "voter"], count(per_poll = ["poll_id"]))]
pub struct Vote {
    pub poll_id: String,
    pub voter: String,
}

#[allow(dead_code, reason = "a fixture: only its ABI is under test")]
#[app::state(version = 1)]
pub struct Polls {
    rows: Table<Vote>,
}

#[test]
fn state_abi_lists_collection_schemas() {
    let abi: serde_json::Value = serde_json::from_str(<Polls as AppState>::ABI).unwrap();
    assert_eq!(
        abi["collections"],
        serde_json::json!({"rows": {
            "kind": "table",
            "fields": {"pollId": "string", "voter": "string"},
            "key": ["pollId", "voter"],
            "indexes": {},
            "unique": {},
            "counts": {"per_poll": ["pollId"]}
        }})
    );
    // Keys are written sorted, like the rest of the state ABI.
    assert!(<Polls as AppState>::ABI.contains(
        r#""rows":{"counts":{"per_poll":["pollId"]},"fields":{"pollId":"string","voter":"string"},"indexes":{},"key":["pollId","voter"],"kind":"table","unique":{}}"#
    ));
}

#[allow(dead_code, reason = "a fixture: only its ABI is under test")]
#[app::state(version = 1)]
pub struct Tallies {
    votes: Tally<String>,
}

#[test]
fn a_tally_is_listed_with_its_kind_only() {
    let abi = <Tallies as AppState>::ABI;
    assert!(
        abi.contains(r#""collections":{"votes":{"kind":"tally"}}"#),
        "{abi}"
    );
}

#[allow(dead_code, reason = "a fixture: only its ABI is under test")]
#[app::state(version = 1)]
pub struct Maps {
    counts: UnorderedMap<String, u64>,
    raw: UnorderedMap<String, Vec<u8>>,
    pairs: UnorderedMap<(String, u64), Option<(u64, String)>>,
    files: UnorderedMap<String, BlobRef>,
    rows: UnorderedMap<String, Row>,
}

/// A map names its value type for the host to convert values between JSON and CBOR: the
/// ABI type of `V`, with `Vec<u8>` a `list<u8>` (plain serde writes it as an array).
#[test]
fn a_map_lists_its_value_type() {
    let abi: serde_json::Value = serde_json::from_str(<Maps as AppState>::ABI).unwrap();
    assert_eq!(
        abi["collections"],
        serde_json::json!({
            "counts": { "kind": "unorderedMap", "value": "u64" },
            "files": { "kind": "unorderedMap", "value": "BlobRef" },
            "pairs": { "kind": "unorderedMap", "value": "option<tuple<u64, string>>" },
            "raw": { "kind": "unorderedMap", "value": "list<u8>" },
            "rows": { "kind": "unorderedMap", "value": "Row" },
        })
    );
}
