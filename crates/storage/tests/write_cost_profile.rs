//! Where does a write's cost actually go as a collection grows?
//!
//! The chat contract still walls, and three separate O(n) fixes moved the
//! ceiling without flattening the curve. Rather than guess at a fourth, this
//! counts storage operations per append directly.

use std::cell::RefCell;
use std::collections::BTreeMap;

use calimero_storage::collections::{Root, UnorderedMap, UnorderedSet, Vector};
use calimero_storage::store::{Key, StorageAdaptor};

thread_local! {
    static STORE: RefCell<BTreeMap<[u8; 32], Vec<u8>>> = const { RefCell::new(BTreeMap::new()) };
    static READS: RefCell<usize> = const { RefCell::new(0) };
    static WRITES: RefCell<usize> = const { RefCell::new(0) };
    static READ_BYTES: RefCell<usize> = const { RefCell::new(0) };
    static DISTINCT: RefCell<std::collections::BTreeSet<[u8; 32]>> =
        const { RefCell::new(std::collections::BTreeSet::new()) };
    static REPEATS: RefCell<BTreeMap<[u8; 32], usize>> = const { RefCell::new(BTreeMap::new()) };
    static WRITE_BYTES: RefCell<usize> = const { RefCell::new(0) };
}

#[derive(Debug)]
struct Counting;

impl StorageAdaptor for Counting {
    fn storage_read(key: Key) -> Option<Vec<u8>> {
        let kb = key.to_bytes();
        let out = STORE.with(|s| s.borrow().get(&kb).cloned());
        READS.with(|r| *r.borrow_mut() += 1);
        DISTINCT.with(|d| {
            let _ = d.borrow_mut().insert(kb);
        });
        REPEATS.with(|m| *m.borrow_mut().entry(kb).or_insert(0) += 1);
        if let Some(v) = &out {
            READ_BYTES.with(|b| *b.borrow_mut() += v.len());
        }
        out
    }
    fn storage_write(key: Key, value: &[u8]) -> bool {
        WRITES.with(|w| *w.borrow_mut() += 1);
        WRITE_BYTES.with(|b| *b.borrow_mut() += value.len());
        let _prev = STORE.with(|s| s.borrow_mut().insert(key.to_bytes(), value.to_vec()));
        true
    }
    fn storage_remove(key: Key) -> bool {
        STORE.with(|s| s.borrow_mut().remove(&key.to_bytes()).is_some())
    }
}

fn reset() {
    READS.with(|r| *r.borrow_mut() = 0);
    WRITES.with(|w| *w.borrow_mut() = 0);
    READ_BYTES.with(|b| *b.borrow_mut() = 0);
    DISTINCT.with(|d| d.borrow_mut().clear());
    REPEATS.with(|m| m.borrow_mut().clear());
    WRITE_BYTES.with(|b| *b.borrow_mut() = 0);
}

fn snapshot() -> (usize, usize, usize, usize) {
    (
        READS.with(|r| *r.borrow()),
        WRITES.with(|w| *w.borrow()),
        READ_BYTES.with(|b| *b.borrow()),
        WRITE_BYTES.with(|b| *b.borrow()),
    )
}

/// A plain append into a Vector, with nothing nested.
#[test]
fn profile_plain_vector_append() {
    let mut v = Root::new(Vector::<u64, Counting>::new);
    println!("\n--- plain Vector::push ---");
    println!("     n |  reads | writes | read B | write B");
    for n in 0..600_u64 {
        if matches!(n, 50 | 150 | 300 | 450 | 599) {
            reset();
            v.push(n).expect("push");
            let (r, w, rb, wb) = snapshot();
            println!("{n:>6} | {r:>6} | {w:>6} | {rb:>6} | {wb:>7}");
        } else {
            v.push(n).expect("push");
        }
    }
}

/// An append whose value carries nested collections, as a chat Message does.
///
/// This is the regression guard for the class of bug that has cost the most
/// time here: an O(n) READ hidden behind an innocuous call on the write path.
/// Writes were already bounded while reads grew with the collection — 7,682
/// reads for a single insert at n=599 — because a logging block asked the trie
/// to enumerate rather than count. Cost must be flat in n, not merely small.
/// The two nested collections each record carries, kept alive for the
/// duration of the profile so their rows stay in the store.
type NestedPair = (UnorderedSet<[u8; 8], Counting>, Vector<u64, Counting>);

#[test]
fn profile_append_with_nested_collections() {
    let mut v = Root::new(Vector::<u64, Counting>::new);
    let mut keep: Vec<NestedPair> = Vec::new();
    let mut samples: Vec<(u64, usize, usize, usize)> = Vec::new();
    println!("\n--- push + 2 nested collections per record ---");
    println!("     n |  reads | writes | read B | write B");
    for n in 0..600_u64 {
        let measure = matches!(n, 50 | 150 | 300 | 450 | 599);
        if measure {
            reset();
        }
        let mut set = UnorderedSet::<[u8; 8], Counting>::new();
        let _ = set.insert(n.to_be_bytes());
        let mut inner = Vector::<u64, Counting>::new();
        drop(inner.push(n));
        keep.push((set, inner));
        v.push(n).expect("push");
        if measure {
            let (r, w, rb, wb) = snapshot();
            let distinct = DISTINCT.with(|d| d.borrow().len());
            let hottest = REPEATS.with(|m| m.borrow().values().copied().max().unwrap_or(0));
            println!(
                "{n:>6} | {r:>6} | {w:>6} | {rb:>6} | {wb:>7} | distinct={distinct:>5} hottest={hottest}"
            );
            samples.push((n, r, w, distinct));
        }
    }

    let (_, first_reads, first_writes, first_distinct) = samples[0];
    for &(n, reads, writes, distinct) in &samples {
        assert!(
            reads <= first_reads * 2,
            "reads per insert must not grow with n: {first_reads} at first sample, {reads} at n={n}"
        );
        assert_eq!(
            writes, first_writes,
            "writes per insert must be constant: {first_writes} vs {writes} at n={n}"
        );
        assert!(
            distinct <= first_distinct * 2,
            "distinct keys touched per insert must not grow with n: \
             {first_distinct} at first sample, {distinct} at n={n}"
        );
    }
}

/// Inserting into a map, the shape `threads`/`reactions` use.
#[test]
fn profile_map_insert() {
    let mut m = Root::new(UnorderedMap::<[u8; 8], u64, Counting>::new);
    println!("\n--- UnorderedMap::insert ---");
    println!("     n |  reads | writes | read B | write B");
    for n in 0..600_u64 {
        if matches!(n, 50 | 150 | 300 | 450 | 599) {
            reset();
            let _ = m.insert(n.to_be_bytes(), n).expect("insert");
            let (r, w, rb, wb) = snapshot();
            println!("{n:>6} | {r:>6} | {w:>6} | {rb:>6} | {wb:>7}");
        } else {
            let _ = m.insert(n.to_be_bytes(), n).expect("insert");
        }
    }
}
