//! Application query, listing, and management functionality.

use calimero_primitives::application::{Application, ApplicationBlob, ApplicationId};
use calimero_primitives::blobs::BlobId;
use calimero_store::key;
use eyre::bail;
use semver::Version;

use crate::client::NodeClient;

impl NodeClient {
    /// List all installed applications.
    pub fn list_applications(&self) -> eyre::Result<Vec<Application>> {
        let handle = self.datastore.handle();
        let mut iter = handle.iter::<key::ApplicationMeta>()?;
        let mut applications = vec![];

        for (id, app) in iter.entries() {
            let (id, app) = (id?, app?);
            applications.push(
                Application::new(
                    id.application_id(),
                    ApplicationBlob {
                        bytecode: app.bytecode.blob_id(),
                        compiled: app.compiled.blob_id(),
                    },
                    app.size,
                    app.source.parse()?,
                    app.metadata.to_vec(),
                )
                .with_bundle_info(
                    app.signer_id.to_string(),
                    app.package.to_string(),
                    app.version.to_string(),
                ),
            );
        }

        Ok(applications)
    }

    /// Returns `true` if `blob_id` is referenced as the bytecode or compiled
    /// artifact of any installed application (including its named services).
    ///
    /// Application artifacts are shared, content-addressed blobs that installed
    /// apps depend on to execute. Because blob deletion is a global,
    /// reference-counted operation with no per-caller ownership, an unrelated
    /// admin-api caller could otherwise release the last reference to an app's
    /// bytecode and brick every context running it. The blob-delete endpoint
    /// consults this guard and refuses such deletes.
    pub fn is_blob_application_artifact(&self, blob_id: &BlobId) -> eyre::Result<bool> {
        for app in self.list_applications()? {
            if app.blob.bytecode == *blob_id || app.blob.compiled == *blob_id {
                return Ok(true);
            }
            for svc in app.services.values() {
                if svc.bytecode == *blob_id || svc.compiled == *blob_id {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Update the compiled blob for an application (or a named service within it).
    pub fn update_compiled_app(
        &self,
        application_id: &ApplicationId,
        compiled_blob_id: &BlobId,
        service_name: Option<&str>,
    ) -> eyre::Result<()> {
        let mut handle = self.datastore.handle();
        let key = key::ApplicationMeta::new(*application_id);

        let Some(mut application) = handle.get(&key)? else {
            bail!("application not found");
        };

        match service_name {
            Some(name) => {
                let svc = application
                    .services
                    .iter_mut()
                    .find(|s| &*s.name == name)
                    .ok_or_else(|| {
                        eyre::eyre!(
                            "service '{}' not found in application when updating compiled blob",
                            name
                        )
                    })?;
                svc.compiled = key::BlobMeta::new(*compiled_blob_id);
            }
            None => {
                application.compiled = key::BlobMeta::new(*compiled_blob_id);
            }
        }

        handle.put(&key, &application)?;
        Ok(())
    }

    /// List all packages.
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

    /// List all versions of a package.
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

    /// Get the latest version of a package (version string and application id).
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
                        let is_newer = match (
                            Version::parse(&version_str),
                            Version::parse(current_version_str),
                        ) {
                            (Ok(new_version), Ok(current_version)) => new_version > current_version,
                            (Ok(_), Err(_)) => true,
                            (Err(_), Ok(_)) => false,
                            (Err(_), Err(_)) => version_str > *current_version_str,
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
