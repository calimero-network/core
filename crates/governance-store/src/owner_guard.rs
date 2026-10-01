//! The root guard on owner-level governance ops.
//!
//! # Why it exists
//!
//! Every governance gate resolves the signing device to its account, and a
//! device key is the account for every purpose but this one. So a stolen owner
//! device could promote an accomplice, transfer the group to them, then demote or
//! remove the owner, and the owner could do nothing about it. The ops that can
//! take a group or a namespace from its owner are therefore refused unless they
//! carry an authorisation signed by the account's **root** key
//! ([`calimero_account::OwnerOpAuthorization`]): `TransferOwnership`,
//! `AdminChanged`, `GroupDelete`, and the TEE policy ops. The TEE policy ops stay
//! admin-level; for them the proof must come from the signing admin's own
//! account, not from the owner.
//!
//! This protects against a leaked or stolen device key: a paired device, a
//! nodeless device, or a device key copied off a machine. When the node itself
//! holds the account root (`merod init` provisions one) the node can mint the
//! proof on the owner's behalf, so compromise of *that node* is not covered. The
//! root is on it.
//!
//! # What is checked, and where each check gets its answer
//!
//! The proof is self-certifying, so most of it is decided from the op alone, the
//! same on every replica: the genesis addresses the account, the chain is valid
//! and reaches the signing epoch, the root key at that epoch signed it, and the
//! statement names this namespace, this group, this kind of op and a digest of
//! this op's own bytes. Two further checks read replica state, and both are
//! trade-offs, not oversights:
//!
//! - **The counter.** The proof names the group's count of guarded ops applied so
//!   far, and applying the op advances it. That makes every proof single-use
//!   without an expiry, so a root that never touches a node can sign one offline.
//!   The count is this replica's, like the per-signer nonce window. It is not read
//!   at the op's cut, because an attacker picks an op's parents and could cite an
//!   old cut to replay a spent proof. The cost is that two guarded ops signed
//!   concurrently for the same count race: each replica keeps whichever it applies
//!   first. An owner performs these ops one at a time, from one root, after reading
//!   the counter, so this does not arise in normal use.
//! - **The recorded epoch.** Any root epoch the chain reaches may sign, as for a
//!   device revocation (see `calimero_account::revocation`). In addition, the chain
//!   must reach the epoch this group has already recorded for the account, and
//!   reach it with the key the group recorded, so a proof whose chain stops before
//!   a rotation this replica has folded, or forks from it, is refused. What a
//!   replica has recorded depends on which rotations it has folded, so two replicas
//!   can briefly disagree: one that has not yet folded a rotation accepts a proof
//!   that stops just before it, and one that has refuses it. The owner avoids that
//!   by always presenting the full chain.

use calimero_account::{root_key_at_epoch, AccountError, AccountId, OwnerOpKind, SignedOwnerOp};
use calimero_context_config::types::ContextGroupId;
use calimero_store::key::GroupOwnerOpCounter;
use calimero_store::Store;
use eyre::Result as EyreResult;
use thiserror::Error;

use crate::AccountBindingRepository;

/// Why an owner-level op was refused by the root guard.
#[derive(Debug, Error)]
pub enum OwnerGuardRefusal {
    /// A guarded op arrived in bare form.
    #[error(
        "{kind} is an owner-level op and needs a root-signed proof from the account \
         (a RootGuarded op); a device key alone cannot perform it"
    )]
    ProofRequired {
        /// The op's kind.
        kind: &'static str,
    },
    /// A `RootGuarded` wrapper carried an op that is not a guarded kind,
    /// including another wrapper.
    #[error("a root-guarded op must carry an owner-level op, not {inner}")]
    NotAGuardedKind {
        /// The op the wrapper carried.
        inner: &'static str,
    },
    /// The signing key speaks for no account here, so no proof can be its.
    #[error("the signing device speaks for no account in this group")]
    SignerUnbound,
    /// The proof is some other account's.
    #[error(
        "the root proof is signed for account {proof}, but the signing device speaks for \
         {signer}; the proof must come from the signer's own account"
    )]
    ProofAccountMismatch {
        /// The account the proof names.
        proof: AccountId,
        /// The account the signing device speaks for.
        signer: AccountId,
    },
    /// The proof does not verify.
    #[error("the root proof does not verify: {0}")]
    ProofInvalid(AccountError),
    /// The proof is genuine but for something else.
    #[error("the root proof is for {what} {named}, not {expected}")]
    ProofMismatch {
        /// Which term disagreed.
        what: &'static str,
        /// What the proof names.
        named: String,
        /// What this op needs.
        expected: String,
    },
    /// The proof names a counter the group is not at: spent, or signed for a
    /// later op.
    #[error(
        "the root proof is for guarded-op counter {found}, but this group is at {expected}; \
         read the current counter and sign a fresh proof"
    )]
    StaleCounter {
        /// The group's counter.
        expected: u64,
        /// The counter the proof names.
        found: u64,
    },
    /// The proof's chain stops before the epoch this group has recorded.
    #[error(
        "the root proof's chain does not reach epoch {recorded}, which this group has \
         recorded for account {account}; present the full handoff chain"
    )]
    BelowRecordedEpoch {
        /// The account.
        account: AccountId,
        /// The epoch the group recorded.
        recorded: u32,
    },
    /// The proof's chain reaches the recorded epoch with a different key.
    #[error(
        "the root proof's chain names a different root key at epoch {epoch} than the one \
         this group recorded for account {account}"
    )]
    ForkedChain {
        /// The account.
        account: AccountId,
        /// The recorded epoch at which the keys disagree.
        epoch: u32,
    },
}

