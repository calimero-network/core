//! A group's owner at an op's cut, for the gates that only the owner passes or that the
//! owner is immune to. The owner row is moved by `TransferOwnership`, so a live read
//! answers by whether a concurrent transfer landed first.

use std::collections::HashMap;

use calimero_account::AccountId;
use calimero_context_client::local_governance::GroupOp;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_types::NamespaceId;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use crate::authorizer::AtCutAuthorizer;
use crate::group_creator::GroupCreatorRepository;
use crate::namespace::{order_causally, NamespaceOpLogService};
use crate::void_ledger::VoidLedger;
use crate::{MetaRepository, NamespaceFoundingRepository, NamespaceRepository, OpenedNamespaceOp};

/// `group`'s owner at `parents`: the causally latest transfer of it there, else the account
/// it began with; live only where no transfer moved it or the cut is sound.
pub(crate) fn owner_at_cut(
    store: &Store,
    group: &ContextGroupId,
    parents: &[[u8; 32]],
    authorizer: &dyn AtCutAuthorizer,
) -> EyreResult<Option<AccountId>> {
    if !parents.is_empty() {
        if let Some(owner) = owner_in_log(store, group, parents)? {
            return Ok(Some(owner));
        }
        if !authorizer.can_resolve_cut(group, parents) {
            bail!(crate::ApplyError::AuthorityUndecidable {
                group_id: group.to_string(),
                signer: "the group's owner at the op's cut".to_owned(),
            });
        }
    }
    Ok(MetaRepository::new(store)
        .load(group)?
        .map(|meta| meta.owner_identity))
}

/// [`owner_at_cut`] from `group`'s applied transfers. `None` when none was applied, an op
/// of `group` or the cut does not read here, or the first owner is not recorded.
fn owner_in_log(
    store: &Store,
    group: &ContextGroupId,
    parents: &[[u8; 32]],
) -> EyreResult<Option<AccountId>> {
    let namespace = NamespaceId::from(NamespaceRepository::new(store).resolve(group)?.to_bytes());
    let op_log = NamespaceOpLogService::new(store, namespace);
    let voided = VoidLedger::new(store, namespace).voided()?;
    let mut new_owners = HashMap::new();
    let mut transfers = Vec::new();
    for entry in op_log.collect_signed_group_ops_for_group(group.to_bytes())? {
        let id = entry
            .signed_op
            .content_hash()
            .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
        let opened = OpenedNamespaceOp::open(store, &entry.signed_op);
        if opened.refused || voided.contains(&id) {
            continue;
        }
        let Some(op) = opened.group_op else {
            return Ok(None);
        };
        let op = match op.into_unguarded() {
            GroupOp::OnBehalf { op, .. } => *op,
            op => op,
        };
        if let GroupOp::TransferOwnership { new_owner } = op {
            let _ = new_owners.insert(id, new_owner);
            transfers.push(entry.signed_op);
        }
    }
    if transfers.is_empty() {
        return Ok(None);
    }
    let Some(ancestry) = op_log.ancestry(parents)? else {
        return Ok(None);
    };
    let inside = transfers
        .into_iter()
        .filter(|op| op.content_hash().is_ok_and(|id| ancestry.contains_key(&id)))
        .collect();
    if let Some(latest) = order_causally(&op_log, inside)?.pop() {
        let id = latest
            .content_hash()
            .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
        return Ok(Some(new_owners[&id]));
    }
    // A subgroup begins owned by its creator, a namespace by its founder.
    if let Some(creator) = GroupCreatorRepository::new(store).creator(group)? {
        return Ok(Some(creator));
    }
    Ok(NamespaceFoundingRepository::new(store)
        .get(group)?
        .map(|(founder, _)| founder))
}
