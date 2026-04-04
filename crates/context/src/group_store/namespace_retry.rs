use calimero_context_client::local_governance::{NamespaceOp, SignedNamespaceOp};
use calimero_context_config::types::ContextGroupId;
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::{load_group_key_by_id, namespace_op_log::NamespaceOpLogService};

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
        let op_log = NamespaceOpLogService::new(self.store, self.namespace_id);
        for entry in op_log.collect_signed_group_ops_for_group(group_id)? {
            let NamespaceOp::Group { key_id, .. } = entry.signed_op.op else {
                continue;
            };
            let Some(group_key) = load_group_key_by_id(self.store, &gid_typed, &key_id)? else {
                continue;
            };
            let signed_op: SignedNamespaceOp = entry.signed_op;
            candidates.push(RetryCandidate {
                signed_op,
                group_key,
            });
        }

        Ok(candidates)
    }
}
