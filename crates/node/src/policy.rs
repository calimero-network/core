//! An organization's data policy: which modules may read its data, who may
//! receive outputs, which connections feed it and where results may go.
//!
//! Versions form a hash chain and each is approved by several admins'
//! passkeys, so no single person (and not Calimero) can change it alone.
//! This module is storage and signing only; enforcement reads
//! [`OrgDataPolicy::current`].

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::types::AccountId;

/// The longest canonical policy, in bytes.
pub const MAX_POLICY_BYTES: usize = 64 * 1024; // bounds what one org can put in the system state
/// The most entries in any one list of a policy.
pub const MAX_POLICY_ENTRIES: usize = 256; // readers, recipients, sources, sinks, sink origins
/// The most signers a policy names.
pub const MAX_SIGNERS: usize = 32; // keeps approval rounds practical
/// The longest single entry, in characters.
pub const MAX_ENTRY_CHARS: usize = 256; // a package name or connection name
/// Approvals the first version needs: no one person adopts a policy alone.
pub const FIRST_VERSION_APPROVALS: usize = 2; // met by two distinct admins
/// What a recovery signature names in the account field, so it can never be
/// an account's recovery start (account ids hold no `:`).
pub const RECOVERY_SUBJECT: &str = "data-policy:approve";

/// A refusal: the policy is not well formed or breaks a bound.
pub const BAD_POLICY: &str = "the data policy is malformed or too large";
/// A refusal: the actor may not take part in this policy's approval.
pub const NOT_A_SIGNER: &str = "only the policy's required approvers may do that";
/// A refusal: nothing is waiting for approval.
pub const NO_PENDING_POLICY: &str = "no data policy is waiting for approval";
/// A refusal: the approval names another proposal than the pending one.
pub const OTHER_POLICY: &str = "that is not the pending data policy";
/// A refusal: the version does not follow the current one.
pub const NOT_NEXT_VERSION: &str =
    "a policy is the next version of the current one, chained to its hash";
/// A refusal: this actor approved already.
pub const ALREADY_APPROVED: &str = "that account approved this data policy already";
/// A refusal: the current signers can still approve, so the recovery key may not.
pub const SIGNERS_CAN_APPROVE: &str =
    "the current policy's signers can still approve its successor";
/// A refusal: recovery approval needs a recovery key on the organization.
pub const NO_POLICY_RECOVERY: &str =
    "a first policy for an organization with fewer than two admins needs its recovery key";

/// What a data policy allows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataPolicy {
    pub version: u64,
    /// SHA-256 (hex) of the previous version's canonical JSON; none for the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev: Option<String>,
    /// Modules allowed to read org data: `pkg:<package>` or `sha256:<hash>`.
    #[serde(default)]
    pub readers: Vec<String>,
    /// Keys outputs may be wrapped to: `x25519:<hex>`.
    #[serde(default)]
    pub recipients: Vec<String>,
    /// Connection names data may be read from.
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub sinks: Vec<Sink>,
    /// Who must approve the next version.
    pub signers: Vec<AccountId>,
    pub threshold: u32,
}

/// A place results may be published, and the steps allowed to publish there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Sink {
    pub name: String,
    /// `<pipeline>.<step>`.
    #[serde(default)]
    pub from: Vec<String>,
}

/// A proposal and who approved it so far.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingPolicy {
    pub policy: DataPolicy,
    pub hash: String,
    pub proposer: AccountId,
    #[serde(default)]
    pub approvals: BTreeSet<AccountId>,
}

/// An organization's policy: the adopted version and any proposal.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgDataPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<DataPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<PendingPolicy>,
}

/// What the organization looks like to a proposal or an approval.
pub struct OrgView<'a> {
    /// Every admin and owner.
    pub admins: &'a BTreeSet<AccountId>,
    pub has_recovery: bool,
}

