//! Bytes of unreadable ops kept, per signer and per namespace: their signers'
//! authority cannot be checked until the key arrives. Each op is charged once.

use calimero_governance_types::{NamespaceId, SignedNamespaceOp};
use calimero_primitives::identity::PublicKey;
use calimero_store::key::Generic as GenericKey;
use calimero_store::slice::Slice;
use calimero_store::types::GenericData;
use calimero_store::Store;
use eyre::Result as EyreResult;
use sha2::{Digest, Sha256};

use crate::errors::ApplyError;

/// Bytes of unreadable ops kept for one signer in one namespace. An honest op is
/// a few hundred bytes, so this is tens of thousands of them.
pub(crate) const MAX_BYTES_PER_SIGNER: u64 = 16 << 20;

/// Bytes of unreadable ops kept for one namespace, whoever signed them.
pub(crate) const MAX_BYTES_PER_NAMESPACE: u64 = 128 << 20;

/// 16-byte `Generic` scope of the counters.
const SCOPE: [u8; 16] = *b"calimero-unrdops";

pub(crate) struct UnreadableOpBudget<'a> {
    store: &'a Store,
    per_signer: u64,
    per_namespace: u64,
}

impl<'a> UnreadableOpBudget<'a> {
    pub(crate) fn new(store: &'a Store) -> Self {
        Self::with_limits(store, MAX_BYTES_PER_SIGNER, MAX_BYTES_PER_NAMESPACE)
    }

    pub(crate) fn with_limits(store: &'a Store, per_signer: u64, per_namespace: u64) -> Self {
        Self {
            store,
            per_signer,
            per_namespace,
        }
    }

    /// Charge `bytes` of an unreadable op by `signer` in `namespace`, or fail with
    /// [`ApplyError::UnreadableOpBudgetExceeded`] having charged nothing.
    pub(crate) fn admit(
        &self,
        namespace: &NamespaceId,
        signer: &PublicKey,
        bytes: u64,
    ) -> EyreResult<()> {
        let by_signer = Self::key(b"signer", namespace, Some(signer));
        let by_namespace = Self::key(b"namespace", namespace, None);
        let signer_used = self.used(&by_signer)?;
        let namespace_used = self.used(&by_namespace)?;
        let signer_after = signer_used.saturating_add(bytes);
        let namespace_after = namespace_used.saturating_add(bytes);
        if signer_after > self.per_signer || namespace_after > self.per_namespace {
            return Err(ApplyError::UnreadableOpBudgetExceeded {
                signer: signer.to_string(),
                signer_bytes: signer_used,
                namespace_bytes: namespace_used,
            }
            .into());
        }
        self.put(&by_signer, signer_after)?;
        self.put(&by_namespace, namespace_after)
    }

    /// [`Self::admit`] for the whole of `op`.
    pub(crate) fn admit_op(&self, op: &SignedNamespaceOp) -> EyreResult<()> {
        let size = borsh::to_vec(op).map_or(u64::MAX, |encoded| encoded.len() as u64);
        self.admit(&op.namespace_id, &op.signer, size)
    }

    fn key(kind: &[u8], namespace: &NamespaceId, signer: Option<&PublicKey>) -> GenericKey {
        let mut hasher = Sha256::new();
        hasher.update(kind);
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

    #[test]
    fn a_signer_is_charged_up_to_its_budget_and_no_further() {
        let store = test_store();
        let budget = UnreadableOpBudget::with_limits(&store, 100, 10_000);
        let ns = NamespaceId::from([1u8; 32]);

        budget.admit(&ns, &signer(1), 60).expect("within budget");
        budget
            .admit(&ns, &signer(1), 40)
            .expect("exactly the budget");
        assert!(
            budget.admit(&ns, &signer(1), 1).is_err(),
            "one byte past the signer's budget"
        );
        budget
            .admit(&ns, &signer(2), 100)
            .expect("another signer has a budget of its own");
    }

    #[test]
    fn the_namespace_has_a_budget_whoever_signs() {
        let store = test_store();
        let budget = UnreadableOpBudget::with_limits(&store, 100, 150);
        let ns = NamespaceId::from([1u8; 32]);

        budget.admit(&ns, &signer(1), 100).expect("first signer");
        budget.admit(&ns, &signer(2), 50).expect("second signer");
        assert!(
            budget.admit(&ns, &signer(3), 1).is_err(),
            "a third signer, well within its own budget, finds the namespace's spent"
        );
        budget
            .admit(&NamespaceId::from([2u8; 32]), &signer(3), 100)
            .expect("another namespace is unaffected");
    }

    #[test]
    fn a_refused_op_is_not_charged() {
        let store = test_store();
        let budget = UnreadableOpBudget::with_limits(&store, 100, 10_000);
        let ns = NamespaceId::from([1u8; 32]);

        assert!(budget.admit(&ns, &signer(1), 101).is_err());
        budget
            .admit(&ns, &signer(1), 100)
            .expect("the refused op cost nothing");
    }
}
