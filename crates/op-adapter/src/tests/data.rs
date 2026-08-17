//! Data plane: [`payload_from_action`].

use calimero_op::OpPayload;
use calimero_storage::action::Action;
use calimero_storage::address::Id;
use calimero_storage::entities::Metadata;

use crate::payload_from_action;

#[test]
fn data_plane_action_mapping() {
    let id = Id::new([1u8; 32]);
    let add = Action::Add {
        id,
        data: vec![1, 2, 3],
        ancestors: Vec::new(),
        metadata: Metadata::default(),
    };
    let upd = Action::Update {
        id,
        data: vec![4, 5],
        ancestors: Vec::new(),
        metadata: Metadata::default(),
    };
    let del = Action::DeleteRef {
        id,
        deleted_at: 0,
        metadata: Metadata::default(),
    };

    assert_eq!(
        payload_from_action(&add),
        Some(OpPayload::Put {
            entity: id,
            value: vec![1, 2, 3]
        })
    );
    assert_eq!(
        payload_from_action(&upd),
        Some(OpPayload::Put {
            entity: id,
            value: vec![4, 5]
        })
    );
    assert_eq!(
        payload_from_action(&del),
        Some(OpPayload::Delete { entity: id })
    );
}
