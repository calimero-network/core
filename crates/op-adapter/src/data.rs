//! Data plane: storage [`Action`] → `Put`/`Delete`.

use calimero_op::OpPayload;
use calimero_storage::action::Action;

/// Encode a storage data [`Action`] as an [`OpPayload`].
///
/// Every state-changing [`Action`] maps to an op, so this currently always
/// returns `Some`; the `Option` is retained so a future non-state-changing
/// action can encode as `None` without a signature change.
#[must_use]
pub fn payload_from_action(action: &Action) -> Option<OpPayload> {
    match action {
        Action::Add { id, data, .. } | Action::Update { id, data, .. } => Some(OpPayload::Put {
            entity: *id,
            value: data.clone(),
        }),
        Action::DeleteRef { id, .. } => Some(OpPayload::Delete { entity: *id }),
    }
}
