//! Where does a READ's cost go as a collection grows?
//!
//! The write wall is fixed (see `write_cost_profile.rs`). This is the mirror
//! image for the read path: the chat contract's `get_messages` materialises
//! every message and *then* slices a page, so a "last 50 messages" call is
//! O(N) in total history. Gas is charged for reads too, at 1e9 points/call.
//!
//! Nothing here modifies the crate under test; it only measures it.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::address::Id;
use calimero_storage::collections::{Root, SortedMap, UnorderedMap, UnorderedSet, Vector};
use calimero_storage::store::{Key, StorageAdaptor};

type IndexKey = (Id, Vec<u8>);

thread_local! {
    static STORE: RefCell<BTreeMap<[u8; 32], Vec<u8>>> = RefCell::new(BTreeMap::new());
    static READS: RefCell<usize> = const { RefCell::new(0) };
    static WRITES: RefCell<usize> = const { RefCell::new(0) };
    static READ_BYTES: RefCell<usize> = const { RefCell::new(0) };
    static DISTINCT: RefCell<BTreeSet<[u8; 32]>> = RefCell::new(BTreeSet::new());
    static INDEX: RefCell<BTreeMap<IndexKey, Id>> = RefCell::new(BTreeMap::new());
    static INDEX_META: RefCell<BTreeMap<Id, Vec<u8>>> = RefCell::new(BTreeMap::new());
    static INDEX_EXAMINED: RefCell<usize> = const { RefCell::new(0) };
}

#[derive(Debug)]
struct Counting;

fn within_end(key: &[u8], end: &Bound<Vec<u8>>) -> bool {
    match end {
        Bound::Unbounded => true,
        Bound::Included(e) => key <= e.as_slice(),
        Bound::Excluded(e) => key < e.as_slice(),
    }
}

impl StorageAdaptor for Counting {
    fn storage_read(key: Key) -> Option<Vec<u8>> {
        let kb = key.to_bytes();
        let out = STORE.with(|s| s.borrow().get(&kb).cloned());
        READS.with(|r| *r.borrow_mut() += 1);
        DISTINCT.with(|d| {
            let _ = d.borrow_mut().insert(kb);
        });
        if let Some(v) = &out {
            READ_BYTES.with(|b| *b.borrow_mut() += v.len());
        }
        out
    }
    fn storage_write(key: Key, value: &[u8]) -> bool {
        WRITES.with(|w| *w.borrow_mut() += 1);
        let _prev = STORE.with(|s| s.borrow_mut().insert(key.to_bytes(), value.to_vec()));
        true
    }
    fn storage_remove(key: Key) -> bool {
        STORE.with(|s| s.borrow_mut().remove(&key.to_bytes()).is_some())
    }

