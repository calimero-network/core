use calimero_sdk::borsh::from_slice;
use calimero_sdk::{app, env};
use calimero_storage::address::Id;
use calimero_storage::collections::Root;
use calimero_storage::integration::Comparison;
use calimero_storage::interface::{Action, MainInterface, StorageError};
use calimero_storage::sync::{self, SyncArtifact};

use crate::KvStore;

#[app::logic]
impl KvStore {
    pub fn __calimero_sync_next() {
        Self::___calimero_sync_next().expect("fatal: sync failed");
    }

    fn ___calimero_sync_next() -> Result<(), StorageError> {
        let args = env::input().expect("fatal: missing input");

        let artifact =
            from_slice::<SyncArtifact>(&args).map_err(StorageError::DeserializationError)?;

        match artifact {
            SyncArtifact::Actions(actions) => {
                for action in actions {
                    let _ignored = match action {
                        Action::Compare { id } => {
                            sync::push_comparison(Comparison {
                                data: MainInterface::find_by_id_raw(id),
                                comparison_data: MainInterface::generate_comparison_data(Some(id))
                                    .unwrap(),
                            });
                        }
                        Action::Add { .. } | Action::Update { .. } | Action::Delete { .. } => {
                            MainInterface::apply_action(action).unwrap();
                        }
                    };
                }
            }
            SyncArtifact::Comparisons(comparisons) => {
                if comparisons.is_empty() {
                    sync::push_comparison(Comparison {
                        data: MainInterface::find_by_id_raw(Id::root()),
                        comparison_data: MainInterface::generate_comparison_data(None)?,
                    });
                }

                for Comparison {
                    data,
                    comparison_data,
                } in comparisons
                {
                    MainInterface::compare_affective(data, comparison_data)?;
                }
            }
        }

        Root::<Self>::commit_headless();

        Ok(())
    }
}
