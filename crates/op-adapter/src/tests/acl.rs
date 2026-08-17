//! Access-control plane: [`set_writers_payload`], and the fold-equivalence
//! against the legacy `rotation_log::resolve_local`.

use std::collections::BTreeMap;

use calimero_account::AccountId;
use calimero_op::{Op, ScopeId};
use calimero_primitives::identity::PublicKey;
use calimero_projection::ScopeState;
use calimero_storage::address::Id;
use calimero_storage::entities::OpMask;
use calimero_storage::rotation_log::{resolve_local, RotationLog, RotationLogEntry};

use crate::set_writers_payload;
use crate::tests::support::{authorship_of, hlc};

/// Build a `SetWriters` op chain from a rotation log and assert the unified
/// projection resolves the **same writer set** the current
/// `rotation_log::resolve_local` does — the equivalence that lets the live
/// ACL resolution route through `ScopeState`.
///
/// Scope: sequential rotations (strictly increasing HLC), the common case.
/// Genuinely-concurrent (equal-HLC) rotations tie-break by `op_id` in
/// `ScopeState` vs signer-digest in `resolve_local`; once `resolve_local` is
/// gone the `op_id` tiebreak is canonical and identical on every node, so
/// there is nothing to align.
#[test]
fn acl_plane_matches_resolve_local_for_sequential_rotations() {
    let object = Id::new([0xA0; 32]);
    let scope = ScopeId::from([0u8; 32]);
    // The admin SIGNS, so it is a key; the writers are granted, so they are
    // accounts. Different domains — the bridge derives a stand-in account for
    // the signer, and passes the writer set through untouched.
    let admin = PublicKey::from([1u8; 32]);
    let w1 = AccountId::from([0x11; 32]);
    let w2 = AccountId::from([0x22; 32]);

    // Three sequential rotations: {w1} → {w1,w2} → {w2}.
    let sets: Vec<BTreeMap<AccountId, OpMask>> = vec![
        [(w1, OpMask::FULL)].into_iter().collect(),
        [(w1, OpMask::FULL), (w2, OpMask::FULL)]
            .into_iter()
            .collect(),
        [(w2, OpMask::FULL)].into_iter().collect(),
    ];

    let mut entries = Vec::new();
    let mut ops = Vec::new();
    let mut prev_id: Option<[u8; 32]> = None;
    for (i, writers) in sets.iter().enumerate() {
        let delta_id = [i as u8 + 1; 32];
        let h = hlc((i as u64 + 1) * 10);
        entries.push(RotationLogEntry {
            delta_id,
            delta_hlc: h,
            signer: Some(admin),
            signature: None,
            signed_payload: None,
            new_writers: writers.clone(),
            writers_nonce: i as u64 + 1,
        });
        let payload = set_writers_payload(object, entries.last().expect("just pushed"));
        let parents: Vec<[u8; 32]> = prev_id.into_iter().collect();
        let authorship = authorship_of(AccountId::from([1u8; 32]), admin);
        let id = Op::compute_id(scope, &parents, &authorship, &h, &payload);
        ops.push(Op::from_parts(
            id, scope, parents, authorship, h, payload, [0u8; 32], [0u8; 64],
        ));
        prev_id = Some(id);
    }

    let log = RotationLog {
        snapshot: None,
        entries,
    };
    let expected = resolve_local(&log).expect("non-empty log resolves");

    let projected = ScopeState::from_ops(&ops);
    let resolved = projected
        .acl_view()
        .acl
        .get(&object)
        .cloned()
        .unwrap_or_default();

    // No mapping any more: both sides speak accounts, because the rotation
    // log's writer set is account-keyed at the source. The equivalence is
    // therefore direct, and still catches the bridge dropping or renaming a
    // writer.
    assert_eq!(
        resolved, expected,
        "ScopeState ACL fold must resolve the same writer set as resolve_local"
    );
    // Sanity: the latest rotation ({w2}) wins.
    assert_eq!(resolved, sets[2]);
}

/// Encoding a rotation's payload then folding it yields the rotation's
/// writer set verbatim.
#[test]
fn set_writers_payload_round_trips_through_projection() {
    let object = Id::new([0xB0; 32]);
    let scope = ScopeId::from([0u8; 32]);
    let admin = PublicKey::from([1u8; 32]);
    let writers: BTreeMap<AccountId, OpMask> = [(AccountId::from([7u8; 32]), OpMask::FULL)]
        .into_iter()
        .collect();

    let entry = RotationLogEntry {
        delta_id: [9u8; 32],
        delta_hlc: hlc(5),
        signer: Some(admin),
        signature: None,
        signed_payload: None,
        new_writers: writers.clone(),
        writers_nonce: 1,
    };
    let payload = set_writers_payload(object, &entry);
    let op = Op::new(
        scope,
        vec![],
        authorship_of(AccountId::from([1u8; 32]), admin),
        entry.delta_hlc,
        payload,
        [0u8; 32],
        [0u8; 64],
    );

    let resolved = ScopeState::from_ops([&op])
        .acl_view()
        .acl
        .get(&object)
        .cloned()
        .unwrap_or_default();
    // Verbatim: the payload carries the log's own account-keyed set, so a
    // round trip must return exactly it.
    assert_eq!(resolved, writers);
}
