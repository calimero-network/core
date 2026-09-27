//! The namespace TEE key: one key a value only the TEE may read is sealed to.
//!
//! Sealing such a value to each TEE authority's own key leaves a TEE admitted
//! later unable to open it until a TEE that can writes it again, which it
//! cannot do if every such TEE is down. Sealing it to one namespace key instead,
//! and handing that key to each TEE authority as it arrives
//! ([`GroupOp::TeeVaultKeyDelivered`]), lets any TEE that holds the key open
//! everything sealed before it was admitted.
//!
//! Two TEEs may each create a key before either sees the other's. Nothing picks
//! a winner: each hands every key it holds to every TEE authority that lacks
//! it, so both end up holding both, a run opens with whichever key a value was
//! sealed to, and seals to the lowest.
//!
//! A key is retired once any TEE it was delivered to is no longer a TEE
//! authority of the namespace ([`retired_tee_vault_keys`]): removed, left, its
//! image dropped from the authoring policy, or its evidence lapsed. That TEE
//! still holds the key. No run seals to a retired key, and a TEE authority
//! creates a new one when no other remains. The TEEs that remain keep the
//! retired keys, so what was sealed to one still opens for them.
//!
//! Retiring is not permanent: it follows the authorities as they are now, so a
//! TEE that becomes an authority again makes its keys live again. It held them
//! all along, and is trusted again.

use std::collections::BTreeSet;

use calimero_context_client::local_governance::GroupOp;
use calimero_context_config::types::ContextGroupId;
use calimero_crypto::SealedEnvelope;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;
use eyre::{eyre, Result as EyreResult};

use super::read_op_log_after;
use crate::tee::decode_group_op;
use crate::NamespaceRepository;

/// One copy of a namespace TEE key on the root's log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TeeVaultDelivery {
    /// The public half of the key, which names it.
    pub vault_key: PublicKey,
    /// The attested key of the TEE the copy is for.
    pub recipient_key: PublicKey,
    /// The private half, sealed to `recipient_key`.
    pub envelope: Vec<u8>,
}

