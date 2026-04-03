use calimero_context_client::local_governance::{NamespaceOp, SignedNamespaceOp};
use calimero_context_config::types::ContextGroupId;
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::load_group_key_by_id;

/// A namespace group operation that can be retried locally because the
/// corresponding group key is now available.
pub struct RetryCandidate {
    pub signed_op: SignedNamespaceOp,
    pub group_key: [u8; 32],
}

/// Service for retrying deferred encrypted group operations after key delivery.
pub struct NamespaceRetryService<'a> {
    store: &'a Store,
    namespace_id: [u8; 32],
}

impl<'a> NamespaceRetryService<'a> {
    pub fn new(store: &'a Store, namespace_id: [u8; 32]) -> Self {
        Self {
            store,
            namespace_id,
        }
    }

    pub fn collect_retry_candidates_for_group(
        &self,
        group_id: [u8; 32],
    ) -> EyreResult<Vec<RetryCandidate>> {
        let mut candidates = Vec::new();
        let gid_typed = ContextGroupId::from(group_id);
        let handle = self.store.handle();
        let start = calimero_store::key::NamespaceGovOp::new(self.namespace_id, [0u8; 32]);
        let mut iter = handle.iter::<calimero_store::key::NamespaceGovOp>()?;
        let first = iter.seek(start).transpose();

        for key_result in first.into_iter().chain(iter.keys()) {
            let key = key_result?;
            if key.namespace_id() != self.namespace_id {
                break;
            }
            let Some(val): Option<calimero_store::key::NamespaceGovOpValue> = handle.get(&key)?
            else {
                continue;
            };
            let Ok(signed_op) = borsh::from_slice::<SignedNamespaceOp>(&val.skeleton_bytes) else {
                continue;
            };
            let NamespaceOp::Group {
                group_id: op_group_id,
                key_id,
                ..
            } = signed_op.op
            else {
                continue;
            };
            if op_group_id != group_id {
                continue;
            }
            let Some(group_key) = load_group_key_by_id(self.store, &gid_typed, &key_id)? else {
                continue;
            };
            candidates.push(RetryCandidate {
                signed_op,
                group_key,
            });
        }

        Ok(candidates)
    }
}
