//! `#[app::state]` lists every field's kind in the ABI's `"state"`, keys sorted, as bytes
//! the TypeScript SDK can reproduce.

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
            r#""zetaMap":"unorderedMap"},"version":2}"#
        )
    );
    assert_eq!(<Every as AppState>::VERSION, 2);
}
