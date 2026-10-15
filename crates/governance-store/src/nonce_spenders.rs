//! Which ops spent each `(group, signer, nonce)` of a namespace, and which envelopes
//! carried each relayed join: what tells a replay apart from a different op reusing it.

use calimero_context_config::types::ContextGroupId;
use calimero_governance_types::NamespaceId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::Generic as GenericKey;
use calimero_store::slice::Slice;
use calimero_store::types::GenericData;
use calimero_store::Store;
use eyre::Result as EyreResult;
use sha2::{Digest, Sha256};

const NO_LIMIT: usize = usize::MAX; // a read lists every spender of one key

pub(crate) struct NonceSpenders<'a> {
    store: &'a Store,
    namespace: NamespaceId,
}

impl<'a> NonceSpenders<'a> {
    pub(crate) const fn new(store: &'a Store, namespace: NamespaceId) -> Self {
        Self { store, namespace }
    }

    /// The ops recorded as having spent `nonce`.
    pub(crate) fn spenders(
        &self,
        group: &ContextGroupId,
        signer: &PublicKey,
        nonce: u64,
    ) -> EyreResult<Vec<[u8; 32]>> {
        self.read(nonce_head(group, signer, nonce))
    }

    /// Record that the op `delta_id` spent `nonce`.
    pub(crate) fn record(
        &self,
        group: &ContextGroupId,
        signer: &PublicKey,
        nonce: u64,
        delta_id: [u8; 32],
    ) -> EyreResult<()> {
        self.write(nonce_head(group, signer, nonce), delta_id)
    }

    /// The relay envelopes recorded as carrying the join `inner_id`.
    pub(crate) fn copies(&self, inner_id: [u8; 32]) -> EyreResult<Vec<[u8; 32]>> {
        self.read(join_head(inner_id))
    }

    /// Record that the relay envelope `delta_id` carries the join `inner_id`.
    pub(crate) fn record_copy(&self, inner_id: [u8; 32], delta_id: [u8; 32]) -> EyreResult<()> {
        self.write(join_head(inner_id), delta_id)
    }

    /// Forget every record of the namespace, as when this node leaves it, so a
    /// rejoin that replays the log does not take each op for its own replay.
    pub(crate) fn clear(&self) -> EyreResult<()> {
        crate::void_ledger::delete_scope(self.store, self.scope())
    }

    /// One row per op, so a key many ops share costs a write each, not a rewrite.
    fn write(&self, head: [u8; 16], delta_id: [u8; 32]) -> EyreResult<()> {
        let data = GenericData::from(Slice::from(delta_id.to_vec()));
        self.store.handle().put(&self.key(head, delta_id), &data)?;
        Ok(())
    }

    fn read(&self, head: [u8; 16]) -> EyreResult<Vec<[u8; 32]>> {
        let mut prefix = [0; 32];
        prefix[..16].copy_from_slice(&self.scope());
        prefix[16..].copy_from_slice(&head);
        let handle = self.store.handle();
        let mut spenders = Vec::new();
        for key in crate::void_ledger::rows_under(self.store, &prefix, NO_LIMIT)? {
            if let Some(data) = handle.get(&key)? {
                let bytes: &[u8] = data.as_ref();
                spenders.push(bytes.try_into()?);
            }
        }
        Ok(spenders)
    }

    fn key(&self, head: [u8; 16], delta_id: [u8; 32]) -> GenericKey {
        let mut fragment = [0; 32];
        fragment[..16].copy_from_slice(&head);
        fragment[16..].copy_from_slice(&delta_id[..16]);
        GenericKey::new(self.scope(), fragment)
    }

    fn scope(&self) -> [u8; 16] {
        truncated(&[b"calimero-noncesp", self.namespace.as_bytes()])
    }
}

fn nonce_head(group: &ContextGroupId, signer: &PublicKey, nonce: u64) -> [u8; 16] {
    let signer: &[u8; 32] = signer.as_ref();
    truncated(&[b"nonce", &group.to_bytes(), signer, &nonce.to_le_bytes()])
}

fn join_head(inner_id: [u8; 32]) -> [u8; 16] {
    truncated(&[b"join", &inner_id])
}

fn truncated(parts: &[&[u8]]) -> [u8; 16] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    let digest: [u8; 32] = hasher.finalize().into();
    let mut head = [0; 16];
    head.copy_from_slice(&digest[..16]);
    head
}

#[cfg(test)]
mod tests {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_primitives::identity::PrivateKey;

    use super::*;
    use crate::test_fixtures::{account_for, test_store};
    use crate::NamespaceOpLogService;

    const NAMESPACE: [u8; 32] = [0xA1; 32]; // the namespace every row is kept in

    fn logged(store: &Store, parents: Vec<[u8; 32]>, nonce: u64) -> [u8; 32] {
        let sk = PrivateKey::from([0xA2; 32]);
        let op = SignedNamespaceOp::sign(
            &sk,
            NAMESPACE.into(),
            parents,
            nonce,
            NamespaceOp::Root(RootOp::AdminChanged {
                new_admin: account_for(&sk.public_key()),
            }),
        )
        .expect("sign an op");
        NamespaceOpLogService::new(store, NAMESPACE.into())
            .store_signed_operation(&op)
            .expect("log the op");
        op.content_hash().expect("hash the op")
    }

    #[test]
    fn the_past_of_a_cut_is_walked_on_the_stored_dag() {
        let store = test_store();
        let root = logged(&store, vec![], 1);
        let left = logged(&store, vec![root], 2);
        let right = logged(&store, vec![root], 3);
        let log = NamespaceOpLogService::new(&store, NAMESPACE.into());
        let reaches = |parents: &[[u8; 32]], targets: &[[u8; 32]]| {
            log.past_reaches(parents, &targets.iter().copied().collect())
                .expect("walk")
        };

        assert_eq!(reaches(&[left], &[right, root]), Some(Some(root)));
        assert_eq!(reaches(&[left], &[right]), Some(None));
        assert_eq!(
            reaches(&[[0xA3; 32]], &[root]),
            None,
            "a past this node does not hold was answered"
        );
    }

    #[test]
    fn every_spender_of_a_nonce_is_kept() {
        let store = test_store();
        let spenders = NonceSpenders::new(&store, NAMESPACE.into());
        let group = ContextGroupId::from(NAMESPACE);
        let signer = PrivateKey::from([0xA4; 32]).public_key();
        for n in 0..100_u8 {
            spenders
                .record(&group, &signer, 7, [n; 32])
                .expect("record");
        }
        assert_eq!(
            spenders.spenders(&group, &signer, 7).expect("read").len(),
            100
        );
    }

    #[test]
    fn leaving_the_namespace_forgets_its_spenders_only() {
        let store = test_store();
        let other = [0xA5; 32];
        let signer = PrivateKey::from([0xA4; 32]).public_key();
        for namespace in [NAMESPACE, other] {
            NonceSpenders::new(&store, namespace.into())
                .record(&ContextGroupId::from(namespace), &signer, 7, [1; 32])
                .expect("record");
        }

        crate::delete_namespace_local_state(&store, &ContextGroupId::from(NAMESPACE))
            .expect("leave the namespace");

        let read = |namespace: [u8; 32]| {
            NonceSpenders::new(&store, namespace.into())
                .spenders(&ContextGroupId::from(namespace), &signer, 7)
                .expect("read")
        };
        assert!(
            read(NAMESPACE).is_empty(),
            "leaving kept the namespace's spenders"
        );
        assert_eq!(
            read(other),
            vec![[1; 32]],
            "leaving one namespace forgot another's"
        );
    }
}