    fn index_supported() -> bool {
        true
    }
    fn index_put(collection: Id, order_key: &[u8], entry: Id) -> bool {
        INDEX.with(|i| {
            let _ = i
                .borrow_mut()
                .insert((collection, order_key.to_vec()), entry);
        });
        true
    }
    fn index_remove(collection: Id, order_key: &[u8]) -> bool {
        INDEX.with(|i| {
            let _ = i.borrow_mut().remove(&(collection, order_key.to_vec()));
        });
        true
    }
    fn index_clear(collection: Id) -> bool {
        INDEX.with(|i| i.borrow_mut().retain(|(c, _), _| *c != collection));
        true
    }
    fn index_range(
        collection: Id,
        start: Bound<Vec<u8>>,
        end: Bound<Vec<u8>>,
        offset: usize,
        limit: Option<usize>,
    ) -> Vec<(Vec<u8>, Id)> {
        let lo: Bound<IndexKey> = match start {
            Bound::Included(s) => Bound::Included((collection, s)),
            Bound::Excluded(s) => Bound::Excluded((collection, s)),
            Bound::Unbounded => Bound::Included((collection, Vec::new())),
        };
        INDEX.with(|i| {
            let i = i.borrow();
            let walked = i
                .range((lo, Bound::Unbounded))
                .take_while(|((c, k), _)| *c == collection && within_end(k, &end))
                .map(|((_, k), e)| {
                    INDEX_EXAMINED.with(|x| *x.borrow_mut() += 1);
                    (k.clone(), *e)
                })
                .skip(offset);
            match limit {
                Some(n) => walked.take(n).collect(),
                None => walked.collect(),
            }
        })
    }
    fn index_prefix(
        collection: Id,
        prefix: &[u8],
        offset: usize,
        limit: Option<usize>,
    ) -> Vec<(Vec<u8>, Id)> {
        INDEX.with(|i| {
            let i = i.borrow();
            let walked = i
                .range((
                    Bound::Included((collection, prefix.to_vec())),
                    Bound::Unbounded,
                ))
                .take_while(|((c, k), _)| *c == collection && k.starts_with(prefix))
                .map(|((_, k), e)| {
                    INDEX_EXAMINED.with(|x| *x.borrow_mut() += 1);
                    (k.clone(), *e)
                })
                .skip(offset);
            match limit {
                Some(n) => walked.take(n).collect(),
                None => walked.collect(),
            }
        })
    }
    fn index_last(collection: Id) -> Option<(Vec<u8>, Id)> {
        INDEX.with(|i| {
            i.borrow()
                .range((
                    Bound::Included((collection, Vec::new())),
                    Bound::Excluded((collection, vec![0xff; 96])),
                ))
                .next_back()
                .map(|((_, k), e)| {
                    INDEX_EXAMINED.with(|x| *x.borrow_mut() += 1);
                    (k.clone(), *e)
                })
        })
    }
    fn index_meta_put(collection: Id, marker: &[u8]) -> bool {
        INDEX_META.with(|m| {
            let _ = m.borrow_mut().insert(collection, marker.to_vec());
        });
        true
    }
    fn index_meta_get(collection: Id) -> Option<Vec<u8>> {
        INDEX_META.with(|m| m.borrow().get(&collection).cloned())
    }
    fn index_meta_clear(collection: Id) -> bool {
        INDEX_META.with(|m| {
            let _ = m.borrow_mut().remove(&collection);
        });
        true
    }
}

fn reset() {
    READS.with(|r| *r.borrow_mut() = 0);
    WRITES.with(|w| *w.borrow_mut() = 0);
    READ_BYTES.with(|b| *b.borrow_mut() = 0);
    DISTINCT.with(|d| d.borrow_mut().clear());
    INDEX_EXAMINED.with(|x| *x.borrow_mut() = 0);
}

fn reads() -> usize {
    READS.with(|r| *r.borrow())
}
fn read_bytes() -> usize {
    READ_BYTES.with(|b| *b.borrow())
}
fn index_examined() -> usize {
    INDEX_EXAMINED.with(|x| *x.borrow())
}

const CHECKPOINTS: [usize; 5] = [250, 500, 1_000, 2_000, 4_000];
const PAGE: usize = 50;

/// A stand-in for the chat contract's `Message`: scalar fields plus the two
/// nested collections every message carries (`mentions`, `files`).
#[derive(BorshSerialize, BorshDeserialize)]
struct MsgLite {
    id: String,
    timestamp: u64,
    sender: [u8; 32],
    text: String,
    mentions: UnorderedSet<[u8; 32], Counting>,
    files: Vector<u64, Counting>,
}

fn make_msg(n: usize, with_mention: bool) -> MsgLite {
    let mut mentions = UnorderedSet::<[u8; 32], Counting>::new();
    if with_mention {
        let mut who = [0_u8; 32];
        who[0] = 7;
        let _ = mentions.insert(who);
    }
    MsgLite {
        id: format!("msg-{n:08}"),
        timestamp: 1_700_000_000_000 + n as u64,
        sender: [(n % 251) as u8; 32],
        text: format!("message body number {n} with some filler text"),
        mentions,
        files: Vector::<u64, Counting>::new(),
    }
}

