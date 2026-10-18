//! A group's owner at an op's cut, for the owner-only gates and the owner's immunities:
//! a live read would answer by whether a concurrent transfer landed first.

use std::collections::HashMap;

use calimero_account::AccountId;
use calimero_context_client::local_governance::{GroupOp, NamespaceOp, RootOp};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_types::NamespaceId;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use crate::authorizer::AtCutAuthorizer;
use crate::namespace::{order_causally, NamespaceOpLogService};
use crate::void_ledger::VoidLedger;
use crate::{MetaRepository, NamespaceRepository, OpenedNamespaceOp};

/// What `group`'s logged transfers say of its owner at a cut.
enum Logged {
    Owner(AccountId),
    NeverTransferred,
    Unread,
}

/// `group`'s owner at `parents`: the causally latest transfer of it there, else the account
/// its creation in the cut names; live only where the cut is sound.
pub(crate) fn owner_at_cut(
    store: &Store,
    group: &ContextGroupId,
    parents: &[[u8; 32]],
    authorizer: &dyn AtCutAuthorizer,
) -> EyreResult<Option<AccountId>> {
    if !parents.is_empty() {
        match owner_in_log(store, group, parents)? {
            Logged::Owner(owner) => return Ok(Some(owner)),
            // A node outside `group` is never served the key its sealed history needs, so
            // the namespace ops it judges (a deletion, a leave) read what its own rows hold.
            Logged::NeverTransferred | Logged::Unread => {
                if !authorizer.can_resolve_cut(group, parents)
                    && !authorizer.history_sealed_in_group(group, parents)
                {
                    bail!(undecidable(group));
                }
            }
        }
    }
    Ok(MetaRepository::new(store)
        .load(group)?
        .map(|meta| meta.owner_identity))
}

fn undecidable(group: &ContextGroupId) -> crate::ApplyError {
    crate::ApplyError::AuthorityUndecidable {
        group_id: group.to_string(),
        signer: "the group's owner at the op's cut".to_owned(),
    }
}

/// [`owner_at_cut`] from `group`'s applied transfers; undecidable when transfers exist,
/// none is in the cut and the cut does not read the account its creation names.
fn owner_in_log(store: &Store, group: &ContextGroupId, parents: &[[u8; 32]]) -> EyreResult<Logged> {
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
            return Ok(Logged::Unread);
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
        return Ok(Logged::NeverTransferred);
    }
    let Some(ancestry) = op_log.ancestry(parents)? else {
        return Ok(Logged::Unread);
    };
    let inside = transfers
        .into_iter()
        .filter(|op| op.content_hash().is_ok_and(|id| ancestry.contains_key(&id)))
        .collect();
    if let Some(latest) = order_causally(&op_log, inside)?.pop() {
        let id = latest
            .content_hash()
            .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
        return Ok(Logged::Owner(new_owners[&id]));
    }
    // Read from the log, not the creator rows: a store older than them holds none.
    ancestry
        .values()
        .find_map(|op| {
            let root = match &op.op {
                NamespaceOp::Root(root) => Some(root.clone()),
                _ => OpenedNamespaceOp::open(store, op).sealed_root,
            };
            root.and_then(|root| first_owner(&root, group, &namespace))
        })
        .map(Logged::Owner)
        .ok_or_else(|| undecidable(group).into())
}

/// The account `root` makes `group`'s first owner: a subgroup's creator, a namespace's founder.
fn first_owner(
    root: &RootOp,
    group: &ContextGroupId,
    namespace: &NamespaceId,
) -> Option<AccountId> {
    match root {
        RootOp::OnBehalf { op, .. } | RootOp::RootGuarded { op, .. } => {
            first_owner(op, group, namespace)
        }
        RootOp::GroupCreated {
            group_id, admin, ..
        } if group_id == group => Some(*admin),
        RootOp::NamespaceCreatedV2 { founder, .. }
        | RootOp::AccountNamespaceCreated { founder, .. }
            if group.to_bytes() == namespace.to_bytes() =>
        {
            Some(*founder)
        }
        _ => None,
    }
}
