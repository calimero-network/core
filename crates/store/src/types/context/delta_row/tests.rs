//! Round-trips of every row shape the node writes, the rows written before the
//! version-1 layout, and the decoder's refusals.

use std::sync::Arc;

use calimero_account::{
    AccountGenesis, AccountId, AccountProof, Delegation, DeviceCert, DeviceId, KemPublicKey,
    Warrant, WarrantTerms,
};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};

use super::{decode, encode, LegacyRow, TAG, VERSION_1};
use crate::db::InMemoryDB;
use crate::key::{self, AsKeyParts};
use crate::types::ContextDagDelta;
use crate::Store;

fn hlc(time: u64, id: u64) -> HybridTimestamp {
    HybridTimestamp::new(Timestamp::new(
        NTP64(time),
        ID::from(core::num::NonZeroU64::new(id).expect("non-zero")),
    ))
}

fn device(
    root: &PrivateKey,
    account: AccountId,
    seed: u8,
) -> (PrivateKey, AccountProof<DeviceCert>) {
    let device_sk = PrivateKey::from([seed; 32]);
    let cert = DeviceCert::sign(
        root,
        account,
        DeviceId::from([seed ^ 0x0F; 32]),
        &device_sk.public_key(),
        &KemPublicKey::from([seed ^ 0xF0; 32]),
        0,
        0,
    )
    .expect("the root signs the device cert");
    let proof = AccountProof {
        genesis: AccountGenesis::new(root.public_key()),
        chain: vec![],
        statement: cert,
    };
    (device_sk, proof)
}

/// A warrant from one account's device to a relay's, as a delegated row holds.
fn delegation() -> Delegation {
    let author_root = PrivateKey::from([0x61; 32]);
    let author_account = AccountGenesis::new(author_root.public_key()).account_id();
    let (author_sk, author_proof) = device(&author_root, author_account, 0x62);
    let relay_root = PrivateKey::from([0x74; 32]);
    let relay_account = AccountGenesis::new(relay_root.public_key()).account_id();
    let (relay_sk, relay_proof) = device(&relay_root, relay_account, 0x75);
    let warrant = Warrant::sign(
        &author_sk,
        WarrantTerms {
            context: ContextId::from([0x76; 32]),
            author_account,
            executor: relay_account,
            app_version: ApplicationId::from([0u8; 32]),
            method: "send_message".to_owned(),
            intent_hash: Warrant::intent_hash("send_message", b"{}"),
            account_heads: vec![[0x41; 32]],
            governance_floor: vec![[0x42; 32]],
            nonce: 1,
            not_after: u64::MAX,
        },
    )
    .expect("the author signs the warrant");
    Delegation {
        warrant: Box::new(warrant),
        author_proof: Box::new(author_proof),
        executor_proof: Box::new(relay_proof),
        executor_key: relay_sk.public_key(),
    }
}

/// `borsh(GovernanceParentEdge { governance_dag_heads })`: a `u32` count and
/// the heads. Written out here because the store does not depend on the type.
fn governance_blob(heads: &[[u8; 32]]) -> Vec<u8> {
    let mut blob = u32::try_from(heads.len())
        .expect("few heads")
        .to_le_bytes()
        .to_vec();
    for head in heads {
        blob.extend_from_slice(head);
    }
    blob
}

/// A self-authored delta as the local execute path writes it.
fn local_authored() -> ContextDagDelta {
    ContextDagDelta {
        parents: vec![[0x11; 32]],
        actions: vec![0x5A; 146],
        hlc: hlc(0x0123_4567_89AB_CDEF, 0x0FED_CBA9_8765_4321),
        applied: true,
        checkpoint_root_hash: None,
        events: None,
        author_id: Some(PublicKey::from([0x22; 32])),
        governance_position_blob: Some(governance_blob(&[[0x33; 32]])),
        delta_signature: Some([0x44; 64]),
        delegation: None,
    }
}

