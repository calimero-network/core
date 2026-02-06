//! Application query and listing functionality.
//!
//! This module provides functions to query and list installed applications,
//! packages, and versions.

use calimero_primitives::application::{Application, ApplicationBlob, ApplicationId};
use calimero_store::key;
use semver::Version;

use crate::client::NodeClient;

impl NodeClient {
    /// List all installed applications
    pub fn list_applications(&self) -> eyre::Result<Vec<Application>> {
        let handle = self.datastore.handle();

        let mut iter = handle.iter::<key::ApplicationMeta>()?;

        let mut applications = vec![];

        for (id, app) in iter.entries() {
            let (id, app) = (id?, app?);
            applications.push(Application::new(
                id.application_id(),
                ApplicationBlob {
                    bytecode: app.bytecode.blob_id(),
                    compiled: app.compiled.blob_id(),
                },
                app.size,
                app.source.parse()?,
                app.metadata.to_vec(),
            ));
        }

        Ok(applications)
    }

    /// List all packages
    pub fn list_packages(&self) -> eyre::Result<Vec<String>> {
        let handle = self.datastore.handle();
        let mut iter = handle.iter::<key::ApplicationMeta>()?;
        let mut packages = std::collections::HashSet::new();

        for (id, app) in iter.entries() {
            let (_, app) = (id?, app?);
            let _ = packages.insert(app.package.to_string());
        }

        Ok(packages.into_iter().collect())
    }

    /// List all versions of a package
    pub fn list_versions(&self, package: &str) -> eyre::Result<Vec<String>> {
        let handle = self.datastore.handle();
        let mut iter = handle.iter::<key::ApplicationMeta>()?;
        let mut versions = Vec::new();

        for (id, app) in iter.entries() {
            let (_, app) = (id?, app?);
            if app.package.as_ref() == package {
                versions.push(app.version.to_string());
            }
        }

        Ok(versions)
    }

    /// Get the latest version of a package (version string and application id)
    pub fn get_latest_version(
        &self,
        package: &str,
    ) -> eyre::Result<Option<(String, ApplicationId)>> {
        let handle = self.datastore.handle();
        let mut iter = handle.iter::<key::ApplicationMeta>()?;
        let mut latest_version: Option<(String, ApplicationId)> = None;

        for (id, app) in iter.entries() {
            let (id, app) = (id?, app?);
            if app.package.as_ref() == package {
                let version_str = app.version.to_string();
                match &latest_version {
                    None => latest_version = Some((version_str, id.application_id())),
                    Some((current_version_str, _)) => {
                        // Try semantic version comparison first
                        let is_newer = match (
                            Version::parse(&version_str),
                            Version::parse(current_version_str),
                        ) {
                            (Ok(new_version), Ok(current_version)) => {
                                // Both are valid semantic versions - use proper comparison
                                new_version > current_version
                            }
                            (Ok(_), Err(_)) => {
                                // New version is valid semver, current is not - prefer semver
                                true
                            }
                            (Err(_), Ok(_)) => {
                                // Current version is valid semver, new is not - keep current
                                false
                            }
                            (Err(_), Err(_)) => {
                                // Neither is valid semver - fall back to lexicographic comparison
                                version_str > *current_version_str
                            }
                        };

                        if is_newer {
                            latest_version = Some((version_str, id.application_id()));
                        }
                    }
                }
            }
        }

        Ok(latest_version)
    }
}
