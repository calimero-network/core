//! Test guest for `calimero:app/collections`: the same state written through the SDK's
//! `Table` (guest code over `storage`) and through the host's collection calls, plus loops
//! that meter, trap and fill the write cap.

use calimero_sdk::__private::serde_json;
use calimero_sdk::bindings::calimero::app::collections as coll;
use calimero_sdk::prelude::*;

/// A 3-field record with one index and one count: the fuel calibration's insert.
#[derive(Record)]
#[table(key = "id", index(by_owner = "owner"), count(per_owner = "owner"))]
pub struct Item {
    pub id: u64,
    pub owner: String,
    pub note: String,
}

#[app::state(version = 1)]
pub struct Coll {
    /// Written by the SDK's `Table` (guest code).
    items: Table<Item>,
    /// Written by the host's `table-insert`. The guest reaches this field and the next two
    /// only by name, through the host calls, so Rust never reads them.
    #[allow(dead_code)]
    mirror: Table<Item>,
    #[allow(dead_code)]
    marks: UnorderedMap<u64, bool>,
    #[allow(dead_code)]
    hits: Tally<String>,
}

/// JSON `true`, a map value.
const TRUE: &str = "true";

fn item(id: u32) -> Item {
    Item {
        id: u64::from(id),
        owner: "alice".to_owned(),
        note: "hello".to_owned(),
    }
}

fn soft(error: coll::CollError) -> AppError {
    AppError::with_fields("Coll", serde_json::json!({ "error": format!("{error:?}") }))
}

fn part(part: &coll::KeyPart) -> String {
    match part {
        coll::KeyPart::None => "none".to_owned(),
        coll::KeyPart::Boolean(b) => format!("bool:{b}"),
        coll::KeyPart::Int(i) => format!("int:{i}"),
        coll::KeyPart::Uint(u) => format!("uint:{u}"),
        coll::KeyPart::Text(s) => format!("text:{s}"),
        coll::KeyPart::Bytes(b) => format!("bytes:{}", b.len()),
    }
}

/// A record as the JSON the host takes.
fn json(record: &Item) -> app::Result<String> {
    serde_json::to_string(record)
        .map_err(|e| AppError::with_fields("Json", serde_json::json!({ "error": e.to_string() })))
}

fn keys(page: &coll::Page) -> Vec<String> {
    page.items
        .iter()
        .map(|e| e.key.iter().map(part).collect::<Vec<_>>().join(","))
        .collect()
}

#[app::logic]
impl Coll {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    /// Nothing: the baseline of the fuel measurements.
    pub fn noop(&mut self) -> app::Result<()> {
        Ok(())
    }

    /// `Table::insert` of record `id` through the SDK.
    pub fn sdk_insert(&mut self, id: u32) -> app::Result<()> {
        self.items.insert(item(id))?;
        Ok(())
    }

    /// The same insert through the host's `table-insert`.
    pub fn host_insert(&mut self, id: u32) -> app::Result<()> {
        coll::table_insert("mirror", &json(&item(id))?).map_err(soft)?;
        Ok(())
    }

    /// `n` host `map-insert`s of new keys.
    pub fn map_inserts(&mut self, n: u32) -> app::Result<()> {
        for i in 0..n {
            coll::map_insert("marks", &[coll::KeyPart::Int(i64::from(i))], TRUE).map_err(soft)?;
        }
        Ok(())
    }

    /// Host `tally-add`s until the run is stopped.
    pub fn tally_forever(&mut self) -> app::Result<()> {
        let key = [coll::KeyPart::Text("a".to_owned())];
        loop {
            coll::tally_add("hits", &key, 1).map_err(soft)?;
        }
    }

    /// A host `map-insert`, then a trap.
    pub fn insert_then_trap(&mut self) -> app::Result<()> {
        coll::map_insert("marks", &[coll::KeyPart::Int(1)], TRUE).map_err(soft)?;
        std::process::abort()
    }

    /// A host `tally-add`, then a trap.
    pub fn tally_then_trap(&mut self) -> app::Result<()> {
        coll::tally_add("hits", &[coll::KeyPart::Text("a".to_owned())], 1).map_err(soft)?;
        std::process::abort()
    }

    /// One host `tally-add`.
    pub fn tally_once(&mut self) -> app::Result<()> {
        coll::tally_add("hits", &[coll::KeyPart::Text("a".to_owned())], 1).map_err(soft)?;
        Ok(())
    }

    /// A record with a field of the wrong type and none of the others through
    /// `table-insert`: the host's `bad-record` message.
    pub fn bad_record(&mut self) -> app::Result<String> {
        match coll::table_insert("mirror", r#"{"id":"x"}"#) {
            Err(coll::CollError::BadRecord(why)) => Ok(why),
            other => Err(AppError::with_fields(
                "NotBadRecord",
                serde_json::json!({ "answer": format!("{other:?}") }),
            )),
        }
    }

    /// A record keyed `u64::MAX` (above 2^53) in with its `id` as a JSON number, then back
    /// out through `table-get` and `table-find`, and a map value through `map-insert` and
    /// `map-get`: the JSON texts the host answers.
    pub fn round_trip(&mut self) -> app::Result<Vec<String>> {
        let record = format!(r#"{{"note":"hi","owner":"bob","id":{}}}"#, u64::MAX);
        coll::table_insert("mirror", &record).map_err(soft)?;
        let key = [coll::KeyPart::Uint(u64::MAX)];
        let got = coll::table_get("mirror", &key).map_err(soft)?;
        let found = coll::table_find("mirror", None, &key, None, 10, false).map_err(soft)?;
        coll::map_insert("marks", &key, " false ").map_err(soft)?;
        let value = coll::map_get("marks", &key).map_err(soft)?;
        let mut out: Vec<String> = got.into_iter().collect();
        out.extend(found.items.into_iter().map(|e| e.value));
        out.extend(value);
        Ok(out)
    }

    /// Keys above `i64::MAX` come back as `uint`, the rest as `int`: a record keyed
    /// `u64::MAX` and one keyed 7 through `table-find`, map keys `uint(u64::MAX)`,
    /// `uint(7)` and `int(-5)` through `map-page`.
    pub fn big_keys(&mut self) -> app::Result<Vec<String>> {
        let mut record = item(0);
        record.id = u64::MAX;
        coll::table_insert("mirror", &json(&record)?).map_err(soft)?;
        coll::table_insert("mirror", &json(&item(7))?).map_err(soft)?;
        for key in [
            coll::KeyPart::Uint(u64::MAX),
            coll::KeyPart::Uint(7),
            coll::KeyPart::Int(-5),
        ] {
            coll::map_insert("marks", &[key], TRUE).map_err(soft)?;
        }
        let found = coll::table_find("mirror", None, &[], None, 10, false).map_err(soft)?;
        let page = coll::map_page("marks", &[], None, 10, false).map_err(soft)?;
        let mut out = keys(&found);
        out.extend(keys(&page));
        Ok(out)
    }
}
