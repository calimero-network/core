//! How far does the child trie actually scale?
//!
//! DEPTH nibbles give 16^DEPTH buckets. Below that, a bucket holds ~1 entry and
//! a link is genuinely constant. Above it, buckets fill and a link pays
//! O(n / 16^DEPTH) — the bucket is read, rewritten and folded whole. This
//! measures where that turns from theory into cost.

use std::cell::RefCell;
use std::collections::BTreeMap;

use calimero_storage::address::Id;
use calimero_storage::child_trie::{ChildTrie, DEPTH};
use calimero_storage::entities::{ChildInfo, Metadata};
use calimero_storage::store::{Key, StorageAdaptor};
use sha2::{Digest, Sha256};

thread_local! {
    static STORE: RefCell<BTreeMap<[u8; 32], Vec<u8>>> = const { RefCell::new(BTreeMap::new()) };
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

    println!(
        "\nDEPTH = {DEPTH} nibbles => {} buckets",
        16_usize.pow(DEPTH as u32)
    );
    println!("        n | write B | read B | rows |  bucket occupancy");

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
        let occupancy = target as f64 / 16_f64.powi(DEPTH as i32);
        println!("{target:>9} | {wb:>7} | {rb:>6} | {rows:>4} |  {occupancy:.2} entries/bucket");
    }
}
