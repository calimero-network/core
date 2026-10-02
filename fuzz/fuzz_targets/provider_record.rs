#![no_main]
//! A blob-provider DHT record, as `kad.rs` verifies one a peer stored or replicated.
//! The input is a raw record value, and a patch applied to a record signed in-target.

use borsh::BorshDeserialize;
use calimero_network::fuzz_api::{blob_provider_record_signed_value, blob_provider_record_verify};
use libfuzzer_sys::fuzz_target;
use libp2p::identity::Keypair;

const SIGNER_SEED: [u8; 32] = [7; 32]; // fixed so a crash replays
const RECORD_KEY: [u8; 64] = [0xC0; 64]; // context id followed by blob id

/// The record's wire layout, read back to check what a verified record claims.
#[derive(BorshDeserialize)]
struct Claims {
    peer_id: Vec<u8>,
    size: u64,
    _public_key: Vec<u8>,
    _signature: Vec<u8>,
}

fuzz_target!(|data: &[u8]| {
    let Some((size, patch)) = data.split_first_chunk::<8>() else {
        return;
    };
    let size = u64::from_le_bytes(*size);
    let mut seed = SIGNER_SEED;
    let keypair = Keypair::ed25519_from_bytes(&mut seed).expect("fixed seed");
    let signer = keypair.public().to_peer_id();
    let signed = blob_provider_record_signed_value(&RECORD_KEY, &keypair, size).expect("signs");
    assert_eq!(
        blob_provider_record_verify(&RECORD_KEY, &signed),
        Some(signer)
    );

    // Only the signer's key can make a value verify; replaying one of its records is allowed.
    if let Some(peer) = blob_provider_record_verify(&RECORD_KEY, patch) {
        assert_eq!(peer, signer);
    }

    // A changed byte may re-encode the same record, but never a different claim.
    let mut mutated = signed.clone();
    for pair in patch.chunks_exact(2) {
        mutated[usize::from(pair[0]) % signed.len()] ^= pair[1];
    }
    if let Some(peer) = blob_provider_record_verify(&RECORD_KEY, &mutated) {
        let claims = Claims::try_from_slice(&mutated).expect("a verified record decodes");
        assert_eq!(peer, signer);
        assert_eq!(claims.peer_id, signer.to_bytes());
        assert_eq!(
            claims.size, size,
            "a verified record claims a size it was not signed for"
        );
    }

    let mut other_key = RECORD_KEY;
    other_key[usize::from(patch.first().copied().unwrap_or(0)) % other_key.len()] ^= 1;
    assert_eq!(blob_provider_record_verify(&other_key, &signed), None);
});
