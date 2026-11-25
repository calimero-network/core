//! A record's fields that are not part of a key (the record key, a unique constraint, an
//! index or a count) are only stored, so they may be any value: lists, floats, nested
//! structs, maps, blob references. Only the keyed fields need `ToKeyPart`; a nested struct
//! needs `AbiType`, as a record's JSON is described in the ABI.

use std::collections::BTreeMap;

use calimero_sdk::prelude::*;
use calimero_sdk::MemStore;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, AbiType)]
pub struct Location {
    pub lat: f64,
    pub lon: f64,
}

#[derive(Record, Clone, Debug, PartialEq)]
#[table(key = "id", index(by_author = ["author"]), count(per_author = ["author"]))]
pub struct Post {
    pub id: String,
    pub author: String,
    pub tags: Vec<String>,
    pub score: f64,
    pub place: Option<Location>,
    pub votes: BTreeMap<String, bool>,
    pub cover: Option<BlobRef>,
}

#[test]
fn a_record_stores_lists_floats_structs_and_maps_beside_its_keys() {
    let mut posts: Table<Post, MemStore> = Table::with_store("posts", MemStore::new());
    let post = Post {
        id: "p1".into(),
        author: "ann".into(),
        tags: vec!["rust".into(), "wasm".into()],
        score: 4.5,
        place: Some(Location {
            lat: 46.05,
            lon: 14.5,
        }),
        votes: BTreeMap::from([("bo".to_string(), true)]),
        cover: None,
    };
    posts.insert(post.clone()).unwrap();
    assert_eq!(posts.get("p1").unwrap(), Some(post.clone()));
    let by_ann: Vec<Post> = posts
        .find_iter("byAuthor", &["ann".to_key_part()], FindOpts::default())
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(by_ann, [post]);
    assert_eq!(posts.count("perAuthor", &["ann".to_key_part()]).unwrap(), 1);
}