/// Every [`GroupOp::TeeVaultKeyDelivered`] on the namespace root's log, in log
/// order.
///
/// Apply refused any delivery not signed by a `ReadOnlyTee` member, so each one
/// here was published by a TEE. That is not checked again against the signer's
/// role now: a TEE removed later did deliver its copies as a TEE, and dropping
/// them would take keys away from the TEEs that remain.
///
/// # Errors
/// Any governance store read error.
pub fn tee_vault_deliveries(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Vec<TeeVaultDelivery>> {
    let root = NamespaceRepository::new(store).resolve(group_id)?;
    let mut deliveries = Vec::new();
    for (seq, bytes) in &read_op_log_after(store, &root, 0, usize::MAX)? {
        let Ok(signed) = decode_group_op(&root, *seq, bytes, "tee_vault_deliveries") else {
            continue;
        };
        let GroupOp::TeeVaultKeyDelivered {
            vault_key,
            recipient_key,
            envelope,
        } = signed.op
        else {
            continue;
        };
        deliveries.push(TeeVaultDelivery {
            vault_key,
            recipient_key,
            envelope,
        });
    }
    Ok(deliveries)
}

/// The namespace TEE keys delivered to the holder of `recipient`, lowest public
/// key first, each once.
///
/// A copy counts only if it opens with `recipient` to the private half of the
/// key it names; any other is ignored.
///
/// # Errors
/// Any governance store read error.
pub fn tee_vault_keys(
    store: &Store,
    group_id: &ContextGroupId,
    recipient: &PrivateKey,
) -> EyreResult<Vec<PrivateKey>> {
    Ok(held_keys(
        &tee_vault_deliveries(store, group_id)?,
        recipient,
    ))
}

fn held_keys(deliveries: &[TeeVaultDelivery], recipient: &PrivateKey) -> Vec<PrivateKey> {
    let mine = recipient.public_key();
    let mut keys: Vec<PrivateKey> = Vec::new();
    for delivery in deliveries {
        if delivery.recipient_key != mine
            || keys.iter().any(|k| k.public_key() == delivery.vault_key)
        {
            continue;
        }
        if let Some(key) = open_vault_key(delivery, recipient) {
            keys.push(key);
        }
    }
    keys.sort_by_key(|key| *key.public_key());
    keys
}

/// The namespace TEE keys some copy of was delivered to a TEE that is not one
/// of `authorities`, the attested keys of the namespace's TEE authorities
/// ([`crate::tee_authority_keys_in_namespace`]). That TEE still holds the key,
/// so nothing new may be sealed to it.
///
/// A delivery names the attested key it was sealed to, which is how the
/// authorities are named too: a TEE whose evidence now binds another key no
/// longer counts as the recipient, and its keys retire with it.
#[must_use]
pub fn retired_tee_vault_keys(
    deliveries: &[TeeVaultDelivery],
    authorities: &[PublicKey],
) -> BTreeSet<PublicKey> {
    deliveries
        .iter()
        .filter(|delivery| !authorities.contains(&delivery.recipient_key))
        .map(|delivery| delivery.vault_key)
        .collect()
}

/// The namespace TEE keys held by the TEE whose key is `recipient`, and the one
/// it seals to.
#[derive(Debug)]
pub struct TeeVault {
    /// Every key delivered to this TEE, lowest public key first. A run opens
    /// with any of them.
    pub held: Vec<PrivateKey>,
    /// The lowest held key that is not retired, or `None` if every held key is
    /// ([`retired_tee_vault_keys`]).
    pub sealing: Option<PublicKey>,
}

/// [`tee_vault_keys`] and the key a run seals to, from one read of the log.
///
/// # Errors
/// Any governance store read error.
pub fn tee_vault(
    store: &Store,
    group_id: &ContextGroupId,
    recipient: &PrivateKey,
) -> EyreResult<TeeVault> {
    let deliveries = tee_vault_deliveries(store, group_id)?;
    let authorities = crate::tee_authority_keys_in_namespace(store, group_id)?;
    let retired = retired_tee_vault_keys(&deliveries, &authorities);
    let held = held_keys(&deliveries, recipient);
    let sealing = held
        .iter()
        .map(PrivateKey::public_key)
        .find(|key| !retired.contains(key));
    Ok(TeeVault { held, sealing })
}

/// The envelope for `vault` sealed to `recipient`, for a
/// [`GroupOp::TeeVaultKeyDelivered`].
///
/// # Errors
/// If `recipient` is not a usable key.
pub fn seal_tee_vault_key(vault: &PrivateKey, recipient: &PublicKey) -> EyreResult<Vec<u8>> {
    calimero_crypto::seal_to_root(&mut rand::rng(), recipient, vault.as_bytes().to_vec())
        .map(|envelope| envelope.to_bytes())
        .map_err(|err| eyre!("could not seal the namespace TEE key: {err:?}"))
}

fn open_vault_key(delivery: &TeeVaultDelivery, recipient: &PrivateKey) -> Option<PrivateKey> {
    let envelope = SealedEnvelope::from_bytes(&delivery.envelope)?;
    let bytes: [u8; 32] = calimero_crypto::open_sealed(recipient, &envelope)
        .ok()?
        .try_into()
        .ok()?;
    let key = PrivateKey::from(bytes);
    (key.public_key() == delivery.vault_key).then_some(key)
}

#[cfg(test)]
mod tests {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_context_config::types::ContextGroupId;
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::{PrivateKey, PublicKey};
    use calimero_store::Store;

    use super::{
        retired_tee_vault_keys, seal_tee_vault_key, tee_vault, tee_vault_deliveries,
        tee_vault_keys, TeeVaultDelivery,
    };
    use crate::local_state::persist_group_op_log_entry;
    use crate::tee::tests::{mock_quote_for, MOCK_MRTD};
    use crate::test_fixtures::{enrol_member, nest_for_test, test_store};
    use crate::{apply_local_signed_group_op, MembershipRepository};

    /// A namespace root with one admin and two TEEs, both TEE authorities:
    /// admitted, with current evidence, under a policy that names their image.
    struct Namespace {
        store: Store,
        root: ContextGroupId,
        admin: PrivateKey,
        tee: PrivateKey,
        other_tee: PrivateKey,
        nonce: std::cell::Cell<u64>,
    }

    impl Namespace {
        fn new() -> Self {
            let ns = Self::with_other_tee_evidence_at(crate::now_secs());
            assert_eq!(ns.authorities().len(), 2);
            ns
        }

        /// As [`new`](Self::new), except that `other_tee`'s only evidence was
        /// appraised at `attested_at`.
        fn with_other_tee_evidence_at(attested_at: u64) -> Self {
            let store = test_store();
            let root = ContextGroupId::from([0xB7; 32]);
            let members = MembershipRepository::new(&store);
            let key = || PrivateKey::random(&mut rand::rng());
            let (admin, tee, other_tee) = (key(), key(), key());
            for (sk, role) in [
                (&admin, GroupMemberRole::Admin),
                (&tee, GroupMemberRole::ReadOnlyTee),
                (&other_tee, GroupMemberRole::ReadOnlyTee),
            ] {
                let account = enrol_member(&store, &root, &sk.public_key());
                members.add_member(&root, &account, role).unwrap();
            }
            let ns = Self {
                store,
                root,
                admin,
                tee,
                other_tee,
                nonce: std::cell::Cell::new(0),
            };
            ns.policy(&[MOCK_MRTD]);
            for tee in [&ns.tee, &ns.other_tee] {
                let member = ns.account_of(tee);
                ns.log(GroupOp::MemberJoinedViaTeeAttestation {
                    member,
                    quote_hash: *tee.public_key(),
                    mrtd: MOCK_MRTD.to_owned(),
                    rtmr0: String::new(),
                    rtmr1: String::new(),
                    rtmr2: String::new(),
                    rtmr3: String::new(),
                    tcb_status: "UpToDate".to_owned(),
                    role: GroupMemberRole::ReadOnlyTee,
                });
            }
            ns.evidence(&ns.tee, crate::now_secs());
            ns.evidence(&ns.other_tee, attested_at);
            ns
        }

        /// Put `op` on the root's log as the admission and evidence readers
        /// find it. Their writers are checked elsewhere; these tests are about
        /// what the vault makes of them.
        fn log(&self, op: GroupOp) {
            self.nonce.set(self.nonce.get() + 1);
            let seq = self.nonce.get();
            let signed =
                SignedGroupOp::sign(&self.admin, self.root.to_bytes().into(), vec![], seq, op)
                    .unwrap();
            persist_group_op_log_entry(
                &self.store,
                &self.root,
                seq,
                vec![],
                &borsh::to_vec(&signed).unwrap(),
            )
            .unwrap();
        }

        fn policy(&self, allowed: &[&str]) {
            self.log(GroupOp::TeeAuthoringPolicySet {
                allowed_mrtd: allowed.iter().map(|m| (*m).to_owned()).collect(),
            });
        }

        fn evidence(&self, tee: &PrivateKey, attested_at: u64) {
            self.log(GroupOp::TeeAuthorityEvidence {
                member: self.account_of(tee),
                attested_key: tee.public_key(),
                quote: mock_quote_for(&tee.public_key()),
                collateral: None,
                attested_at,
            });
        }

        fn account_of(&self, member: &PrivateKey) -> calimero_account::AccountId {
            crate::member_account_in_namespace(&self.store, &self.root, &member.public_key())
                .unwrap()
                .unwrap()
        }

        fn authorities(&self) -> Vec<PublicKey> {
            crate::tee_authority_keys_in_namespace(&self.store, &self.root).unwrap()
        }

        /// Both TEEs hold `vault`, handed out by `tee`.
        fn share(&self, vault: &PrivateKey) {
            for recipient in [&self.tee, &self.other_tee] {
                self.deliver(self.root, &self.tee, vault, &recipient.public_key())
                    .unwrap();
            }
        }

        fn sealing(&self) -> Option<PublicKey> {
            tee_vault(&self.store, &self.root, &self.tee)
                .unwrap()
                .sealing
        }

        /// `tee` still opens `vault` and seals nothing to it.
        fn assert_retired(&self, vault: &PrivateKey) {
            let now = tee_vault(&self.store, &self.root, &self.tee).unwrap();
            assert_eq!(
                now.held
                    .iter()
                    .map(PrivateKey::public_key)
                    .collect::<Vec<_>>(),
                vec![vault.public_key()],
                "the remaining TEE still opens what was sealed to the old key"
            );
            assert_eq!(now.sealing, None, "and seals nothing new to it");
        }

        /// Deliver `vault` to `recipient`, signed by `signer`, through apply.
        fn deliver(
            &self,
            group: ContextGroupId,
            signer: &PrivateKey,
            vault: &PrivateKey,
            recipient: &PublicKey,
        ) -> eyre::Result<()> {
            self.deliver_named(group, signer, vault.public_key(), vault, recipient)
        }

        fn deliver_named(
            &self,
            group: ContextGroupId,
            signer: &PrivateKey,
            vault_key: PublicKey,
            vault: &PrivateKey,
            recipient: &PublicKey,
        ) -> eyre::Result<()> {
            self.nonce.set(self.nonce.get() + 1);
            let op = SignedGroupOp::sign(
                signer,
                group.to_bytes().into(),
                vec![],
                self.nonce.get(),
                GroupOp::TeeVaultKeyDelivered {
                    vault_key,
                    recipient_key: *recipient,
                    envelope: seal_tee_vault_key(vault, recipient).unwrap(),
                },
            )
            .unwrap();
            apply_local_signed_group_op(&self.store, &op)
        }

        fn keys_of(&self, holder: &PrivateKey) -> Vec<PublicKey> {
            tee_vault_keys(&self.store, &self.root, holder)
                .unwrap()
                .iter()
                .map(PrivateKey::public_key)
                .collect()
        }
    }

    /// The copy a TEE was handed opens for it, and for nobody else.
    #[test]
    fn a_tee_holds_the_key_delivered_to_it() {
        let ns = Namespace::new();
        let vault = PrivateKey::random(&mut rand::rng());
        ns.deliver(ns.root, &ns.tee, &vault, &ns.tee.public_key())
            .unwrap();

        assert_eq!(ns.keys_of(&ns.tee), vec![vault.public_key()]);
        assert!(ns.keys_of(&ns.other_tee).is_empty());

        ns.deliver(ns.root, &ns.tee, &vault, &ns.other_tee.public_key())
            .unwrap();
        assert_eq!(ns.keys_of(&ns.other_tee), vec![vault.public_key()]);
    }

    /// An admin may not hand the TEEs a key: it would hold the key itself and
    /// read everything sealed to it.
    #[test]
    fn only_an_admitted_tee_may_deliver_the_key() {
        let ns = Namespace::new();
        let vault = PrivateKey::random(&mut rand::rng());
        assert!(ns
            .deliver(ns.root, &ns.admin, &vault, &ns.tee.public_key())
            .is_err());
        assert!(tee_vault_deliveries(&ns.store, &ns.root)
            .unwrap()
            .is_empty());
        assert!(ns.keys_of(&ns.tee).is_empty());
    }

    #[test]
    fn the_key_lives_on_the_namespace_root() {
        let ns = Namespace::new();
        let child = ContextGroupId::from([0xB8; 32]);
        nest_for_test(&ns.store, &ns.root, &child);
        let vault = PrivateKey::random(&mut rand::rng());
        assert!(ns
            .deliver(child, &ns.tee, &vault, &ns.tee.public_key())
            .is_err());
    }

    /// A copy that opens to some other key than the one it names is ignored,
    /// so a delivery cannot pass off one key under another's name.
    #[test]
    fn a_copy_that_opens_to_another_key_is_ignored() {
        let ns = Namespace::new();
        let named = PrivateKey::random(&mut rand::rng()).public_key();
        let sealed = PrivateKey::random(&mut rand::rng());
        ns.deliver_named(ns.root, &ns.tee, named, &sealed, &ns.tee.public_key())
            .unwrap();
        assert!(ns.keys_of(&ns.tee).is_empty());
    }

    /// Removing a TEE retires every key it was handed: nothing new is sealed to
    /// one until a key it never held is delivered. The TEEs that remain keep the
    /// retired key, and the copies the removed TEE delivered, so what was sealed
    /// to it still opens for them.
    #[test]
    fn removing_a_tee_retires_the_keys_it_held() {
        let ns = Namespace::new();
        let old = PrivateKey::random(&mut rand::rng());
        // The TEE that is about to be removed created the key and handed it on.
        for recipient in [&ns.other_tee, &ns.tee] {
            ns.deliver(ns.root, &ns.other_tee, &old, &recipient.public_key())
                .unwrap();
        }
        let vault = tee_vault(&ns.store, &ns.root, &ns.tee).unwrap();
        assert_eq!(vault.sealing, Some(old.public_key()));

        MembershipRepository::new(&ns.store)
            .remove_member(&ns.root, &ns.account_of(&ns.other_tee))
            .unwrap();
        ns.assert_retired(&old);

        let new = PrivateKey::random(&mut rand::rng());
        ns.deliver(ns.root, &ns.tee, &new, &ns.tee.public_key())
            .unwrap();
        let vault = tee_vault(&ns.store, &ns.root, &ns.tee).unwrap();
        assert_eq!(vault.sealing, Some(new.public_key()));
        assert_eq!(vault.held.len(), 2);
    }

    /// A TEE whose image the authoring policy no longer names is still a
    /// member and still holds the key, so the key retires as if it were
    /// removed. Naming the image again makes the key live again: its holders
    /// are trusted again, and held it all along.
    ///
    /// Every mock quote reports the same image, so the policy drops both TEEs
    /// here; which TEE the key was handed to is what the pure test below varies.
    #[test]
    fn a_tee_dropped_from_the_authoring_policy_retires_its_keys() {
        let ns = Namespace::new();
        let vault = PrivateKey::random(&mut rand::rng());
        ns.share(&vault);
        assert_eq!(ns.sealing(), Some(vault.public_key()));

        ns.policy(&["an-image-neither-tee-runs"]);
        assert!(ns.authorities().is_empty());
        ns.assert_retired(&vault);

        ns.policy(&[MOCK_MRTD]);
        assert_eq!(ns.sealing(), Some(vault.public_key()));
    }

    /// Evidence older than the maximum age confers no authority, so a key
    /// handed to a TEE whose evidence has lapsed is retired, while the TEE
    /// whose evidence is current stays an authority and opens it.
    #[test]
    fn a_tee_whose_evidence_lapsed_retires_its_keys() {
        let lapsed = crate::now_secs() - crate::TEE_EVIDENCE_MAX_AGE_SECS - 1;
        let ns = Namespace::with_other_tee_evidence_at(lapsed);
        assert_eq!(ns.authorities(), vec![ns.tee.public_key()]);
        let vault = PrivateKey::random(&mut rand::rng());
        ns.share(&vault);
        ns.assert_retired(&vault);
    }

    /// A key retires when any copy of it went to a key that is not an
    /// authority, and only then.
    #[test]
    fn a_key_retires_when_any_holder_is_not_an_authority() {
        let key = |byte: u8| PublicKey::from([byte; 32]);
        let delivery = |vault: u8, recipient: u8| TeeVaultDelivery {
            vault_key: key(vault),
            recipient_key: key(recipient),
            envelope: Vec::new(),
        };
        let deliveries = [delivery(8, 1), delivery(8, 2), delivery(9, 1)];
        assert!(retired_tee_vault_keys(&deliveries, &[key(1), key(2)]).is_empty());
        assert_eq!(
            retired_tee_vault_keys(&deliveries, &[key(1)]),
            [key(8)].into()
        );
        assert_eq!(
            retired_tee_vault_keys(&deliveries, &[key(2)]),
            [key(8), key(9)].into()
        );
    }

    /// Two keys created concurrently are both held, lowest first, which is the
    /// one a run seals to.
    #[test]
    fn every_key_delivered_is_held_lowest_first() {
        let ns = Namespace::new();
        let (a, b) = (
            PrivateKey::random(&mut rand::rng()),
            PrivateKey::random(&mut rand::rng()),
        );
        for vault in [&a, &b, &a] {
            ns.deliver(ns.root, &ns.other_tee, vault, &ns.tee.public_key())
                .unwrap();
        }
        let mut expected = vec![a.public_key(), b.public_key()];
        expected.sort();
        assert_eq!(ns.keys_of(&ns.tee), expected);
    }
}
