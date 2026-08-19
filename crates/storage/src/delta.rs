//! Storage delta for synchronization.
//!
//! Represents the output of storage operations that needs to be synchronized
//! across nodes using a DAG (Directed Acyclic Graph) structure.

use core::cell::RefCell;
use std::collections::BTreeMap;
use std::io;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_account::AccountId;
use sha2::{Digest, Sha256};

use crate::action::Action;
use crate::address::Id;
use crate::entities::{Metadata, OpMask, SignatureData, StorageType};
use crate::env;
use crate::logical_clock::HybridTimestamp;

/// A causal delta in the DAG representing a set of CRDT actions.
///
/// Each delta has a unique ID (content hash) and references its parent delta(s),
/// forming a DAG structure that preserves causal ordering.
///
/// # Timestamp Strategy
///
/// Uses Hybrid Logical Clock (HLC) which contains:
/// - **Logical clock**: Guarantees causal ordering
/// - **Physical time**: Embedded in NTP64 format (first 32 bits = seconds since epoch)
///
/// The DAG provides coarse-grained ordering (delta-level), while HLC provides
/// fine-grained ordering (action-level).
///
/// **Note**: The delta ID does NOT include the HLC. It DOES include the
/// per-action timestamps, which is what lets a receiver authenticate them via
/// [`CausalDelta::content_address_matches`].
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize, PartialEq, Eq)]
pub struct CausalDelta {
    /// Unique ID: SHA256(parents || actions) - deterministic, excludes timestamp
    pub id: [u8; 32],

    /// Parent delta IDs (empty for root, 1 for sequential, 2+ for merges)
    pub parents: Vec<[u8; 32]>,

    /// CRDT actions in this delta
    pub actions: Vec<Action>,

    /// Hybrid timestamp for this delta (last/max HLC of actions).
    ///
    /// Provides both:
    /// - Causal ordering across deltas (logical clock)
    /// - Wall-clock semantics (physical time embedded in NTP64)
    pub hlc: HybridTimestamp,
}

