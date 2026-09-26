//! The TEE authority is resolved at a delta's cut, not against current state.
//!
//! A TEE write merges as `TEE_AUTHORITY` only when the cut the delta cites makes
//! its signer a TEE authority: a `ReadOnlyTee` member of the namespace root with
//! verified evidence whose MRTD the authoring policy names, and whose key speaks
//! for it. Two peers that have applied a later policy change to different depths
//! must still resolve the same write the same way.

use std::sync::Arc;

use calimero_account::AccountId;
use calimero_context::scope_projection::ScopeProjections;
use calimero_context_config::types::ContextGroupId;
use calimero_op::{Authorship, Op, OpPayload, ScopeId};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use calimero_store::db::InMemoryDB;
use calimero_store::Store;
use core::num::NonZeroU128;
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;

const MRTD: &str = "approved-td";

/// When every write in these tests is judged, and when their evidence is
/// appraised unless a test says otherwise.
const NOW: u64 = 1_800_000_000;

fn hlc(ns: u64) -> HybridTimestamp {
    HybridTimestamp::new(Timestamp::new(
        NTP64(ns),
        ID::from(NonZeroU128::new(1).unwrap()),
    ))
}

/// A namespace with one admin (the author of every op here) and a projection.
struct Namespace {
    store: Store,
    ns: ContextGroupId,
    proj: ScopeProjections,
    admin: Authorship,
    clock: u64,
}

impl Namespace {
    fn new(byte: u8) -> Self {
        let admin = AccountId::from([0xAD; 32]);
        Self {
            store: Store::new(Arc::new(InMemoryDB::owned())),
            ns: ContextGroupId::from([byte; 32]),
            proj: ScopeProjections::new(),
            admin: Authorship {
                account: admin,
                device: calimero_account::DeviceId::from([0xAD; 32]),
                device_key: PublicKey::from([0xAD; 32]),
            },
            clock: 0,
        }
    }

    /// Fold `payload` as an op whose only parent is `parent`; returns its id.
    fn fold(&mut self, parent: Option<[u8; 32]>, payload: OpPayload) -> [u8; 32] {
        self.clock += 1;
        let op = Op::new(
            ScopeId::from(self.ns.to_bytes()),
            parent.into_iter().collect(),
            self.admin,
            hlc(self.clock),
            payload,
            [0; 32],
            [0; 64],
        );
        self.proj.ingest_op(&op);
        op.id()
    }

    /// Admit a TEE whose device key is `key`: a `ReadOnlyTee` join at the root
    /// that binds that key, as a fleet replica's admission folds.
    fn admit_tee(
        &mut self,
        parent: Option<[u8; 32]>,
        key: &PublicKey,
        device: u8,
    ) -> (AccountId, [u8; 32]) {
        let root_sk = PrivateKey::random(&mut UnwrapErr(SysRng));
        let genesis = calimero_account::AccountGenesis::new(root_sk.public_key());
        let cert = calimero_account::DeviceCert::sign(
            &root_sk,
            genesis.account_id(),
            calimero_account::DeviceId::from([device; 32]),
            key,
            &calimero_account::KemPublicKey::from([0x2B; 32]),
            0,
            0,
        )
        .expect("sign the device cert");
        let account = cert.account;
        let id = self.fold(
            parent,
            OpPayload::MemberJoinedWithDevice {
                group: self.ns,
                member: account,
                role: GroupMemberRole::ReadOnlyTee,
                genesis,
                chain: vec![],
                cert,
            },
        );
        (account, id)
    }

    fn evidence(&mut self, parent: [u8; 32], member: AccountId, key: PublicKey) -> [u8; 32] {
        self.evidence_at(parent, member, key, NOW)
    }

    fn evidence_at(
        &mut self,
        parent: [u8; 32],
        member: AccountId,
        key: PublicKey,
        attested_at: u64,
    ) -> [u8; 32] {
        self.fold(
            Some(parent),
            OpPayload::TeeAuthorityEvidence {
                group: self.ns,
                member,
                attested_key: key,
                mrtd: MRTD.to_owned(),
                attested_at,
            },
        )
    }

    fn policy(&mut self, parent: [u8; 32], allowed: &[&str]) -> [u8; 32] {
        self.fold(
            Some(parent),
            OpPayload::TeeAuthoringPolicySet {
                group: self.ns,
                allowed_mrtd: allowed.iter().map(|m| (*m).to_owned()).collect(),
            },
        )
    }

    fn writer(&self, key: &PublicKey, cut: [u8; 32]) -> Option<AccountId> {
        self.writer_at(key, cut, NOW)
    }

    fn writer_at(&self, key: &PublicKey, cut: [u8; 32], at: u64) -> Option<AccountId> {
        self.proj
            .writer_account_at_cut(&self.store, self.ns, key, &[cut], at)
    }
}

