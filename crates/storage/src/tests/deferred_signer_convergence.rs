//! Two receivers at different fold depths must converge.
//!
//! The writer set names accounts; a signature names a key. The bridge is
//! `ApplyContext.signer_account`, resolved by the node at the delta's cut — so a
//! receiver that has not yet folded the writer's device binding cannot answer
//! "which account is this?" for a delta a better-informed peer accepts.
//!
//! That gap has exactly one safe shape: **refuse retryably**. The delta must not
//! apply (the receiver cannot authorize it) and must not be consumed (it will be
//! authorizable once the binding folds). Collapsing "cannot answer yet" into
//! "that key speaks for nobody" turns a timing gap into permanent divergence —
//! the class of bug this plane has produced repeatedly, most recently as two
//! account spaces where a node's own writes were refused by every peer.
//!
//! These tests pin the three outcomes apart, because from the caller's side the
//! first two look identical (`Err(InvalidSignature)`):
//!
//! | resolution | outcome | why |
//! | --- | --- | --- |
//! | `None` — not folded yet | refused, retryable, converges later | timing, not authority |
//! | resolves to a NON-writer | refused, and refused again forever | authority |
//! | resolves to a writer | applied | the ordinary path |

use ed25519_dalek::SigningKey;

use crate::address::Id;
use crate::entities::ChildInfo;
use crate::index::Index;
use crate::interface::{ApplyContext, Interface, StorageError};
use crate::store::{MockedStorage, StorageAdaptor};
use crate::tests::common::{account_of_key, build_signed_shared_action};

type S<const SCOPE: usize> = MockedStorage<SCOPE>;

fn make_signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

/// Register the root and return the `ChildInfo` to hang children off, so the
/// child actions pass `verify_ancestor_integrity`.
fn setup_root<A: StorageAdaptor>() -> ChildInfo {
    let root_id = Id::root();
    let root_meta = crate::entities::Metadata::default();
    Index::<A>::add_root(ChildInfo::new(root_id, [0; 32], root_meta.clone()))
        .expect("register root");
    let (full_hash, _) = Index::<A>::get_hashes_for(root_id)
        .expect("root hashes")
        .expect("root present");
    ChildInfo::new(root_id, full_hash, root_meta)
}

/// The context root's full hash — what two receivers must agree on.
fn root_hash<A: StorageAdaptor>() -> [u8; 32] {
    Index::<A>::get_hashes_for(Id::root())
        .expect("root hashes")
        .expect("root present")
        .0
}

/// An apply context carrying a resolved signer, with no `effective_writers` — so
/// the verifier falls back to the entity's stored writer set, which is where the
/// bootstrap put the granted account.
fn ctx_resolving(account: Option<calimero_account::AccountId>) -> ApplyContext {
    ApplyContext {
        effective_writers: None,
        delta_id: None,
        delta_hlc: None,
        signer_account: account,
    }
}

/// **A receiver that cannot resolve the writer's account defers, and converges
/// once it can.**
///
/// Both receivers see the same two actions. The difference is only what their
/// resolver can answer at the moment the second arrives — which in production is
/// how much of the device-binding history each has folded.
#[test]
fn a_receiver_that_cannot_resolve_the_signer_converges_once_it_can() {
    let alice_sk = make_signing_key(0xA1);
    let alice = account_of_key(&alice_sk);
    let id = Id::new([0x5A; 32]);

    // Receiver 1 — has folded the binding, so it resolves Alice and applies.
    let informed_root = {
        crate::env::reset_for_testing();
        let root = setup_root::<S<8300>>();

        let bootstrap = build_signed_shared_action(
            true,
            id,
            b"v0".to_vec(),
            [alice].into_iter().collect(),
            1_000,
            &alice_sk,
            vec![root],
        );
        Interface::<S<8300>>::apply_action(bootstrap, &ctx_resolving(Some(alice)))
            .expect("bootstrap applies for a receiver that can resolve the signer");

        let update = build_signed_shared_action(
            false,
            id,
            b"v1".to_vec(),
            [alice].into_iter().collect(),
            2_000,
            &alice_sk,
            vec![],
        );
        Interface::<S<8300>>::apply_action(update, &ctx_resolving(Some(alice)))
            .expect("the write applies");

        root_hash::<S<8300>>()
    };

    // Receiver 2 — same actions, but its resolver cannot answer for the update.
    let lagging_root = {
        crate::env::reset_for_testing();
        let root = setup_root::<S<8301>>();

        let bootstrap = build_signed_shared_action(
            true,
            id,
            b"v0".to_vec(),
            [alice].into_iter().collect(),
            1_000,
            &alice_sk,
            vec![root],
        );
        Interface::<S<8301>>::apply_action(bootstrap, &ctx_resolving(Some(alice)))
            .expect("bootstrap applies");
        let before = root_hash::<S<8301>>();

        // The binding has not folded here yet: the node has no account to name.
        let mk_update = || {
            build_signed_shared_action(
                false,
                id,
                b"v1".to_vec(),
                [alice].into_iter().collect(),
                2_000,
                &alice_sk,
                vec![],
            )
        };
        let deferred = Interface::<S<8301>>::apply_action(mk_update(), &ctx_resolving(None));
        assert!(
            matches!(deferred, Err(StorageError::InvalidSignature)),
            "an unresolvable signer must be refused, not admitted: {deferred:?}"
        );
        assert_eq!(
            root_hash::<S<8301>>(),
            before,
            "a refused write must leave no trace — a partial apply would diverge \
             this receiver from one that deferred cleanly"
        );

        // The binding folds. The SAME delta is re-driven, as the node's retry does.
        Interface::<S<8301>>::apply_action(mk_update(), &ctx_resolving(Some(alice)))
            .expect("the deferred write applies once the signer resolves");

        root_hash::<S<8301>>()
    };

    assert_eq!(
        informed_root, lagging_root,
        "two receivers that folded the bindings at different times must reach the \
         same root — deferring may cost a retry, never convergence"
    );
}

