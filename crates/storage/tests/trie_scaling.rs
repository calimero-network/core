//! How far does the child trie actually scale?
//!
//! The trie splits a subtree once it holds more than BUCKET_MAX children, so a
//! link touches about log16(n / BUCKET_MAX) + 1 rows and a bucket never holds
//! more than BUCKET_MAX entries. This measures what that costs as n grows, and
//! how many rows the whole trie takes.

use std::cell::RefCell;
use std::collections::BTreeMap;

use calimero_storage::address::Id;
use calimero_storage::child_trie::{ChildTrie, BUCKET_MAX};
use calimero_storage::entities::{ChildInfo, Metadata};
use calimero_storage::store::{Key, StorageAdaptor};
use sha2::{Digest, Sha256};

thread_local! {
    static STORE: RefCell<BTreeMap<[u8; calimero_storage::store::KEY_LEN], Vec<u8>>> = const { RefCell::new(BTreeMap::new()) };
    static WRITE_BYTES: RefCell<usize> = const { RefCell::new(0) };
    static READ_BYTES: RefCell<usize> = const { RefCell::new(0) };
    static ROWS: RefCell<usize> = const { RefCell::new(0) };
}

#[derive(Debug)]
struct Counting;

impl StorageAdaptor for Counting {
    fn storage_read(key: Key) -> Option<Vec<u8>> {
        let out = STORE.with(|s| s.borrow().get(&key.to_bytes()).cloned());
        if let Some(v) = &out {
            READ_BYTES.with(|b| *b.borrow_mut() += v.len());
        }
        out
    }
    fn storage_write(key: Key, value: &[u8]) -> bool {
        WRITE_BYTES.with(|b| *b.borrow_mut() += value.len());
        ROWS.with(|r| *r.borrow_mut() += 1);
        let _prev = STORE.with(|s| s.borrow_mut().insert(key.to_bytes(), value.to_vec()));
        true
    }
    fn storage_remove(key: Key) -> bool {
        STORE.with(|s| s.borrow_mut().remove(&key.to_bytes()).is_some())
    }
}

#[test]
#[ignore = "scaling probe: run explicitly, takes minutes at 1M"]
fn how_far_does_a_single_parent_scale() {
    let parent = Id::new(Sha256::digest(b"scale").into());
    let trie = ChildTrie::<Counting>::new(parent);

    println!("\nBUCKET_MAX = {BUCKET_MAX}");
    println!("        n | write B | read B | rows written | trie rows per child");

    let checkpoints = [1_000_usize, 10_000, 100_000, 1_000_000];
    let mut next = 0_usize;
    for target in checkpoints {
        while next < target {
            let id = Id::new(Sha256::digest(next.to_be_bytes()).into());
            let _root = trie.insert(ChildInfo::new(id, [1; 32], Metadata::default()));
            next += 1;
        }
        // Measure the very next insert in isolation.
        WRITE_BYTES.with(|b| *b.borrow_mut() = 0);
        READ_BYTES.with(|b| *b.borrow_mut() = 0);
        ROWS.with(|r| *r.borrow_mut() = 0);
        let id = Id::new(Sha256::digest(next.to_be_bytes()).into());
        let _root = trie.insert(ChildInfo::new(id, [2; 32], Metadata::default()));
        next += 1;

        let wb = WRITE_BYTES.with(|b| *b.borrow());
        let rb = READ_BYTES.with(|b| *b.borrow());
        let rows = ROWS.with(|r| *r.borrow());
        let total_rows = STORE.with(|s| s.borrow().len());
        let per_child = total_rows as f64 / next as f64;
        println!("{target:>9} | {wb:>7} | {rb:>6} | {rows:>12} | {per_child:.3}");
    }
}
