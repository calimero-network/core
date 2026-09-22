//! Nested-collection catch-up over the real sync paths. A collection a node
//! never created locally has to learn its container's own `Key::Entry` row from
//! the peer; without it the node rebuilds the container from its children's
//! ancestor chains, holds an index row with no data, and its Merkle root can
//! never match. Sessions run the production protocols over `SimStream`.

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::context::ContextId;
use calimero_storage::address::Id;
use calimero_storage::collections::crdt_meta::MergeError;
use calimero_storage::collections::rekey::{field_child_id, RekeyTarget};
use calimero_storage::collections::{
    Counter, LwwRegister, MergeStrategy, Mergeable, Root, UnorderedMap,
};
use calimero_storage::delta::clear_pending_delta;
use calimero_storage::env::take_last_artifact;
use calimero_storage::{register_rekey_if_supported, rekey_field_if_supported};

use crate::sync_sim::actions::SelectedProtocol;
use crate::sync_sim::node::SimNode;
use crate::sync_sim::protocol::{execute_hash_comparison_sync, execute_level_wise_sync};

const SYNC_ROUNDS: usize = 8; // ordered-pair passes before a group is called divergent
const TALLY_KEY: &str = "t"; // the one map key every scenario writes

// =============================================================================
// State types
// =============================================================================

/// The minimal shape that puts a lazily created collection under the context
/// root: the `tallies` map exists on every node, each `Counter` in it does not.
#[derive(BorshSerialize, BorshDeserialize, Default)]
struct NestedCounters {
    tallies: UnorderedMap<String, Counter>,
}

#[diagnostic::do_not_recommend]
impl MergeStrategy for NestedCounters {
    const DISPATCHED: bool = false;
}

impl Mergeable for NestedCounters {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.tallies.merge(&other.tallies)
    }
}

impl RekeyTarget for NestedCounters {
    fn rekey_relative_to(&mut self, parent_id: Id) {
        rekey_field_if_supported!(&mut self.tallies, field_child_id(parent_id, "tallies"));
    }
}

/// A map value with an inline field beside a nested collection: `title` lives in
/// the entry's own bytes, `hits` becomes a separate container.
#[derive(BorshSerialize, BorshDeserialize, Default)]
struct DocRecord {
    title: LwwRegister<String>,
    hits: Counter,
}

#[diagnostic::do_not_recommend]
impl MergeStrategy for DocRecord {
    const DISPATCHED: bool = false;
}

impl Mergeable for DocRecord {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.title.merge(&other.title);
        self.hits.merge(&other.hits)
    }
}

impl RekeyTarget for DocRecord {
    fn rekey_relative_to(&mut self, parent_id: Id) {
        rekey_field_if_supported!(&mut self.title, field_child_id(parent_id, "title"));
        rekey_field_if_supported!(&mut self.hits, field_child_id(parent_id, "hits"));
    }
}

#[derive(BorshSerialize, BorshDeserialize, Default)]
struct InlineDocs {
    docs: UnorderedMap<String, DocRecord>,
}

#[diagnostic::do_not_recommend]
impl MergeStrategy for InlineDocs {
    const DISPATCHED: bool = false;
}

impl Mergeable for InlineDocs {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.docs.merge(&other.docs)
    }
}

impl RekeyTarget for InlineDocs {
    fn rekey_relative_to(&mut self, parent_id: Id) {
        rekey_field_if_supported!(&mut self.docs, field_child_id(parent_id, "docs"));
    }

    fn register_nested_value_types() {
        register_rekey_if_supported!(DocRecord);
    }
}

/// A map whose values themselves hold a map, so the first writer creates two
/// levels of container the receiver has never seen.
#[derive(BorshSerialize, BorshDeserialize, Default)]
struct Teams {
    teams: UnorderedMap<String, NestedCounters>,
}

#[diagnostic::do_not_recommend]
impl MergeStrategy for Teams {
    const DISPATCHED: bool = false;
}

impl Mergeable for Teams {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.teams.merge(&other.teams)
    }
}

impl RekeyTarget for Teams {
    fn rekey_relative_to(&mut self, parent_id: Id) {
        rekey_field_if_supported!(&mut self.teams, field_child_id(parent_id, "teams"));
    }

