//! Pending specialized node invite request state tracking.
//!
//! This module provides state tracking for standard nodes to track pending invites
//! by nonce when handling verification responses from specialized nodes.
//!
//! ## State Machine
//!
//! ```text
//! Pending → AwaitingConfirmation → (confirmed) → Removed
//!                 ↓
//!           (TTL expired, 60s)
//!                 ↓
//!              Pending (retry allowed)
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use dashmap::DashMap;

/// TTL for awaiting confirmation before allowing retry (60 seconds).
pub const CONFIRMATION_TTL: Duration = Duration::from_secs(60);

/// State of a pending specialized node invite.
#[derive(Debug, Clone)]
pub enum InviteState {
    /// Waiting for a specialized node to send verification request.
    Pending,
    /// Invitation sent, waiting for join confirmation from specialized node.
    AwaitingConfirmation {
        /// When the invitation was sent.
        invited_at: Instant,
        /// The public key of the invited specialized node.
        invitee_public_key: PublicKey,
    },
}

impl InviteState {
    /// Check if this state is expired (AwaitingConfirmation past TTL).
    #[must_use]
    pub fn is_expired(&self) -> bool {
        match self {
            Self::Pending => false,
            Self::AwaitingConfirmation { invited_at, .. } => {
                invited_at.elapsed() > CONFIRMATION_TTL
            }
        }
    }

    /// Check if this state can accept a new verification request.
    /// Returns true if Pending or if AwaitingConfirmation has expired.
    #[must_use]
    pub fn can_accept_request(&self) -> bool {
        match self {
            Self::Pending => true,
            Self::AwaitingConfirmation { .. } => self.is_expired(),
        }
    }
}

/// Action to perform when a specialized node invitation response is received.
#[derive(Debug, Clone)]
pub enum SpecializedNodeInviteAction {
    /// Create a regular invitation and send it to the specialized node.
    HandleContextInvite {
        /// The context to invite the specialized node to.
        context_id: ContextId,
        /// The identity of the user initiating the invite.
        inviter_id: PublicKey,
    },
}

/// State for a pending specialized node invite request (standard node side).
#[derive(Debug, Clone)]
pub struct PendingSpecializedNodeInvite {
    /// Action to perform when response is received.
    pub action: SpecializedNodeInviteAction,
    /// Current state of this invite.
    pub state: InviteState,
}

impl PendingSpecializedNodeInvite {
    /// Create a new pending specialized node invite in Pending state.
    #[must_use]
    pub fn new(action: SpecializedNodeInviteAction) -> Self {
        Self {
            action,
            state: InviteState::Pending,
        }
    }

    /// Transition to AwaitingConfirmation state.
    pub fn transition_to_awaiting(&mut self, invitee_public_key: PublicKey) {
        self.state = InviteState::AwaitingConfirmation {
            invited_at: Instant::now(),
            invitee_public_key,
        };
    }

    /// Reset to Pending state (e.g., after TTL expiry).
    pub fn reset_to_pending(&mut self) {
        self.state = InviteState::Pending;
    }
}

/// Map of nonce -> pending action for specialized node invites (standard node side).
///
/// Uses DashMap for concurrent access since responses may arrive
/// on different threads/actors.
pub type PendingSpecializedNodeInvites = Arc<DashMap<[u8; 32], PendingSpecializedNodeInvite>>;

/// Create a new empty pending specialized node invites map.
#[must_use]
pub fn new_pending_specialized_node_invites() -> PendingSpecializedNodeInvites {
    Arc::new(DashMap::new())
}
