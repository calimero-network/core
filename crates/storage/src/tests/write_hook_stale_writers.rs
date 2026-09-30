//! Two writers of one `Shared` cell that stamp the same nonce on different bytes
//! must converge whatever order the writes arrive in. This case is purely about a
//! storage-internal invariant, so it lives here and not with the DAG tests in the node crate.

use ed25519_dalek::SigningKey;

use std::collections::BTreeSet;

use calimero_account::AccountId;

use crate::address::Id;
use crate::entities::{ChildInfo, Metadata};
use crate::index::Index;
use crate::interface::{ApplyContext, Interface};
use crate::store::{MockedStorage, StorageAdaptor};
use crate::tests::common::{account_of_key, build_signed_shared_action, cell_at};

type S<const SCOPE: usize> = MockedStorage<SCOPE>;

fn make_signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn hlc_at(step: u64) -> u64 {
    crate::env::time_now().saturating_add(step.saturating_mul(1_000_000_000))
}

fn setup_root<S: StorageAdaptor>() -> ChildInfo {
    let root_id = Id::root();
    let root_meta = Metadata::default();
    Index::<S>::add_root(ChildInfo::new(root_id, [0; 32], root_meta.clone())).unwrap();
    // Fetch the post-`add_root` full_hash so the returned `ChildInfo`'s
    // merkle_hash matches what the apply path's `verify_ancestor_integrity`
    // will read from the index. Without this, every test using `setup_root`
    // as an ancestor would fail with `TreeStateMismatch`.
    let (full_hash, _) = Index::<S>::get_hashes_for(root_id).unwrap().unwrap();
    ChildInfo::new(root_id, full_hash, root_meta)
}

/// Apply context with no `effective_writers` (the verifier falls through to the stored writers).
///
/// Takes the signing key so `signer_account` names the account that key speaks for: the node
/// resolves that at the delta's cut in production, and a context without it refuses every signed
/// `Shared` action. Passing the key rather than the account keeps the two from drifting apart at
/// a call site.
fn ctx(signer_sk: &SigningKey) -> ApplyContext {
    ApplyContext {
        effective_writers: None,
        signer_account: Some(account_of_key(signer_sk)),
    }
}

/// Regression: two distinct writers in the same `Shared` writer set each
/// write a *different* value with the *same* nonce. Both nodes must converge
/// to the SAME value regardless of the order the two writes arrive in.
///
/// Reproduces the intermittent `shared-storage` e2e split-brain ("Wait for
/// post-rotation value to sync" — job 78652650934): after the writer set
/// rotates from node-1 to node-2, node-2's post-rotation write was assigned
/// a nonce equal to the value already stored on node-1, so node-1's
/// Shared-upsert replay guard (`new_nonce <= last_nonce`) silently dropped
/// it as an "authentic but no-op" stale action, leaving the two nodes
/// permanently diverged on the same DAG heads.
///
/// The guard's correctness comment assumes `equal nonce + valid signature ⇒
/// equal payload`. That holds for a SINGLE writer, but not across a writer
/// set: a second writer can sign different bytes with the same nonce and its
/// signature verifies too, so the equal-nonce silent-skip drops a
/// genuinely-new write.
///
/// The required invariant is **order-independent convergence**: a node that
/// applies A-then-B must end on the same value as a node that applies
/// B-then-A. (The canonical equal-timestamp tiebreak in this codebase is
/// "higher node_id wins" — see `LwwRegister::merge`.) Asserting "B always
/// wins" would be wrong: LWW with "incoming wins on equal ts" flips
/// symmetrically and still diverges.
fn apply_two_shared_writes_in_order<const SCOPE: usize>(
    first_data: &[u8],
    first_sk: &SigningKey,
    second_data: &[u8],
    second_sk: &SigningKey,
    writers: &BTreeSet<AccountId>,
    nonce: u64,
) -> Vec<u8> {
    crate::env::reset_for_testing();
    let root = setup_root::<S<SCOPE>>();
    let id = cell_at(0x49, writers);

    let first = build_signed_shared_action(
        true,
        id,
        first_data.to_vec(),
        writers.clone(),
        nonce,
        first_sk,
        vec![root.clone()],
    );
    Interface::<S<SCOPE>>::apply_action(first, &ctx(first_sk)).unwrap();

    let second = build_signed_shared_action(
        false,
        id,
        second_data.to_vec(),
        writers.clone(),
        nonce,
        second_sk,
        vec![],
    );
    Interface::<S<SCOPE>>::apply_action(second, &ctx(second_sk)).unwrap();

    Interface::<S<SCOPE>>::find_by_id_raw(id).expect("entity must exist after two writes")
}

// Regression guard for the shared-storage post-rotation split-brain
// (e2e flake job 78652650934): fixed by the equal-HLC content-hash tiebreak
// in `interface.rs` (`try_merge_non_root`'s `lww_pick`) + letting equal-nonce
// writes fall through the Shared/User replay guard instead of being skipped.
#[test]
fn shared_equal_nonce_different_writers_converge_regardless_of_order() {
    let alice_sk = make_signing_key(0xA1);
    let bob_sk = make_signing_key(0xB2);
    let alice = account_of_key(&alice_sk);
    let bob = account_of_key(&bob_sk);
    let writers: BTreeSet<AccountId> = [alice, bob].into_iter().collect();
    let nonce = hlc_at(0);

    // Node X applies Alice's write then Bob's (same nonce, different data).
    let x = apply_two_shared_writes_in_order::<409>(
        b"alice-value",
        &alice_sk,
        b"bob-value",
        &bob_sk,
        &writers,
        nonce,
    );
    // Node Y applies them in the opposite order.
    let y = apply_two_shared_writes_in_order::<410>(
        b"bob-value",
        &bob_sk,
        b"alice-value",
        &alice_sk,
        &writers,
        nonce,
    );

    assert_eq!(
        x, y,
        "two writers at the same nonce must converge to the SAME value \
         regardless of apply order (shared-storage post-rotation split-brain): \
         A-then-B gave {x:?}, B-then-A gave {y:?}"
    );
}
