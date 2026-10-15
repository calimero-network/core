//! Which ops spent each `(group, signer, nonce)` of a namespace: what tells a replay
//! of the op that spent a nonce from a different op reusing it.

use calimero_context_config::types::ContextGroupId;
use calimero_governance_types::NamespaceId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::Generic as GenericKey;
use calimero_store::slice::Slice;
use calimero_store::types::GenericData;
use calimero_store::Store;
use eyre::Result as EyreResult;
use sha2::{Digest, Sha256};

pub(crate) struct NonceSpenders<'a> {
    store: &'a Store,
    namespace: NamespaceId,
}

impl<'a> NonceSpenders<'a> {
    pub(crate) const fn new(store: &'a Store, namespace: NamespaceId) -> Self {
        Self { store, namespace }
    }

    /// The ops recorded as having spent `nonce`, oldest first.
    pub(crate) fn spenders(
        &self,
        group: &ContextGroupId,
        signer: &PublicKey,
        nonce: u64,
    ) -> EyreResult<Vec<[u8; 32]>> {
        let handle = self.store.handle();
        let Some(data) = handle.get(&self.key(group, signer, nonce))? else {
            return Ok(Vec::new());
        };
        let bytes: &[u8] = data.as_ref();
        Ok(borsh::from_slice(bytes)?)
    }

    /// Record that the op `delta_id` spent `nonce`. Unbounded: only the nonce's own
    /// signer mints a second spender, and each one is an op this node already stores.
    pub(crate) fn record(
        &self,
        group: &ContextGroupId,
        signer: &PublicKey,
        nonce: u64,
        delta_id: [u8; 32],
    ) -> EyreResult<()> {
        let mut spenders = self.spenders(group, signer, nonce)?;
        if spenders.contains(&delta_id) {
            return Ok(());
        }
        spenders.push(delta_id);
        let data = GenericData::from(Slice::from(borsh::to_vec(&spenders)?));
        self.store
            .handle()
            .put(&self.key(group, signer, nonce), &data)?;
        Ok(())
    }

    /// Forget every spender of the namespace, as when this node leaves it, so a
    /// rejoin that replays the log does not take each op for its own replay.
    pub(crate) fn clear(&self) -> EyreResult<()> {
        crate::void_ledger::delete_scope(self.store, self.scope())
    }

    fn scope(&self) -> [u8; 16] {
        let mut hasher = Sha256::new();
        hasher.update(b"calimero-noncesp");
        hasher.update(self.namespace.as_bytes());
        let digest: [u8; 32] = hasher.finalize().into();
        let mut scope = [0; 16];
        scope.copy_from_slice(&digest[..16]);
        scope
    }

    fn key(&self, group: &ContextGroupId, signer: &PublicKey, nonce: u64) -> GenericKey {
        let mut hasher = Sha256::new();
        hasher.update(group.to_bytes());
        let signer: &[u8; 32] = signer.as_ref();
        hasher.update(signer);
        hasher.update(nonce.to_le_bytes());
        GenericKey::new(self.scope(), hasher.finalize().into())
    }
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