/// Every shape a row takes on some node path.
fn shapes() -> Vec<(&'static str, ContextDagDelta)> {
    vec![
        (
            "genesis: one zero parent, no author, no signature",
            ContextDagDelta {
                parents: vec![[0; 32]],
                actions: vec![1, 2, 3],
                hlc: hlc(7, 1),
                applied: true,
                checkpoint_root_hash: None,
                events: None,
                author_id: None,
                governance_position_blob: None,
                delta_signature: None,
                delegation: None,
            },
        ),
        ("local authored", local_authored()),
        (
            "received, pending, with events and a merge of two parents",
            ContextDagDelta {
                parents: vec![[0x11; 32], [0x12; 32]],
                applied: false,
                events: Some(vec![0xEE; 300]),
                governance_position_blob: Some(governance_blob(&[[0x33; 32], [0x34; 32]])),
                ..local_authored()
            },
        ),
        (
            "events cleared after the handlers ran",
            ContextDagDelta {
                events: Some(Vec::new()),
                ..local_authored()
            },
        ),
        (
            "snapshot checkpoint",
            ContextDagDelta {
                parents: vec![],
                actions: borsh::to_vec(&Vec::<u8>::new()).expect("empty actions"),
                hlc: hlc(9, 3),
                applied: true,
                checkpoint_root_hash: Some([0x99; 32]),
                events: None,
                author_id: None,
                governance_position_blob: None,
                delta_signature: None,
                delegation: None,
            },
        ),
        (
            "governance position at group genesis (no heads)",
            ContextDagDelta {
                governance_position_blob: Some(governance_blob(&[])),
                ..local_authored()
            },
        ),
        (
            "delegated, pending",
            ContextDagDelta {
                applied: false,
                delegation: Some(delegation()),
                ..local_authored()
            },
        ),
        (
            "every field, with lengths past one varint byte",
            ContextDagDelta {
                parents: (0..200_u8).map(|i| [i; 32]).collect(),
                actions: vec![0xAB; 70_000],
                hlc: hlc(u64::MAX, u64::MAX),
                applied: true,
                checkpoint_root_hash: Some([0x98; 32]),
                events: Some(vec![0xCD; 128]),
                author_id: Some(PublicKey::from([0x23; 32])),
                governance_position_blob: Some(vec![0xCD; 16_384]),
                delta_signature: Some([0x45; 64]),
                delegation: Some(delegation()),
            },
        ),
    ]
}

fn legacy_bytes(delta_id: [u8; 32], row: &ContextDagDelta) -> Vec<u8> {
    borsh::to_vec(&LegacyRow {
        delta_id,
        parents: row.parents.clone(),
        actions: row.actions.clone(),
        hlc: row.hlc,
        applied: row.applied,
        checkpoint_root_hash: row.checkpoint_root_hash,
        events: row.events.clone(),
        author_id: row.author_id,
        governance_position_blob: row.governance_position_blob.clone(),
        delta_signature: row.delta_signature,
        delegation: row.delegation.clone(),
    })
    .expect("legacy encode")
}

#[test]
fn every_row_shape_round_trips() {
    for (name, row) in shapes() {
        let bytes = encode(&row).expect("encode");
        assert_eq!(&bytes[..2], &[TAG, VERSION_1], "{name}: tag");
        assert_eq!(decode(&bytes).expect(name), row, "{name}");
    }
}

#[test]
fn every_row_shape_written_before_version_1_still_decodes() {
    for (name, row) in shapes() {
        let bytes = legacy_bytes([0x5E; 32], &row);
        assert_eq!(decode(&bytes).expect(name), row, "{name}");
    }
}

/// The one ambiguous case: an old row whose delta id begins with the tag. It is
/// still read as the old layout, because the rest does not parse as version 1.
#[test]
fn an_old_row_whose_delta_id_starts_with_the_tag_still_decodes() {
    let mut delta_id = [0x5E; 32];
    delta_id[0] = TAG;
    delta_id[1] = VERSION_1;
    for (name, row) in shapes() {
        let bytes = legacy_bytes(delta_id, &row);
        assert_eq!(decode(&bytes).expect(name), row, "{name}");
    }
}

/// The figure the layout exists for, pinned. Saved: the 32-byte delta id, the
/// `applied` byte, six presence bytes, and three bytes from each of the parents
/// and governance `u32` lengths and two from the actions length (146 needs two
/// varint bytes) — 47 bytes — against a tag, a version and a flags byte.
#[test]
fn a_local_authored_row_is_44_bytes_smaller() {
    let row = local_authored();
    let legacy = legacy_bytes([0x5E; 32], &row).len();
    let v1 = encode(&row).expect("encode").len();
    // header 3, parents 1 + 32, actions 2 + 146, hlc 16, author 32,
    // governance 1 + 36, signature 64.
    assert_eq!(v1, 3 + 33 + 148 + 16 + 32 + 37 + 64);
    assert_eq!(legacy - v1, 44);
}

