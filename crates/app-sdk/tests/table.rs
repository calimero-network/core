//! Tables over `MemStore`: constraints, indexes, paging, counts, read cost.

use calimero_app_sdk::collections::{Counter, UnorderedMap};
use calimero_app_sdk::encoding::value;
use calimero_app_sdk::table::FieldSet;
use calimero_app_sdk::{
    FindOpts, KeyPart, MemStore, Record, StorageError, Store, Table, TableError, ToKey,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Contact {
    id: String,
    email: String,
    owner: String,
    name_key: String,
    stage: String,
}

impl Record for Contact {
    const KEY: &'static [&'static str] = &["id"];
    const UNIQUE: &'static [FieldSet] = &[("email", &["email"])];
    const INDEXES: &'static [FieldSet] = &[("byOwnerName", &["owner", "nameKey"])];
    const COUNTS: &'static [FieldSet] = &[("byStage", &["stage"])];

    fn key_parts(&self, fields: &[&str]) -> Vec<KeyPart> {
        fields
            .iter()
            .map(|f| match *f {
                "id" => KeyPart::Str(self.id.clone()),
                "email" => KeyPart::Str(self.email.clone()),
                "owner" => KeyPart::Str(self.owner.clone()),
                "nameKey" => KeyPart::Str(self.name_key.clone()),
                "stage" => KeyPart::Str(self.stage.clone()),
                other => panic!("no field {other}"),
            })
            .collect()
    }
}

fn contact(id: &str, email: &str, owner: &str, name_key: &str, stage: &str) -> Contact {
    Contact {
        id: id.into(),
        email: email.into(),
        owner: owner.into(),
        name_key: name_key.into(),
        stage: stage.into(),
    }
}

fn table() -> (Table<Contact, MemStore>, MemStore) {
    let store = MemStore::new();
    (Table::with_store("contacts", store.clone()), store)
}

fn s(v: &str) -> KeyPart {
    KeyPart::Str(v.into())
}

#[test]
fn unique_clash_is_duplicate() {
    let (mut t, _) = table();
    t.insert(contact("1", "a@x", "ada", "a", "lead")).unwrap();
    let err = t
        .insert(contact("2", "a@x", "ada", "b", "lead"))
        .unwrap_err();
    assert_eq!(
        err,
        TableError::Duplicate {
            index: "email".into()
        }
    );
    assert!(!t.contains("2").unwrap());
    let err = t
        .insert(contact("1", "z@x", "ada", "c", "lead"))
        .unwrap_err();
    assert_eq!(
        err,
        TableError::Duplicate {
            index: "key".into()
        }
    );
}

#[test]
fn find_by_prefix_pages() {
    let (mut t, _) = table();
    for i in 0..25 {
        t.insert(contact(
            &format!("{i:02}"),
            &format!("{i}@x"),
            "ada",
            &format!("n{:02}", 24 - i),
            "lead",
        ))
        .unwrap();
    }
    t.insert(contact("zz", "zz@x", "bob", "a", "lead")).unwrap();
    let prefix = [s("ada")];
    let mut seen = Vec::new();
    let mut sizes = Vec::new();
    let mut opts = FindOpts::default().limit(10);
    loop {
        let page = t.find("byOwnerName", &prefix, opts.clone()).unwrap();
        sizes.push(page.items.len());
        seen.extend(page.items.into_iter().map(|c| c.name_key));
        match page.next {
            Some(next) => opts = opts.cursor(next),
            None => break,
        }
    }
    assert_eq!(sizes, [10, 10, 5]);
    let mut sorted = seen.clone();
    sorted.sort();
    assert_eq!(seen, sorted);
    assert_eq!(seen.len(), 25);

    let rev = t
        .find(
            "byOwnerName",
            &prefix,
            FindOpts::default().limit(3).reverse(),
        )
        .unwrap();
    assert_eq!(
        rev.items
            .iter()
            .map(|c| c.name_key.as_str())
            .collect::<Vec<_>>(),
        ["n24", "n23", "n22"]
    );
    let rev2 = t
        .find(
            "byOwnerName",
            &prefix,
            FindOpts::default()
                .limit(3)
                .reverse()
                .cursor(rev.next.unwrap()),
        )
        .unwrap();
    assert_eq!(rev2.items[0].name_key, "n21");

    let ranged = t
        .find(
            "byOwnerName",
            &prefix,
            FindOpts::default().from(vec![s("n03")]).to(vec![s("n06")]),
        )
        .unwrap();
    assert_eq!(ranged.items.len(), 3);
    assert!(matches!(
        t.find("nope", &prefix, FindOpts::default()),
        Err(StorageError::UnknownName(_))
    ));
    assert!(matches!(
        t.find("byOwnerName", &prefix, FindOpts::default().cursor("!!")),
        Err(StorageError::BadCursor)
    ));
}