    fn register_nested_value_types() {
        register_rekey_if_supported!(NestedCounters);
    }
}

// =============================================================================
// Harness
// =============================================================================

fn context() -> ContextId {
    ContextId::from(SimNode::DEFAULT_CONTEXT_ID)
}

/// Materialise the app root the way production `init` does, re-keying the
/// top-level field so every replica addresses the same container.
fn seed<T: BorshSerialize + BorshDeserialize + Default + RekeyTarget + 'static>(
    node: &SimNode,
    reassign: impl FnOnce(&mut T),
) {
    register_rekey_if_supported!(T);
    clear_pending_delta();
    node.storage().with_index(|| {
        Root::new(|| {
            let mut state = T::default();
            reassign(&mut state);
            state
        })
        .commit();
        let _ignored = take_last_artifact();
    });
}

fn edit<T: BorshSerialize + BorshDeserialize>(node: &SimNode, f: impl FnOnce(&mut Root<T>)) {
    clear_pending_delta();
    node.storage().with_index(|| {
        let mut state = Root::<T>::fetch().expect("state root should exist");
        f(&mut state);
        state.commit();
        // Dropped on purpose: the joiner must learn this write from a sync
        // session, not from a delta.
        let _ignored = take_last_artifact();
    });
}

fn read<T: BorshSerialize + BorshDeserialize, R>(
    node: &SimNode,
    f: impl FnOnce(&Root<T>) -> R,
) -> R {
    node.storage()
        .with_index(|| f(&Root::<T>::fetch().expect("state root should exist")))
}

/// `i` runs a HashComparison session against `j`. The protocol is bidirectional,
/// so both ends hold the union afterwards.
async fn pull(nodes: &mut [SimNode], i: usize, j: usize) {
    if i == j {
        return;
    }
    if i < j {
        let (left, right) = nodes.split_at_mut(j);
        execute_hash_comparison_sync(&mut left[i], &right[0])
            .await
            .expect("hash-comparison sync should succeed");
    } else {
        let (left, right) = nodes.split_at_mut(i);
        execute_hash_comparison_sync(&mut right[0], &left[j])
            .await
            .expect("hash-comparison sync should succeed");
    }
}

async fn converge_pair(nodes: &mut [SimNode]) -> bool {
    for _ in 0..SYNC_ROUNDS {
        if nodes[0].root_hash() == nodes[1].root_hash() {
            return true;
        }
        pull(nodes, 1, 0).await;
        pull(nodes, 0, 1).await;
    }
    nodes[0].root_hash() == nodes[1].root_hash()
}

/// [`pull`]'s level-wise twin. Level-wise is initiator-pull, so a caller that
/// wants both ends repaired has to run it in both directions.
async fn level_pull(nodes: &mut [SimNode], i: usize, j: usize) {
    if i == j {
        return;
    }
    if i < j {
        let (left, right) = nodes.split_at_mut(j);
        execute_level_wise_sync(&mut left[i], &right[0])
            .await
            .expect("level-wise sync should succeed");
    } else {
        let (left, right) = nodes.split_at_mut(i);
        execute_level_wise_sync(&mut right[0], &left[j])
            .await
            .expect("level-wise sync should succeed");
    }
}

async fn converge_pair_level_wise(nodes: &mut [SimNode]) -> bool {
    for _ in 0..SYNC_ROUNDS {
        if nodes[0].root_hash() == nodes[1].root_hash() {
            return true;
        }
        level_pull(nodes, 1, 0).await;
        level_pull(nodes, 0, 1).await;
    }
    nodes[0].root_hash() == nodes[1].root_hash()
}

/// Every entity hanging directly off the context root. A lazily created
/// collection shows up here the moment the node learns it, which is how a test
/// names one without recomputing the SDK's id derivation.
fn root_children(node: &SimNode) -> Vec<Id> {
    node.storage()
        .get_children(node.storage().root_id())
        .into_iter()
        .map(|child| child.id())
        .collect()
}

