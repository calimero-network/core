#![no_main]
//! Signed governance ops. The input is decoded as gossip on a namespace topic, then
//! read as an op the target signs, encodes, patches and decodes again.

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_types::{
    GovernanceError, GroupOp, NamespaceId, NamespaceOp, NamespaceTopicMsg, SignedGroupOp,
    SignedNamespaceOp,
};
use calimero_node_primitives::sync::snapshot::MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use libfuzzer_sys::fuzz_target;

const SIGNER_SEED: [u8; 32] = [7; 32]; // fixed so a crash replays

fuzz_target!(|data: &[u8]| {
    let signer = PrivateKey::from(SIGNER_SEED);
    if data.len() <= MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES {
        gossip(data, &signer.public_key());
    }

    let Some((&kind, mut rest)) = data.split_first() else {
        return;
    };
    if kind % 2 == 0 {
        let Ok((id, parents, nonce, op)) =
            <(NamespaceId, Vec<[u8; 32]>, u64, NamespaceOp)>::deserialize(&mut rest)
        else {
            return;
        };
        let Ok(op) = SignedNamespaceOp::sign(&signer, id, parents, nonce, op) else {
            return;
        };
        // The endorsement is unsigned by design; everything else is covered.
        patched(&op, rest, SignedNamespaceOp::verify_signature, |op| {
            let mut covered = op.clone();
            covered.signature = [0; 64];
            covered.admitter_endorsement = None;
            covered
        });
    } else {
        let Ok((id, parents, nonce, op)) =
            <(ContextGroupId, Vec<[u8; 32]>, u64, GroupOp)>::deserialize(&mut rest)
        else {
            return;
        };
        let Ok(op) = SignedGroupOp::sign(&signer, id, parents, nonce, op) else {
            return;
        };
        patched(&op, rest, SignedGroupOp::verify_signature, |op| {
            let mut covered = op.clone();
            covered.signature = [0; 64];
            covered
        });
    }
});

/// Decodes a topic message as the namespace handler does, then runs the checks the
/// apply path adds. Only the target's key signs anything, so whatever verifies names it.
fn gossip(data: &[u8], signer: &PublicKey) {
    let Ok(message) = borsh::from_slice::<NamespaceTopicMsg>(data) else {
        return;
    };
    let verified_by = match &message {
        NamespaceTopicMsg::Op(op) => {
            let _ = op.validate();
            let _ = op.content_hash();
            op.verify_signature().is_ok().then_some(op.signer)
        }
        NamespaceTopicMsg::Ack(ack) => ack.verify_signature().is_ok().then_some(ack.signer_pubkey),
        NamespaceTopicMsg::ReadinessBeacon(beacon) => beacon
            .verify_signature()
            .is_ok()
            .then_some(beacon.peer_pubkey),
        NamespaceTopicMsg::MigrationHeartbeat(beat) => {
            beat.verify_signature().is_ok().then_some(beat.peer_pubkey)
        }
        NamespaceTopicMsg::ReadinessProbe(_) => None,
    };
    if let Some(key) = verified_by {
        assert_eq!(
            key, *signer,
            "a message verified under a key nobody signed with"
        );
    }
}

/// `patch` is the input after `kind, borsh (id, parents, nonce, op)`: 3-byte (u16 offset,
/// xor) edits. A patched op may verify only if every `covered` field is unchanged.
fn patched<T: BorshSerialize + BorshDeserialize>(
    op: &T,
    patch: &[u8],
    verify: fn(&T) -> Result<(), GovernanceError>,
    covered: fn(&T) -> T,
) {
    verify(op).expect("a freshly signed op verifies");
    let encoded = borsh::to_vec(op).expect("op encodes");
    let decoded = T::try_from_slice(&encoded).expect("an encoded op decodes");
    verify(&decoded).expect("a round-tripped op verifies");

    let mut mutated = encoded.clone();
    for pair in patch.chunks_exact(3) {
        let at = usize::from(u16::from_le_bytes([pair[0], pair[1]])) % mutated.len();
        mutated[at] ^= pair[2];
    }
    let Ok(decoded) = T::try_from_slice(&mutated) else {
        return;
    };
    if verify(&decoded).is_ok() {
        assert_eq!(
            borsh::to_vec(&covered(&decoded)).expect("op encodes"),
            borsh::to_vec(&covered(op)).expect("op encodes"),
            "a changed op still verifies"
        );
    }
}