impl CausalDelta {
    /// Compute the ID for a delta
    ///
    /// The ID content-addresses the delta's `parents` and the full content of
    /// each action — id, data, `ancestors`, the per-action timestamp, and the
    /// access-control triple.
    ///
    /// # What the preimage does NOT cover
    ///
    /// Not `hlc` (the parameter is unused), and not the action signature bytes
    /// (zeroed by
    /// `hash_metadata_storage_type_for_id`, so the id is stable across an
    /// action's placeholder-signature and stamped-signature states). Anything
    /// outside the preimage is NOT authenticated by
    /// [`Self::content_address_matches`], so it must not be treated as trusted
    /// input on a receive path — that is precisely how the `updated_at` hole
    /// this preimage now closes came to exist.
    ///
    /// # Action variants are domain-separated
    ///
    /// `Add` and `Update` are tagged distinctly. They previously shared a match
    /// arm and hashed identically, so a delta's `Add` could be relabelled an
    /// `Update` (or vice versa) without changing its id.
    ///
    /// # Parents are order-sensitive here
    ///
    /// Parents are hashed in the order given, so reordering them changes the
    /// id. This deliberately differs from `calimero_op::Op::compute_id`, which
    /// SORTS parents so the id is independent of the order a builder listed
    /// them in. Do not assume the two schemes agree: the order-sensitivity is
    /// what lets `content_address_matches` detect a re-parented delta whose
    /// parent SET is unchanged.
    pub fn compute_id(
        parents: &[[u8; 32]],
        actions: &[Action],
        _hlc: &HybridTimestamp,
    ) -> [u8; 32] {
        let mut hasher = Sha256::new();

        // Hash parents
        for parent in parents {
            hasher.update(parent);
        }

        // Hash each action's content, INCLUDING the per-action timestamps.
        //
        // The timestamps used to be excluded here, on the reasoning that they
        // are physical time and hashing them would stop two nodes executing the
        // same operations from deriving the same id. That exclusion opened a
        // hole once `content_address_matches` became a receive-path gate: the
        // gate authenticates exactly the preimage and nothing else, so anything
        // left out is unauthenticated. `metadata.updated_at` is the LWW
        // comparison key and is signed by NOTHING for `Public` / `Frozen`
        // entities (`sign_authorized_actions` covers only User/Shared/
        // SharedMember), and on the DAG-catchup and parent-fetch paths the
        // actions arrive as plaintext rather than sealed under the group key.
        // A responder could therefore inflate `updated_at` on a `Public` action
        // and win last-write-wins permanently — unbounded, unlike the HLC,
        // which a drift guard caps at 5s.
        //
        // Including them costs the cross-node determinism property, which
        // nothing actually relied on: a state delta has exactly one author, and
        // receivers content-address the bytes they were sent rather than
        // re-deriving an id from an independent execution. The HLC stays out
        // (see below) — it is a separate field, bounded by the drift guard, and
        // the `delta_id_deterministic_regardless_of_hlc` test still pins it.
        //
        // The signature bytes stay out of the preimage via
        // `hash_metadata_storage_type_for_id`, which zeroes them. That is load
        // bearing and must not be "simplified" into a full borsh hash of the
        // action: the id has to be stable across the placeholder-signature and
        // stamped-signature states of the same action.
        for action in actions {
            match action {
                Action::Add {
                    id,
                    data,
                    ancestors,
                    metadata,
                } => {
                    let id_bytes: [u8; 32] = (*id).into();
                    hasher.update(b"add");
                    hasher.update(id_bytes);
                    hasher.update(data);
                    hasher.update(borsh::to_vec(ancestors).unwrap_or_default());
                    hasher.update((*metadata.updated_at).to_le_bytes());
                    hash_metadata_storage_type_for_id(&mut hasher, metadata);
                }
                Action::Update {
                    id,
                    data,
                    ancestors,
                    metadata,
                } => {
                    let id_bytes: [u8; 32] = (*id).into();
                    hasher.update(b"update");
                    hasher.update(id_bytes);
                    hasher.update(data);
                    hasher.update(borsh::to_vec(ancestors).unwrap_or_default());
                    hasher.update((*metadata.updated_at).to_le_bytes());
                    hash_metadata_storage_type_for_id(&mut hasher, metadata);
                }
                Action::DeleteRef {
                    id,
                    deleted_at,
                    metadata,
                } => {
                    let id_bytes: [u8; 32] = (*id).into();
                    hasher.update(b"delete");
                    hasher.update(id_bytes);
                    hasher.update(deleted_at.to_le_bytes());
                    hash_metadata_storage_type_for_id(&mut hasher, metadata);
                }
            }
        }

        // HLC is NOT hashed - it's metadata for ordering/LWW conflict resolution.

        hasher.finalize().into()
    }

    /// Whether `delta_id` actually content-addresses `parents` + `actions`.
    ///
    /// [`Self::compute_id`] is the producer-side binding between a delta's id
    /// and its content, but for a long time nothing re-derived it on the
    /// receive side. The envelope signature covers the *id* only, so a delta's
    /// `parents` could be rewritten in flight while the signature still
    /// verified — and an empty `parents` then applied as a disconnected DAG
    /// head. Receivers call this before the DAG insert so `parents` and
    /// `actions` inherit the signature that covers the id.
    ///
    /// Mirrors [`Self::compute_id`]'s parameter list exactly, including the
    /// currently-unhashed `hlc`, so the two can never silently drift apart if
    /// the preimage changes. Note that this means `hlc` is NOT authenticated by
    /// this check today; it stays malleable on the wire and must not be relied
    /// on as a trusted input.
    ///
    /// A forged `hlc` is bounded rather than decisive, which is why closing it
    /// was not folded in here: the only consumer that acts on a remote delta's
    /// HLC is the local clock (`Root::sync` -> `env::update_hlc`), and
    /// `LogicalClock::update` refuses a remote timestamp more than 5s ahead of
    /// the local wall clock and takes a `max()` otherwise — so a past value is
    /// inert and a future one buys at most the drift tolerance, which real time
    /// then erases. Entity-level LWW keys off `metadata.updated_at`, not this
    /// field.
    #[must_use]
    pub fn content_address_matches(
        delta_id: &[u8; 32],
        parents: &[[u8; 32]],
        actions: &[Action],
        hlc: &HybridTimestamp,
    ) -> bool {
        Self::compute_id(parents, actions, hlc) == *delta_id
    }