/// An entity the Merkle index lists but whose `Key::Entry` row is absent hashes
/// off a stale `own_hash` and is dropped from any snapshot this node serves.
fn assert_every_indexed_entity_has_data(label: &str, node: &SimNode) {
    let mut pending = vec![node.storage().root_id()];
    while let Some(id) = pending.pop() {
        assert!(
            node.storage().get_entity_data(id).is_some(),
            "{label}: {} holds an index entry for {id} with no entity data row",
            node.id()
        );
        pending.extend(node.storage().get_children(id).into_iter().map(|c| c.id()));
    }
}

/// Every entity's id, own hash and stored bytes, so two nodes can be compared
/// for more than root-hash equality.
fn state_fingerprint(node: &SimNode) -> Vec<(Id, [u8; 32], Vec<u8>)> {
    let mut pending = vec![node.storage().root_id()];
    let mut out = Vec::new();
    while let Some(id) = pending.pop() {
        let own = node
            .storage()
            .get_hashes(id)
            .map_or([0u8; 32], |(_full, own)| own);
        out.push((
            id,
            own,
            node.storage().get_entity_data(id).unwrap_or_default(),
        ));
        pending.extend(node.storage().get_children(id).into_iter().map(|c| c.id()));
    }
    out.sort_by_key(|(id, _, _)| *id);
    out
}

/// An author holding one nested `Counter` and a peer that only ever receives.
fn nested_counter_pair(names: [&str; 2]) -> Vec<SimNode> {
    let nodes: Vec<SimNode> = names
        .into_iter()
        .map(|name| SimNode::new_in_context(name, context()))
        .collect();
    for node in &nodes {
        seed::<NestedCounters>(node, |state| {
            state.tallies.reassign_deterministic_id("tallies");
        });
    }
    edit::<NestedCounters>(&nodes[0], |app| {
        let mut tally = app
            .tallies
            .entry(TALLY_KEY.to_owned())
            .expect("entry")
            .or_default()
            .expect("or_default");
        tally.increment().expect("increment");
    });
    nodes
}

fn tally_value(node: &SimNode, key: &str) -> u64 {
    read::<NestedCounters, _>(node, |app| {
        app.tallies
            .get(key)
            .expect("map read should succeed")
            .map_or(0, |tally| tally.value().expect("counter value"))
    })
}

/// An author holding a map value with an inline field beside a nested
/// collection, and a peer that only ever receives.
fn inline_docs_pair(names: [&str; 2]) -> Vec<SimNode> {
    let nodes: Vec<SimNode> = names
        .into_iter()
        .map(|name| SimNode::new_in_context(name, context()))
        .collect();
    for node in &nodes {
        seed::<InlineDocs>(node, |state| {
            state.docs.reassign_deterministic_id("docs");
        });
    }
    edit::<InlineDocs>(&nodes[0], |app| {
        let mut doc = app
            .docs
            .entry("notes".to_owned())
            .expect("entry")
            .or_default()
            .expect("or_default");
        doc.title.set("a title only the author typed".to_owned());
        doc.hits.increment().expect("increment");
    });
    nodes
}

fn doc_fields(node: &SimNode) -> (String, u64) {
    read::<InlineDocs, _>(node, |app| {
        let doc = app
            .docs
            .get("notes")
            .expect("map read should succeed")
            .expect("the document should have arrived");
        (doc.title.get().clone(), doc.hits.value().expect("value"))
    })
}

/// An author holding a map inside a map value, so the first writer creates two
/// levels of container the peer has never seen.
fn teams_pair(names: [&str; 2]) -> Vec<SimNode> {
    let nodes: Vec<SimNode> = names
        .into_iter()
        .map(|name| SimNode::new_in_context(name, context()))
        .collect();
    for node in &nodes {
        seed::<Teams>(node, |state| {
            state.teams.reassign_deterministic_id("teams");
        });
    }
    edit::<Teams>(&nodes[0], |app| {
        let mut team = app
            .teams
            .entry("platform".to_owned())
            .expect("entry")
            .or_default()
            .expect("or_default");
        let mut tally = team
            .tallies
            .entry(TALLY_KEY.to_owned())
            .expect("entry")
            .or_default()
            .expect("or_default");
        tally.increment().expect("increment");
    });
    nodes
}