/// **A resolvable signer that is not a writer is refused, and stays refused.**
///
/// The other side of the same coin: this must NOT look like a deferral, or a node
/// would retry a write it can never accept, forever. The two are distinguishable
/// only by what the resolver answers, which is why the resolution has to be a
/// separate step from the signature check.
#[test]
fn a_resolvable_non_writer_is_refused_and_a_retry_does_not_help() {
    crate::env::reset_for_testing();
    let root = setup_root::<S<8302>>();

    let alice_sk = make_signing_key(0xA1);
    let alice = account_of_key(&alice_sk);
    let mallory_sk = make_signing_key(0x3D);
    let mallory = account_of_key(&mallory_sk);
    let id = Id::new([0x5B; 32]);

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"v0".to_vec(),
        [alice].into_iter().collect(),
        1_000,
        &alice_sk,
        vec![root],
    );
    Interface::<S<8302>>::apply_action(bootstrap, &ctx_resolving(Some(alice))).expect("bootstrap");
    let before = root_hash::<S<8302>>();

    // Mallory signs, and the resolver answers honestly: Mallory's own account,
    // which the writer set does not name.
    let mk_forged = || {
        build_signed_shared_action(
            false,
            id,
            b"forged".to_vec(),
            [alice].into_iter().collect(),
            2_000,
            &mallory_sk,
            vec![],
        )
    };
    for attempt in 1..=3 {
        let result = Interface::<S<8302>>::apply_action(mk_forged(), &ctx_resolving(Some(mallory)));
        assert!(
            matches!(result, Err(StorageError::InvalidSignature)),
            "attempt {attempt}: a non-writer must be refused however often it retries: \
             {result:?}"
        );
    }
    assert_eq!(
        root_hash::<S<8302>>(),
        before,
        "no amount of retrying admits a non-writer"
    );
}

/// **The refusal must come from the ACCOUNT not being a writer, not from the
/// signature.**
///
/// Without this, `a_resolvable_non_writer_is_refused_and_a_retry_does_not_help`
/// would pass even if the account plane were bypassed entirely — a forged
/// signature is refused by the crypto alone. Here the signature is genuinely
/// valid and the *only* thing wrong is which account it speaks for.
#[test]
fn the_refusal_tracks_the_account_and_not_the_signature() {
    crate::env::reset_for_testing();
    let root = setup_root::<S<8303>>();

    let alice_sk = make_signing_key(0xA1);
    let alice = account_of_key(&alice_sk);
    let id = Id::new([0x5C; 32]);

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"v0".to_vec(),
        [alice].into_iter().collect(),
        1_000,
        &alice_sk,
        vec![root],
    );
    Interface::<S<8303>>::apply_action(bootstrap, &ctx_resolving(Some(alice))).expect("bootstrap");

    // One action, signed by Alice — valid crypto either way. Only the resolved
    // account differs between the two applies.
    let mk_update = || {
        build_signed_shared_action(
            false,
            id,
            b"v1".to_vec(),
            [alice].into_iter().collect(),
            2_000,
            &alice_sk,
            vec![],
        )
    };

    let stranger = account_of_key(&make_signing_key(0x77));
    let misresolved =
        Interface::<S<8303>>::apply_action(mk_update(), &ctx_resolving(Some(stranger)));
    assert!(
        matches!(misresolved, Err(StorageError::InvalidSignature)),
        "a valid signature attributed to a non-writer account must be refused: \
         {misresolved:?}"
    );

    Interface::<S<8303>>::apply_action(mk_update(), &ctx_resolving(Some(alice)))
        .expect("the same signature, resolved to the granted account, applies");
}