impl DataPolicy {
    /// The RFC 8785 canonical JSON.
    ///
    /// # Errors
    /// [`BAD_POLICY`] when it cannot be serialized.
    pub fn canonical(&self) -> Result<Vec<u8>, &'static str> {
        serde_json_canonicalizer::to_vec(self).map_err(|_| BAD_POLICY)
    }

    /// SHA-256 (hex) of [`Self::canonical`]: the version's identity.
    ///
    /// # Errors
    /// As [`Self::canonical`].
    pub fn hash(&self) -> Result<String, &'static str> {
        Ok(hex::encode(calimero_identity::sha256(&self.canonical()?)))
    }

    /// Bounds and entry formats, independent of any organization.
    fn check_shape(&self) -> Result<(), &'static str> {
        if self.canonical()?.len() > MAX_POLICY_BYTES {
            return Err(BAD_POLICY);
        }
        let lists = [&self.readers, &self.recipients, &self.sources];
        let sinks_ok = self.sinks.len() <= MAX_POLICY_ENTRIES
            && self.sinks.iter().all(|s| {
                entry_fits(&s.name)
                    && s.from.len() <= MAX_POLICY_ENTRIES
                    && s.from.iter().all(|f| step_fits(f))
            });
        let lists_ok = lists
            .iter()
            .all(|l| l.len() <= MAX_POLICY_ENTRIES && l.iter().all(|e| entry_fits(e)));
        let readers_ok = self.readers.iter().all(|r| reader_fits(r));
        let recipients_ok = self.recipients.iter().all(|r| recipient_fits(r));
        let signers: BTreeSet<_> = self.signers.iter().collect();
        let signers_ok = !self.signers.is_empty()
            && self.signers.len() <= MAX_SIGNERS
            && signers.len() == self.signers.len();
        let threshold_ok = self.threshold >= 1 && self.threshold as usize <= self.signers.len();
        if sinks_ok && lists_ok && readers_ok && recipients_ok && signers_ok && threshold_ok {
            Ok(())
        } else {
            Err(BAD_POLICY)
        }
    }
}

