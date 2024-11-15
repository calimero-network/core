use calimero_sdk::borsh::from_slice;
use calimero_sdk::{app, env};
use calimero_storage::address::Id;
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

        match artifact {
            SyncArtifact::Actions(actions) => {
                for action in actions {
                    let _ignored = match action {
                        Action::Compare { id } => {
                            sync::push_comparison(Comparison {
                                data: Interface::find_by_id_raw(id)?,
                                comparison_data: Interface::generate_comparison_data(Some(id))?,
                            });
                        }
                        Action::Add { .. } | Action::Update { .. } | Action::Delete { .. } => {
                            Interface::apply_action(action)?;
                        }
                    };
                }
            }
            SyncArtifact::Comparisons(comparisons) => {
                if comparisons.is_empty() {
                    sync::push_comparison(Comparison {
                        data: Interface::find_by_id_raw(Id::root())?,
                        comparison_data: Interface::generate_comparison_data(None)?,
                    });
                }

                for Comparison {
                    data,
                    comparison_data,
                } in comparisons
                {
                    Interface::compare_affective(data, comparison_data)?;
                }
            }
        }

        Ok(())
    }
}