fn team_tally(node: &SimNode) -> u64 {
    read::<Teams, _>(node, |app| {
        app.teams
            .get("platform")
            .expect("map read should succeed")
            .expect("the team should have arrived")
            .tallies
            .get(TALLY_KEY)
            .expect("nested map read should succeed")
            .map_or(0, |tally| tally.value().expect("counter value"))
    })
}

// =============================================================================
// Tests
// =============================================================================

/// The minimal case: a `Counter` under a map, learned entirely from a peer.
#[tokio::test]
async fn nested_collection_container_row_reaches_a_hash_comparison_joiner() {
    let mut nodes = nested_counter_pair(["nc-author", "nc-joiner"]);

    assert!(
        converge_pair(&mut nodes).await,
        "the joiner never reached the author's Merkle root"
    );
    assert_every_indexed_entity_has_data("nested counter", &nodes[1]);
    assert_eq!(tally_value(&nodes[1], TALLY_KEY), 1);
}

/// The joiner pulls first, so it materialises the container from the leaves'
/// ancestor chain before anyone pushes the container's own row. From then on
/// both sides hold the same children, so nothing but an explicit repair of the
/// container row can close the gap.
#[tokio::test]
async fn nested_collection_container_row_reaches_a_joiner_that_pulled_first() {
    let mut nodes = nested_counter_pair(["pf-author", "pf-joiner"]);

    pull(&mut nodes, 1, 0).await;
    assert!(
        converge_pair(&mut nodes).await,
        "the joiner that pulled first never reached the author's Merkle root"
    );
    assert_every_indexed_entity_has_data("pull-first", &nodes[1]);
    assert_eq!(tally_value(&nodes[1], TALLY_KEY), 1);
}

/// A map value that holds both an inline field and a nested collection: the
/// inline bytes live in the entry's own row, so they must survive the same
/// catch-up that carries the nested collection.
#[tokio::test]
async fn inline_field_beside_a_nested_collection_reaches_a_joiner() {
    let mut nodes = inline_docs_pair(["if-author", "if-joiner"]);

    pull(&mut nodes, 1, 0).await;
    assert!(
        converge_pair(&mut nodes).await,
        "the joiner never reached the author's Merkle root"
    );
    assert_every_indexed_entity_has_data("inline field", &nodes[1]);
    assert_eq!(
        doc_fields(&nodes[1]),
        ("a title only the author typed".to_owned(), 1)
    );
}

/// Two levels the receiver never created: a per-team map inside the `teams`
/// map, and a `Counter` inside that.
#[tokio::test]
async fn two_levels_of_nesting_reach_a_joiner() {
    let mut nodes = teams_pair(["tl-author", "tl-joiner"]);

    assert!(
        converge_pair(&mut nodes).await,
        "the joiner never reached the author's Merkle root"
    );
    assert_every_indexed_entity_has_data("two levels", &nodes[1]);
    assert_eq!(team_tally(&nodes[1]), 1);
}

/// Re-applying a container row must not move anything: a duplicated session has
/// to land the joiner on the author's tree byte for byte, and a further session
/// after convergence has to leave it there.
#[tokio::test]
async fn repeated_and_duplicate_sessions_are_idempotent() {
    let mut nodes = nested_counter_pair(["dup-author", "dup-joiner"]);
    pull(&mut nodes, 1, 0).await;
    pull(&mut nodes, 1, 0).await;
    assert!(
        converge_pair(&mut nodes).await,
        "the pair that ran a duplicate session never converged"
    );
    let settled = state_fingerprint(&nodes[0]);
    assert_eq!(
        state_fingerprint(&nodes[1]),
        settled,
        "a duplicated session left the joiner on a different tree than the author"
    );

    pull(&mut nodes, 1, 0).await;
    pull(&mut nodes, 0, 1).await;
    assert_eq!(
        state_fingerprint(&nodes[1]),
        settled,
        "a session after convergence changed the joiner's tree"
    );
    assert_eq!(
        state_fingerprint(&nodes[0]),
        settled,
        "a session after convergence changed the author's tree"
    );
}

