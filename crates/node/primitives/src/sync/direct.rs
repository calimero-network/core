use std::borrow::Cow;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_crypto::Nonce;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};

#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub enum StreamMessage<'a> {
    Init {
        context_id: ContextId,
        party_id: PublicKey,
        payload: InitPayload,
        next_nonce: Nonce,
    },
    Message {
        sequence_id: usize,
        payload: MessagePayload<'a>,
        next_nonce: Nonce,
    },
    OpaqueError,
}

#[derive(Copy, Clone, Debug, BorshSerialize, BorshDeserialize)]
pub enum InitPayload {
    BlobShare {
        blob_id: BlobId,
    },
    KeyShare,
    DeltaRequest {
        context_id: ContextId,
        delta_id: [u8; 32],
    },
    DagHeadsRequest {
        context_id: ContextId,
    },
}

#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub enum MessagePayload<'a> {
    BlobShare {
        chunk: Cow<'a, [u8]>,
    },
    KeyShare {
        sender_key: PrivateKey,
    },
    DeltaResponse {
        delta: Cow<'a, [u8]>,
    },
    DeltaNotFound,
    DagHeadsResponse {
        dag_heads: Vec<[u8; 32]>,
        root_hash: Hash,
    },
    Challenge {
        challenge: [u8; 32],
    },
    ChallengeResponse {
        signature: [u8; 64],
    },
}
