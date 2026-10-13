//! What a removal voids on the apply path: a void op is logged and not applied,
//! and a removal arriving after the ops it voids rebuilds the rows they wrote.

use std::collections::{BTreeMap, BTreeSet};

use calimero_account::AccountId;
use calimero_context_client::local_governance::{GroupOp, NamespaceOp, RootOp, SignedNamespaceOp};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_op::{Op, OpPayload};
use calimero_op_adapter::payload_from_group_op;
use calimero_primitives::context::GroupMemberRole;
use calimero_storage::logical_clock::HybridTimestamp;
use eyre::Result as EyreResult;

use super::super::op_log::Hole;
use super::NamespaceGovernance;
use crate::authorizer::GroupRows;
use crate::op_budget::OpBudget;
use crate::void_ledger::{Parked, VoidLedger, STORED_BEFORE};
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
            | OpPayload::MemberLeft { .. }
            | OpPayload::MemberAdded { .. }
            | OpPayload::MemberCapabilitySet { .. }
            | OpPayload::DeviceRevoked { .. }
    )
}

/// The capability bits that admit a member who is no admin to `op`, for the kinds
/// the projection folds as nothing; it reads the rest from the payload. Mirrors the
/// `require_*` gates the apply runs.
fn capability_beyond_payload(op: &GroupOp, author: &AccountId) -> u32 {
    let bits = match op {
        GroupOp::TargetApplicationSet { .. }
        | GroupOp::GroupMigrationSet { .. }
        | GroupOp::CascadeUpgrade { .. } => MemberCapabilities::MANAGE_APPLICATION,
        GroupOp::GroupMetadataSet { .. } | GroupOp::ContextMetadataSet { .. } => {
            MemberCapabilities::CAN_MANAGE_METADATA
        }
        // A member sets its own metadata without one.
        GroupOp::MemberMetadataSet { member, .. } if member != author => {
            MemberCapabilities::CAN_MANAGE_METADATA
        }
        GroupOp::ContextRegistered { .. } => MemberCapabilities::CAN_CREATE_CONTEXT,
        GroupOp::ContextCapabilityGranted { .. } | GroupOp::ContextCapabilityRevoked { .. } => {
            MemberCapabilities::MANAGE_MEMBERS
        }
        // Its apply asks for a member in standing who holds `ADMIN` in the cell's
        // `prior` set, never a member capability, so no capability set voids it. A
        // concurrent removal or demotion of its signer still does.
        GroupOp::SharedWritersRotated { .. } => return 0,
        _ => return 0,
    };
    bits.bits()
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

    /// May the bytes of `op`, which this node cannot read, be kept? An op that is
    /// refused still takes its place in the log, so the DAG moves on.
    pub(super) fn admit_unreadable(&self, op: &SignedNamespaceOp) -> EyreResult<bool> {
        OpBudget::unreadable(self.store).admit_op(op)
    }

    /// Mark an op kept unread as parked, for a replay to judge; clear a mark an
    /// earlier, unfinished arrival of an op now read on arrival left behind.
    pub(super) fn mark_parked(
        &self,
        op: &SignedNamespaceOp,
        delta_id: [u8; 32],
        parked: bool,
    ) -> EyreResult<()> {
        let ledger = VoidLedger::new(self.store, self.namespace_id);
        if parked {
            ledger.note_parked(delta_id)
        } else if crate::unified_op_decode::can_be_parked(&op.op) {
            ledger.forget_parked(delta_id)
        } else {
            Ok(())
        }
    }

    /// Record a replay's verdict on a parked op, and drop what the authorizer folded
    /// when it changed. Logged rather than raised: an unrecorded verdict leaves a hole.
    pub(super) fn settle_parked(&self, op: &SignedNamespaceOp, applied: bool) {
        let settled = op
            .content_hash()
            .map_err(|e| eyre::eyre!("content_hash: {e}"))
            .and_then(|id| {
                VoidLedger::new(self.store, self.namespace_id).settle_parked(id, applied)
            });
        match settled {
            Ok(true) => self.authorizer.forget(),
            Ok(false) => {}
            Err(err) => tracing::warn!(
                namespace_id = %hex::encode(self.namespace_id.as_bytes()),
                %err,
                "could not record a replay's verdict on a parked op; it stays a hole"
            ),
        }
    }

    /// What a replay owes `op` if this node parked it and none has applied it yet. An
    /// unreadable mark reads as undecided: the op is still judged at its cut.
    pub(super) fn parked_state(&self, op: &SignedNamespaceOp) -> Option<Parked> {
        let parked = op
            .content_hash()
            .map_err(|e| eyre::eyre!("content_hash: {e}"))
            .and_then(|id| VoidLedger::new(self.store, self.namespace_id).parked(id));
        parked.unwrap_or_else(|err| {
            tracing::warn!(
                namespace_id = %hex::encode(self.namespace_id.as_bytes()),
                %err,
                "could not read whether an op is parked"
            );
            Some(Parked::Undecided)
        })
    }

    /// How `op` is stored: whole, or as the hole that keeps its place in the log when
    /// its bytes are over the unreadable budget, or over the void one for a void op.
    pub(super) fn storage_for(
        &self,
        op: &SignedNamespaceOp,
        delta_id: [u8; 32],
        unreadable_kept: bool,
    ) -> EyreResult<Option<Hole>> {
        if !unreadable_kept {
            return Ok(Some(Hole::Unreadable));
        }
        let ledger = VoidLedger::new(self.store, self.namespace_id);
        if !ledger.voided()?.contains(&delta_id) || OpBudget::void(self.store).admit_op(op)? {
            return Ok(None);
        }
        Ok(Some(Hole::Void))
    }

    /// Is `signed`, acting in `group`, void? `false` when the authorizer has no log.
    /// A void verdict is remembered, so a later removal can put the op back.
    pub(super) fn op_is_void(
        &self,
        group: &ContextGroupId,
        signed: &SignedNamespaceOp,
        decrypted: Option<&GroupOp>,
        opened_root: Option<&RootOp>,
        delta_id: [u8; 32],
    ) -> EyreResult<bool> {
        let op = self.unified_op(signed, decrypted, opened_root, delta_id);
        let capability =
            decrypted.map_or(0, |inner| capability_beyond_payload(inner, &op.author()));
        let void = self
            .authorizer
            .op_is_void(group, capability, &op)
            .unwrap_or(false);
        if void {
            VoidLedger::new(self.store, self.namespace_id).note_voided(delta_id)?;
        }
        Ok(void)
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

    /// Set what void ops wrote to what the log without them holds, and keep the keys
    /// they stored from being current. Covers ops judged void before. Idempotent.
    pub(super) fn reconcile_voided(
        &self,
        applied: Option<(&Op, &SignedNamespaceOp, [u8; 32])>,
    ) -> EyreResult<()> {
        let root = ContextGroupId::from(self.namespace_id.to_bytes());
        let extra = applied.map(|(op, _, _)| op);
        let ledger = VoidLedger::new(self.store, self.namespace_id);
        let intros = ledger.key_intros()?;
        let held: Vec<([u8; 32], ContextGroupId)> = intros
            .iter()
            .filter(|intro| intro.op != STORED_BEFORE)
            .map(|intro| (intro.op, ContextGroupId::from(intro.group)))
            .collect();
        let Some(voided) = self.authorizer.voided_ops(&root, extra, &held) else {
            return Ok(());
        };
        let earlier = ledger.voided()?;
        // A key is void when every op that stored it is.
        let mut keys: BTreeMap<([u8; 32], [u8; 32]), bool> = BTreeMap::new();
        for intro in &intros {
            let all_void = keys.entry((intro.group, intro.key)).or_insert(true);
            *all_void &= voided.contains(&intro.op);
        }
        for ((group, key), void) in keys {
            GroupKeyring::new(self.store, ContextGroupId::from(group))
                .set_key_voided(&key, void)?;
        }
        if voided.is_empty() && earlier.is_empty() {
            return Ok(());
        }

        let op_log = NamespaceOpLogService::new(self.store, self.namespace_id);
        let mut written: BTreeMap<ContextGroupId, Written> = BTreeMap::new();
        for id in voided.union(&earlier) {
            let signed = match applied {
                Some((_, signed, applied_id)) if *id == applied_id => Some(signed.clone()),
                _ => op_log.get_signed_op(*id)?,
            };
            let Some(signed) = signed else {
                // A void op over its budget has no bytes left; what it said is kept.
                if let Some(payload) = self.stored_payload(*id)? {
                    if let Some(group) = payload_written_group(&payload) {
                        record_write(written.entry(group).or_default(), &payload);
                    }
                }
                continue;
            };
            let NamespaceOp::Group {
                group_id,
                key_id,
                encrypted,
                ..
            } = &signed.op
            else {
                continue;
            };
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
            if let Some(payload) = payload_from_group_op(*group_id, &inner) {
                record_write(written.entry(*group_id).or_default(), &payload);
            }
        }

        // Every group's rows first: a rebuild that cannot read one changes nothing, and
        // leaves what it judged void for the next to act on.
        let mut plans = Vec::with_capacity(written.len());
        for (group, written) in &written {
            let Some(rows) = self.authorizer.group_rows(group, extra) else {
                return Ok(());
            };
            plans.push((group, written, rows));
        }

        for (group, written, rows) in &plans {
            self.rebuild_rows(group, written, rows)?;
        }
        ledger.set_voided(&voided)
    }

    /// What the unified log holds for op `id`, if it holds one.
    fn stored_payload(&self, id: [u8; 32]) -> EyreResult<Option<OpPayload>> {
        let scope = calimero_op::ScopeId::from(self.namespace_id.to_bytes());
        let key = calimero_store::key::ScopeUnifiedOp::new(*scope.as_bytes(), id);
        let handle = self.store.handle();
        let Some(value) = handle.get(&key)? else {
            return Ok(None);
        };
        let bytes: &[u8] = value.as_ref();
        Ok(borsh::from_slice::<Op>(bytes).ok().map(|op| op.payload))
    }

    fn rebuild_rows(
        &self,
        group: &ContextGroupId,
        written: &Written,
        rows: &GroupRows,
    ) -> EyreResult<()> {
        let membership = MembershipRepository::new(self.store);
        let capabilities = CapabilitiesRepository::new(self.store);

        if written.default_caps {
            // Another op's value stands; with none, the group gets back what it held
            // before the first change this node applied.
            let seed =
                VoidLedger::new(self.store, self.namespace_id).default_seed(group.to_bytes())?;
            let wanted = match (rows.default_caps, seed) {
                (Some(bits), _) => Some(Some(bits)),
                (None, Some(seed)) => Some(seed),
                (None, None) => None,
            };
            if let Some(wanted) = wanted {
                if capabilities.default_capabilities(group)? != wanted {
                    match wanted {
                        Some(bits) => capabilities.set_default_capabilities(group, bits)?,
                        None => capabilities.delete_default(group)?,
                    }
                }
            }
        }

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
                (None, Some(_)) if !rows.anchored.contains(account) => {
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

        Ok(())
    }
}

/// The group a payload writes rows in, if it is one that does.
fn payload_written_group(payload: &OpPayload) -> Option<ContextGroupId> {
    match payload {
        OpPayload::MemberAdded { group, .. }
        | OpPayload::MemberRemoved { group, .. }
        | OpPayload::MemberLeft { group, .. }
        | OpPayload::MemberCapabilitySet { group, .. }
        | OpPayload::DefaultCapabilitiesSet { group, .. } => Some(*group),
        _ => None,
    }
}

fn record_write(entry: &mut Written, payload: &OpPayload) {
    match payload {
        OpPayload::MemberAdded { member, .. }
        | OpPayload::MemberRemoved { member, .. }
        | OpPayload::MemberLeft { member, .. } => {
            let _ = entry.members.insert(*member);
        }
        OpPayload::MemberCapabilitySet { member, .. } => {
            let _ = entry.explicit_caps.insert(*member);
        }
        OpPayload::DefaultCapabilitiesSet { .. } => entry.default_caps = true,
        _ => {}
    }
}