/// **A signature that speaks for nobody costs no signature verification.**
///
/// The writer-set check runs first, so a non-writer is refused before any
/// `ed25519_verify`. Both orders refuse the write, which is why this asserts the
/// count rather than the result: with the checks the other way round, a peer
/// sending garbage makes the receiver verify once per writer, on a path any peer
/// can drive. The property is cheap to keep and invisible to lose.
#[test]
fn a_non_writer_is_refused_before_any_signature_is_verified() {
    crate::env::reset_for_testing();
    let root = setup_root::<S<8304>>();

    let alice_sk = make_signing_key(0xA1);
    let alice = account_of_key(&alice_sk);
    let mallory_sk = make_signing_key(0x3D);
    let mallory = account_of_key(&mallory_sk);
    let id = Id::new([0x5D; 32]);

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"v0".to_vec(),
        [alice].into_iter().collect(),
        1_000,
        &alice_sk,
        vec![root],
    );
    Interface::<S<8304>>::apply_action(bootstrap, &ctx_resolving(Some(alice))).expect("bootstrap");

    let forged = build_signed_shared_action(
        false,
        id,
        b"forged".to_vec(),
        [alice].into_iter().collect(),
        2_000,
        &mallory_sk,
        vec![],
    );
    crate::env::reset_ed25519_verify_calls();
    let refused = Interface::<S<8304>>::apply_action(forged, &ctx_resolving(Some(mallory)));
    assert!(
        matches!(refused, Err(StorageError::InvalidSignature)),
        "precondition: the non-writer's action must be refused: {refused:?}"
    );
    assert_eq!(
        crate::env::ed25519_verify_calls(),
        0,
        "a signer the writer set does not name must be refused on the cheap check \
         alone — verifying first lets any peer impose one verification per writer"
    );

    // And the counter is measuring something: the authorized path does verify.
    let genuine = build_signed_shared_action(
        false,
        id,
        b"v1".to_vec(),
        [alice].into_iter().collect(),
        3_000,
        &alice_sk,
        vec![],
    );
    crate::env::reset_ed25519_verify_calls();
    Interface::<S<8304>>::apply_action(genuine, &ctx_resolving(Some(alice))).expect("the write");
    assert!(
        crate::env::ed25519_verify_calls() > 0,
        "a test that never observes a verification would pass with the verifier \
         removed altogether"
    );
}

/// **A writer's account does not excuse a bad signature.**
///
/// The fourth resolution case, and the one an account-keyed writer set makes
/// easy to lose: the account is granted and the resolution succeeds, so only the
/// signature stands between a corrupted action and the state.
#[test]
fn a_granted_account_with_a_broken_signature_is_refused() {
    crate::env::reset_for_testing();
    let root = setup_root::<S<8305>>();

    let alice_sk = make_signing_key(0xA1);
    let alice = account_of_key(&alice_sk);
    let id = Id::new([0x5E; 32]);

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"v0".to_vec(),
        [alice].into_iter().collect(),
        1_000,
        &alice_sk,
        vec![root],
    );
    Interface::<S<8305>>::apply_action(bootstrap, &ctx_resolving(Some(alice))).expect("bootstrap");
    let before = root_hash::<S<8305>>();

    // Alice's own action, signed by Alice, with the payload swapped after signing
    // — the shape a corrupted or tampered delta takes on the wire.
    let mut tampered = build_signed_shared_action(
        false,
        id,
        b"v1".to_vec(),
        [alice].into_iter().collect(),
        2_000,
        &alice_sk,
        vec![],
    );
    match &mut tampered {
        crate::action::Action::Add { data, .. } | crate::action::Action::Update { data, .. } => {
            *data = b"tampered".to_vec();
        }
        other => panic!("expected an upsert action, got {other:?}"),
    }

    let refused = Interface::<S<8305>>::apply_action(tampered, &ctx_resolving(Some(alice)));
    assert!(
        matches!(refused, Err(StorageError::InvalidSignature)),
        "a granted account is not a licence to skip the signature: {refused:?}"
    );
    assert_eq!(
        root_hash::<S<8305>>(),
        before,
        "and nothing of the tampered payload reached the state"
    );
}