    /// [`Self::content_address_matches`] applied to this delta's own fields.
    #[must_use]
    pub fn id_matches_content(&self) -> bool {
        Self::content_address_matches(&self.id, &self.parents, &self.actions, &self.hlc)
    }

    /// Get the physical timestamp (nanoseconds since epoch).
    #[must_use]
    pub fn physical_time(&self) -> u64 {
        // Extract physical time from HLC (first 32 bits of NTP64)
        let ntp64 = self.hlc.get_time().as_u64();
        let seconds = ntp64 >> 32;
        // Convert to nanoseconds
        seconds * 1_000_000_000
    }

    /// Get the logical clock value from HLC.
    #[must_use]
    pub fn logical_clock(&self) -> u64 {
        crate::logical_clock::logical_counter(&self.hlc) as u64
    }
}

/// Delta produced by storage operations for synchronization.
///
/// Two variants:
/// - [`StorageDelta::Actions`] — local apply, snapshot leaf push,
///   SDK→host commits. The verifier falls back to v2 stored-writers
///   semantics for `Shared` actions in this variant.
/// - [`StorageDelta::CausalActions`] — DAG-causal apply. The writer set
///   per Shared entity is pre-resolved from the rotation log + DAG
///   ancestry and the verifier validates Shared signatures against that
///   set instead of stored writers.
///
/// # This is a host→guest envelope, not a peer→peer one
///
/// Only `Actions` crosses the network. `CausalActions` is built by the
/// *applying* node (`ContextStorageApplier::apply`) and handed straight to
/// the guest's sync entrypoint, because its writer set is an authorization
/// input: whoever chooses it decides who may write. A peer that could
/// choose it would name itself a writer of any Shared object and its
/// signature would then verify against its own set. So the receive paths
/// accept the `Actions` variant only — `decrypt_delta_actions` refuses
/// anything else — and the applying node re-wraps those actions with a set
/// it resolved itself.
///
/// Wire tags are assigned by hand (see the [`BorshSerialize`]/
/// [`BorshDeserialize`] impls below): `Actions` = 0, `CausalActions` = 2.
/// Tag 1 was the removed state-based-Merkle-sync variant; `CausalActions`
/// deliberately keeps tag 2 so every persisted and in-flight delta stays
/// decodable.
#[derive(Debug)]
pub enum StorageDelta {
    /// A list of actions from direct operations.
    Actions(Vec<Action>),
    /// Actions delivered with DAG-causal context (#2266).
    ///
    /// `effective_writers` carries the pre-resolved writer set for
    /// every `Shared` entity touched by `actions`, computed by the
    /// *applying* node — never by the sender — via
    /// `rotation_log_reader::writers_at_authenticated(rotation_log, delta.parents, happens_before, verify)`
    /// over its own copy of the rotation log. Entries for non-Shared
    /// entities are omitted (the verifier doesn't consult them).
    CausalActions {
        /// Actions to apply.
        actions: Vec<Action>,
        /// Hash of the originating `CausalDelta`. Used by the
        /// rotation-log write hook to record the delta on detected
        /// writer-set changes.
        delta_id: [u8; 32],
        /// Hybrid timestamp of the originating `CausalDelta`. Used by
        /// the rotation-log write hook for sibling tiebreak (ADR 0001).
        delta_hlc: HybridTimestamp,
        /// Pre-resolved writer set per Shared entity touched by
        /// `actions`. The verifier validates Shared signatures against
        /// the entry for the action's entity id; non-Shared entities
        /// (User / Frozen / Public) are absent from the map.
        ///
        /// Trusted, and only because it never comes off the wire — see
        /// the variant docs above.
        effective_writers: BTreeMap<Id, BTreeMap<AccountId, OpMask>>,
        /// The ACCOUNT the delta's author speaks for, resolved by the applying
        /// node at the governance cut this delta cites.
        ///
        /// Rides here rather than in a separate channel because this and
        /// `effective_writers` are two halves of one question — "who may write"
        /// and "who is writing" — and a writer set resolved at one cut matched
        /// against a principal resolved at another can disagree.
        ///
        /// One value for the whole batch: a delta is authored by a single device,
        /// so a single account answers for every action in it.
        ///
        /// Trusted for exactly the same reason as the writer set above, and no
        /// other: it never comes off the wire. `None` means the applying node
        /// could not resolve the author, and every consumer treats that as a
        /// refusal — never as "authorize as whoever is applying".
        signer_account: Option<AccountId>,
    },
}

