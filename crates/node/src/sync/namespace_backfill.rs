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

/// Most request rounds one ancestry fetch makes; each round reaches one level
/// deeper than the last.
pub(crate) const MAX_ANCESTRY_ROUNDS: usize = 8;

/// Most ops one ancestry fetch collects, the refused op included.
pub(crate) const MAX_ANCESTRY_OPS: usize = 2 * MAX_BACKFILL_OPS;

/// Collect `op` and the ancestors of it that this node lacks, parents first.
///
/// An empty backfill request always returns the same first `MAX_BACKFILL_OPS`
/// ops in hash order, so a parent beyond them never arrives that way. This asks
/// for the missing parents by id instead, then for what those need, for at most
/// [`MAX_ANCESTRY_ROUNDS`] rounds and [`MAX_ANCESTRY_OPS`] ops. Nothing is
/// applied between rounds: an older ancestor may be what certifies a newer op's
/// signer, so the caller applies the whole set in the returned order.
///
/// `is_local` says whether an op is already stored here; `request` asks the
/// source for op ids and yields the raw entries, or `None` when the source could
/// not answer. Only ops that were asked for are kept.
pub(crate) async fn collect_ancestry<L, R, Fut>(
    namespace_id: [u8; 32],
    op: SignedNamespaceOp,
    is_local: L,
    mut request: R,
) -> Vec<([u8; 32], SignedNamespaceOp)>
where
    L: Fn(&[u8; 32]) -> bool,
    R: FnMut(Vec<[u8; 32]>) -> Fut,
    Fut: std::future::Future<Output = Option<Vec<([u8; 32], Vec<u8>)>>>,
{
    let mut collected: Vec<([u8; 32], SignedNamespaceOp)> = Vec::new();
    let mut known: std::collections::HashSet<[u8; 32]> = std::collections::HashSet::new();
    let mut requested: std::collections::HashSet<[u8; 32]> = std::collections::HashSet::new();

    let Ok(op_hash) = op.content_hash() else {
        return Vec::new();
    };
    let _ = known.insert(op_hash);
    collected.push((op_hash, op));

    for _ in 0..MAX_ANCESTRY_ROUNDS {
        let mut want: Vec<[u8; 32]> = Vec::new();
        for (_, held) in &collected {
            for parent in &held.parent_op_hashes {
                if !known.contains(parent)
                    && !requested.contains(parent)
                    && !want.contains(parent)
                    && !is_local(parent)
                {
                    want.push(*parent);
                }
            }
        }
        want.truncate(MAX_BACKFILL_OPS);
        if want.is_empty() {
            break;
        }
        requested.extend(want.iter().copied());

        let Some(response) = request(want).await else {
            break;
        };
        for (_, received) in decode_backfill(namespace_id, response) {
            let Ok(hash) = received.content_hash() else {
                continue;
            };
            if collected.len() >= MAX_ANCESTRY_OPS {
                break;
            }
            if requested.contains(&hash) && known.insert(hash) {
                collected.push((hash, received));
            }
        }
    }

    calimero_governance_types::order_parents_first(collected, |(_, op)| op)
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

    /// A responder holding `ops`, answering the way the real one does: an empty
    /// request gets the first `MAX_BACKFILL_OPS` in hash order, ids get those ops.
    struct Responder {
        by_hash: std::collections::BTreeMap<[u8; 32], Vec<u8>>,
        requests: std::cell::RefCell<Vec<usize>>,
    }

    impl Responder {
        fn holding(ops: &[SignedNamespaceOp]) -> Self {
            Self {
                by_hash: ops.iter().map(entry).collect(),
                requests: std::cell::RefCell::new(Vec::new()),
            }
        }

        fn answer(&self, ids: &[[u8; 32]]) -> Vec<([u8; 32], Vec<u8>)> {
            self.requests.borrow_mut().push(ids.len());
            if ids.is_empty() {
                return self
                    .by_hash
                    .iter()
                    .take(MAX_BACKFILL_OPS)
                    .map(|(id, bytes)| (*id, bytes.clone()))
                    .collect();
            }
            ids.iter()
                .take(MAX_BACKFILL_OPS)
                .filter_map(|id| Some((*id, self.by_hash.get(id)?.clone())))
                .collect()
        }
    }

    /// More than `MAX_BACKFILL_OPS` unrelated ops, plus a chain `refused -> p -> q`
    /// where neither `p` nor `q` is within the first `MAX_BACKFILL_OPS` by hash,
    /// so an empty request never returns them. Yields the stored ops, the refused
    /// op and the hashes of `p` and `q`.
    fn namespace_with_ancestors_past_the_first_window() -> (
        Vec<SignedNamespaceOp>,
        SignedNamespaceOp,
        [u8; 32],
        [u8; 32],
    ) {
        let mut ops: Vec<SignedNamespaceOp> = (0..MAX_BACKFILL_OPS as u64 + 100)
            .map(|nonce| op_in(NAMESPACE, nonce + 1, vec![]))
            .collect();
        ops.sort_by_key(|op| op.content_hash().expect("hash"));
        // The highest-hashed filler is past the window by construction.
        let q = ops.pop().expect("a last op");
        let q_hash = q.content_hash().expect("hash");
        let past_window = |hash: &[u8; 32]| {
            ops.iter()
                .filter(|op| op.content_hash().expect("hash") < *hash)
                .count()
                >= MAX_BACKFILL_OPS
        };
        let mut nonce = 9_000;
        let p = loop {
            let candidate = op_in(NAMESPACE, nonce, vec![q_hash]);
            if past_window(&candidate.content_hash().expect("hash")) {
                break candidate;
            }
            nonce += 1;
        };
        let p_hash = p.content_hash().expect("hash");
        ops.push(q);
        ops.push(p);
        let refused = op_in(NAMESPACE, 20_000, vec![p_hash]);
        (ops, refused, p_hash, q_hash)
    }

    #[tokio::test]
    async fn ancestors_beyond_the_first_window_are_fetched_by_id() {
        let (stored, refused, p_hash, q_hash) = namespace_with_ancestors_past_the_first_window();
        let responder = Responder::holding(&stored);

        // The window an empty request returns holds neither ancestor.
        let window = responder.answer(&[]);
        assert!(window.iter().all(|(id, _)| *id != p_hash && *id != q_hash));

        let ordered = collect_ancestry(
            NAMESPACE,
            refused.clone(),
            |_| false,
            |ids| {
                let answer = responder.answer(&ids);
                async move { Some(answer) }
            },
        )
        .await;

        let hashes: Vec<[u8; 32]> = ordered.iter().map(|(id, _)| *id).collect();
        let refused_hash = refused.content_hash().expect("hash");
        assert_eq!(hashes, vec![q_hash, p_hash, refused_hash], "parents first");
    }

    #[tokio::test]
    async fn ancestry_stops_at_what_is_already_stored_here() {
        let (stored, refused, p_hash, _q_hash) = namespace_with_ancestors_past_the_first_window();
        let responder = Responder::holding(&stored);

        let ordered = collect_ancestry(
            NAMESPACE,
            refused,
            |id| *id == p_hash,
            |ids| {
                let answer = responder.answer(&ids);
                async move { Some(answer) }
            },
        )
        .await;

        assert_eq!(ordered.len(), 1, "nothing is missing but the refused op");
        assert!(
            responder.requests.borrow().is_empty(),
            "no request was made"
        );
    }

    #[tokio::test]
    async fn ancestry_rounds_are_bounded() {
        // A chain deeper than the round limit.
        let mut chain = vec![op_in(NAMESPACE, 1, vec![])];
        for nonce in 2..=(MAX_ANCESTRY_ROUNDS as u64 + 5) {
            let parent = chain.last().expect("chain").content_hash().expect("hash");
            chain.push(op_in(NAMESPACE, nonce, vec![parent]));
        }
        let refused = chain.pop().expect("tip");
        let responder = Responder::holding(&chain);

        let ordered = collect_ancestry(
            NAMESPACE,
            refused,
            |_| false,
            |ids| {
                let answer = responder.answer(&ids);
                async move { Some(answer) }
            },
        )
        .await;

        assert_eq!(responder.requests.borrow().len(), MAX_ANCESTRY_ROUNDS);
        assert_eq!(ordered.len(), MAX_ANCESTRY_ROUNDS + 1);
    }

    #[tokio::test]
    async fn ops_that_were_not_asked_for_are_not_kept() {
        let refused = op_in(NAMESPACE, 2, vec![[0xAB; 32]]);
        let unasked = op_in(NAMESPACE, 3, vec![]);

        let ordered = collect_ancestry(
            NAMESPACE,
            refused,
            |_| false,
            |_| {
                let answer = vec![entry(&unasked)];
                async move { Some(answer) }
            },
        )
        .await;

        assert_eq!(ordered.len(), 1);
    }

    #[tokio::test]
    async fn an_unanswered_request_ends_the_fetch() {
        let refused = op_in(NAMESPACE, 2, vec![[0xAB; 32]]);

        let ordered = collect_ancestry(NAMESPACE, refused, |_| false, |_| async { None }).await;

        assert_eq!(ordered.len(), 1);
    }
}