#[test]
fn trailing_bytes_are_refused() {
    let mut bytes = encode(&local_authored()).expect("encode");
    bytes.push(0);
    assert!(decode(&bytes).is_err());
}

#[test]
fn a_truncated_row_is_refused() {
    let bytes = encode(&local_authored()).expect("encode");
    for len in 0..bytes.len() {
        assert!(decode(&bytes[..len]).is_err(), "prefix of {len} bytes");
    }
}

#[test]
fn a_reserved_flag_bit_is_refused() {
    let mut bytes = encode(&local_authored()).expect("encode");
    bytes[2] |= 0x80;
    assert!(decode(&bytes).is_err());
}

#[test]
fn a_varint_not_in_shortest_form_is_refused() {
    let row = ContextDagDelta {
        parents: vec![],
        ..local_authored()
    };
    let bytes = encode(&row).expect("encode");
    // Parent count 0 at offset 3, written as the two-byte form 0x80 0x00.
    assert_eq!(bytes[3], 0);
    let mut padded = bytes[..3].to_vec();
    padded.extend_from_slice(&[0x80, 0x00]);
    padded.extend_from_slice(&bytes[4..]);
    assert!(decode(&padded).is_err());
}

#[test]
fn a_parent_count_past_the_row_is_refused_before_allocating() {
    let mut bytes = vec![TAG, VERSION_1, 0];
    // u32::MAX parents.
    bytes.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
    assert!(decode(&bytes).is_err());
}

#[test]
fn a_zero_hlc_id_is_refused() {
    let row = local_authored();
    let mut bytes = encode(&row).expect("encode");
    // tag/version/flags 3, parents 1 + 32, actions 2 + 146, then time 8, id 8.
    let id = 3 + 33 + 148 + 8;
    bytes[id..id + 8].fill(0);
    assert!(decode(&bytes).is_err());
}

/// Through the typed store API every node reader uses: `put`, `get`, and a
/// prefix walk with `iter`.
#[test]
fn rows_round_trip_through_the_store_handle() {
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let context = ContextId::from([0x07; 32]);
    let mut handle = store.handle();
    let rows: Vec<_> = shapes()
        .into_iter()
        .enumerate()
        .map(|(i, (_, row))| {
            let id = [u8::try_from(i).expect("few shapes"); 32];
            (key::ContextDagDelta::new(context, id), row)
        })
        .collect();
    for (k, row) in &rows {
        handle.put(k, row).expect("put");
    }
    for (k, row) in &rows {
        assert_eq!(handle.get(k).expect("get").as_ref(), Some(row));
    }

    // The way `load_persisted_deltas` walks a context: seek, read the first
    // value with `get`, then stream the rest with `entries`.
    let mut iter = handle.iter::<key::ContextDagDelta>().expect("iter");
    let first = iter
        .seek(key::ContextDagDelta::new(context, [0; 32]))
        .expect("seek")
        .expect("a first row");
    let mut walked = vec![(first, handle.get(&first).expect("get").expect("row"))];
    for (k, v) in iter.entries() {
        walked.push((k.expect("key"), v.expect("value")));
    }
    let mut seen = 0;
    for (k, row) in walked {
        let (_, expected) = rows
            .iter()
            .find(|(stored, _)| stored.delta_id() == k.delta_id())
            .expect("a stored row");
        assert_eq!(&row, expected);
        seen += 1;
    }
    assert_eq!(seen, rows.len());
}

/// A row written by the previous binary, read back through the store.
#[test]
fn an_old_row_on_disk_reads_through_the_store_handle() {
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let k = key::ContextDagDelta::new(ContextId::from([0x07; 32]), [0x5E; 32]);
    let row = local_authored();
    store
        .raw_put(
            crate::db::Column::Delta,
            k.as_key().as_bytes(),
            &legacy_bytes(k.delta_id(), &row),
        )
        .expect("raw put");
    assert_eq!(store.handle().get(&k).expect("get"), Some(row));
}