/// BASELINE: what a *whole-history* materialisation costs.
///
/// This is exactly what `collect_messages_with_reactions(&self.messages, ..)`
/// does today — `messages.iter()` walks every child, loads every entry, and
/// only then does `paginate` slice the last `limit`.
#[test]
fn profile_full_materialisation_vs_page() {
    let mut v = Root::new(Vector::<MsgLite, Counting>::new);
    println!("\n=== A. get_messages TODAY: collect-all-then-slice (page = {PAGE}) ===");
    println!("      N |    reads | read KiB | reads/msg | drop to serve {PAGE}");
    let mut samples: Vec<(usize, usize)> = Vec::new();
    let mut n = 0_usize;
    for &target in &CHECKPOINTS {
        while n < target {
            let _ = v.push(make_msg(n, n % 10 == 0)).expect("push");
            n += 1;
        }
        reset();
        // Faithful shape of `collect_messages_with_reactions` + `paginate`:
        // materialise every message (incl. its nested collections), then slice.
        let mut all: Vec<(u64, String, Vec<[u8; 32]>)> = Vec::new();
        if let Ok(iter) = v.iter() {
            for m in iter {
                let mentions: Vec<[u8; 32]> =
                    m.mentions.iter().map(|i| i.collect()).unwrap_or_default();
                let _files: Vec<u64> = m.files.iter().map(|i| i.collect()).unwrap_or_default();
                all.push((m.timestamp, m.text, mentions));
            }
        }
        let total = all.len();
        let start = total.saturating_sub(PAGE);
        let page = &all[start..];
        assert_eq!(page.len(), PAGE.min(total));
        let r = reads();
        println!(
            "{target:>7} | {r:>8} | {:>8} | {:>9.1} | {:>5} materialised, {PAGE} returned",
            read_bytes() / 1024,
            r as f64 / target as f64,
            total
        );
        samples.push((target, r));
    }
    let (n0, r0) = samples[0];
    let (n1, r1) = *samples.last().expect("samples");
    println!(
        "  growth: N x{:.0} -> reads x{:.1}  (linear would be x{:.0})",
        n1 as f64 / n0 as f64,
        r1 as f64 / r0 as f64,
        n1 as f64 / n0 as f64
    );
}

/// The unread-count shape: a full scan that returns a single integer.
/// Cheaper per message than `get_messages` (no nested collections touched)
/// but still strictly linear in total history.
#[test]
fn profile_unread_count_scan() {
    let mut v = Root::new(Vector::<MsgLite, Counting>::new);
    println!("\n=== B. get_unread_count / get_unread_mentions: full scan ===");
    println!("      N |  count reads | mentions reads | reads/msg");
    let mut n = 0_usize;
    for &target in &CHECKPOINTS {
        while n < target {
            let _ = v.push(make_msg(n, n % 10 == 0)).expect("push");
            n += 1;
        }
        let last_read = 1_700_000_000_000_u64 + (target as u64) - 10;

        // get_unread_count: scalar-only scan.
        reset();
        let mut count = 0_u32;
        if let Ok(iter) = v.iter() {
            for m in iter {
                if m.timestamp <= last_read {
                    continue;
                }
                count += 1;
            }
        }
        let count_reads = reads();

        // get_unread_mentions: same scan, plus per-surviving-message nested reads.
        reset();
        let mut mentions_hits = 0_u32;
        if let Ok(iter) = v.iter() {
            for m in iter {
                if m.timestamp <= last_read {
                    continue;
                }
                if let Ok(mut it) = m.mentions.iter() {
                    if it.any(|u| u[0] == 7) {
                        mentions_hits += 1;
                    }
                }
            }
        }
        let mention_reads = reads();
        assert!(count >= mentions_hits);
        println!(
            "{target:>7} | {count_reads:>12} | {mention_reads:>14} | {:>9.1}",
            count_reads as f64 / target as f64
        );
    }
}

