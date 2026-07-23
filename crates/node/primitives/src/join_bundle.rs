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
}

impl JoinBundle {
    pub fn has_key(&self) -> bool {
        !self.key_envelope_bytes.is_empty()
    }

    /// An empty bundle: no key, no contexts, no governance ops, zero
    /// application id, default capabilities `0`. Used as the graceful
    /// fallback when the direct namespace-join request cannot reach a mesh
    /// peer, so the joiner can still record local membership from its
    /// (already signature-verified) invitation and catch the rest up via the
    /// gossip `KeyDelivery` fallback and namespace sync, rather than aborting
    /// the join and leaving the node not-a-member.
    pub fn empty() -> Self {
        Self {
            key_envelope_bytes: Vec::new(),
            context_ids: Vec::new(),
            application_id: ApplicationId::from([0u8; 32]),
            governance_ops: Vec::new(),
            default_capabilities: 0,
        }
    }
}
