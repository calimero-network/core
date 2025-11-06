use std::borrow::Cow;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_crypto::Nonce;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;

#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[non_exhaustive]
#[expect(
    clippy::large_enum_variant,
    reason = "Broadcast payload may carry large artifacts"
)]
pub enum Message<'a> {
    StateDelta {
        context_id: ContextId,
        author_id: PublicKey,
        delta_id: [u8; 32],
        parent_ids: Vec<[u8; 32]>,
        hlc: calimero_storage::logical_clock::HybridTimestamp,
        root_hash: Hash,
        artifact: Cow<'a, [u8]>,
        nonce: Nonce,
        events: Option<Cow<'a, [u8]>>,
    },
    HashHeartbeat {
        context_id: ContextId,
        root_hash: Hash,
        dag_heads: Vec<[u8; 32]>,
    },
}