/// The property the whole change exists for: a policy change a delta did not
/// cite does not change how it merges, on a node that has already folded it.
#[test]
fn a_later_policy_change_does_not_change_an_earlier_cut() {
    let mut n = Namespace::new(0x31);
    let tee = PrivateKey::random(&mut UnwrapErr(SysRng)).public_key();
    let (account, joined) = n.admit_tee(None, &tee, 0x41);
    let evidenced = n.evidence(joined, account, tee);

    assert_eq!(
        n.writer(&tee, evidenced),
        Some(account),
        "no policy at the cut: TEE authorship is off"
    );

    let on = n.policy(evidenced, &[MRTD]);
    assert_eq!(n.writer(&tee, on), Some(AccountId::TEE_AUTHORITY));

    let off = n.policy(on, &[]);
    assert_eq!(
        n.writer(&tee, off),
        Some(account),
        "a cut past the empty policy has no TEE authority"
    );
    assert_eq!(
        n.writer(&tee, on),
        Some(AccountId::TEE_AUTHORITY),
        "and a write that cited the earlier cut still merges as the TEE authority \
         on a node that has folded the change"
    );
}

/// A policy that does not name the evidence's MRTD grants nothing, and neither
/// does a policy with no evidence behind it.
#[test]
fn authority_needs_evidence_the_policy_names() {
    let mut n = Namespace::new(0x32);
    let tee = PrivateKey::random(&mut UnwrapErr(SysRng)).public_key();
    let (account, joined) = n.admit_tee(None, &tee, 0x42);

    let no_evidence = n.policy(joined, &[MRTD]);
    assert_eq!(n.writer(&tee, no_evidence), Some(account));

    let other_image = n.policy(no_evidence, &["another-td"]);
    let evidenced = n.evidence(other_image, account, tee);
    assert_eq!(n.writer(&tee, evidenced), Some(account));

    let named = n.policy(evidenced, &["another-td", MRTD]);
    assert_eq!(n.writer(&tee, named), Some(AccountId::TEE_AUTHORITY));
}

/// Evidence names the key its quote binds. Relabelling one TEE's evidence for
/// another TEE's account does not make the other one an authority.
#[test]
fn evidence_relabelled_for_another_account_is_not_an_authority() {
    let mut n = Namespace::new(0x33);
    let tee = PrivateKey::random(&mut UnwrapErr(SysRng)).public_key();
    let other = PrivateKey::random(&mut UnwrapErr(SysRng)).public_key();
    let (_, joined) = n.admit_tee(None, &tee, 0x43);
    let (other_account, other_joined) = n.admit_tee(Some(joined), &other, 0x44);
    let relabelled = n.evidence(other_joined, other_account, tee);
    let on = n.policy(relabelled, &[MRTD]);

    assert_eq!(n.writer(&other, on), Some(other_account));
    assert_ne!(n.writer(&tee, on), Some(AccountId::TEE_AUTHORITY));
}

/// Evidence lapses by the delta's clock, not the reader's: a write judged
/// within the evidence's window merges as the TEE authority, and one judged
/// past it does not, on every node alike.
#[test]
fn evidence_lapses_by_the_time_the_write_is_judged_at() {
    use calimero_governance_store::{TEE_EVIDENCE_MAX_AGE_SECS, TEE_EVIDENCE_MAX_CLOCK_SKEW_SECS};

    let mut n = Namespace::new(0x34);
    let tee = PrivateKey::random(&mut UnwrapErr(SysRng)).public_key();
    let (account, joined) = n.admit_tee(None, &tee, 0x45);
    let evidenced = n.evidence(joined, account, tee);
    let on = n.policy(evidenced, &[MRTD]);

    assert_eq!(
        n.writer_at(&tee, on, NOW + TEE_EVIDENCE_MAX_AGE_SECS),
        Some(AccountId::TEE_AUTHORITY)
    );
    assert_eq!(
        n.writer_at(&tee, on, NOW + TEE_EVIDENCE_MAX_AGE_SECS + 1),
        Some(account),
        "a write judged past the evidence's window is the TEE's own"
    );
    assert_eq!(
        n.writer_at(&tee, on, NOW - TEE_EVIDENCE_MAX_CLOCK_SKEW_SECS - 1),
        Some(account),
        "evidence dated ahead of the write by more than the skew does not count"
    );
}

/// The most recent appraisal counts whatever order it was folded in, and
/// evidence dated beyond the write cannot shadow the one current then.
#[test]
fn the_most_recent_appraisal_counts() {
    use calimero_governance_store::TEE_EVIDENCE_MAX_AGE_SECS;

    let mut n = Namespace::new(0x35);
    let tee = PrivateKey::random(&mut UnwrapErr(SysRng)).public_key();
    let (account, joined) = n.admit_tee(None, &tee, 0x46);
    let fresh = n.evidence_at(joined, account, tee, NOW - 60);
    let stale = n.evidence_at(fresh, account, tee, NOW - TEE_EVIDENCE_MAX_AGE_SECS - 60);
    let future = n.evidence_at(stale, account, tee, NOW + 10 * TEE_EVIDENCE_MAX_AGE_SECS);
    let on = n.policy(future, &[MRTD]);

    assert_eq!(
        n.writer(&tee, on),
        Some(AccountId::TEE_AUTHORITY),
        "an older appraisal folded later, and a future-dated one, leave the current one standing"
    );
}
