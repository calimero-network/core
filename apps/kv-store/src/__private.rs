use calimero_sdk::borsh::{from_slice, to_vec};
use calimero_sdk::{app, env};
use calimero_storage::collections::unordered_map::{Entry, UnorderedMap};
use calimero_storage::entities::Data;
use calimero_storage::integration::Comparison;
use calimero_storage::interface::{Action, Interface, StorageError};
use calimero_storage::sync::{self, SyncArtifact};

use crate::KvStore;

#[app::logic]
impl KvStore {
    pub fn __calimero_sync_next() -> Result<(), StorageError> {
        let args = env::input().expect("fatal: missing input");

        let artifact =
            from_slice::<SyncArtifact>(&args).map_err(StorageError::DeserializationError)?;

        let this = Interface::root::<Self>()?;

        match artifact {
            SyncArtifact::Actions(actions) => {
                for action in actions {
                    let _ignored = match action {
                        Action::Add { type_id, .. } | Action::Update { type_id, .. } => {
                            match type_id {
                                1 => Interface::apply_action::<KvStore>(action)?,
                                254 => Interface::apply_action::<Entry<String, String>>(action)?,
                                255 => {
                                    Interface::apply_action::<UnorderedMap<String, String>>(action)?
                                }
                                _ => return Err(StorageError::UnknownType(type_id)),
                            }
                        }
                        Action::Delete { .. } => {
                            todo!("how are we supposed to identify the entity to delete???????")
                        }
                        Action::Compare { .. } => {
                            todo!("how are we supposed to compare when `Comparison` needs `type_id`???????")
                        }
                    };
                }

                if let Some(this) = this {
                    return Interface::commit_root(this);
                }
            }
            SyncArtifact::Comparisons(comparisons) => {
                if comparisons.is_empty() {
                    sync::push_comparison(Comparison {
                        type_id: <Self as Data>::type_id(),
                        data: this
                            .as_ref()
                            .map(to_vec)
                            .transpose()
                            .map_err(StorageError::SerializationError)?,
                        comparison_data: Interface::generate_comparison_data(this.as_ref())?,
                    });
                }

                for Comparison {
                    type_id,
                    data,
                    comparison_data,
                } in comparisons
                {
                    match type_id {
                        1 => Interface::compare_affective::<KvStore>(data, comparison_data)?,
                        254 => Interface::compare_affective::<Entry<String, String>>(
                            data,
                            comparison_data,
                        )?,
                        255 => Interface::compare_affective::<UnorderedMap<String, String>>(
                            data,
                            comparison_data,
                        )?,
                        _ => return Err(StorageError::UnknownType(type_id)),
                    };
                }
            }
        }

        Ok(())
    }
}
