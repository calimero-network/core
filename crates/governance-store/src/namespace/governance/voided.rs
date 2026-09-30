//! What a removal voids on the apply path: a void op is logged and not applied,
//! and a removal arriving after the ops it voids rebuilds the rows they wrote.

use std::collections::{BTreeMap, BTreeSet};

use calimero_account::AccountId;
use calimero_context_client::local_governance::{GroupOp, NamespaceOp, RootOp, SignedNamespaceOp};
use calimero_context_config::types::ContextGroupId;
use calimero_op::{Op, OpPayload};
use calimero_op_adapter::payload_from_group_op;
use calimero_primitives::context::GroupMemberRole;
use calimero_storage::logical_clock::HybridTimestamp;
use eyre::Result as EyreResult;

use super::NamespaceGovernance;
use crate::authorizer::GroupRows;
use crate::{
    cascade_remove_member_from_group_tree, restore_member_context_identities,
    CapabilitiesRepository, GroupKeyring, MembershipRepository, NamespaceOpLogService,
    ReentryRepository,
};

/// Can `op` take authority away from someone, and so void ops already applied?
pub(super) fn may_void_others(op: &Op) -> bool {
    matches!(
        op.payload,
        OpPayload::MemberRemoved { .. }
            | OpPayload::MemberAdded { .. }
            | OpPayload::DeviceRevoked { .. }
    )
}

/// What the void ops of one group wrote.
#[derive(Default)]
struct Written {
    members: BTreeSet<AccountId>,
    explicit_caps: BTreeSet<AccountId>,
    default_caps: bool,
}

