//! `#[app::state]` lists every field's kind in the ABI's `"state"`, and every field's search
//! fields (empty but for a searchable table), keys sorted, as bytes the TypeScript SDK can
//! reproduce.

use calimero_app_sdk::prelude::*;

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
            r#"{"fields":{"blobs":"frozen","body":"text","count":"counter","doc":"richText","#,
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
        r#"{"fields":{"notes":"table","rows":"table"},"search":{"notes":["title","bodyText"],"rows":[]},"version":1}"#
    );
}
