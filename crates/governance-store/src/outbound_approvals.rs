//! The URL prefixes a namespace lets each publisher's apps send outbound requests
//! under, as `GroupOp::OutboundApprovalSet` sets them on the namespace root.
//!
//! One row per (root, publisher signer), replaced whole by each apply. The node's
//! outbound gate reads it for every run that may send, so it is materialized here
//! rather than read back from the op log.

use calimero_context_config::types::ContextGroupId;
use calimero_store::key::{
    GroupOutboundApproval, GroupOutboundApprovalValue, GROUP_OUTBOUND_APPROVAL_PREFIX,
};
use calimero_store::Store;
use eyre::Result as EyreResult;
use sha2::{Digest, Sha256};

use super::collect_keys_with_prefix;

/// What a namespace approved for one publisher.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OutboundApproval {
    /// The publisher's signer id (`did:key`).
    pub signer: String,
    /// The approved URL prefixes.
    pub prefixes: Vec<String>,
    /// The prefixes the approver was shown, approved or not.
    pub reviewed: Vec<String>,
}

impl From<GroupOutboundApprovalValue> for OutboundApproval {
    fn from(value: GroupOutboundApprovalValue) -> Self {
        Self {
            signer: value.signer,
            prefixes: value.prefixes,
            reviewed: value.reviewed,
        }
    }
}

/// Typed repository for a namespace's outbound approvals.
pub struct OutboundApprovalRepository<'a> {
    store: &'a Store,
}

impl<'a> OutboundApprovalRepository<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    fn key(root: &ContextGroupId, signer: &str) -> GroupOutboundApproval {
        GroupOutboundApproval::new(root.to_bytes(), Sha256::digest(signer.as_bytes()).into())
    }

    /// Replace what `root` approves for `approval.signer`.
    pub fn set(&self, root: &ContextGroupId, approval: &OutboundApproval) -> EyreResult<()> {
        self.store.handle().put(
            &Self::key(root, &approval.signer),
            &GroupOutboundApprovalValue {
                signer: approval.signer.clone(),
                prefixes: approval.prefixes.clone(),
                reviewed: approval.reviewed.clone(),
            },
        )?;
        Ok(())
    }

    /// What `root` approves for `signer`, `None` when nothing was ever set.
    pub fn get(&self, root: &ContextGroupId, signer: &str) -> EyreResult<Option<OutboundApproval>> {
        let value = self.store.handle().get(&Self::key(root, signer))?;
        // A hash collision is not a real risk, but the row names its signer, so
        // check it rather than reason about it.
        Ok(value
            .filter(|value| value.signer == signer)
            .map(OutboundApproval::from))
    }

    /// Every publisher `root` has an approval row for, ordered by signer hash.
    pub fn list(&self, root: &ContextGroupId) -> EyreResult<Vec<OutboundApproval>> {
        let handle = self.store.handle();
        let mut approvals = Vec::new();
        for key in self.keys(root)? {
            if let Some(value) = handle.get(&key)? {
                approvals.push(value.into());
            }
        }
        Ok(approvals)
    }

    /// Delete every approval row of `root`, with the group.
    pub fn delete_all_for_group(&self, root: &ContextGroupId) -> EyreResult<()> {
        let keys = self.keys(root)?;
        let mut handle = self.store.handle();
        for key in keys {
            handle.delete(&key)?;
        }
        Ok(())
    }

    fn keys(&self, root: &ContextGroupId) -> EyreResult<Vec<GroupOutboundApproval>> {
        let gid = root.to_bytes();
        collect_keys_with_prefix(
            self.store,
            GroupOutboundApproval::new(gid, [0u8; 32]),
            GROUP_OUTBOUND_APPROVAL_PREFIX,
            |k| k.group_id() == gid,
        )
    }
}