/// What a page of reactions costs, given `reactions: UnorderedMap<MessageId,
/// UnorderedMap<String, UnorderedSet<String>>>` — three nested levels, walked
/// per message on the page.
#[test]
fn profile_reactions_for_a_page() {
    let mut m = Root::new(
        UnorderedMap::<
            String,
            UnorderedMap<String, UnorderedSet<String, Counting>, Counting>,
            Counting,
        >::new,
    );
    println!("\n=== D. reactions for one page of {PAGE} messages ===");
    println!("      N | miss-only reads | 2-emoji x 3-users reads | reads/msg");
    let mut n = 0_usize;
    for &target in &[250_usize, 500, 1_000, 2_000] {
        while n < target {
            // Every 4th message carries reactions, so a page mixes hits/misses.
            if n % 4 == 0 {
                let mut inner =
                    UnorderedMap::<String, UnorderedSet<String, Counting>, Counting>::new();
                for emoji in ["thumbsup", "heart"] {
                    let mut users = UnorderedSet::<String, Counting>::new();
                    for u in 0..3 {
                        let _ = users.insert(format!("user-{}-{}", n % 17, u));
                    }
                    let _ = inner.insert(emoji.to_owned(), users);
                }
                let _ = m.insert(format!("msg-{n:08}"), inner).expect("insert");
            }
            n += 1;
        }

        // A page of 50 message ids that have NO reactions (pure lookup misses).
        reset();
        for i in 0..PAGE {
            let id = format!("msg-{:08}", (n - 1 - i * 4).max(1));
            let _ = m.get(&id);
        }
        let miss_reads = reads();

        // A page of 50 message ids that DO have reactions, fully expanded the
        // way `get_reactions_for_message` expands them.
        reset();
        let mut expanded = 0_usize;
        for i in 0..PAGE {
            let id = format!("msg-{:08}", ((n - 4 - i * 4) / 4) * 4);
            if let Ok(Some(inner)) = m.get(&id) {
                if let Ok(entries) = inner.entries() {
                    for (_emoji, users) in entries {
                        if let Ok(it) = users.iter() {
                            expanded += it.count();
                        }
                    }
                }
            }
        }
        let hit_reads = reads();
        println!(
            "{target:>7} | {miss_reads:>15} | {hit_reads:>23} | {:>9.1}   (users expanded: {expanded})",
            hit_reads as f64 / PAGE as f64
        );
    }
}

/// THE FIX, measured: the same page served through the ordered index.
///
/// `SortedMap` keyed by `timestamp_be ‖ id` gives ascending time order for
/// free, and `page(offset, limit)` seeks the index rather than materialising
/// the collection. Reads should be flat in N.
#[test]
fn profile_index_backed_page() {
    let mut sm = Root::new(SortedMap::<Vec<u8>, MsgLite, Counting>::new);
    println!("\n=== C. THE FIX: SortedMap::page via ordered index (page = {PAGE}) ===");
    println!("      N |    reads | idx items | reads/page-item | note");
    let mut samples: Vec<(usize, usize)> = Vec::new();
    let mut n = 0_usize;
    for &target in &CHECKPOINTS {
        while n < target {
            let mut key = Vec::with_capacity(16);
            key.extend_from_slice(&(1_700_000_000_000_u64 + n as u64).to_be_bytes());
            key.extend_from_slice(&(n as u64).to_be_bytes());
            let _ = sm.insert(key, make_msg(n, n % 10 == 0)).expect("insert");
            n += 1;
        }

        // Warm read: pays the index rebuild if the marker went stale on write.
        let _ = sm.page(0, 1).expect("warm");

        reset();
        let page = sm.page(target - PAGE, PAGE).expect("page");
        assert_eq!(page.len(), PAGE);
        let r = reads();
        println!(
            "{target:>7} | {r:>8} | {:>9} | {:>15.1} | steady-state (index warm)",
            index_examined(),
            r as f64 / PAGE as f64
        );
        samples.push((target, r));

        // Keyset ("cursor") paging: instead of offset-skipping, seek straight
        // to the caller's last-seen order key. Measures the raw index
        // primitive, which is what an offset-free contract API would use.
        let cursor = {
            let mut k = Vec::with_capacity(16);
            k.extend_from_slice(&(1_700_000_000_000_u64 + (target - PAGE) as u64).to_be_bytes());
            k.extend_from_slice(&((target - PAGE) as u64).to_be_bytes());
            k
        };
        reset();
        let hits = <Counting as StorageAdaptor>::index_range(
            INDEX.with(|i| *i.borrow().keys().next().map(|(c, _)| c).expect("indexed")),
            Bound::Excluded(cursor),
            Bound::Unbounded,
            0,
            Some(PAGE),
        );
        println!(
            "        |          |           |                 | keyset seek: {} idx items for {} hits",
            index_examined(),
            hits.len()
        );
    }
    let (n0, r0) = samples[0];
    let (n1, r1) = *samples.last().expect("samples");
    println!(
        "  growth: N x{:.0} -> reads x{:.2}  (flat would be x1.0)",
        n1 as f64 / n0 as f64,
        r1 as f64 / r0 as f64
    );
    assert!(
        r1 <= r0 * 2,
        "index-backed page must be flat in N: {r0} reads at N={n0}, {r1} at N={n1}"
    );
}

