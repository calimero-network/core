use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;

/// Everything a joining node needs from the namespace join response.
/// Single source of truth -- built by the responder, serialized over the wire,
/// consumed by the join handler.
#[derive(Debug, Clone)]
pub struct JoinBundle {
    /// ECDH-wrapped group key envelope (borsh-serialized KeyEnvelope).
    pub key_envelope_bytes: Vec<u8>,
    /// Context IDs registered under this namespace/group.
    pub context_ids: Vec<ContextId>,
    /// The application ID used by contexts in this group.
    pub application_id: ApplicationId,
    /// All namespace governance ops (borsh-serialized SignedNamespaceOp).
    pub governance_ops: Vec<Vec<u8>>,
    /// Namespace's `default_capabilities` value at the moment the
    /// invitation is fulfilled (issue #2256). Carries the bit set that
    /// new direct members of the namespace should inherit, replacing
    /// the previous joiner-side hard-coded fallback that could ignore
    /// admin overrides if the `DefaultCapabilitiesSet` governance op
    /// hadn't propagated by join time.
    pub default_capabilities: u32,
    /// The responder's consent to this join, when the responder was entitled to
    /// give it.
    ///
    /// `None` when the peer that served the exchange is not named in the
    /// invitation's `admitters` — it can still hand over the key and the
    /// governance history, because those are things a member may share, but it
    /// cannot authorise a membership. The joiner must then reach one that can;
    /// publishing without an endorsement is refused by every peer at apply.
    ///
    /// Borsh bytes rather than the type, to keep this crate off
    /// `calimero-governance-types`.
    pub admitter_endorsement_bytes: Option<Vec<u8>>,
}

impl JoinBundle {
    pub fn has_key(&self) -> bool {
        !self.key_envelope_bytes.is_empty()
    }

    /// An empty bundle: no key, no contexts, no governance ops, zero
    /// application id, default capabilities `0`. Used as the graceful
    /// fallback when the direct namespace-join request cannot reach a mesh
    /// peer.
    ///
    /// It no longer carries a joinable membership. A join needs an admitter's
    /// endorsement, and reaching no peer means reaching no admitter — so this
    /// bundle records what a joiner can still salvage locally, and the join
    /// itself fails rather than publishing an op every peer will refuse.
    ///
    /// It used to be the opposite: the joiner recorded membership from its
    /// signature-verified invitation and caught up via the gossip
    /// `KeyDelivery` fallback. That worked because the invitation alone was
    /// sufficient to admit — which is what made `admitters` unenforceable, so
    /// the two cannot both be true.
    pub fn empty() -> Self {
        Self {
            key_envelope_bytes: Vec::new(),
            context_ids: Vec::new(),
            application_id: ApplicationId::from([0u8; 32]),
            governance_ops: Vec::new(),
            default_capabilities: 0,
            admitter_endorsement_bytes: None,
        }
    }
}