fn entry_fits(s: &str) -> bool {
    !s.is_empty() && s.chars().count() <= MAX_ENTRY_CHARS
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn reader_fits(r: &str) -> bool {
    match r.split_once(':') {
        Some(("pkg", name)) => !name.is_empty(),
        Some(("sha256", hash)) => is_hex64(hash),
        _ => false,
    }
}

fn recipient_fits(r: &str) -> bool {
    r.strip_prefix("x25519:").is_some_and(is_hex64)
}

fn step_fits(s: &str) -> bool {
    s.split_once('.')
        .is_some_and(|(p, st)| !p.is_empty() && !st.is_empty() && !st.contains('.'))
        && entry_fits(s)
}

impl OrgDataPolicy {
    /// Whether `account` is a required approver of the next version: an
    /// admin now and, after the first version, one of the current signers.
    fn is_approver(&self, account: &str, org: &OrgView<'_>) -> bool {
        org.admins.contains(account)
            && self
                .current
                .as_ref()
                .is_none_or(|c| c.signers.iter().any(|s| s == account))
    }

    /// How many approvals adopt the next version.
    fn needed(&self) -> usize {
        self.current
            .as_ref()
            .map_or(FIRST_VERSION_APPROVALS, |c| c.threshold as usize)
    }

    /// Check a proposal against the chain and the organization.
    fn check_proposal(
        &self,
        policy: &DataPolicy,
        actor: &str,
        org: &OrgView<'_>,
    ) -> Result<(), &'static str> {
        policy.check_shape()?;
        if !self.is_approver(actor, org) {
            return Err(NOT_A_SIGNER);
        }
        let next = self
            .current
            .as_ref()
            .map_or(1, |c| c.version.saturating_add(1));
        if policy.version != next || policy.prev != self.current_hash {
            return Err(NOT_NEXT_VERSION);
        }
        if !policy.signers.iter().all(|s| org.admins.contains(s)) {
            return Err(BAD_POLICY);
        }
        if self.current.is_none() {
            let alone = org.admins.len() < FIRST_VERSION_APPROVALS;
            if alone && !org.has_recovery {
                return Err(NO_POLICY_RECOVERY);
            }
            if !alone && (policy.threshold as usize) < FIRST_VERSION_APPROVALS {
                return Err(BAD_POLICY);
            }
        }
        Ok(())
    }

    /// Propose `policy`: it replaces any pending proposal and counts the
    /// proposer's consent as the first approval. Answers whether it was adopted.
    ///
    /// # Errors
    /// The policy is malformed, does not chain, or the actor may not propose.
    pub fn propose(
        &mut self,
        policy: DataPolicy,
        actor: &str,
        org: &OrgView<'_>,
    ) -> Result<bool, &'static str> {
        self.check_proposal(&policy, actor, org)?;
        let hash = policy.hash()?;
        self.pending = Some(PendingPolicy {
            policy,
            hash,
            proposer: actor.to_owned(),
            approvals: BTreeSet::from([actor.to_owned()]),
        });
        Ok(self.adopt_if_approved(org))
    }

    /// Add `actor`'s approval of the pending `hash`. Answers whether it was adopted.
    ///
    /// # Errors
    /// No such pending policy, an actor who is not a required approver, or
    /// one who approved already.
    pub fn approve(
        &mut self,
        hash: &str,
        actor: &str,
        org: &OrgView<'_>,
    ) -> Result<bool, &'static str> {
        let pending = self.pending.as_ref().ok_or(NO_PENDING_POLICY)?;
        if pending.hash != hash {
            return Err(OTHER_POLICY);
        }
        if !self.is_approver(actor, org) {
            return Err(NOT_A_SIGNER);
        }
        if pending.approvals.contains(actor) {
            return Err(ALREADY_APPROVED);
        }
        if let Some(p) = self.pending.as_mut() {
            let _new = p.approvals.insert(actor.to_owned());
        }
        Ok(self.adopt_if_approved(org))
    }

    /// Adopt the pending `hash` on the recovery key's say-so alone: the first version, or a
    /// later one whose current signers still admins cannot reach its threshold.
    ///
    /// # Errors
    /// No such pending policy, or signers who can still approve it.
    pub fn approve_by_recovery(
        &mut self,
        hash: &str,
        org: &OrgView<'_>,
    ) -> Result<(), &'static str> {
        let pending = self.pending.as_ref().ok_or(NO_PENDING_POLICY)?;
        if pending.hash != hash {
            return Err(OTHER_POLICY);
        }
        // After the first version the key only breaks a lockout, never outvotes live signers.
        if let Some(c) = &self.current {
            let live = c.signers.iter().filter(|s| org.admins.contains(*s)).count();
            if live >= c.threshold as usize {
                return Err(SIGNERS_CAN_APPROVE);
            }
        }
        self.adopt();
        Ok(())
    }

    /// Clear the pending proposal: its proposer's or any required approver's.
    ///
    /// # Errors
    /// Nothing pending, or an actor who is neither.
    pub fn withdraw(&mut self, actor: &str, org: &OrgView<'_>) -> Result<(), &'static str> {
        let pending = self.pending.as_ref().ok_or(NO_PENDING_POLICY)?;
        if pending.proposer != actor && !self.is_approver(actor, org) {
            return Err(NOT_A_SIGNER);
        }
        self.pending = None;
        Ok(())
    }

    /// Adopt the pending version once enough of its approvers (still
    /// qualifying now) approved it.
    fn adopt_if_approved(&mut self, org: &OrgView<'_>) -> bool {
        let counted = self.pending.as_ref().map_or(0, |p| {
            p.approvals
                .iter()
                .filter(|a| self.is_approver(a, org))
                .count()
        });
        let adopted = counted >= self.needed();
        if adopted {
            self.adopt();
        }
        adopted
    }

    fn adopt(&mut self) {
        if let Some(p) = self.pending.take() {
            self.current = Some(p.policy);
            self.current_hash = Some(p.hash);
        }
    }
}
