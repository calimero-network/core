//! Preparing a namespace backfill response for the governance DAG.
//!
//! The responder serves ops in hash order, so parents are put before children
//! here; otherwise an op whose signer's join sits later in the batch is refused.

use calimero_context_client::local_governance::SignedNamespaceOp;
use tracing::warn;

use crate::sync::MAX_BACKFILL_OPS;

/// Decode `deltas`, keep what belongs to `namespace_id`, and put parents before
/// children.
///
/// Reads at most [`MAX_BACKFILL_OPS`] entries. Entries that do not decode or
/// name another namespace are dropped; the DAG's entry point checks signatures.
pub(crate) fn decode_backfill(
    namespace_id: [u8; 32],
    deltas: Vec<([u8; 32], Vec<u8>)>,
) -> Vec<([u8; 32], SignedNamespaceOp)> {
    if deltas.len() > MAX_BACKFILL_OPS {
        warn!(
            namespace_id = %hex::encode(namespace_id),
            received = deltas.len(),
            cap = MAX_BACKFILL_OPS,
            "namespace backfill response exceeds cap; reading only the first cap ops"
        );
    }

    let mut ops = Vec::new();
    for (delta_id, bytes) in deltas.into_iter().take(MAX_BACKFILL_OPS) {
        let op = match borsh::from_slice::<SignedNamespaceOp>(&bytes) {
            Ok(op) => op,
            Err(err) => {
                warn!(
                    namespace_id = %hex::encode(namespace_id),
                    delta_id = %hex::encode(delta_id),
                    len = bytes.len(),
                    %err,
                    "failed to decode namespace governance op from backfill"
                );
                continue;
            }
        };
        if op.namespace_id.to_bytes() != namespace_id {
            warn!(
                namespace_id = %hex::encode(namespace_id),
                delta_id = %hex::encode(delta_id),
                "namespace backfill carried an op for another namespace; dropping it"
            );
            continue;
        }
        ops.push((delta_id, op));
    }
    calimero_governance_types::order_parents_first(ops, |(_, op)| op)
}

#[cfg(test)]
mod tests {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp};
    use calimero_primitives::identity::PrivateKey;

    use super::*;

    const NAMESPACE: [u8; 32] = [0x71; 32];

    fn op_in(namespace: [u8; 32], nonce: u64, parents: Vec<[u8; 32]>) -> SignedNamespaceOp {
        SignedNamespaceOp::sign(
            &PrivateKey::from([0x72; 32]),
            namespace.into(),
            parents,
            nonce,
            NamespaceOp::Root(RootOp::PolicyUpdated {
                policy_bytes: vec![nonce as u8],
            }),
        )
        .expect("sign a namespace op")
    }

    fn entry(op: &SignedNamespaceOp) -> ([u8; 32], Vec<u8>) {
        (
            op.content_hash().expect("hash the op"),
            borsh::to_vec(op).expect("encode the op"),
        )
    }

    /// A chain `first <- second <- third <- fourth`, each naming the one before.
    fn chain() -> Vec<SignedNamespaceOp> {
        let first = op_in(NAMESPACE, 1, vec![]);
        let second = op_in(NAMESPACE, 2, vec![first.content_hash().expect("hash")]);
        let third = op_in(NAMESPACE, 3, vec![second.content_hash().expect("hash")]);
        let fourth = op_in(NAMESPACE, 4, vec![third.content_hash().expect("hash")]);
        vec![first, second, third, fourth]
    }

    fn nonces(decoded: &[([u8; 32], SignedNamespaceOp)]) -> Vec<u64> {
        decoded.iter().map(|(_, op)| op.nonce).collect()
    }

    #[test]
    fn parents_come_before_children_whatever_order_the_peer_sent() {
        let ops = chain();
        let shuffled = vec![
            entry(&ops[3]),
            entry(&ops[1]),
            entry(&ops[0]),
            entry(&ops[2]),
        ];

        let decoded = decode_backfill(NAMESPACE, shuffled);

        assert_eq!(nonces(&decoded), vec![1, 2, 3, 4]);
    }

    #[test]
    fn ops_with_no_link_between_them_keep_their_arrival_order() {
        let a = op_in(NAMESPACE, 7, vec![]);
        let b = op_in(NAMESPACE, 5, vec![]);
        let c = op_in(NAMESPACE, 6, vec![[0xEE; 32]]);

        let decoded = decode_backfill(NAMESPACE, vec![entry(&a), entry(&b), entry(&c)]);

        assert_eq!(nonces(&decoded), vec![7, 5, 6]);
    }

    #[test]
    fn an_op_for_another_namespace_is_dropped() {
        let ours = op_in(NAMESPACE, 1, vec![]);
        let theirs = op_in([0x99; 32], 2, vec![]);

        let decoded = decode_backfill(NAMESPACE, vec![entry(&theirs), entry(&ours)]);

        assert_eq!(nonces(&decoded), vec![1]);
    }

    #[test]
    fn an_entry_that_does_not_decode_is_dropped() {
        let ours = op_in(NAMESPACE, 1, vec![]);

        let decoded = decode_backfill(NAMESPACE, vec![([0x01; 32], vec![0xFF; 7]), entry(&ours)]);

        assert_eq!(nonces(&decoded), vec![1]);
    }

    #[test]
    fn a_response_past_the_cap_is_read_only_up_to_the_cap() {
        let entries: Vec<_> = (0..MAX_BACKFILL_OPS as u64 + 25)
            .map(|nonce| entry(&op_in(NAMESPACE, nonce + 1, vec![])))
            .collect();

        let decoded = decode_backfill(NAMESPACE, entries);

        assert_eq!(decoded.len(), MAX_BACKFILL_OPS);
    }
}