/// The level-wise twin of the minimal case. A `LevelNode` carries a container's
/// own row alongside `has_children`, so the joiner applies the row and still
/// descends instead of choosing between the two.
#[tokio::test]
async fn nested_collection_container_row_reaches_a_level_wise_joiner() {
    let mut nodes = nested_counter_pair(["lw-author", "lw-joiner"]);

    for _ in 0..SYNC_ROUNDS {
        if nodes[0].root_hash() == nodes[1].root_hash() {
            break;
        }
        level_pull(&mut nodes, 1, 0).await;
    }
    assert_eq!(
        nodes[1].root_hash(),
        nodes[0].root_hash(),
        "the level-wise joiner never reached the author's Merkle root"
    );
    assert_every_indexed_entity_has_data("level-wise", &nodes[1]);
    assert_eq!(tally_value(&nodes[1], TALLY_KEY), 1);
}

/// The level-wise twin of the inline-field case: the entry's own bytes must
/// survive the same catch-up that carries the nested collection.
#[tokio::test]
async fn inline_field_beside_a_nested_collection_reaches_a_level_wise_joiner() {
    let mut nodes = inline_docs_pair(["lwif-author", "lwif-joiner"]);

    assert!(
        converge_pair_level_wise(&mut nodes).await,
        "the level-wise joiner never reached the author's Merkle root"
    );
    assert_every_indexed_entity_has_data("level-wise inline field", &nodes[1]);
    assert_eq!(
        doc_fields(&nodes[1]),
        ("a title only the author typed".to_owned(), 1)
    );
}

/// The level-wise twin of the two-level case, which is also what proves the
/// initiator still descends through a container it just applied a row for.
#[tokio::test]
async fn two_levels_of_nesting_reach_a_level_wise_joiner() {
    let mut nodes = teams_pair(["lwtl-author", "lwtl-joiner"]);

    assert!(
        converge_pair_level_wise(&mut nodes).await,
        "the level-wise joiner never reached the author's Merkle root"
    );
    assert_every_indexed_entity_has_data("level-wise two levels", &nodes[1]);
    assert_eq!(team_tally(&nodes[1]), 1);
}

/// Re-applying a container row over level-wise must not move anything, and a
/// further session after convergence has to leave both trees where they are.
#[tokio::test]
async fn repeated_level_wise_sessions_are_idempotent() {
    let mut nodes = nested_counter_pair(["lwdup-author", "lwdup-joiner"]);
    level_pull(&mut nodes, 1, 0).await;
    level_pull(&mut nodes, 1, 0).await;
    assert!(
        converge_pair_level_wise(&mut nodes).await,
        "the pair that ran a duplicate level-wise session never converged"
    );
    let settled = state_fingerprint(&nodes[0]);
    assert_eq!(
        state_fingerprint(&nodes[1]),
        settled,
        "a duplicated level-wise session left the joiner on a different tree"
    );

    level_pull(&mut nodes, 1, 0).await;
    level_pull(&mut nodes, 0, 1).await;
    assert_eq!(
        state_fingerprint(&nodes[1]),
        settled,
        "a level-wise session after convergence changed the joiner's tree"
    );
    assert_eq!(
        state_fingerprint(&nodes[0]),
        settled,
        "a level-wise session after convergence changed the author's tree"
    );
}

/// Level-wise is otherwise initiator-pull, so a peer holding a container index
/// with no bytes - what a leaf-only apply path leaves behind - can never ask for
/// the row it is missing. The holder has to push it back.
#[tokio::test]
async fn level_wise_pushes_a_container_row_back_to_the_peer_that_lost_it() {
    let mut nodes = nested_counter_pair(["lwpb-author", "lwpb-joiner"]);
    let seeded = root_children(&nodes[1]);
    assert!(
        converge_pair_level_wise(&mut nodes).await,
        "the level-wise joiner never reached the author's Merkle root"
    );

    // The `Counter` container the joiner learned from the author, reduced to the
    // index-without-bytes state.
    let nested = *root_children(&nodes[1])
        .iter()
        .find(|id| !seeded.contains(id))
        .expect("the joiner should hold the author's nested container");
    nodes[1].storage().drop_entity_data(nested);

    // Move the author past the peer so the container's hashes disagree; nothing
    // the peer can request would hand it the bytes.
    edit::<NestedCounters>(&nodes[0], |app| {
        let mut tally = app
            .tallies
            .entry(TALLY_KEY.to_owned())
            .expect("entry")
            .or_default()
            .expect("or_default");
        tally.increment().expect("increment");
    });
    level_pull(&mut nodes, 0, 1).await;

    assert!(
        nodes[1].storage().get_entity_data(nested).is_some(),
        "the initiator did not push its container row to the peer that lost it"
    );
    assert!(
        converge_pair_level_wise(&mut nodes).await,
        "the repaired pair never converged"
    );
    assert_every_indexed_entity_has_data("level-wise push-back", &nodes[1]);
    assert_eq!(tally_value(&nodes[1], TALLY_KEY), 2);
}

