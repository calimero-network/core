use std::sync::Arc;

use calimero_primitives::application::{Application, ApplicationId, ApplicationSource};
use calimero_primitives::blobs::BlobId;
use calimero_primitives::hash::Hash;
use calimero_store::{key, types};
use camino::Utf8PathBuf;
use eyre::bail;
use reqwest::Url;
use tokio::fs::File;
use tokio_util::compat::TokioAsyncReadCompatExt;

use super::NodeClient;

impl NodeClient {
    pub fn get_application(
        &self,
        application_id: &ApplicationId,
    ) -> eyre::Result<Option<Application>> {
        let handle = self.datastore.handle();

        let key = key::ApplicationMeta::new(*application_id);

        let Some(application) = handle.get(&key)? else {
            return Ok(None);
        };

        let application = Application::new(
            *application_id,
            application.blob.blob_id(),
            application.size,
            application.source.parse()?,
            application.metadata.into_vec(),
        );

        Ok(Some(application))
    }

    pub async fn get_application_bytes(
        &self,
        application_id: &ApplicationId,
    ) -> eyre::Result<Option<Arc<[u8]>>> {
        let handle = self.datastore.handle();

        let key = key::ApplicationMeta::new(*application_id);

        let Some(application) = handle.get(&key)? else {
            return Ok(None);
        };

        let Some(bytes) = self.get_blob_bytes(&application.blob.blob_id()).await? else {
            bail!("fatal: application points to dangling blob");
        };

        Ok(Some(bytes))
    }

    pub async fn get_precompiled_application_bytes(
        &self,
        application_id: &ApplicationId,
    ) -> eyre::Result<Option<Arc<[u8]>>> {
        let handle = self.datastore.handle();

        let key = key::ApplicationMeta::new(*application_id);

        let Some(application) = handle.get(&key)? else {
            return Ok(None);
        };

        if application.precompiled_blob.blob_id() == BlobId::from([0; 32]) {
            return Ok(None);
        }

        let Some(bytes) = self
            .get_blob_bytes(&application.precompiled_blob.blob_id())
            .await?
        else {
            bail!("fatal: application points to dangling precompiled blob");
        };

        Ok(Some(bytes))
    }

    pub fn has_application(&self, application_id: &ApplicationId) -> eyre::Result<bool> {
        let handle = self.datastore.handle();

        let key = key::ApplicationMeta::new(*application_id);

        if let Some(application) = handle.get(&key)? {
            return self.has_blob(&application.blob.blob_id());
        }

        Ok(false)
    }

    async fn install_application(
        &self,
        blob_id: &BlobId,
        size: u64,
        source: &ApplicationSource,
        metadata: Vec<u8>,
    ) -> eyre::Result<ApplicationId> {
        let application = types::ApplicationMeta::new(
            key::BlobMeta::new(*blob_id),
            size,
            source.to_string().into_boxed_str(),
            metadata.into_boxed_slice(),
            key::BlobMeta::new(BlobId::from([0; 32])),
        );

        let application_id = ApplicationId::from(*Hash::hash_borsh(&application)?);

        let mut handle = self.datastore.handle();

        let key = key::ApplicationMeta::new(application_id);

        handle.put(&key, &application)?;

        Ok(application_id)
    }

    pub async fn install_application_from_path(
        &self,
        path: Utf8PathBuf,
        metadata: Vec<u8>,
    ) -> eyre::Result<ApplicationId> {
        let path = path.canonicalize_utf8()?;

        let file = File::open(&path).await?;

        let expected_size = file.metadata().await?.len();

        let (blob_id, size) = self
            .add_blob(file.compat(), Some(expected_size), None)
            .await?;

        let Ok(uri) = Url::from_file_path(path) else {
            bail!("non-absolute path")
        };

        self.install_application(&blob_id, size, &(uri.as_str().parse()?), metadata)
            .await
    }

    pub async fn install_application_from_url(
        &self,
        url: Url,
        metadata: Vec<u8>,
        expected_hash: Option<&Hash>,
    ) -> eyre::Result<ApplicationId> {
        let uri = url.as_str().parse()?;

        let response = reqwest::Client::new().get(url).send().await?;

        let _expected_size = response.content_length();

        // Collect bytes
        let wasm_bytes = response.bytes().await?;

        let (blob_id, size) = self
            .add_blob(
                wasm_bytes.as_ref(),
                Some(wasm_bytes.len() as u64),
                expected_hash,
            )
            .await?;

        self.install_application(&blob_id, size, &uri, metadata)
            .await
    }

    pub fn uninstall_application(&self, application_id: ApplicationId) -> eyre::Result<()> {
        let mut handle = self.datastore.handle();

        let key = key::ApplicationMeta::new(application_id);

        handle.delete(&key)?;

        Ok(())
    }

    pub fn list_applications(&self) -> eyre::Result<Vec<Application>> {
        let handle = self.datastore.handle();

        let mut iter = handle.iter::<key::ApplicationMeta>()?;

        let mut applications = vec![];

        for (id, app) in iter.entries() {
            let (id, app) = (id?, app?);
            applications.push(Application::new(
                id.application_id(),
                app.blob.blob_id(),
                app.size,
                app.source.parse()?,
                app.metadata.to_vec(),
            ));
        }

        Ok(applications)
    }

    pub async fn update_precompiled_blob(
        &self,
        application_id: &ApplicationId,
        precompiled_blob_id: BlobId,
    ) -> eyre::Result<()> {
        let mut handle = self.datastore.handle();

        let key = key::ApplicationMeta::new(*application_id);

        let Some(mut application) = handle.get(&key)? else {
            bail!("Application not found");
        };

        application.precompiled_blob = key::BlobMeta::new(precompiled_blob_id);

        handle.put(&key, &application)?;

        Ok(())
    }
}
