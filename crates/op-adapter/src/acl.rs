//! Access-control plane: a writer-set rotation → `SetWriters`.

use calimero_op::OpPayload;
use calimero_storage::address::Id;
use calimero_storage::rotation_log::RotationLogEntry;

/// Encode a writer-set rotation ([`RotationLogEntry`]) as a `SetWriters` op for
/// `object` (the Shared anchor whose ACL is being rotated).
///
/// The op's `parents` carry the rotation's causal position and its author is
/// `entry.signer`; this function captures only the payload — the caller
/// assembles the full `Op` (id/parents/author/hlc/signature) from the entry's
/// `delta_id`/`delta_hlc`/`signer`/`signature`.
#[must_use]
pub fn set_writers_payload(object: Id, entry: &RotationLogEntry) -> OpPayload {
    OpPayload::SetWriters {
        object,
        // Passed through, not bridged: a rotation log's writer set is ALREADY
        // account-keyed, so there is no key here to stand in for. The entry's
        // `signer` names a KEY, not an account, because a signature names a
        // key — which is exactly the split the account plane draws.
        writers: entry.new_writers.clone(),
    }
}