/// The wide shallow shape level-wise exists for, driven through the production
/// `select_protocol` rather than by calling the protocol directly: a peer that
/// fell a few collections behind on a tree of depth 2 with many root children.
#[tokio::test]
async fn level_wise_is_the_selected_protocol_for_a_wide_shallow_tree() {
    const SHARED_TALLIES: usize = 20;
    const NEW_TALLIES: usize = 4;

    let mut nodes = nested_counter_pair(["ws-author", "ws-laggard"]);
    for i in 0..SHARED_TALLIES {
        edit::<NestedCounters>(&nodes[0], |app| {
            let mut tally = app
                .tallies
                .entry(format!("shared-{i}"))
                .expect("entry")
                .or_default()
                .expect("or_default");
            tally.increment().expect("increment");
        });
    }
    assert!(
        converge_pair(&mut nodes).await,
        "the laggard never caught up on the shared base"
    );

    for i in 0..NEW_TALLIES {
        edit::<NestedCounters>(&nodes[0], |app| {
            let mut tally = app
                .tallies
                .entry(format!("new-{i}"))
                .expect("entry")
                .or_default()
                .expect("or_default");
            tally.increment().expect("increment");
        });
    }
    for node in &mut nodes {
        node.has_state = true;
    }

    let author = nodes[0].build_handshake();
    let (protocol, reason) = nodes[1].select_protocol_for_sync(&author);
    assert!(
        matches!(protocol, SelectedProtocol::LevelWise { .. }),
        "expected level-wise for {} entities at depth {}, got {protocol:?} ({reason})",
        author.entity_count,
        author.max_depth
    );

    assert!(
        converge_pair_level_wise(&mut nodes).await,
        "the laggard never reached the author's Merkle root"
    );
    assert_every_indexed_entity_has_data("wide shallow", &nodes[1]);
    assert_eq!(tally_value(&nodes[1], "new-0"), 1);
    assert_eq!(tally_value(&nodes[1], "shared-0"), 1);
}

/// A pair does not get to pick one protocol for life: whichever ran first, the
/// other has to finish on the same tree and then leave it alone.
#[tokio::test]
async fn the_two_protocols_mix_in_either_order() {
    for (label, hash_comparison_first) in [("hc-then-lw", true), ("lw-then-hc", false)] {
        let mut nodes = nested_counter_pair([&format!("{label}-author"), &format!("{label}-peer")]);

        if hash_comparison_first {
            pull(&mut nodes, 1, 0).await;
            assert!(
                converge_pair_level_wise(&mut nodes).await,
                "{label}: the pair never converged"
            );
        } else {
            level_pull(&mut nodes, 1, 0).await;
            assert!(
                converge_pair(&mut nodes).await,
                "{label}: the pair never converged"
            );
        }
        assert_every_indexed_entity_has_data(label, &nodes[1]);
        assert_eq!(tally_value(&nodes[1], TALLY_KEY), 1);

        let settled = state_fingerprint(&nodes[0]);
        assert_eq!(
            state_fingerprint(&nodes[1]),
            settled,
            "{label}: trees differ"
        );

        pull(&mut nodes, 1, 0).await;
        level_pull(&mut nodes, 1, 0).await;
        pull(&mut nodes, 0, 1).await;
        level_pull(&mut nodes, 0, 1).await;
        assert_eq!(
            state_fingerprint(&nodes[1]),
            settled,
            "{label}: a session after convergence changed the peer's tree"
        );
        assert_eq!(
            state_fingerprint(&nodes[0]),
            settled,
            "{label}: a session after convergence changed the author's tree"
        );
    }
}
