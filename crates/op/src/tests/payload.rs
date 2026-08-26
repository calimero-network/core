//! The append-only wire format.
//!
//! One test, and it is the crate's most consequential one: it pins the borsh
//! tag of every variant, and its exhaustive `match` means a new variant does
//! not compile until someone appends it here with a tag of its own — which is
//! the point at which they find out that inserting it in the middle would have
//! invalidated the signature of every stored op after it.

use crate::tests::support::every_op_payload;
use crate::OpPayload;

#[test]
fn op_payload_discriminants_are_pinned() {
    // Every variant, paired with the borsh discriminant it MUST keep forever
    // (see the append-only note on `OpPayload`). The exhaustive `match` below
    // means adding a variant fails to compile until it is appended here with
    // its own pinned tag — never inserted in the middle.
    let all = every_op_payload();

    // Exhaustive: a new variant forces a new arm here.
    fn pinned_tag(p: &OpPayload) -> u8 {
        match p {
            OpPayload::Put { .. } => 0,
            OpPayload::Delete { .. } => 1,
            OpPayload::SetWriters { .. } => 2,
            OpPayload::MemberAdded { .. } => 3,
            OpPayload::MemberRemoved { .. } => 4,
            OpPayload::AdminChanged { .. } => 5,
            OpPayload::PolicyUpdated { .. } => 6,
            OpPayload::SubgroupCreated { .. } => 7,
            OpPayload::SubgroupReparented { .. } => 8,
            OpPayload::SubgroupDeleted { .. } => 9,
            OpPayload::SubgroupVisibilitySet { .. } => 10,
            OpPayload::DefaultCapabilitiesSet { .. } => 11,
            OpPayload::MemberCapabilitySet { .. } => 12,
            OpPayload::Noop => 13,
            OpPayload::DeviceLinked { .. } => 14,
            OpPayload::DeviceRevoked { .. } => 15,
            OpPayload::AccountKeysRotated { .. } => 16,
            OpPayload::MemberJoinedWithDevice { .. } => 17,
            OpPayload::Opaque { .. } => 18,
        }
    }

    assert_eq!(all.len(), 19, "every OpPayload variant must be listed");
    for payload in &all {
        let bytes = borsh::to_vec(payload).expect("serialize");
        assert_eq!(
            bytes[0],
            pinned_tag(payload),
            "borsh discriminant drifted for {payload:?} — variants must be append-only"
        );
    }
}