/// `deleted_messages: UnorderedSet<String>` is consulted once per message on
/// every read path. `UnorderedSet::contains` computes the entry id and then
/// asks `Collection::contains`, which goes through `children_cache()` — a FULL
/// child enumeration, cached per collection handle. So the first `contains` in
/// a call materialises the entire deleted-message set.
#[test]
fn profile_deleted_set_contains() {
    let mut s = Root::new(UnorderedSet::<String, Counting>::new);
    println!("\n=== E. deleted_messages.contains(): first call vs cached ===");
    println!("  deleted D | first contains | next 49 contains | note");
    let mut d = 0_usize;
    for &target in &[10_usize, 100, 500, 1_000, 2_000] {
        while d < target {
            let _ = s.insert(format!("msg-{d:08}"));
            d += 1;
        }
        // Fresh handle per checkpoint so the cache starts cold, as it does on
        // each contract call (state is re-fetched per invocation).
        let fresh: Root<UnorderedSet<String, Counting>, calimero_storage::store::MainStorage> =
            Root::fetch().expect("root should exist");
        reset();
        let _ = fresh.contains("msg-00000001");
        let first = reads();
        reset();
        for i in 1..50 {
            let _ = fresh.contains(&format!("msg-{i:08}"));
        }
        let rest = reads();
        println!(
            "{target:>11} | {first:>14} | {rest:>16} | {:.1} reads/deleted-entry on first touch",
            first as f64 / target as f64
        );
    }
}

/// Kills the cheapest-looking workaround: "keep the AuthoredVector, add a
/// side index of (order_key -> vector index), then `Vector::get(i)` the page".
///
/// `Vector::get` is documented O(1) — but only *given* the child-id cache.
/// Cold (as every contract call starts), `Collection::nth` goes through
/// `children_cache()`, which enumerates the entire child trie. So the first
/// positional get in a call is O(N), and an index-of-indices buys nothing.
#[test]
fn profile_cold_positional_get() {
    let mut v = Root::new(Vector::<MsgLite, Counting>::new);
    println!("\n=== F. Vector::get(i) — first (cold) get in a call vs the rest ===");
    println!("      N | cold get(N-1) | next 49 gets | note");
    let mut n = 0_usize;
    for &target in &CHECKPOINTS {
        while n < target {
            let _ = v.push(make_msg(n, false)).expect("push");
            n += 1;
        }
        let fresh: Root<Vector<MsgLite, Counting>, calimero_storage::store::MainStorage> =
            Root::fetch().expect("root should exist");
        reset();
        let _ = fresh.get(target - 1).expect("get");
        let cold = reads();
        reset();
        for i in 2..51 {
            let _ = fresh.get(target - i).expect("get");
        }
        let warm = reads();
        println!(
            "{target:>7} | {cold:>13} | {warm:>12} | cold is {:.1} reads/entry in the collection",
            cold as f64 / target as f64
        );
    }
}
