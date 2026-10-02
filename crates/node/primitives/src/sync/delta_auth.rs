//! Delta-envelope signature primitive.
//!
//! Closes the anti-impersonation gap on the delta-envelope level: a
//! current group-key holder can no longer write a delta claiming
//! another member as `author_id`. The author signs a canonical
//! payload that binds `(context_id, delta_id, author_id,
//! governance_position)`; every receive path verifies before
//! applying.
//!
//! The signature primitive is intentionally separate from per-action
//! signatures (which live in `StorageType::{User, Shared}::signature_data`
//! and verify in `Interface::apply_action`). Per-action signatures
//! attribute INDIVIDUAL writes within a delta; the envelope
//! signature binds the WHOLE delta to its author. Both are needed for
//! full coverage — per-action sigs don't catch envelope forgery
//! (a current member relabeling a foreign delta as their own), and
//! the envelope signature doesn't catch per-action forgery within a
//! Public-only delta.
//!
//! ## Payload shape
//!
//! ```ignore
//! DeltaSignaturePayload {
//!     context_id,        // pins to the context (cross-context replay)
//!     delta_id,          // hash(events_hash || parents || actions); commits to the content
//!     author_id,         // claimed author
//!     governance_position, // cited cut for the membership check
//! }
//! ```
//!
//! Borsh-serialized. Signed with the author's ed25519 identity key.
//! `delta_id` is the existing content hash, so committing to it covers
//! the action bytes via the hash chain.

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_account::{Delegation, VerifiedWarrant, Warrant};
use calimero_context_config::types::GovernanceParentEdge;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::{domain_hash, PublicKey};
use calimero_storage::logical_clock::HybridTimestamp;

/// What a delta-envelope signature is for. The first field of every signed
/// payload in this module, so a signature made for one kind can never verify
/// as another: a self-authored signature cannot be passed off as a delegated
/// one (dropping the warrant in flight), an ordinary TEE write cannot pass as
/// a triggered one, and a fired statement cannot pass as a delta.
///
/// It encodes as one tag byte. That also keeps these payloads apart from
/// anything else the same keys sign (ephemeral envelopes, governance ops,
/// device certificates): each of those starts with its own byte-string label,
/// none of which begins with a byte this small.
#[derive(BorshSerialize, Clone, Copy, Debug, PartialEq, Eq)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum SignatureDomain {
    /// [`DeltaSignaturePayload`]: the author signed its own delta.
    Delta = 0,
    /// [`DelegatedDeltaSignaturePayload`]: an executor signed for an author.
    Delegated = 1,
    /// [`TeeDeltaSignaturePayload`]: a TEE signed a triggered run's delta.
    Tee = 2,
    /// [`TeeFiredPayload`]: a TEE ran a trigger that wrote nothing.
    TeeFired = 3,
    /// [`StateBeaconPayload`]: a member's state as of its last heartbeat.
    StateBeacon = 4,
}

/// Canonical payload for the delta-envelope signature. Borsh-serialized
/// and signed by `author_id`'s ed25519 key. Only used for serialization —
/// receivers re-construct it from their own data and compare signature
/// bytes, so `BorshDeserialize` isn't needed (and wouldn't work with the
/// `&GovernanceParentEdge` borrow anyway).
///
/// `domain` is always [`SignatureDomain::Delta`].
#[derive(BorshSerialize)]
pub struct DeltaSignaturePayload<'a> {
    pub domain: SignatureDomain,
    pub context_id: ContextId,
    pub delta_id: [u8; 32],
    pub author_id: PublicKey,
    pub governance_position: Option<&'a GovernanceParentEdge>,
    /// Hybrid logical clock, bound because it is a CLEARTEXT wire field that
    /// `CausalDelta::compute_id` deliberately excludes (hashing it would defeat
    /// id determinism), so `content_address_matches` cannot cover it. Its only
    /// consumer is the local clock, which caps remote drift at 5s — bounded, but
    /// there is no reason to leave a signable field unsigned.
    pub hlc: HybridTimestamp,
}

/// Canonical payload for a delegated delta-envelope signature, signed by the
/// EXECUTOR rather than by the author.
///
/// This is the deliberate re-opening of the gap [`DeltaSignaturePayload`] was
/// built to close: a delta whose `author_id` is not the key that signed it. What
/// makes it safe is that the author's own signature travels with it, inside
/// `warrant` — so "somebody else wrote this for me" is a claim the author made
/// and every peer can check, instead of one a group-key holder can assert.
///
/// Both new fields exist to stop the parts being recombined:
///
/// * `executor_key` binds WHICH process signed, so a signature cannot be lifted
///   onto a delta claiming a different executor.
/// * `warrant` binds the consent itself, embedded whole rather than hashed —
///   the bytes are on the wire anyway, so the verifier reconstructs them
///   exactly. Without it a relay holding two warrants for the same author could
///   swap them between deltas, and each delta would still verify.
#[derive(BorshSerialize)]
pub struct DelegatedDeltaSignaturePayload<'a> {
    pub domain: SignatureDomain,
    pub context_id: ContextId,
    pub delta_id: [u8; 32],
    /// The member the change is attributed to. Same meaning and same consumers
    /// as on the self-authored path — it must equal `warrant.author_device_key`,
    /// which [`verify_delegated_delta_signature`] checks rather than assumes.
    pub author_id: PublicKey,
    /// The key that produced this signature.
    pub executor_key: PublicKey,
    /// The author's consent, embedded whole.
    pub warrant: &'a Warrant,
    pub governance_position: Option<&'a GovernanceParentEdge>,
    pub hlc: HybridTimestamp,
}

/// Domain separator for the id of an event trigger.
const EVENT_TRIGGER_DOMAIN: &[u8] = b"calimero.tee-trigger.event";

/// Domain separator for the id of a timer trigger.
const TIMER_TRIGGER_DOMAIN: &[u8] = b"calimero.tee-trigger.timer";

/// What fired a TEE-triggered run: the thing a TEE envelope
/// commits to.
///
/// Carried whole rather than as its hashed id so a receiver derives the id
/// itself ([`Self::id`]) instead of trusting one, and can read which method
/// the run was.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub enum TeeTriggerCause {
    /// A `tee:<method>` handler on the delta `cause`.
    Event {
        /// The delta whose event the handler fired on.
        cause: [u8; 32],
        /// The `#[app::tee]` method the handler named.
        method: String,
    },
    /// The `tick`th period of an `#[app::tee(every = "..")]` method.
    Timer {
        /// The timer method.
        method: String,
        /// Periods since the Unix epoch.
        tick: u64,
        /// The method's period, in seconds.
        ///
        /// Part of the id, so a receiver can check the tick against the
        /// delta's clock without reading the module: a TEE that names another
        /// period names another trigger, one no scheduler fires.
        every_secs: u64,
    },
}

