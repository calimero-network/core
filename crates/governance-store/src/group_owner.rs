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
use crate::{
    GenesisKind, MetaRepository, NamespaceFoundingRepository, NamespaceRepository,
    OpenedNamespaceOp,
};

/// What `group`'s logged transfers say of its owner at a cut.
enum Logged {
    Owner(AccountId),
    NeverTransferred,
    Unknown,
}

/// `group`'s owner at `parents`: the causally latest transfer of it there, else the account
/// its creation in the cut names; live only where no transfer moved it or the cut is sound.
pub(crate) fn owner_at_cut(
    store: &Store,
    group: &ContextGroupId,
    parents: &[[u8; 32]],
    authorizer: &dyn AtCutAuthorizer,
) -> EyreResult<Option<AccountId>> {
    if !parents.is_empty() {
        match owner_in_log(store, group, parents)? {
            Logged::Owner(owner) => return Ok(Some(owner)),
            // No transfer moved the live row, whatever the cut lacks.
            Logged::NeverTransferred => {}
            // A node outside `group` is never served the key its sealed history needs, so
            // the namespace ops it judges (a deletion, a leave) read what its own rows hold.
            Logged::Unknown => {
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

/// The owner a transfer in the log moved `group` to at `parents`, and `None` where none did: the
/// creator then owns it. Never reads the live meta; undecidable where the cut cannot say.
pub(crate) fn transferred_owner_at_cut(
    store: &Store,
    group: &ContextGroupId,
    parents: &[[u8; 32]],
    authorizer: &dyn AtCutAuthorizer,
) -> EyreResult<Option<AccountId>> {
    if parents.is_empty() {
        return Ok(None);
    }
    match owner_in_log(store, group, parents)? {
        Logged::Owner(owner) => Ok(Some(owner)),
        Logged::NeverTransferred => Ok(None),
        Logged::Unknown => {
            if !authorizer.can_resolve_cut(group, parents)
                && !authorizer.history_sealed_in_group(group, parents)
            {
                bail!(undecidable(group));
            }
            Ok(None)
        }
    }
}

/// The account that founded `namespace`: what its founding row or its logged genesis names,
/// else the live meta of a store holding neither. A transfer never moves it.
pub fn namespace_founder(
    store: &Store,
    namespace: &ContextGroupId,
) -> EyreResult<Option<AccountId>> {
    if let Some((founder, _)) = NamespaceFoundingRepository::new(store).get(namespace)? {
        return Ok(Some(founder));
    }
    let id = NamespaceId::from(namespace.to_bytes());
    let logged = NamespaceOpLogService::new(store, id)
        .collect_root_ops()?
        .into_iter()
        .find_map(|op| match op.op {
            NamespaceOp::Root(root) => first_owner(&root, namespace, &id),
            _ => None,
        });
    if logged.is_some() {
        return Ok(logged);
    }
    Ok(MetaRepository::new(store)
        .load(namespace)?
        .map(|meta| meta.admin_identity))
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
    let mut moved_by_void = false;
    for entry in op_log.collect_signed_group_ops_for_group(group.to_bytes())? {
        let id = entry
            .signed_op
            .content_hash()
            .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
        let opened = OpenedNamespaceOp::open(store, &entry.signed_op);
        let void = voided.contains(&id);
        if opened.refused {
            continue;
        }
        let Some(op) = opened.group_op else {
            if void {
                continue;
            }
            return Ok(Logged::Unknown);
        };
        let op = match op.into_unguarded() {
            GroupOp::OnBehalf { op, .. } => *op,
            op => op,
        };
        if let GroupOp::TransferOwnership { new_owner } = op {
            // A void transfer that arrived before its voiding op still moved the live row.
            moved_by_void |= void;
            if !void {
                let _ = new_owners.insert(id, new_owner);
                transfers.push(entry.signed_op);
            }
        }
    }
    if transfers.is_empty() {
        return Ok(if moved_by_void {
            Logged::Unknown
        } else {
            Logged::NeverTransferred
        });
    }
    let Some(ancestry) = op_log.ancestry(parents)? else {
        return Ok(Logged::Unknown);
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
        // A genesis logged on an established namespace applied as a no-op, so only one
        // that derives the id names its founder.
        RootOp::NamespaceCreatedV2 { founder, salt, .. }
            if group.to_bytes() == namespace.to_bytes()
                && GenesisKind::Ordinary.derives(&namespace.to_bytes(), founder, salt) =>
        {
            Some(*founder)
        }
        RootOp::AccountNamespaceCreated { founder, salt, .. }
            if group.to_bytes() == namespace.to_bytes()
                && GenesisKind::AccountNamespace.derives(&namespace.to_bytes(), founder, salt) =>
        {
            Some(*founder)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use calimero_context_client::local_governance::SignedNamespaceOp;
    use calimero_primitives::identity::PrivateKey;

    use super::*;
    use crate::namespace::NamespaceGovernance;
    use crate::test_fixtures::{namespace_genesis_v2_for, sample_meta_with_admin, test_store};

    const SALT: [u8; 32] = [0x5D; 32];

    fn sk(seed: u8) -> PrivateKey {
        PrivateKey::from([seed; 32])
    }

    /// `founder`'s genesis, signed, and the namespace it founds.
    fn genesis(founder: &PrivateKey) -> (SignedNamespaceOp, AccountId, ContextGroupId) {
        let (op, account, id) = namespace_genesis_v2_for(founder, SALT);
        let signed = SignedNamespaceOp::sign(founder, id.into(), vec![], 0, op).expect("sign");
        (signed, account, ContextGroupId::from(id))
    }

    fn move_meta_admin(store: &Store, namespace: &ContextGroupId, to: AccountId) {
        MetaRepository::new(store)
            .save(namespace, &sample_meta_with_admin(to))
            .expect("write the meta");
    }

    #[test]
    fn the_founder_outlives_a_transfer_of_the_meta_admin() {
        let store = test_store();
        let (signed, founder, namespace) = genesis(&sk(0x51));
        let _applied = NamespaceGovernance::new(&store, namespace.to_bytes().into())
            .apply_signed_op(&signed)
            .expect("the genesis applies");
        move_meta_admin(&store, &namespace, AccountId::from([0x52; 32]));

        assert_eq!(
            namespace_founder(&store, &namespace).unwrap(),
            Some(founder)
        );
    }

    #[test]
    fn without_a_founding_row_the_logged_genesis_names_the_founder() {
        let store = test_store();
        let (signed, founder, namespace) = genesis(&sk(0x53));
        NamespaceOpLogService::new(&store, namespace.to_bytes().into())
            .store_signed_operation(&signed)
            .expect("log the genesis");
        move_meta_admin(&store, &namespace, AccountId::from([0x54; 32]));

        assert_eq!(
            namespace_founder(&store, &namespace).unwrap(),
            Some(founder)
        );
    }

    #[test]
    fn a_logged_genesis_that_does_not_derive_the_id_names_no_founder() {
        let store = test_store();
        let (_, _, namespace) = genesis(&sk(0x55));
        let (forged, _, _) = namespace_genesis_v2_for(&sk(0x56), SALT);
        let forged =
            SignedNamespaceOp::sign(&sk(0x56), namespace.to_bytes().into(), vec![], 0, forged)
                .expect("sign");
        NamespaceOpLogService::new(&store, namespace.to_bytes().into())
            .store_signed_operation(&forged)
            .expect("log the forgery");
        let live = AccountId::from([0x57; 32]);
        move_meta_admin(&store, &namespace, live);

        assert_eq!(namespace_founder(&store, &namespace).unwrap(), Some(live));
    }
}
