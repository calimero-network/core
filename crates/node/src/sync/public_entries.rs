//! Test fixture: Public entries under a context's root, and tombstones for them.

use calimero_node_primitives::sync::{create_runtime_env, EntityDeletion};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_storage::action::Action;
use calimero_storage::address::Id;
use calimero_storage::entities::{ChildInfo, Metadata, SignatureData, StorageType};
use calimero_storage::env::{with_runtime_env, RuntimeEnv};
use calimero_storage::index::Index;
use calimero_storage::interface::{ApplyContext, Interface};
use calimero_storage::store::MainStorage;
use calimero_storage::tests::common::collection_at;
use calimero_store::Store;

const CELL_ANCHOR: [u8; 32] = [0x44; 32]; // the cell holding the second entry

pub(crate) struct PublicEntries {
    runtime_env: RuntimeEnv,
    ids: [Id; 2],
}

impl PublicEntries {
    /// Two Public entries under `context_id`'s root in `store`, the second at a
    /// collection id inside a cell, where a cell member's stamp also fits.
    pub(crate) fn seed(store: &Store, context_id: ContextId) -> Self {
        let runtime_env = create_runtime_env(
            store,
            context_id,
            PublicKey::from([0u8; 32]),
            calimero_account::AccountId::from([0xAC; 32]),
        );
        let root_id = Id::new(*context_id.as_ref());
        let ids = [
            Id::new([0x5E; 32]),
            collection_at(Id::new(CELL_ANCHOR), "entries"),
        ];
        with_runtime_env(runtime_env.clone(), || {
            Interface::<MainStorage>::apply_action(
                Action::Update {
                    id: root_id,
                    data: vec![],
                    ancestors: vec![],
                    metadata: Metadata::default(),
                },
                &ApplyContext::empty(),
            )
            .expect("create root");
            for id in ids {
                let (root_hash, _) = Index::<MainStorage>::get_hashes_for(root_id)
                    .expect("root hash")
                    .expect("root exists");
                Interface::<MainStorage>::apply_action(
                    Action::Add {
                        id,
                        data: b"kept".to_vec(),
                        ancestors: vec![ChildInfo::new(root_id, root_hash, Metadata::default())],
                        metadata: Metadata::new(100, 100),
                    },
                    &ApplyContext::empty(),
                )
                .expect("seed entry");
            }
        });
        Self { runtime_env, ids }
    }

    /// A tombstone for entry `i` carrying the entry's own metadata.
    pub(crate) fn tombstone(&self, i: usize) -> EntityDeletion {
        EntityDeletion {
            id: *self.ids[i].as_bytes(),
            deleted_at: 200,
            metadata: Metadata::new(100, 100),
        }
    }

    /// A tombstone for the entry inside the cell whose metadata claims a cell
    /// member stamp signed by `signer`.
    pub(crate) fn tombstone_claiming(&self, signer: PublicKey) -> EntityDeletion {
        let mut deletion = self.tombstone(1);
        deletion.metadata.storage_type = StorageType::SharedMember {
            anchor: Id::new(CELL_ANCHOR),
            signature_data: Some(SignatureData {
                signature: [0u8; 64],
                nonce: 200,
                signer: Some(signer),
            }),
        };
        deletion
    }

    pub(crate) fn exists(&self, id: [u8; 32]) -> bool {
        with_runtime_env(self.runtime_env.clone(), || {
            Index::<MainStorage>::get_index(Id::new(id))
                .expect("read index")
                .is_some()
        })
    }

    pub(crate) fn deleted(&self, i: usize) -> bool {
        with_runtime_env(self.runtime_env.clone(), || {
            Index::<MainStorage>::get_index(self.ids[i])
                .expect("read index")
                .and_then(|index| index.deleted_at)
                .is_some()
        })
    }
}