impl TeeTriggerCause {
    /// The id of the firing this cause names in `context_id`.
    ///
    /// Every TEE authority derives the same id for the same firing, which is
    /// what lets one stand down when another has fired it. An event trigger
    /// needs no context, because the delta that caused it is unique to one; a
    /// tick is not, so a timer's id includes the context.
    #[must_use]
    pub fn id(&self, context_id: &ContextId) -> [u8; 32] {
        match self {
            Self::Event { cause, method } => {
                domain_hash(EVENT_TRIGGER_DOMAIN, &[cause.as_slice(), method.as_bytes()])
            }
            Self::Timer {
                method,
                tick,
                every_secs,
            } => domain_hash(
                TIMER_TRIGGER_DOMAIN,
                &[
                    AsRef::<[u8; 32]>::as_ref(context_id).as_slice(),
                    method.as_bytes(),
                    &tick.to_le_bytes(),
                    &every_secs.to_le_bytes(),
                ],
            ),
        }
    }

    /// When a timer's tick began, in seconds since the Unix epoch, or `None`
    /// for an event trigger. `None` too for a tick past the end of time, which
    /// no scheduler offers.
    #[must_use]
    pub fn tick_start_secs(&self) -> Option<u64> {
        match self {
            Self::Event { .. } => None,
            Self::Timer {
                tick, every_secs, ..
            } => tick.checked_mul(*every_secs),
        }
    }

    /// The method the run executed.
    #[must_use]
    pub fn method(&self) -> &str {
        match self {
            Self::Event { method, .. } | Self::Timer { method, .. } => method,
        }
    }
}

/// Canonical payload for the envelope of a TEE-triggered delta, signed by the
/// TEE authority's attested key: the self-authored fields, plus the trigger.
///
/// Committing to the trigger is what lets a receiver tell such a delta from any
/// other a TEE signs, instead of trusting the attested `merod` to sign nothing
/// else, and it is where the fired marker comes from: a relay holding the group
/// key can re-seal a delta's events, but it cannot change this.
#[derive(BorshSerialize)]
pub struct TeeDeltaSignaturePayload<'a> {
    pub domain: SignatureDomain,
    pub context_id: ContextId,
    pub delta_id: [u8; 32],
    pub author_id: PublicKey,
    pub trigger: &'a TeeTriggerCause,
    pub governance_position: Option<&'a GovernanceParentEdge>,
    pub hlc: HybridTimestamp,
}

/// Borsh-serialize the canonical payload of a TEE-triggered delta. Used at sign
/// time on the TEE and at verify time on every receive path.
///
/// # Errors
/// Only if borsh fails on the in-memory buffer.
pub fn tee_delta_signature_payload(
    context_id: ContextId,
    delta_id: [u8; 32],
    author_id: PublicKey,
    trigger: &TeeTriggerCause,
    governance_position: Option<&GovernanceParentEdge>,
    hlc: HybridTimestamp,
) -> Result<Vec<u8>, borsh::io::Error> {
    borsh::to_vec(&TeeDeltaSignaturePayload {
        domain: SignatureDomain::Tee,
        context_id,
        delta_id,
        author_id,
        trigger,
        governance_position,
        hlc,
    })
}

/// Canonical payload of a TEE's fired statement: which trigger it ran, in which
/// context, as which key. Signed by the TEE authority's attested key.
///
/// There is no delta to bind, so nothing else: the statement only ever records
/// a marker, and recording one twice is the same as once, so a replay gains
/// nothing.
#[derive(BorshSerialize)]
pub struct TeeFiredPayload<'a> {
    pub domain: SignatureDomain,
    pub context_id: ContextId,
    pub author_id: PublicKey,
    pub trigger: &'a TeeTriggerCause,
}

/// Borsh-encode the fired statement `author_id` signs for `trigger`.
///
/// # Errors
/// Borsh encoding error (unreachable for these field types).
pub fn tee_fired_payload(
    context_id: ContextId,
    author_id: PublicKey,
    trigger: &TeeTriggerCause,
) -> Result<Vec<u8>, borsh::io::Error> {
    borsh::to_vec(&TeeFiredPayload {
        domain: SignatureDomain::TeeFired,
        context_id,
        author_id,
        trigger,
    })
}

/// Verify that `author_id` signed the fired statement for `trigger` in
/// `context_id`. Whether that key is an attested TEE's is the caller's check.
///
/// # Errors
/// The statement does not verify under `author_id`.
pub fn verify_tee_fired(
    context_id: ContextId,
    author_id: PublicKey,
    trigger: &TeeTriggerCause,
    signature: &[u8; 64],
) -> eyre::Result<()> {
    let payload = tee_fired_payload(context_id, author_id, trigger)
        .map_err(|err| eyre::eyre!("failed to serialize a TEE fired statement: {err}"))?;
    author_id
        .verify_raw_signature(&payload, signature)
        .map_err(|err| eyre::eyre!("TEE fired statement signature verification failed: {err}"))
}

/// What a member signs to say "this is my state": the DAG heads it has applied
/// and the root hash they produced.
///
/// Tombstone GC collects a delete only once every member device has shown,
/// with one of these, a state equal to this node's own at a moment after the
/// delete was applied here (`calimero-node`'s `tombstone_stability`). Equal
/// heads mean the member holds no delta this node lacks — none of its offline
/// writes is still to come — and equal roots mean it holds no live copy of
/// what was deleted. Signed, because a forged one would let a non-member
/// release tombstones a stale replica still needs.
///
/// `dag_heads` must be sorted, so the signer and every verifier encode the same
/// bytes for the same set.
#[derive(BorshSerialize)]
pub struct StateBeaconPayload<'a> {
    pub domain: SignatureDomain,
    pub context_id: ContextId,
    pub signer: PublicKey,
    pub root_hash: [u8; 32],
    pub dag_heads: &'a [[u8; 32]],
}

/// Borsh-encode the beacon `signer` signs for its state (`root_hash` over the
/// sorted `dag_heads`).
///
/// # Errors
/// Borsh encoding error (unreachable for these field types).
pub fn state_beacon_payload(
    context_id: ContextId,
    signer: PublicKey,
    root_hash: [u8; 32],
    dag_heads: &[[u8; 32]],
) -> Result<Vec<u8>, borsh::io::Error> {
    borsh::to_vec(&StateBeaconPayload {
        domain: SignatureDomain::StateBeacon,
        context_id,
        signer,
        root_hash,
        dag_heads,
    })
}

