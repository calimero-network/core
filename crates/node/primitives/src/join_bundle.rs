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
}
