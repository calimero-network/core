use std::collections::HashMap;
use std::fs;

use calimero_network::client::NetworkClient;
use camino::Utf8PathBuf;
use tracing::info;

#[derive(Clone)]
pub struct Application {
    pub name: String,
    pub path: Utf8PathBuf,
}

pub(crate) struct ApplicationManager {
    pub network_client: NetworkClient,
    pub application_dir: Utf8PathBuf,
}

impl ApplicationManager {
    pub fn new(network_client: NetworkClient, application_dir: Utf8PathBuf) -> Self {
        Self {
            network_client,
            application_dir,
        }
    }

    // unused ATM, uncomment when used
    // pub fn get_registered_applications(
    //     &self,
    // ) -> Vec<&calimero_primitives::application::ApplicationId> {
    //     Vec::from_iter(self.applications.keys())
    // }

    pub fn is_application_registered(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
    ) -> bool {
        if let Some(latest_version_path) = self.get_latest_application_path(application_id) {
            true
        } else {
            false
        }
    }

    pub fn load_application_blob(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
    ) -> eyre::Result<Vec<u8>> {
        if let Some(latest_version_path) = self.get_latest_application_path(application_id) {
            Ok(fs::read(&latest_version_path)?)
        } else {
            eyre::bail!("failed to get application with id: {}", application_id)
        }
    }

    fn get_latest_application_path(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
    ) -> Option<String> {
        let application_base_path = self.application_dir.join(application_id.to_string());

        if let Ok(entries) = fs::read_dir(&application_base_path) {
            // Collect version folders that contain binary.wasm into a vector
            let mut versions_with_binary = entries
                .filter_map(|entry| {
                    if let Ok(entry) = entry {
                        let entry_path = entry.path();
                        if entry_path.is_dir() {
                            if let Some(folder_name) = entry_path.file_name() {
                                let folder_name_str = folder_name.to_string_lossy().into_owned();
                                if let Ok(version) = semver::Version::parse(&folder_name_str) {
                                    let binary_path = entry_path.join("binary.wasm");
                                    if binary_path.exists() {
                                        Some((version, entry_path))
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();

            versions_with_binary.sort_by(|a, b| b.0.cmp(&a.0));

            if let Some((_, path)) = versions_with_binary.first() {
                Some(path.to_string_lossy().into_owned())
            } else {
                None
            }
        } else {
            None
        }
    }
}
