//! In-order path keys for [`FugueTree`].
//!
//! A path key encodes a node's position in the depth-first in-order traversal
//! as a byte string, such that **lexicographic byte comparison of path keys
//! reproduces the traversal exactly**. That is what lets document order live
//! in the entity keyspace, so an ordered read is a native range scan and no
//! derived index sits in the critical path.
//!
//! Encoding: walk the root path top-down. Each descent emits a tag byte
//! followed by the child's fixed-width big-endian id — [`LEFT_TAG`] (`0x00`)
//! for a left descent, [`RIGHT_TAG`] (`0x02`) for a right descent. The node's
//! own position is then emitted as [`SELF_TAG`] (`0x01`).
//!
//! Correctness follows from `0x00 < 0x01 < 0x02`: everything in a left subtree
//! shares the parent's prefix and continues with `0x00`, sorting before the
//! parent's own `0x01`; everything in a right subtree continues with `0x02`,
//! sorting after. Sibling order falls out of the fixed-width big-endian id,
//! whose byte order is its numeric order.
//!
//! Key length is `O(depth)`. Under Fugue's insert rule a sequential append is
//! the right child of its predecessor, so an append-only document is a right
//! spine and keys grow linearly — see `report_key_depth` in this module for
//! the measured distribution.

use super::{FugueTree, NodeId, RawId, Side};

/// Tag for a left descent. Sorts before [`SELF_TAG`].
pub const LEFT_TAG: u8 = 0x00;

/// Tag for the node's own position. Sorts between the two descent tags.
pub const SELF_TAG: u8 = 0x01;

/// Tag for a right descent. Sorts after [`SELF_TAG`].
pub const RIGHT_TAG: u8 = 0x02;

/// Bytes per descent step: one tag plus `u64` replica plus `u32` counter.
pub const STEP_LEN: usize = 1 + 8 + 4;

/// The in-order path key of `id` within `tree`.
///
/// The root's key is the single byte [`SELF_TAG`]. A node whose ancestry is
/// not fully present in `tree` (an unattached node) yields the key of the
/// deepest attached prefix, which is not a position any caller should rely on;
/// callers hold only attached nodes.
#[must_use]
pub fn path_key(tree: &FugueTree, id: NodeId) -> Vec<u8> {
    unimplemented!("fugue: path_key")
}

/// The number of edges between `id` and the root.
#[must_use]
pub fn depth(tree: &FugueTree, id: NodeId) -> usize {
    unimplemented!("fugue: depth")
}

#[cfg(test)]
mod tests {
    use super::super::{figure_3_tree, FugueTree, Rng, SeqNo};
    use super::{depth, path_key};

    /// The document text as reconstructed by sorting every live node by its
    /// path key — i.e. what a byte-ordered range scan would return.
    fn text_by_path_key(tree: &FugueTree) -> String {
        let mut live: Vec<(Vec<u8>, char)> = tree
            .nodes()
            .filter_map(|node| node.value.map(|ch| (path_key(tree, Some(node.id)), ch)))
            .collect();
        live.sort_by(|a, b| a.0.cmp(&b.0));
        live.into_iter().map(|(_, ch)| ch).collect()
    }

    /// (g) Path-key order equals traversal order on Figure 3's tree.
    #[test]
    fn path_key__reproduces_traversal_on_figure_3() {
        let tree = figure_3_tree();
        assert_eq!(tree.values(), "abcdef");
        assert_eq!(text_by_path_key(&tree), tree.values());
    }

    /// The root sorts inside the tree, not before or after everything.
    #[test]
    fn path_key__root_is_a_single_self_byte() {
        let tree = figure_3_tree();
        assert_eq!(path_key(&tree, None), vec![super::SELF_TAG]);
    }

    /// (g) Path-key order equals traversal order on randomly built trees.
    #[test]
    fn path_key__reproduces_traversal_on_random_trees() {
        for seed in 0..16_u64 {
            let mut rng = Rng::new(0xabc_0000 + seed);
            let mut tree = FugueTree::new();
            let mut seq: SeqNo = 0;
            for step in 0..200 {
                let len = tree.len();
                if len > 3 && step % 5 == 0 {
                    let _ = tree.delete(rng.below(len)).unwrap();
                } else {
                    let ch = char::from(b'a' + u8::try_from(rng.below(26)).unwrap());
                    let _ = tree.insert(rng.below(len + 1), ch, (seed, seq)).unwrap();
                    seq += 1;
                }
            }
            assert!(tree.len() > 0);
            assert_eq!(
                text_by_path_key(&tree),
                tree.values(),
                "path-key order diverged from traversal order at seed {seed}"
            );
        }
    }

    /// Measurement, not an assertion: how long do path keys actually get?
    ///
    /// Run with `cargo test -p calimero-storage report_key_depth -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore = "measurement, prints a table"]
    fn report_key_depth() {
        const N: usize = 1000;

        let workloads: [(&str, fn(usize, usize) -> usize); 3] = [
            ("1000 sequential appends", |_i, len| len),
            ("1000 inserts at index 0", |_i, _len| 0),
            ("1000 inserts at midpoint", |_i, len| len / 2),
        ];

        println!();
        println!(
            "{:<26} {:>7} {:>7} {:>7} {:>9} {:>9}",
            "workload", "d_max", "d_p50", "d_p99", "key_max", "key_p99"
        );
        for (name, position) in workloads {
            let mut tree = FugueTree::new();
            for i in 0..N {
                let at = position(i, tree.len());
                let _ = tree
                    .insert(at, 'x', (1, u32::try_from(i).unwrap()))
                    .unwrap();
            }

            let mut depths: Vec<usize> = tree
                .nodes()
                .map(|node| depth(&tree, Some(node.id)))
                .collect();
            let mut keys: Vec<usize> = tree
                .nodes()
                .map(|node| path_key(&tree, Some(node.id)).len())
                .collect();
            depths.sort_unstable();
            keys.sort_unstable();

            let pick = |v: &[usize], q: f64| v[((v.len() - 1) as f64 * q) as usize];
            println!(
                "{:<26} {:>7} {:>7} {:>7} {:>9} {:>9}",
                name,
                depths[depths.len() - 1],
                pick(&depths, 0.50),
                pick(&depths, 0.99),
                keys[keys.len() - 1],
                pick(&keys, 0.99),
            );
        }
        println!();
        println!("key length = 13 * depth + 1 bytes (tag + u64 replica + u32 counter per step)");
    }
}