impl BorshSerialize for StorageDelta {
    fn serialize<W: io::Write>(&self, writer: &mut W) -> io::Result<()> {
        match self {
            // Tag 0 — kept byte-identical to `actions_artifact`'s hand-rolled
            // encode (pinned by `actions_artifact_matches_enum_encoding`).
            StorageDelta::Actions(actions) => {
                0u8.serialize(writer)?;
                actions.serialize(writer)?;
            }
            // Tag 2, not 1: tag 1 was the removed state-based-Merkle-sync variant.
            // Keeping `CausalActions` at its original tag preserves wire and
            // on-disk compatibility for every persisted and in-flight delta.
            StorageDelta::CausalActions {
                actions,
                delta_id,
                delta_hlc,
                effective_writers,
                signer_account,
            } => {
                2u8.serialize(writer)?;
                actions.serialize(writer)?;
                delta_id.serialize(writer)?;
                delta_hlc.serialize(writer)?;
                effective_writers.serialize(writer)?;
                signer_account.serialize(writer)?;
            }
        }
        Ok(())
    }
}

impl BorshDeserialize for StorageDelta {
    fn deserialize_reader<R: io::Read>(reader: &mut R) -> io::Result<Self> {
        // A genuinely-empty artifact (zero bytes) is the "no-op" sentinel that
        // `commit_root` emits when there are no actions. Distinguish it explicitly
        // from a *truncated* artifact: only a clean EOF on the very first byte maps
        // to the no-op. Once a tag byte is present the remaining fields must decode
        // in full — a short read there is an error, never a silently-accepted no-op.
        let mut tag = [0u8; 1];
        match read_tag_or_eof(reader, &mut tag)? {
            None => Ok(StorageDelta::Actions(vec![])), // empty artifact: no-op
            Some(0) => Ok(StorageDelta::Actions(Vec::deserialize_reader(reader)?)),
            // Tag 1 was the removed state-based-Merkle-sync variant; reject it.
            Some(2) => Ok(StorageDelta::CausalActions {
                actions: Vec::deserialize_reader(reader)?,
                delta_id: <[u8; 32]>::deserialize_reader(reader)?,
                delta_hlc: HybridTimestamp::deserialize_reader(reader)?,
                effective_writers: BTreeMap::deserialize_reader(reader)?,
                signer_account: Option::deserialize_reader(reader)?,
            }),
            Some(_) => Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid tag")),
        }
    }
}