#[test]
fn count_is_maintained() {
    let (mut t, store) = table();
    for i in 0..3 {
        t.insert(contact(
            &i.to_string(),
            &format!("{i}@x"),
            "ada",
            "n",
            "lead",
        ))
        .unwrap();
    }
    assert_eq!(t.count("byStage", &[s("lead")]).unwrap(), 3);
    assert!(t.update("0", |c| c.stage = "won".into()).unwrap());
    assert!(t.remove("1").unwrap());
    assert_eq!(t.count("byStage", &[s("lead")]).unwrap(), 1);
    assert_eq!(t.count("byStage", &[s("won")]).unwrap(), 1);
    assert_eq!(t.count("byStage", &[]).unwrap(), 2);
    assert!(!t.remove("missing").unwrap());
    assert!(store.writes() > 0);
}

#[test]
fn update_reindexes_and_guards_key() {
    let (mut t, _) = table();
    t.insert(contact("1", "a@x", "ada", "a", "lead")).unwrap();
    t.insert(contact("2", "b@x", "ada", "b", "lead")).unwrap();
    t.update("1", |c| c.owner = "bob".into()).unwrap();
    assert_eq!(
        t.find("byOwnerName", &[s("ada")], FindOpts::default())
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        t.find("byOwnerName", &[s("bob")], FindOpts::default())
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        t.update("1", |c| c.email = "b@x".into()).unwrap_err(),
        TableError::Duplicate {
            index: "email".into()
        }
    );
    assert_eq!(
        t.update("1", |c| c.id = "9".into()).unwrap_err(),
        TableError::Storage(StorageError::KeyChanged)
    );
    t.upsert(contact("1", "c@x", "ada", "z", "won")).unwrap();
    assert_eq!(t.count("byStage", &[s("lead")]).unwrap(), 1);
    assert_eq!(t.scan(FindOpts::default()).unwrap().items.len(), 2);
    // the old unique entry is gone: the email can be reused
    t.insert(contact("3", "a@x", "ada", "q", "lead")).unwrap();
}

#[test]
fn find_reads_only_what_it_returns() {
    let (mut t, store) = table();
    for i in 0..50_000 {
        t.insert(contact(
            &format!("{i:05}"),
            &format!("{i}@x"),
            "ada",
            &format!("{i:05}"),
            "lead",
        ))
        .unwrap();
    }
    store.reset_pulled();
    let page = t
        .find("byOwnerName", &[s("ada")], FindOpts::default().limit(10))
        .unwrap();
    assert_eq!(page.items.len(), 10);
    assert!(page.next.is_some());
    assert!(store.pulled() <= 11, "pulled {}", store.pulled());
}

#[test]
fn long_keys_are_refused_before_the_store() {
    let (mut t, store) = table();
    let err = t
        .insert(contact(&"k".repeat(2000), "a@x", "ada", "n", "lead"))
        .unwrap_err();
    assert!(matches!(
        err,
        TableError::Storage(StorageError::KeyTooLong { .. })
    ));
    assert_eq!(store.writes(), 0);
}

#[test]
fn counter_add_and_increment() {
    let store = MemStore::new();
    let mut c = Counter::with_store("n", store.clone());
    assert_eq!(c.get().unwrap(), 0);
    c.add(5);
    c.add(-2);
    assert_eq!(c.get().unwrap(), 3);
    assert_eq!(c.increment().unwrap(), 4);
    let key = calimero_app_sdk::encoding::key::encode(&[s("n"), s("n")]);
    assert_eq!(value::int_from_slice(&store.get(&key).unwrap()).unwrap(), 4);
    c.add(-10);
    assert!(c.get().is_err());
}

#[test]
fn map_roundtrip_with_tuple_keys() {
    let mut m: UnorderedMap<(String, u64), String, MemStore> =
        UnorderedMap::with_store("slots", MemStore::new());
    let k = ("room".to_string(), 7u64);
    assert!(!m.contains(&k).unwrap());
    m.insert(&k, &"b1".to_string()).unwrap();
    assert_eq!(m.get(&k).unwrap().as_deref(), Some("b1"));
    m.remove(&k).unwrap();
    assert_eq!(m.get(&k).unwrap(), None);
    assert_eq!(("a".to_string(), 1u8).to_key().len(), 2);
}