/// Verify that `signer` signed the beacon for this state. Whether `signer` is
/// a member is the caller's check.
///
/// # Errors
/// The beacon does not verify under `signer`.
pub fn verify_state_beacon(
    context_id: ContextId,
    signer: PublicKey,
    root_hash: [u8; 32],
    dag_heads: &[[u8; 32]],
    signature: &[u8; 64],
) -> eyre::Result<()> {
    let payload = state_beacon_payload(context_id, signer, root_hash, dag_heads)
        .map_err(|err| eyre::eyre!("failed to serialize a state beacon: {err}"))?;
    signer
        .verify_raw_signature(&payload, signature)
        .map_err(|err| eyre::eyre!("state beacon signature verification failed: {err}"))
}

// NOT in this payload, deliberately: `producing_bytecode_id`.
//
// Forging it is a censorship primitive rather than a forgery one — it drives the
// HLC fence, so a rewritten value gets an otherwise valid delta buffered or
// dropped rather than applied — so it is worth binding on its own merits. It
// cannot be bound yet: it is a GOSSIP-ONLY envelope field. It is absent from the
// persisted `ContextDagDelta` row and from the `DeltaResponse` wire, so no
// catchup or parent-fetch receiver could reconstruct the payload, and every
// delta arriving by those paths would fail verification. Binding it means
// persisting it on the row and serving it on the wire first.
//
// NOT in this payload, deliberately: `expected_root_hash`.
//
// It cannot go here. The signature is verified BEFORE decryption on the gossip
// path — it has to be, because the author-keyed gates (ReadOnly,
// `membership_status_at`) all key off `author_id` and must not run until the
// authorship claim is established — and on that path `expected_root_hash` is
// not a wire field at all: it rides sealed inside `SealedDeltaPayload`, so it is
// unavailable to a pre-decrypt verifier.
//
// It cannot go into the `compute_id` preimage either, which would otherwise
// cover it on every path. The rotation-log self-log leg reassigns
// `delta.expected_root_hash` AFTER the id is computed, and it cannot be reordered
// to run first because the self-log needs `delta.id` to exist. That circularity
// is why `content_address_survives_post_id_root_hash_mutation` pins the field as
// being outside the preimage.
//
// Living with it is acceptable because the field is advisory: `DeltaStore` stores
// the root hash it COMPUTED, never this one, and a mismatch never rejects a
// delta. Forging it on the catchup paths (where it is plaintext; gossip seals it)
// only flips the merge classification that decides how a delta's children are
// handled. Closing it properly means having catchup responders serve the original
// sealed artifact instead of a plaintext `CausalDelta` — tracked separately.

/// Borsh-serialize the canonical payload. Used at sign time (execute
/// path) and verify time (every delta receive path).
///
/// Returns `borsh::io::Error` only if the borsh writer fails on the
/// in-memory buffer — practically infallible for these field types,
/// but the result type matches `borsh::to_vec`'s shape.
pub fn delta_signature_payload(
    context_id: ContextId,
    delta_id: [u8; 32],
    author_id: PublicKey,
    governance_position: Option<&GovernanceParentEdge>,
    hlc: HybridTimestamp,
) -> Result<Vec<u8>, borsh::io::Error> {
    let payload = DeltaSignaturePayload {
        domain: SignatureDomain::Delta,
        context_id,
        delta_id,
        author_id,
        governance_position,
        hlc,
    };
    borsh::to_vec(&payload)
}

/// Verify a per-delta envelope signature against the canonical payload.
///
/// Reconstructs the payload the author signed at send time
/// (`delta_signature_payload`) and verifies the ed25519 signature with
/// the claimed author's public key. Receivers call this on every apply
/// path (gossip receive, DAG-catchup receive, snapshot-buffer replay)
/// before the delta touches storage.
///
/// Returns `Ok(())` only on a valid signature. Any borsh-serialize
/// failure on the payload, or signature mismatch, returns `Err`.
///
/// **Caller contract:** the `author_id` passed here MUST be the same
/// author bound into the payload — verification doesn't check that
/// invariant for you, it just verifies that `author_id`'s key signed
/// THIS payload bytes. If you pass a different author for the
/// verification key vs. the payload, you're checking the wrong thing.
pub fn verify_delta_signature(
    context_id: ContextId,
    delta_id: [u8; 32],
    author_id: PublicKey,
    governance_position: Option<&GovernanceParentEdge>,
    hlc: HybridTimestamp,
    signature: &[u8; 64],
) -> eyre::Result<()> {
    let payload =
        delta_signature_payload(context_id, delta_id, author_id, governance_position, hlc)
            .map_err(|err| eyre::eyre!("failed to serialize delta signature payload: {err}"))?;
    author_id
        .verify_raw_signature(&payload, signature)
        .map_err(|err| eyre::eyre!("delta envelope signature verification failed: {err}"))
}

/// Borsh-serialize the canonical delegated payload. Used at sign time on the
/// relay and at verify time on every receive path.
///
/// # Errors
/// Only if borsh fails on the in-memory buffer.
pub fn delegated_delta_signature_payload(
    context_id: ContextId,
    delta_id: [u8; 32],
    author_id: PublicKey,
    delegation: &Delegation,
    governance_position: Option<&GovernanceParentEdge>,
    hlc: HybridTimestamp,
) -> Result<Vec<u8>, borsh::io::Error> {
    let payload = DelegatedDeltaSignaturePayload {
        domain: SignatureDomain::Delegated,
        context_id,
        delta_id,
        author_id,
        executor_key: delegation.executor_key,
        warrant: &delegation.warrant,
        governance_position,
        hlc,
    };
    borsh::to_vec(&payload)
}

/// What a verified envelope establishes about who authored a delta.
///
/// Returned rather than a bare `Ok(())` so a caller cannot forget which shape it
/// just checked: the delegated arm hands back the warrant, and the at-cut checks
/// the caller still owes are all reads off it.
#[derive(Clone, Debug)]
pub enum VerifiedEnvelope {
    /// The author signed it themselves. Nothing further about authorship.
    SelfAuthored,
    /// An executor signed it under the author's warrant.
    ///
    /// The caller still owes, and only the projection can answer:
    /// * neither device revoked in this group,
    /// * `author_account` a member at the cited cut,
    /// * `executor` holding the authorship capability on the owning group,
    /// * this `nonce` unseen for `author_device_key`,
    /// * `not_after` not yet passed.
    ///
    /// Boxed for the same `large_enum_variant` reason the bundle's own fields
    /// are: a warrant dwarfs the unit variant beside it.
    Delegated(Box<VerifiedWarrant>),
    /// The author signed it under `SignatureDomain::Tee`, for the firing `trigger`.
    ///
    /// Establishes only that the author's key signed this trigger; whether that
    /// key is a TEE the context accepts writes from is the read-only gate's
    /// question, answered off the author as for any other delta.
    Tee(TeeTriggerCause),
}