/// Read the leading tag byte, distinguishing a truly-empty stream (`Ok(None)`)
/// from a present tag (`Ok(Some(byte))`). Retries on `Interrupted`. Any other
/// I/O error propagates, so a partial/short read is never swallowed.
fn read_tag_or_eof<R: io::Read>(reader: &mut R, buf: &mut [u8; 1]) -> io::Result<Option<u8>> {
    loop {
        match reader.read(buf) {
            Ok(0) => return Ok(None),
            Ok(_) => return Ok(Some(buf[0])),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
}

/// Thread-local context for DAG delta creation
struct DeltaContext {
    actions: Vec<Action>,
    current_heads: Vec<[u8; 32]>,
    /// Maximum HLC timestamp for actions in this delta
    max_hlc: Option<HybridTimestamp>,
}

impl DeltaContext {
    const fn new() -> Self {
        Self {
            actions: Vec::new(),
            current_heads: Vec::new(),
            max_hlc: None,
        }
    }

    /// Record an HLC timestamp for an action (tracks maximum)
    fn record_hlc(&mut self, ts: HybridTimestamp) {
        match &mut self.max_hlc {
            None => {
                self.max_hlc = Some(ts);
            }
            Some(max) => {
                if ts > *max {
                    *max = ts;
                }
            }
        }
    }

    /// Get HLC timestamp, creating default if empty
    fn get_hlc(&mut self) -> HybridTimestamp {
        self.max_hlc.unwrap_or_else(env::hlc_timestamp)
    }
}

thread_local! {
    static DELTA_CONTEXT: RefCell<DeltaContext> = const { RefCell::new(DeltaContext::new()) };
}

/// Records an action for eventual synchronisation.
///
/// This also captures an HLC timestamp to track fine-grained causal ordering.
///
/// # Parameters
///
/// * `action` - The action to record.
///
pub fn push_action(action: Action) {
    DELTA_CONTEXT.with(|ctx| {
        let mut context = ctx.borrow_mut();

        // Capture HLC timestamp for this action
        let hlc_ts = env::hlc_timestamp();
        context.record_hlc(hlc_ts);

        context.actions.push(action);
    });
}

/// Sets the current DAG heads for the next delta.
///
/// This should be called when initializing a context or after receiving deltas from peers.
pub fn set_current_heads(heads: Vec<[u8; 32]>) {
    DELTA_CONTEXT.with(|ctx| {
        ctx.borrow_mut().current_heads = heads;
    });
}

/// Gets the current DAG heads.
pub fn get_current_heads() -> Vec<[u8; 32]> {
    DELTA_CONTEXT.with(|ctx| ctx.borrow().current_heads.clone())
}

/// Creates a causal delta from the current context and commits it.
///
/// Returns the created CausalDelta which should be broadcast to peers.
///
/// # Errors
///
/// This function will return an error if there are issues serializing the delta.
pub fn commit_causal_delta(root_hash: &[u8; 32]) -> eyre::Result<Option<CausalDelta>> {
    DELTA_CONTEXT.with(|ctx| {
        let mut context = ctx.borrow_mut();

        // If no actions, nothing to commit
        if context.actions.is_empty() {
            return Ok(None);
        }

        // Create delta with current heads as parents
        let parents = std::mem::take(&mut context.current_heads);
        let actions = std::mem::take(&mut context.actions);
        let hlc = context.get_hlc();

        // Compute ID
        let id = CausalDelta::compute_id(&parents, &actions, &hlc);

        // Serialize for the environment directly from a borrow — avoids
        // cloning every action just to encode the artifact.
        let artifact = actions_artifact(&actions)?;

        let delta = CausalDelta {
            id,
            parents,
            actions,
            hlc,
        };

        // Update heads - this delta is now the new head
        context.current_heads = vec![delta.id];

        env::commit(root_hash, &artifact);

        Ok(Some(delta))
    })
}

/// Encode the [`StorageDelta::Actions`] artifact directly from a borrowed
/// slice: variant tag `0` followed by the borsh-encoded list. This is the
/// same wire format the manual [`BorshSerialize`] produces for the enum (and
/// that the manual [`BorshDeserialize`] above reads back), without cloning
/// the actions or round-tripping them through enum construction.
/// `actions_artifact_matches_enum_encoding` pins the equivalence.
fn actions_artifact(actions: &[Action]) -> io::Result<Vec<u8>> {
    let mut artifact = Vec::new();
    BorshSerialize::serialize(&0u8, &mut artifact)?;
    BorshSerialize::serialize(actions, &mut artifact)?;
    Ok(artifact)
}

/// Commits the root hash to the runtime, flushing any recorded actions as the
/// sync artifact. An empty action set commits the zero-byte no-op sentinel.
/// This function must only be called once.
///
/// # Errors
///
/// This function will return an error if there are issues serializing the
/// pending actions into the artifact.
pub fn commit_root(root_hash: &[u8; 32]) -> eyre::Result<()> {
    DELTA_CONTEXT.with(|ctx| {
        let mut context = ctx.borrow_mut();

        let actions = std::mem::take(&mut context.actions);

        let artifact = if actions.is_empty() {
            // Zero-byte no-op sentinel (decodes back to `Actions(vec![])`).
            vec![]
        } else {
            actions_artifact(&actions)?
        };

        env::commit(root_hash, &artifact);

        Ok(())
    })
}

/// Discards pending delta actions for the current thread.
///
/// This is useful for host-side storage flows that intentionally use
/// `Interface::save_raw()` for index/hash maintenance but do not produce a
/// sync artifact via `commit_root()` / `commit_causal_delta()`.
///
/// Note: this intentionally preserves `current_heads` so DAG head tracking is
/// unaffected for the surrounding execution context.
pub fn clear_pending_delta() {
    DELTA_CONTEXT.with(|ctx| {
        let mut context = ctx.borrow_mut();
        context.actions.clear();
        context.max_hlc = None;
    });
}

/// Resets the delta context for testing.
///
/// Clears all pending actions and heads. Use this between test commits to
/// simulate separate execution contexts.
#[cfg(test)]
pub fn reset_delta_context() {
    DELTA_CONTEXT.with(|ctx| {
        *ctx.borrow_mut() = DeltaContext::new();
    });
}

// Helper function to hash Metadata storage type
fn hash_metadata_storage_type_for_id(hasher: &mut Sha256, metadata: &Metadata) {
    match &metadata.storage_type {
        StorageType::Public | StorageType::Frozen => {
            hasher.update(borsh::to_vec(&metadata.storage_type).unwrap_or_default());
        }
        StorageType::User {
            owner,
            signature_data,
        } => {
            // Hash the User variant *without* the signature
            let partial_type = StorageType::User {
                owner: *owner,
                signature_data: signature_data.as_ref().map(|sig_data| SignatureData {
                    nonce: sig_data.nonce,
                    signature: [0; 64], // Use placeholder for hash
                    signer: sig_data.signer,
                }),
            };
            hasher.update(borsh::to_vec(&partial_type).unwrap_or_default());
        }
        StorageType::Shared {
            writers,
            signature_data,
        } => {
            // Hash the Shared variant *without* the signature
            let partial_type = StorageType::Shared {
                writers: writers.clone(),
                signature_data: signature_data.as_ref().map(|sig_data| SignatureData {
                    nonce: sig_data.nonce,
                    signature: [0; 64], // Use placeholder for hash
                    signer: sig_data.signer,
                }),
            };
            hasher.update(borsh::to_vec(&partial_type).unwrap_or_default());
        }
        StorageType::SharedMember {
            anchor,
            signature_data,
        } => {
            // Hash the SharedMember variant *without* the signature. Only the
            // anchor id is committed — the writer set lives at the anchor, so a
            // rotation leaves every member's hash unchanged.
            let partial_type = StorageType::SharedMember {
                anchor: *anchor,
                signature_data: signature_data.as_ref().map(|sig_data| SignatureData {
                    nonce: sig_data.nonce,
                    signature: [0; 64], // Use placeholder for hash
                    signer: sig_data.signer,
                }),
            };
            hasher.update(borsh::to_vec(&partial_type).unwrap_or_default());
        }
    }
}

#[cfg(test)]
mod borsh_roundtrip_tests {
    //! `StorageDelta` has hand-rolled `BorshSerialize`/`BorshDeserialize` impls
    //! that branch on a leading u8 tag (0=Actions, 2=CausalActions). Tag 1 was
    //! the removed state-based-Merkle-sync variant and `CausalActions` keeps tag
    //! 2 for wire/on-disk compatibility. These tests guard the wire format: a
    //! regression here silently corrupts every delta sent over the network.

    use core::num::NonZeroU128;

    use borsh::{from_slice, to_vec};

    use super::*;
    use crate::logical_clock::{Timestamp, ID, NTP64};

    fn make_action(byte: u8) -> Action {
        Action::Add {
            id: Id::from([byte; 32]),
            data: vec![byte; 4],
            ancestors: vec![],
            metadata: Metadata::new(100, 200),
        }
    }

    fn make_hlc(time: u64) -> HybridTimestamp {
        let ts = Timestamp::new(NTP64(time), ID::from(NonZeroU128::new(1).unwrap()));
        HybridTimestamp::new(ts)
    }

    fn assert_actions_equal(a: &[Action], b: &[Action]) {
        assert_eq!(a.len(), b.len(), "action count mismatch");
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x, y, "action mismatch");
        }
    }

    #[test]
    fn actions_variant_roundtrips() {
        let original = StorageDelta::Actions(vec![make_action(1), make_action(2)]);
        let bytes = to_vec(&original).unwrap();
        // Tag 0 prefix preserved.
        assert_eq!(bytes[0], 0);

        let decoded: StorageDelta = from_slice(&bytes).unwrap();
        match decoded {
            StorageDelta::Actions(actions) => {
                assert_actions_equal(&actions, &[make_action(1), make_action(2)]);
            }
            other => panic!("expected Actions, got {other:?}"),
        }
    }

    #[test]
    fn actions_artifact_matches_enum_encoding() {
        let actions = vec![make_action(1), make_action(2)];
        let direct = actions_artifact(&actions).unwrap();
        let via_enum = to_vec(&StorageDelta::Actions(actions)).unwrap();
        assert_eq!(
            direct, via_enum,
            "direct artifact encoding diverged from StorageDelta::Actions"
        );
    }

    #[test]
    fn empty_input_falls_back_to_empty_actions_noop() {
        // The zero-byte no-op sentinel (a commit with no pending actions)
        // decodes to `Actions(vec![])`, which applies nothing.
        let empty: StorageDelta = from_slice(&[]).unwrap();
        assert!(
            matches!(&empty, StorageDelta::Actions(v) if v.is_empty()),
            "expected fallback Actions(vec![]), got {empty:?}"
        );
    }

    #[test]
    fn removed_comparisons_tag_1_is_rejected() {
        // Tag 1 was the removed state-based-Merkle-sync variant. A stream that
        // leads with it must error rather than be misinterpreted.
        let result: Result<StorageDelta, _> = from_slice(&[1_u8]);
        assert!(
            result.is_err(),
            "expected error for removed tag 1, got {result:?}"
        );
    }

    #[test]
    fn causal_actions_variant_roundtrips() {
        // The #2266 wire format: actions + delta_id + delta_hlc +
        // BTreeMap<Id, BTreeMap<AccountId, OpMask>> — accounts, because the
        // writer set names people rather than the keys that sign for them.
        let entity_a = Id::from([0xA1_u8; 32]);
        let entity_b = Id::from([0xB2_u8; 32]);
        let writer1 = AccountId::from([0xAA_u8; 32]);
        let writer2 = AccountId::from([0xBB_u8; 32]);

        let mut effective_writers = BTreeMap::new();
        let _ = effective_writers.insert(
            entity_a,
            BTreeMap::from([(writer1, OpMask::FULL), (writer2, OpMask::FULL)]),
        );
        let _ = effective_writers.insert(entity_b, BTreeMap::from([(writer1, OpMask::FULL)]));

        let original = StorageDelta::CausalActions {
            actions: vec![make_action(0xFE)],
            delta_id: [0xCD; 32],
            delta_hlc: make_hlc(12_345),
            effective_writers: effective_writers.clone(),
            signer_account: Some(AccountId::from([0xAC; 32])),
        };

        let bytes = to_vec(&original).unwrap();
        assert_eq!(bytes[0], 2, "CausalActions must use tag 2");

        let decoded: StorageDelta = from_slice(&bytes).unwrap();
        match decoded {
            StorageDelta::CausalActions {
                actions,
                delta_id,
                delta_hlc,
                effective_writers: ew,
                signer_account: _,
            } => {
                assert_actions_equal(&actions, &[make_action(0xFE)]);
                assert_eq!(delta_id, [0xCD; 32]);
                assert_eq!(delta_hlc, make_hlc(12_345));
                assert_eq!(ew, effective_writers);
            }
            other => panic!("expected CausalActions, got {other:?}"),
        }
    }

    #[test]
    fn causal_actions_with_empty_effective_writers_roundtrips() {
        // Non-Shared-only deltas resolve nothing; the map is empty.
        // Receiver verifier sees None for every entity → v2 fallback.
        let original = StorageDelta::CausalActions {
            actions: vec![make_action(7)],
            delta_id: [0; 32],
            delta_hlc: make_hlc(0),
            effective_writers: BTreeMap::new(),
            signer_account: None,
        };
        let bytes = to_vec(&original).unwrap();
        let decoded: StorageDelta = from_slice(&bytes).unwrap();
        assert!(
            matches!(
                &decoded,
                StorageDelta::CausalActions { effective_writers, .. }
                    if effective_writers.is_empty()
            ),
            "expected CausalActions with empty effective_writers, got {decoded:?}"
        );
    }

    #[test]
    fn truncated_actions_artifact_errors_not_noop() {
        // Tag 0 (Actions) present but the following Vec<Action> is missing: a
        // truncated artifact must surface as an error, not be silently accepted
        // as an empty no-op. Only a zero-byte stream is the no-op sentinel.
        for truncated in [
            vec![0_u8],          // Actions tag, no length prefix
            vec![0_u8, 5, 0, 0], // Actions tag, partial (3/4-byte) length prefix
            vec![2_u8],          // CausalActions tag, no body
        ] {
            let result: Result<StorageDelta, _> = from_slice(&truncated);
            assert!(
                result.is_err(),
                "truncated artifact {truncated:?} must error, got {result:?}"
            );
        }

        // The zero-byte no-op sentinel still decodes.
        assert!(matches!(
            from_slice::<StorageDelta>(&[]).unwrap(),
            StorageDelta::Actions(v) if v.is_empty()
        ));
    }

    #[test]
    fn unknown_tag_errors() {
        // Forward-compat guard: a tag the receiver doesn't know must
        // surface as an error, not silent misinterpretation.
        let bytes = vec![99_u8];
        let result: Result<StorageDelta, _> = from_slice(&bytes);
        assert!(
            result.is_err(),
            "expected error for unknown tag, got {result:?}"
        );
    }

    /// A delta as an honest producer builds one: id derived from the parents
    /// and actions it actually carries.
    fn honest_delta(parents: Vec<[u8; 32]>) -> CausalDelta {
        let actions = vec![make_action(1), make_action(2)];
        let hlc = make_hlc(42);
        CausalDelta {
            id: CausalDelta::compute_id(&parents, &actions, &hlc),
            parents,
            actions,
            hlc,
        }
    }

    #[test]
    fn content_address_matches_accepts_honest_delta() {
        assert!(honest_delta(vec![[7_u8; 32]]).id_matches_content());
        // Genesis, on the `[0; 32]` convention the state-delta write path uses.
        assert!(honest_delta(vec![[0_u8; 32]]).id_matches_content());
    }

    #[test]
    fn content_address_matches_rejects_emptied_parents() {
        // The attack this check exists to stop: strip `parents` to empty so the
        // delta hits the vacuously-true branch of `DagStore::can_apply` and
        // applies immediately as a disconnected head. The signed `id` is left
        // untouched, because the envelope signature covers it and the attacker
        // cannot re-sign.
        let mut delta = honest_delta(vec![[7_u8; 32]]);
        delta.parents = vec![];
        assert!(
            !delta.id_matches_content(),
            "emptied parents must not content-address the original id"
        );
    }

    #[test]
    fn content_address_matches_rejects_reparented_delta() {
        // Not just the empty case — any re-parenting moves the causal cut that
        // at-cut authorization resolves against, so a swapped non-empty parent
        // set must be rejected too.
        let mut delta = honest_delta(vec![[7_u8; 32]]);
        delta.parents = vec![[9_u8; 32]];
        assert!(!delta.id_matches_content());

        // Including adding a parent alongside the genuine one.
        let mut delta = honest_delta(vec![[7_u8; 32]]);
        delta.parents = vec![[7_u8; 32], [9_u8; 32]];
        assert!(!delta.id_matches_content());
    }

    #[test]
    fn content_address_matches_rejects_swapped_actions() {
        let mut delta = honest_delta(vec![[7_u8; 32]]);
        delta.actions = vec![make_action(3)];
        assert!(!delta.id_matches_content());
    }

    #[test]
    fn content_address_does_not_cover_hlc() {
        // Documents a KNOWN LIMIT rather than desired behaviour: `compute_id`
        // deliberately excludes the HLC for determinism, so this check leaves
        // `hlc` malleable on the wire. Anything that needs a trusted HLC needs
        // a separate binding. If the preimage ever starts covering the HLC,
        // this test failing is the intended signal to revisit that.
        //
        // The exposure was traced rather than assumed: the local clock is the
        // only consumer of a remote delta's HLC, and its update rule rejects
        // anything >5s ahead and otherwise takes a max(), so the field is
        // bounded, not decisive. See `content_address_matches`.
        let mut delta = honest_delta(vec![[7_u8; 32]]);
        delta.hlc = make_hlc(9_999);
        assert!(delta.id_matches_content());
    }
}