/// How many guarded ops `group` has applied: the counter the next proof must
/// name. Zero for a group that has applied none.
///
/// # Errors
/// Propagates the store read failure.
pub fn owner_op_counter(store: &Store, group: &ContextGroupId) -> EyreResult<u64> {
    Ok(store
        .handle()
        .get(&GroupOwnerOpCounter::new(group.to_bytes()))?
        .unwrap_or(0))
}

/// Spend the current counter. Called once the guarded op has applied, so an op
/// that fails, or parks to be retried, leaves its proof valid.
pub(crate) fn advance_owner_op_counter(store: &Store, group: &ContextGroupId) -> EyreResult<()> {
    let next = owner_op_counter(store, group)?.saturating_add(1);
    store
        .handle()
        .put(&GroupOwnerOpCounter::new(group.to_bytes()), &next)?;
    Ok(())
}

/// What a guarded op must be authorised for.
#[derive(Clone, Copy, Debug)]
pub struct GuardedOp {
    /// The namespace the op is published in.
    pub namespace: ContextGroupId,
    /// The group whose counter the op spends.
    pub group: ContextGroupId,
    /// The kind the op is.
    pub kind: OwnerOpKind,
    /// The digest of the op's own bytes.
    pub digest: [u8; 32],
}

fn mismatch(what: &'static str, named: String, expected: String) -> EyreResult<()> {
    Err(OwnerGuardRefusal::ProofMismatch {
        what,
        named,
        expected,
    }
    .into())
}

/// Check `proof` authorises `op` for `signer_account`. See the module docs for
/// what each check reads.
///
/// The apply path's check, and also the publisher's: a node verifies a proof
/// before it publishes, so a bad one is refused to the caller who can fix it
/// rather than to every replica.
///
/// # Errors
/// An [`OwnerGuardRefusal`] naming the first check that failed, or a store
/// failure.
pub fn check_root_proof(
    store: &Store,
    signer_account: AccountId,
    op: GuardedOp,
    proof: &SignedOwnerOp,
) -> EyreResult<()> {
    let statement = &proof.statement;

    // The account first: a proof from any other account is not this signer's,
    // however genuine. Checked before verifying, so a foreign proof costs no
    // signature check.
    if statement.account != signer_account {
        return Err(OwnerGuardRefusal::ProofAccountMismatch {
            proof: statement.account,
            signer: signer_account,
        }
        .into());
    }
    let _verified = proof
        .verify(signer_account)
        .map_err(OwnerGuardRefusal::ProofInvalid)?;

    if statement.namespace_id != op.namespace.to_bytes() {
        return mismatch(
            "namespace",
            hex::encode(statement.namespace_id),
            hex::encode(op.namespace.to_bytes()),
        );
    }
    if statement.group_id != op.group.to_bytes() {
        return mismatch(
            "group",
            hex::encode(statement.group_id),
            hex::encode(op.group.to_bytes()),
        );
    }
    if statement.kind != op.kind {
        return mismatch(
            "op kind",
            statement.kind.label().to_owned(),
            op.kind.label().to_owned(),
        );
    }
    if statement.op_digest != op.digest {
        return mismatch(
            "op digest",
            hex::encode(statement.op_digest),
            hex::encode(op.digest),
        );
    }

    let expected = owner_op_counter(store, &op.group)?;
    if statement.counter != expected {
        return Err(OwnerGuardRefusal::StaleCounter {
            expected,
            found: statement.counter,
        }
        .into());
    }

    // The floor. The group's own record and the namespace root's both count:
    // a rotation folds wherever the account's ops land, and a subgroup may
    // not hold a row of its own.
    let bindings = AccountBindingRepository::new(store);
    let mut recorded = bindings.account_key(&op.group, signer_account)?;
    if op.group != op.namespace {
        if let Some(at_root) = bindings.account_key(&op.namespace, signer_account)? {
            if recorded.is_none_or(|(epoch, _)| at_root.0 > epoch) {
                recorded = Some(at_root);
            }
        }
    }
    if let Some((epoch, root_pk)) = recorded {
        let reached = root_key_at_epoch(&proof.genesis, &proof.chain, epoch).map_err(|_| {
            OwnerGuardRefusal::BelowRecordedEpoch {
                account: signer_account,
                recorded: epoch,
            }
        })?;
        if reached != root_pk {
            return Err(OwnerGuardRefusal::ForkedChain {
                account: signer_account,
                epoch,
            }
            .into());
        }
    }
    Ok(())
}