impl VerifiedEnvelope {
    /// The trigger a TEE envelope committed to, if it was one.
    #[must_use]
    pub const fn tee_trigger(&self) -> Option<&TeeTriggerCause> {
        match self {
            Self::Tee(trigger) => Some(trigger),
            Self::SelfAuthored | Self::Delegated(_) => None,
        }
    }
}

/// The ONE entry point every receive path uses to check a delta's envelope.
///
/// Self-authored and delegated deltas differ in who signed and in what has to be
/// established before the author-keyed gates may run. Writing that branch at each
/// receive site is how a delta comes to verify on gossip and be refused on
/// catchup — the failure mode `producing_bytecode_id` still cannot be bound because
/// of, and the one this function exists to make impossible. Gossip receive,
/// DAG-catchup receive and snapshot-buffer replay all call this and nothing else.
///
/// # Errors
/// Whatever the branch it took reports. A delegated delta whose bundle is
/// internally inconsistent fails here rather than reaching the gates.
#[expect(
    clippy::too_many_arguments,
    reason = "every envelope field the signature covers, one per argument"
)]
pub fn verify_delta_envelope(
    context_id: ContextId,
    delta_id: [u8; 32],
    author_id: PublicKey,
    delegation: Option<&Delegation>,
    tee_trigger: Option<&TeeTriggerCause>,
    governance_position: Option<&GovernanceParentEdge>,
    hlc: HybridTimestamp,
    signature: &[u8; 64],
) -> eyre::Result<VerifiedEnvelope> {
    if let Some(trigger) = tee_trigger {
        // A TEE run on someone's behalf is not a thing the TEE path defines, and
        // the executor refuses to produce one; a delta claiming both is forged.
        if delegation.is_some() {
            eyre::bail!("a delta cannot be both delegated and TEE-triggered");
        }
        let payload = tee_delta_signature_payload(
            context_id,
            delta_id,
            author_id,
            trigger,
            governance_position,
            hlc,
        )
        .map_err(|err| eyre::eyre!("failed to serialize TEE delta payload: {err}"))?;
        author_id
            .verify_raw_signature(&payload, signature)
            .map_err(|err| {
                eyre::eyre!("TEE delta envelope signature verification failed: {err}")
            })?;
        return Ok(VerifiedEnvelope::Tee(trigger.clone()));
    }

    let Some(delegation) = delegation else {
        verify_delta_signature(
            context_id,
            delta_id,
            author_id,
            governance_position,
            hlc,
            signature,
        )?;
        return Ok(VerifiedEnvelope::SelfAuthored);
    };

    // Order matters. The bundle is checked before the envelope signature so a
    // delta carrying a malformed credential is refused for that, rather than for
    // a signature failure that sends the reader looking at the wrong thing.
    let warrant = delegation
        .verify()
        .map_err(|err| eyre::eyre!("delegated delta carries an invalid delegation: {err}"))?;

    if warrant.author_device_key != author_id {
        eyre::bail!(
            "delegated delta names author {author_id} but its warrant was signed by {}",
            warrant.author_device_key
        );
    }
    if warrant.context != context_id {
        eyre::bail!("delegated delta is in context {context_id} but its warrant is for another");
    }

    let payload = delegated_delta_signature_payload(
        context_id,
        delta_id,
        author_id,
        delegation,
        governance_position,
        hlc,
    )
    .map_err(|err| eyre::eyre!("failed to serialize delegated delta payload: {err}"))?;

    delegation
        .executor_key
        .verify_raw_signature(&payload, signature)
        .map_err(|err| {
            eyre::eyre!("delegated delta envelope signature verification failed: {err}")
        })?;

    Ok(VerifiedEnvelope::Delegated(Box::new(warrant)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use calimero_primitives::identity::PrivateKey;

    /// A fixed HLC, so the payload bytes are deterministic across runs.
    fn hlc() -> HybridTimestamp {
        HybridTimestamp::default()
    }

    fn fixture() -> (ContextId, [u8; 32], PrivateKey, PublicKey) {
        let private_key = PrivateKey::from([3u8; 32]);
        let author_id = private_key.public_key();
        let context_id = ContextId::from([7u8; 32]);
        let delta_id = [9u8; 32];
        (context_id, delta_id, private_key, author_id)
    }

    #[test]
    fn sign_then_verify_roundtrip_no_position() {
        let (context_id, delta_id, sk, pk) = fixture();
        let payload = delta_signature_payload(context_id, delta_id, pk, None, hlc()).unwrap();
        let sig = sk.sign(&payload).unwrap().to_bytes();
        assert!(verify_delta_signature(context_id, delta_id, pk, None, hlc(), &sig).is_ok());
    }

    #[test]
    fn verify_rejects_tampered_context_id() {
        let (context_id, delta_id, sk, pk) = fixture();
        let payload = delta_signature_payload(context_id, delta_id, pk, None, hlc()).unwrap();
        let sig = sk.sign(&payload).unwrap().to_bytes();
        // Author signed for `context_id`; verifier reconstructs payload
        // with a *different* context_id, so the bytes diverge and the
        // signature should not verify. This is the anti-cross-context-
        // replay property the payload buys us.
        let other_context = ContextId::from([1u8; 32]);
        assert!(verify_delta_signature(other_context, delta_id, pk, None, hlc(), &sig).is_err());
    }

    #[test]
    fn verify_rejects_tampered_author() {
        let (context_id, delta_id, sk, pk) = fixture();
        let payload = delta_signature_payload(context_id, delta_id, pk, None, hlc()).unwrap();
        let sig = sk.sign(&payload).unwrap().to_bytes();
        // Same signature bytes, but a *different* author is claimed on
        // the wire. `verify_raw_signature` uses the claimed author's key,
        // which never signed this payload — must fail. This is the
        // anti-impersonation property that gossip's
        // `membership_status_at` check alone doesn't catch.
        let other_pk = PrivateKey::from([4u8; 32]).public_key();
        assert!(verify_delta_signature(context_id, delta_id, other_pk, None, hlc(), &sig).is_err());
    }

    #[test]
    fn verify_rejects_tampered_hlc() {
        let (context_id, delta_id, sk, pk) = fixture();
        let payload = delta_signature_payload(context_id, delta_id, pk, None, hlc()).unwrap();
        let sig = sk.sign(&payload).unwrap().to_bytes();

        // `hlc` is a CLEARTEXT wire field that `CausalDelta::compute_id`
        // deliberately excludes, so the content-address gate cannot cover it —
        // binding it here is the only thing that does. Before it was in this
        // payload, a republisher could rewrite the HLC of a member's delta and
        // every gate still passed.
        let mut far_future = HybridTimestamp::default();
        far_future = HybridTimestamp::new(calimero_storage::logical_clock::Timestamp::new(
            calimero_storage::logical_clock::NTP64(u64::MAX),
            *far_future.get_id(),
        ));
        assert!(
            verify_delta_signature(context_id, delta_id, pk, None, far_future, &sig).is_err(),
            "a rewritten hlc must not verify against the author's signature"
        );
    }

    #[test]
    fn verify_rejects_tampered_signature_bytes() {
        let (context_id, delta_id, sk, pk) = fixture();
        let payload = delta_signature_payload(context_id, delta_id, pk, None, hlc()).unwrap();
        let mut sig = sk.sign(&payload).unwrap().to_bytes();
        sig[0] ^= 0xff;
        assert!(verify_delta_signature(context_id, delta_id, pk, None, hlc(), &sig).is_err());
    }

    #[test]
    fn sign_then_verify_roundtrip_with_edge() {
        let (context_id, delta_id, sk, pk) = fixture();
        let edge = calimero_context_config::types::GovernanceParentEdge {
            governance_dag_heads: vec![[6u8; 32], [7u8; 32]],
        };
        let payload =
            delta_signature_payload(context_id, delta_id, pk, Some(&edge), hlc()).unwrap();
        let sig = sk.sign(&payload).unwrap().to_bytes();
        assert!(verify_delta_signature(context_id, delta_id, pk, Some(&edge), hlc(), &sig).is_ok());
    }

    #[test]
    fn verify_rejects_tampered_governance_edge() {
        let (context_id, delta_id, sk, pk) = fixture();
        let edge_signed = calimero_context_config::types::GovernanceParentEdge {
            governance_dag_heads: vec![[6u8; 32]],
        };
        let payload =
            delta_signature_payload(context_id, delta_id, pk, Some(&edge_signed), hlc()).unwrap();
        let sig = sk.sign(&payload).unwrap().to_bytes();

        // Verifier reconstructs the payload with a different edge (different
        // heads); the signature must not verify. This is the per-cut binding
        // property — a signature for one governance cut can't be reused for a
        // different cut.
        let edge_other = calimero_context_config::types::GovernanceParentEdge {
            governance_dag_heads: vec![[6u8; 32], [9u8; 32]],
        };
        assert!(
            verify_delta_signature(context_id, delta_id, pk, Some(&edge_other), hlc(), &sig)
                .is_err()
        );
    }

    // ---------------------------------------------------------------- delegated

    use calimero_account::{
        AccountGenesis, AccountProof, Delegation, DeviceCert, DeviceId, KemPublicKey, Warrant,
        WarrantTerms,
    };
    use calimero_primitives::application::ApplicationId;

    /// The v2 terms these fixtures start from: the delta-auth path reads the
    /// context, the parties and the intent hash, and none of the fields added in
    /// #3933, so they are held fixed rather than varied.
    fn terms(
        context: ContextId,
        author_account: calimero_account::AccountId,
        executor: calimero_account::AccountId,
        intent_hash: [u8; 32],
        nonce: u64,
        not_after: u64,
    ) -> WarrantTerms {
        WarrantTerms {
            context,
            author_account,
            executor,
            app_version: ApplicationId::from([0u8; 32]),
            method: "send_message".to_owned(),
            intent_hash,
            account_heads: vec![],
            governance_floor: vec![],
            nonce,
            not_after,
        }
    }

    /// One party: a root key, the account it addresses, one device under it, and
    /// a root-signed certificate for that device.
    struct Party {
        account: calimero_account::AccountId,
        device_sk: PrivateKey,
        proof: Box<AccountProof<DeviceCert>>,
    }

    fn party(root_seed: u8, device_seed: u8, nonce: u8) -> Party {
        let root = PrivateKey::from([root_seed; 32]);
        let genesis = AccountGenesis::new(root.public_key());
        let account = genesis.account_id();
        let device = DeviceId::mint(account, [nonce; 16]);
        let device_sk = PrivateKey::from([device_seed; 32]);
        let kem_pk = KemPublicKey::from([nonce; 32]);
        let cert = DeviceCert::sign(
            &root,
            account,
            device,
            &device_sk.public_key(),
            &kem_pk,
            0,
            0,
        )
        .expect("cert must sign");
        Party {
            account,
            device_sk,
            proof: Box::new(AccountProof {
                genesis,
                chain: vec![],
                statement: cert,
            }),
        }
    }

    /// An author, an executor, and a delegation bundle from one to the other.
    fn bundle_for(context_id: ContextId) -> (Party, Party, Delegation) {
        let author = party(1, 2, 0x01);
        let executor = party(3, 4, 0x02);
        let warrant = Warrant::sign(
            &author.device_sk,
            terms(
                context_id,
                author.account,
                executor.account,
                [0xab; 32],
                7,
                1_755_903_600,
            ),
        )
        .expect("warrant must sign");
        let delegation = Delegation {
            warrant: Box::new(warrant),
            author_proof: author.proof.clone(),
            executor_proof: executor.proof.clone(),
            executor_key: executor.device_sk.public_key(),
        };
        (author, executor, delegation)
    }

    /// Sign a delegated envelope the way the relay would.
    fn sign_delegated(
        ctx: ContextId,
        delta: [u8; 32],
        author_id: PublicKey,
        d: &Delegation,
        ex_sk: &PrivateKey,
    ) -> [u8; 64] {
        let payload =
            delegated_delta_signature_payload(ctx, delta, author_id, d, None, hlc()).unwrap();
        ex_sk.sign(&payload).unwrap().to_bytes()
    }

    #[test]
    fn a_delegated_envelope_verifies_and_reports_its_warrant() {
        let ctx = ContextId::from([7u8; 32]);
        let delta = [9u8; 32];
        let (author, executor, d) = bundle_for(ctx);
        let author_id = author.device_sk.public_key();
        let sig = sign_delegated(ctx, delta, author_id, &d, &executor.device_sk);

        match verify_delta_envelope(ctx, delta, author_id, Some(&d), None, None, hlc(), &sig)
            .expect("a well-formed delegated envelope must verify")
        {
            VerifiedEnvelope::Delegated(w) => {
                assert_eq!(w.author_account, author.account);
                assert_eq!(w.executor, executor.account);
                assert_eq!(w.nonce, 7);
            }
            VerifiedEnvelope::SelfAuthored | VerifiedEnvelope::Tee(_) => {
                panic!("must report the delegated arm")
            }
        }
    }

    #[test]
    fn a_self_authored_envelope_reports_the_self_authored_arm() {
        let (ctx, delta, sk, pk) = fixture();
        let payload = delta_signature_payload(ctx, delta, pk, None, hlc()).unwrap();
        let sig = sk.sign(&payload).unwrap().to_bytes();

        assert!(matches!(
            verify_delta_envelope(ctx, delta, pk, None, None, None, hlc(), &sig).unwrap(),
            VerifiedEnvelope::SelfAuthored
        ));
    }

    /// The reason for a second domain: neither shape may pass as the other, or a
    /// relay could strip the warrant and present the result as self-authored.
    #[test]
    fn the_two_envelope_shapes_are_not_interchangeable() {
        let ctx = ContextId::from([7u8; 32]);
        let delta = [9u8; 32];
        let (author, executor, d) = bundle_for(ctx);
        let author_id = author.device_sk.public_key();

        // A delegated signature presented with no delegation, i.e. as self-authored.
        let deleg_sig = sign_delegated(ctx, delta, author_id, &d, &executor.device_sk);
        let _refused =
            verify_delta_envelope(ctx, delta, author_id, None, None, None, hlc(), &deleg_sig)
                .expect_err("a delegated signature must not verify as self-authored");

        // And a genuine self-authored signature presented as delegated.
        let self_payload = delta_signature_payload(ctx, delta, author_id, None, hlc()).unwrap();
        let self_sig = author.device_sk.sign(&self_payload).unwrap().to_bytes();
        let _also = verify_delta_envelope(
            ctx,
            delta,
            author_id,
            Some(&d),
            None,
            None,
            hlc(),
            &self_sig,
        )
        .expect_err("a self-authored signature must not verify as delegated");
    }

    #[test]
    fn an_envelope_naming_a_different_author_than_the_warrant_is_refused() {
        let ctx = ContextId::from([7u8; 32]);
        let delta = [9u8; 32];
        let (_author, executor, d) = bundle_for(ctx);
        let stranger = PrivateKey::from([0x31; 32]).public_key();

        // Signed honestly for the stranger, so only the mismatch can refuse it.
        let sig = sign_delegated(ctx, delta, stranger, &d, &executor.device_sk);

        let err = verify_delta_envelope(ctx, delta, stranger, Some(&d), None, None, hlc(), &sig)
            .expect_err("the envelope author must match the warrant's signer");
        assert!(
            err.to_string().contains("warrant was signed by"),
            "expected the author/warrant mismatch, got: {err}"
        );
    }

    #[test]
    fn a_warrant_for_another_context_is_refused() {
        let ctx = ContextId::from([7u8; 32]);
        let delta = [9u8; 32];
        let (author, executor, d) = bundle_for(ctx);
        let author_id = author.device_sk.public_key();
        let elsewhere = ContextId::from([0x41; 32]);

        let sig = sign_delegated(elsewhere, delta, author_id, &d, &executor.device_sk);

        let err = verify_delta_envelope(
            elsewhere,
            delta,
            author_id,
            Some(&d),
            None,
            None,
            hlc(),
            &sig,
        )
        .expect_err("a warrant must not be spendable in another context");
        assert!(
            err.to_string().contains("its warrant is for another"),
            "expected the context mismatch, got: {err}"
        );
    }

    /// Swapping the warrant between two deltas by the same executor for the same
    /// author is the recombination the embedded warrant exists to stop.
    #[test]
    fn a_signature_does_not_carry_over_to_a_substituted_warrant() {
        let ctx = ContextId::from([7u8; 32]);
        let delta = [9u8; 32];
        let (author, executor, d) = bundle_for(ctx);
        let author_id = author.device_sk.public_key();
        let sig = sign_delegated(ctx, delta, author_id, &d, &executor.device_sk);

        // A second, equally genuine warrant — same author, same executor, a
        // different intent.
        let other_warrant = Warrant::sign(
            &author.device_sk,
            terms(
                ctx,
                author.account,
                executor.account,
                [0xcd; 32],
                8,
                d.warrant.not_after,
            ),
        )
        .unwrap();
        let swapped = Delegation {
            warrant: Box::new(other_warrant),
            ..d.clone()
        };

        let _refused = verify_delta_envelope(
            ctx,
            delta,
            author_id,
            Some(&swapped),
            None,
            None,
            hlc(),
            &sig,
        )
        .expect_err("a signature must not verify against a substituted warrant");
    }

    /// A delegated delta whose bundle does not hang together is refused before
    /// any author-keyed gate could run on it.
    #[test]
    fn a_bundle_whose_executor_key_is_uncertified_is_refused() {
        let ctx = ContextId::from([7u8; 32]);
        let delta = [9u8; 32];
        let (author, _executor, d) = bundle_for(ctx);
        let author_id = author.device_sk.public_key();

        let rogue_sk = PrivateKey::from([0x77; 32]);
        let forged = Delegation {
            executor_key: rogue_sk.public_key(),
            ..d
        };
        // Signed by the key the bundle now names, so only the certificate check
        // can refuse it.
        let sig = sign_delegated(ctx, delta, author_id, &forged, &rogue_sk);

        let err = verify_delta_envelope(
            ctx,
            delta,
            author_id,
            Some(&forged),
            None,
            None,
            hlc(),
            &sig,
        )
        .expect_err("an executor key the operator never certified must be refused");
        assert!(
            err.to_string().contains("invalid delegation"),
            "expected the delegation to be refused, got: {err}"
        );
    }

    // ------------------------------------------------- recorded wire preimages
    //
    // Two byte-for-byte pins. They exist because merobox cannot reach this
    // class of break at all: every node in an e2e run is the SAME build, so a
    // change to a signed preimage stays invisible there — both sides compute
    // the new bytes and agree — and only shows up between a new node and one
    // that has not been restarted yet, as a signature that will not verify.
    //
    // What a recorded constant does and does not prove: it freezes the layout
    // as of the commit that recorded it. It cannot tell you the layout is
    // correct, only that it stopped being what it was. When one fails, the
    // change moved what every node signs, so every node must move with it.

    /// The SELF-AUTHORED preimage.
    #[test]
    fn the_self_authored_preimage_is_byte_frozen() {
        let (context_id, delta_id, _sk, author_id) = fixture();
        let payload = delta_signature_payload(context_id, delta_id, author_id, None, hlc())
            .expect("the payload must encode");

        assert_eq!(hex::encode(&payload), "0007070707070707070707070707070707070707070707070707070707070707070909090909090909090909090909090909090909090909090909090909090909ed4928c628d1c2c6eae90338905995612959273a5c63f93636c14614ac8737d10000000000000000000100000000000000");

        // Spelled out separately: the domain is the first byte, and it is the
        // self-authored one. A pin on the whole string would still pass if the
        // domain moved and something else moved to compensate.
        assert_eq!(payload[0], SignatureDomain::Delta as u8);
    }

    /// The DELEGATED preimage. The warrant is embedded whole (see
    /// [`the_warrant_is_embedded_verbatim_in_the_preimage`]), so any change to
    /// its encoding moves these bytes by construction.
    #[test]
    fn the_delegated_preimage_is_byte_frozen() {
        let context_id = ContextId::from([7u8; 32]);
        let (_author, _executor, delegation) = bundle_for(context_id);
        let author_id = delegation.warrant.author_device_key;

        let payload = delegated_delta_signature_payload(
            context_id,
            [9u8; 32],
            author_id,
            &delegation,
            None,
            hlc(),
        )
        .expect("the payload must encode");

        assert_eq!(hex::encode(&payload), "01070707070707070707070707070707070707070707070707070707070707070709090909090909090909090909090909090909090909090909090909090909098139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b394ca93ac1705187071d67b83c7ff0efe8108e8ec4530575d7726879333dbdabe7c070707070707070707070707070707070707070707070707070707070707070704cfa21629a77f8cd8ddd3f821ed514009a9f572b2ce8e0a11f5cbb5e25340b08139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b3943c9e2afa5cf44dc025651097c17af3363cecb1e3b3564705e6fc4354bb0b37a400000000000000000000000000000000000000000000000000000000000000000c00000073656e645f6d657373616765abababababababababababababababababababababababababababababababab0000000000000000070000000000000070f6a868000000009ed5be9e0252f5e8c67b4fb325ffc76b6316f0917fcdeba47b3d11f46f23a11f4fdd56e870e63911c40da917585efd7f29f10f7501e3b6c65d292b493211b50c0000000000000000000100000000000000");

        // A different first byte from the self-authored preimage, so neither can
        // ever be the other — which is what stops a self-authored signature
        // verifying as delegated and losing the warrant in flight.
        assert_eq!(payload[0], SignatureDomain::Delegated as u8);
    }

    /// The warrant is embedded whole, not by reference or by hash.
    ///
    /// A verifier reconstructs these bytes from what is on the wire, so if the
    /// warrant were ever hashed into the preimage instead of written into it,
    /// this length would drop by roughly the warrant's size and every peer
    /// would still agree with itself. The check is on the containment, not the
    /// number: the warrant's own signature must appear verbatim inside the
    /// preimage.
    #[test]
    fn the_warrant_is_embedded_verbatim_in_the_preimage() {
        let context_id = ContextId::from([7u8; 32]);
        let (_author, _executor, delegation) = bundle_for(context_id);
        let author_id = delegation.warrant.author_device_key;

        let payload = delegated_delta_signature_payload(
            context_id,
            [9u8; 32],
            author_id,
            &delegation,
            None,
            hlc(),
        )
        .expect("the payload must encode");

        let warrant_sig = delegation.warrant.signature;
        assert!(
            payload
                .windows(warrant_sig.len())
                .any(|w| w == warrant_sig.as_slice()),
            "the warrant's signature must appear in the signed bytes verbatim"
        );
    }

    fn deal() -> TeeTriggerCause {
        TeeTriggerCause::Event {
            cause: [5u8; 32],
            method: "deal".to_owned(),
        }
    }

    fn sign_tee(
        context_id: ContextId,
        delta_id: [u8; 32],
        sk: &PrivateKey,
        trigger: &TeeTriggerCause,
    ) -> [u8; 64] {
        let payload = tee_delta_signature_payload(
            context_id,
            delta_id,
            sk.public_key(),
            trigger,
            None,
            hlc(),
        )
        .unwrap();
        sk.sign(&payload).unwrap().to_bytes()
    }

    /// A TEE-triggered delta verifies as one, and hands back what fired it.
    #[test]
    fn a_tee_envelope_verifies_and_names_its_trigger() {
        let (ctx, delta, sk, pk) = fixture();
        let sig = sign_tee(ctx, delta, &sk, &deal());
        let verified =
            verify_delta_envelope(ctx, delta, pk, None, Some(&deal()), None, hlc(), &sig).unwrap();
        assert_eq!(verified.tee_trigger(), Some(&deal()));
    }

    /// The trigger is signed: a relay cannot claim a TEE's delta fired some
    /// other trigger, which is what a forged fired marker would amount to.
    #[test]
    fn a_tee_envelope_refuses_a_changed_trigger() {
        let (ctx, delta, sk, pk) = fixture();
        let sig = sign_tee(ctx, delta, &sk, &deal());
        let other = TeeTriggerCause::Timer {
            method: "reshuffle".to_owned(),
            tick: 1,
            every_secs: 60,
        };
        assert!(
            verify_delta_envelope(ctx, delta, pk, None, Some(&other), None, hlc(), &sig).is_err()
        );
    }

    /// The three domains keep the three shapes apart: an ordinary signature
    /// does not verify as a TEE-triggered one, nor the reverse.
    #[test]
    fn tee_and_self_authored_signatures_do_not_cross() {
        let (ctx, delta, sk, pk) = fixture();
        let plain = sk
            .sign(&delta_signature_payload(ctx, delta, pk, None, hlc()).unwrap())
            .unwrap()
            .to_bytes();
        assert!(
            verify_delta_envelope(ctx, delta, pk, None, Some(&deal()), None, hlc(), &plain)
                .is_err(),
            "adding a trigger to an ordinary delta must not verify"
        );
        let tee = sign_tee(ctx, delta, &sk, &deal());
        assert!(
            verify_delta_envelope(ctx, delta, pk, None, None, None, hlc(), &tee).is_err(),
            "stripping the trigger from a TEE delta must not verify"
        );
    }

    #[test]
    fn a_delta_cannot_be_both_delegated_and_tee_triggered() {
        let ctx = ContextId::from([7u8; 32]);
        let (_author, executor, delegation) = bundle_for(ctx);
        let author_id = delegation.warrant.author_device_key;
        let sig = sign_tee(ctx, [9u8; 32], &executor.device_sk, &deal());
        let err = verify_delta_envelope(
            ctx,
            [9u8; 32],
            author_id,
            Some(&delegation),
            Some(&deal()),
            None,
            hlc(),
            &sig,
        )
        .expect_err("a delegated TEE delta must be refused");
        assert!(err.to_string().contains("both delegated and TEE-triggered"));
    }

    /// The TEE preimage. See the note above the self-authored pin for what a
    /// recorded constant does and does not prove.
    #[test]
    fn the_tee_preimage_is_byte_frozen() {
        let (ctx, delta, _sk, author_id) = fixture();
        let payload =
            tee_delta_signature_payload(ctx, delta, author_id, &deal(), None, hlc()).unwrap();
        assert_eq!(hex::encode(&payload), "0207070707070707070707070707070707070707070707070707070707070707070909090909090909090909090909090909090909090909090909090909090909ed4928c628d1c2c6eae90338905995612959273a5c63f93636c14614ac8737d1000505050505050505050505050505050505050505050505050505050505050505040000006465616c0000000000000000000100000000000000");
        assert_eq!(payload[0], SignatureDomain::Tee as u8);
    }

    /// Every TEE authority must derive the same id for one firing, or one would
    /// fire a trigger another already recorded as fired under a different id.
    #[test]
    fn trigger_ids_are_byte_frozen() {
        let ctx = ContextId::from([7u8; 32]);
        assert_eq!(
            hex::encode(deal().id(&ctx)),
            "c2e67b1bcb0fb9287111e1420fa1c2284845acf71a879894d0336377ede00fd7"
        );
        let timer = TeeTriggerCause::Timer {
            method: "reshuffle".to_owned(),
            tick: 42,
            every_secs: 60,
        };
        assert_eq!(
            hex::encode(timer.id(&ctx)),
            "462635ef4c93a9c7a659cc5cc7ef7a12b4fe1efee0d746096743e818a379fa89"
        );
    }

    #[test]
    fn a_timer_names_when_its_tick_began() {
        let timer = |tick, every_secs| TeeTriggerCause::Timer {
            method: "reshuffle".to_owned(),
            tick,
            every_secs,
        };
        assert_eq!(timer(42, 60).tick_start_secs(), Some(2520));
        assert_eq!(timer(u64::MAX, 60).tick_start_secs(), None);
        assert_eq!(deal().tick_start_secs(), None);
        // A TEE claiming another period names another trigger.
        let ctx = ContextId::from([7u8; 32]);
        assert_ne!(timer(42, 60).id(&ctx), timer(42, 1).id(&ctx));
    }

    #[test]
    fn a_trigger_id_names_its_whole_cause() {
        let ctx = ContextId::from([1; 32]);
        let event = |cause, method: &str| TeeTriggerCause::Event {
            cause,
            method: method.to_owned(),
        };
        let a = event([1; 32], "resolve").id(&ctx);
        assert_ne!(a, event([2; 32], "resolve").id(&ctx));
        assert_ne!(a, event([1; 32], "deal").id(&ctx));
        // The delta it names is unique to one context already.
        assert_eq!(a, event([1; 32], "resolve").id(&ContextId::from([2; 32])));

        let timer = |method: &str, tick| TeeTriggerCause::Timer {
            method: method.to_owned(),
            tick,
            every_secs: 60,
        };
        let t = timer("tick", 7).id(&ctx);
        assert_ne!(t, timer("tick", 8).id(&ctx));
        assert_ne!(t, timer("sweep", 7).id(&ctx));
        assert_ne!(t, timer("tick", 7).id(&ContextId::from([2; 32])));
    }

    fn sign_fired(ctx: ContextId, sk: &PrivateKey, trigger: &TeeTriggerCause) -> [u8; 64] {
        let payload = tee_fired_payload(ctx, sk.public_key(), trigger).unwrap();
        sk.sign(&payload).unwrap().to_bytes()
    }

    #[test]
    fn a_fired_statement_verifies_for_its_trigger_only() {
        let (ctx, _, sk, pk) = fixture();
        let sig = sign_fired(ctx, &sk, &deal());
        verify_tee_fired(ctx, pk, &deal(), &sig).unwrap();

        let other = TeeTriggerCause::Event {
            cause: [1; 32],
            method: "deal".to_owned(),
        };
        assert!(verify_tee_fired(ctx, pk, &other, &sig).is_err());
        assert!(verify_tee_fired(ContextId::from([1; 32]), pk, &deal(), &sig).is_err());
    }

    /// A state beacon verifies for exactly the state it was signed over: not
    /// another root, other heads, another context, or under another key, and a
    /// delta envelope's signature is not one.
    #[test]
    fn a_state_beacon_verifies_for_its_state_only() {
        let (ctx, delta, sk, pk) = fixture();
        let heads = [[1; 32], [2; 32]];
        let payload = state_beacon_payload(ctx, pk, [9; 32], &heads).unwrap();
        assert_eq!(payload[0], SignatureDomain::StateBeacon as u8);
        let sig = sk.sign(&payload).unwrap().to_bytes();
        verify_state_beacon(ctx, pk, [9; 32], &heads, &sig).unwrap();

        assert!(verify_state_beacon(ctx, pk, [8; 32], &heads, &sig).is_err());
        assert!(verify_state_beacon(ctx, pk, [9; 32], &heads[..1], &sig).is_err());
        assert!(verify_state_beacon(ContextId::from([1; 32]), pk, [9; 32], &heads, &sig).is_err());
        let other = PrivateKey::from([0x42; 32]).public_key();
        assert!(verify_state_beacon(ctx, other, [9; 32], &heads, &sig).is_err());

        let envelope = sign_tee(ctx, delta, &sk, &deal());
        assert!(verify_state_beacon(ctx, pk, [9; 32], &heads, &envelope).is_err());
    }

    /// A fired statement is not a delta envelope, nor the reverse.
    #[test]
    fn a_fired_statement_and_a_tee_envelope_do_not_cross() {
        let (ctx, delta, sk, pk) = fixture();
        let fired = sign_fired(ctx, &sk, &deal());
        assert!(
            verify_delta_envelope(ctx, delta, pk, None, Some(&deal()), None, hlc(), &fired)
                .is_err()
        );
        let envelope = sign_tee(ctx, delta, &sk, &deal());
        assert!(verify_tee_fired(ctx, pk, &deal(), &envelope).is_err());
    }

    #[test]
    fn the_fired_preimage_is_byte_frozen() {
        let payload =
            tee_fired_payload(ContextId::from([7; 32]), PublicKey::from([9; 32]), &deal()).unwrap();
        assert_eq!(payload[0], SignatureDomain::TeeFired as u8);
        assert_eq!(hex::encode(&payload), "0307070707070707070707070707070707070707070707070707070707070707070909090909090909090909090909090909090909090909090909090909090909000505050505050505050505050505050505050505050505050505050505050505040000006465616c");
    }
}