impl NamespaceGovernance<'_> {
    /// `signed` as the unified log holds it.
    pub(super) fn unified_op(
        &self,
        signed: &SignedNamespaceOp,
        decrypted: Option<&GroupOp>,
        opened_root: Option<&RootOp>,
        delta_id: [u8; 32],
    ) -> Op {
        let namespace = ContextGroupId::from(self.namespace_id.to_bytes());
        let binding =
            crate::unified_op_decode::signer_binding_for(self.store, &namespace, &signed.signer);
        crate::unified_op_decode::op_from_namespace_op_with_binding(
            signed,
            decrypted,
            opened_root,
            binding,
            delta_id,
            HybridTimestamp::default(),
            &signed.parent_op_hashes,
        )
    }

    /// Is `signed`, acting in `group`, void? `false` when the authorizer has no log.
    pub(super) fn op_is_void(
        &self,
        group: &ContextGroupId,
        signed: &SignedNamespaceOp,
        decrypted: Option<&GroupOp>,
        opened_root: Option<&RootOp>,
        delta_id: [u8; 32],
    ) -> EyreResult<bool> {
        let op = self.unified_op(signed, decrypted, opened_root, delta_id);
        Ok(self.authorizer.op_is_void(group, &op).unwrap_or(false))
    }

    /// [`Self::op_is_void`] for a root op, which acts in the namespace root.
    /// A join is not judged: the invitation it presents is what admits it.
    pub(super) fn root_op_is_void(
        &self,
        signed: &SignedNamespaceOp,
        root: &RootOp,
        delta_id: [u8; 32],
    ) -> EyreResult<bool> {
        if matches!(
            root,
            RootOp::MemberJoined { .. }
                | RootOp::MemberJoinedAt { .. }
                | RootOp::MemberJoinedOpen { .. }
                | RootOp::MemberJoinedViaTeeAttestation { .. }
                | RootOp::NamespaceCreatedV2 { .. }
        ) {
            return Ok(false);
        }
        let group = ContextGroupId::from(self.namespace_id.to_bytes());
        self.op_is_void(&group, signed, None, Some(root), delta_id)
    }

    /// Set what the void ops wrote (members, roles, capabilities) to what the log
    /// without them holds, and void their rotated keys. Idempotent.
    pub(super) fn reconcile_voided(
        &self,
        applied: Option<(&Op, &SignedNamespaceOp, [u8; 32])>,
    ) -> EyreResult<()> {
        let root = ContextGroupId::from(self.namespace_id.to_bytes());
        let extra = applied.map(|(op, _, _)| op);
        let Some(voided) = self.authorizer.voided_ops(&root, extra) else {
            return Ok(());
        };
        if voided.is_empty() {
            return Ok(());
        }

        let op_log = NamespaceOpLogService::new(self.store, self.namespace_id);
        let mut written: BTreeMap<ContextGroupId, Written> = BTreeMap::new();
        let mut rotated: Vec<(ContextGroupId, [u8; 32])> = Vec::new();
        for id in &voided {
            let signed = match applied {
                Some((_, signed, applied_id)) if *id == applied_id => Some(signed.clone()),
                _ => op_log.get_signed_op(*id)?,
            };
            let Some(signed) = signed else { continue };
            let NamespaceOp::Group {
                group_id,
                key_id,
                encrypted,
                key_rotation,
            } = &signed.op
            else {
                continue;
            };
            if let Some(rotation) = key_rotation {
                rotated.push((*group_id, rotation.new_key_id.to_bytes()));
            }
            // An op this node cannot read wrote nothing here.
            let Some(inner) = crate::decrypt_group_op(
                self.store,
                self.namespace_id,
                *group_id,
                key_id.as_bytes(),
                encrypted,
            )
            .ok()
            .flatten() else {
                continue;
            };
            let entry = written.entry(*group_id).or_default();
            match payload_from_group_op(*group_id, &inner) {
                Some(OpPayload::MemberAdded { member, .. })
                | Some(OpPayload::MemberRemoved { member, .. }) => {
                    let _ = entry.members.insert(member);
                }
                Some(OpPayload::MemberCapabilitySet { member, .. }) => {
                    let _ = entry.explicit_caps.insert(member);
                }
                Some(OpPayload::DefaultCapabilitiesSet { .. }) => entry.default_caps = true,
                _ => {}
            }
        }

        for (group, written) in &written {
            let Some(rows) = self.authorizer.group_rows(group, extra) else {
                continue;
            };
            self.rebuild_rows(group, written, &rows)?;
        }
        for (group, key_id) in rotated {
            GroupKeyring::new(self.store, group).void_key(&key_id)?;
        }
        Ok(())
    }

    fn rebuild_rows(
        &self,
        group: &ContextGroupId,
        written: &Written,
        rows: &GroupRows,
    ) -> EyreResult<()> {
        let membership = MembershipRepository::new(self.store);
        let capabilities = CapabilitiesRepository::new(self.store);

        for account in &written.members {
            let wanted = rows.members.get(account);
            let held = membership.role_of(group, account)?;
            match (wanted, held) {
                (Some(role), None) => {
                    // As an admin re-adding the member would: the row, the lifted
                    // re-entry block a removal wrote, and the member's contexts.
                    membership.add_member(group, account, role.clone())?;
                    ReentryRepository::new(self.store).clear_block(group, account)?;
                    restore_member_context_identities(self.store, group, account)?;
                }
                (Some(role), Some(held)) if held != *role => {
                    membership.set_role(group, account, role.clone())?;
                }
                (None, Some(_)) => {
                    cascade_remove_member_from_group_tree(self.store, group, account)?;
                    membership.remove_member(group, account)?;
                }
                _ => {}
            }
        }

        for account in &written.explicit_caps {
            let Some(role) = rows.members.get(account) else {
                continue;
            };
            // A member without a grant of its own holds the default the row was
            // seeded with, as a join writes it.
            let seeded = (*role != GroupMemberRole::Admin)
                .then(|| capabilities.default_capabilities(group))
                .transpose()?
                .flatten()
                .filter(|defaults| *defaults != 0);
            let wanted = rows.member_caps.get(account).copied().or(seeded);
            if wanted != capabilities.member_capability(group, account)? {
                match wanted {
                    Some(bits) => capabilities.set_member_capability(group, account, bits)?,
                    None => capabilities.delete_member_capability(group, account)?,
                }
            }
        }

        if written.default_caps && rows.default_caps != capabilities.default_capabilities(group)? {
            match rows.default_caps {
                Some(bits) => capabilities.set_default_capabilities(group, bits)?,
                None => capabilities.delete_default(group)?,
            }
        }
        Ok(())
    }
}
