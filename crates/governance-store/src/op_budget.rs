//! Bytes kept of ops whose signer's authority the apply could not check in full, per
//! signer and namespace. Past a budget an op's bytes are not kept, its place is.

use calimero_governance_types::{NamespaceId, SignedNamespaceOp};
use calimero_primitives::identity::PublicKey;
use calimero_store::key::Generic as GenericKey;
use calimero_store::slice::Slice;
use calimero_store::types::GenericData;
use calimero_store::Store;
use eyre::Result as EyreResult;
use sha2::{Digest, Sha256};

/// Bytes of unreadable ops kept for one signer in one namespace. An honest op is
/// a few hundred bytes, so this is tens of thousands of them.
pub(crate) const UNREADABLE_PER_SIGNER: u64 = 16 << 20;

/// Bytes of unreadable ops kept for one namespace, whoever signed them.
pub(crate) const UNREADABLE_PER_NAMESPACE: u64 = 128 << 20;

/// Bytes of void ops kept for one signer: what an admin did offline before its
/// removal is a handful of ops.
pub(crate) const VOID_PER_SIGNER: u64 = 256 << 10;

/// Bytes of void ops kept for one namespace, whoever signed them.
pub(crate) const VOID_PER_NAMESPACE: u64 = 4 << 20;

/// 16-byte `Generic` scope of the counters.
const SCOPE: [u8; 16] = *b"calimero-unrdops";

pub(crate) struct OpBudget<'a> {
    store: &'a Store,
    kind: &'static [u8],
    per_signer: u64,
    per_namespace: u64,
}

impl<'a> OpBudget<'a> {
    /// The budget of ops sealed under a key this node lacks.
    pub(crate) fn unreadable(store: &'a Store) -> Self {
        Self::with_limits(
            store,
            b"unreadable",
            UNREADABLE_PER_SIGNER,
            UNREADABLE_PER_NAMESPACE,
        )
    }

    /// The budget of ops whose signer's removal is concurrent with them.
    pub(crate) fn void(store: &'a Store) -> Self {
        Self::with_limits(store, b"void", VOID_PER_SIGNER, VOID_PER_NAMESPACE)
    }

    fn with_limits(
        store: &'a Store,
        kind: &'static [u8],
        per_signer: u64,
        per_namespace: u64,
    ) -> Self {
        Self {
            store,
            kind,
            per_signer,
            per_namespace,
        }
    }

    /// Charge `bytes` by `signer` in `namespace` and say `true`, or say `false`
    /// having charged nothing when either budget cannot take them.
    pub(crate) fn admit(
        &self,
        namespace: &NamespaceId,
        signer: &PublicKey,
        bytes: u64,
    ) -> EyreResult<bool> {
        let by_signer = self.key(b"signer", namespace, Some(signer));
        let by_namespace = self.key(b"namespace", namespace, None);
        let signer_after = self.used(&by_signer)?.saturating_add(bytes);
        let namespace_after = self.used(&by_namespace)?.saturating_add(bytes);
        if signer_after > self.per_signer || namespace_after > self.per_namespace {
            return Ok(false);
        }
        self.put(&by_signer, signer_after)?;
        self.put(&by_namespace, namespace_after)?;
        Ok(true)
    }

    /// [`Self::admit`] for the whole of `op`.
    pub(crate) fn admit_op(&self, op: &SignedNamespaceOp) -> EyreResult<bool> {
        let size = borsh::to_vec(op).map_or(u64::MAX, |encoded| encoded.len() as u64);
        self.admit(&op.namespace_id, &op.signer, size)
    }

    fn key(&self, scope: &[u8], namespace: &NamespaceId, signer: Option<&PublicKey>) -> GenericKey {
        let mut hasher = Sha256::new();
        hasher.update(self.kind);
        hasher.update(scope);
        hasher.update(namespace.as_bytes());
        if let Some(signer) = signer {
            let bytes: &[u8; 32] = signer.as_ref();
            hasher.update(bytes);
        }
        GenericKey::new(SCOPE, hasher.finalize().into())
    }

    fn used(&self, key: &GenericKey) -> EyreResult<u64> {
        let handle = self.store.handle();
        let Some(data) = handle.get(key)? else {
            return Ok(0);
        };
        let bytes: &[u8] = data.as_ref();
        Ok(bytes.try_into().map(u64::from_le_bytes).unwrap_or(u64::MAX))
    }

    fn put(&self, key: &GenericKey, used: u64) -> EyreResult<()> {
        let data = GenericData::from(Slice::from(used.to_le_bytes().to_vec()));
        self.store.handle().put(key, &data)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::test_store;

    fn signer(n: u8) -> PublicKey {
        PublicKey::from([n; 32])
    }

    fn budget(store: &Store, per_signer: u64, per_namespace: u64) -> OpBudget<'_> {
        OpBudget::with_limits(store, b"test", per_signer, per_namespace)
    }

    #[test]
    fn a_signer_is_charged_up_to_its_budget_and_no_further() {
        let store = test_store();
        let budget = budget(&store, 100, 10_000);
        let ns = NamespaceId::from([1u8; 32]);

        assert!(budget.admit(&ns, &signer(1), 60).unwrap());
        assert!(
            budget.admit(&ns, &signer(1), 40).unwrap(),
            "the budget exactly"
        );
        assert!(
            !budget.admit(&ns, &signer(1), 1).unwrap(),
            "one byte past the signer's budget"
        );
        assert!(
            budget.admit(&ns, &signer(2), 100).unwrap(),
            "another signer has a budget of its own"
        );
    }

    #[test]
    fn the_namespace_has_a_budget_whoever_signs() {
        let store = test_store();
        let budget = budget(&store, 100, 150);
        let ns = NamespaceId::from([1u8; 32]);

        assert!(budget.admit(&ns, &signer(1), 100).unwrap());
        assert!(budget.admit(&ns, &signer(2), 50).unwrap());
        assert!(
            !budget.admit(&ns, &signer(3), 1).unwrap(),
            "a third signer, well within its own budget, finds the namespace's spent"
        );
        assert!(
            budget
                .admit(&NamespaceId::from([2u8; 32]), &signer(3), 100)
                .unwrap(),
            "another namespace is unaffected"
        );
    }

    #[test]
    fn a_refused_op_is_not_charged() {
        let store = test_store();
        let budget = budget(&store, 100, 10_000);
        let ns = NamespaceId::from([1u8; 32]);

        assert!(!budget.admit(&ns, &signer(1), 101).unwrap());
        assert!(
            budget.admit(&ns, &signer(1), 100).unwrap(),
            "the refused op cost nothing"
        );
    }

    #[test]
    fn the_two_kinds_are_counted_apart() {
        let store = test_store();
        let ns = NamespaceId::from([1u8; 32]);

        assert!(OpBudget::void(&store)
            .admit(&ns, &signer(1), VOID_PER_SIGNER)
            .unwrap());
        assert!(
            OpBudget::unreadable(&store)
                .admit(&ns, &signer(1), UNREADABLE_PER_SIGNER)
                .unwrap(),
            "spending the void budget leaves the unreadable one whole"
        );
    }
}
