use std::io;

use calimero_primitives::application::{Application, ApplicationId, ApplicationSource};
use calimero_primitives::hash::Hash;
use calimero_store::{key, types};
use camino::Utf8PathBuf;
use eyre::bail;
use futures_util::TryStreamExt;
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

        let Some(application) = handle.get(&key::ApplicationMeta::new(*application_id))? else {
            return Ok(None);
        };

        Ok(Some(Application::new(
            *application_id,
            application.blob.blob_id(),
            application.size,
            application.source.parse()?,
            application.metadata.into_vec(),
        )))
    }

    pub async fn get_application_blob(
        &self,
        application_id: &ApplicationId,
    ) -> eyre::Result<Option<Vec<u8>>> {
        let handle = self.datastore.handle();

        let Some(application) = handle.get(&key::ApplicationMeta::new(*application_id))? else {
            return Ok(None);
        };

        let Some(mut stream) = self.get_blob(application.blob.blob_id())? else {
            bail!("fatal: application points to dangling blob");
        };

        // todo! we can preallocate the right capacity here
        // todo! once `blob_manager::get` -> Blob{size}:Stream
        let mut buf = vec![];

        // todo! guard against loading excessively large blobs into memory
        while let Some(chunk) = stream.try_next().await? {
            buf.extend_from_slice(&chunk);
        }

        Ok(Some(buf))
    }

    pub fn has_application(&self, application_id: &ApplicationId) -> eyre::Result<bool> {
        let handle = self.datastore.handle();

        if let Some(application) = handle.get(&key::ApplicationMeta::new(*application_id))? {
            return self.has_blob(application.blob.blob_id());
        }

        Ok(false)
    }

    fn install_application(
        &self,
        blob_id: calimero_primitives::blobs::BlobId,
        size: u64,
        source: &ApplicationSource,
        metadata: Vec<u8>,
    ) -> eyre::Result<ApplicationId> {
        let application = types::ApplicationMeta::new(
            key::BlobMeta::new(blob_id),
            size,
            source.to_string().into_boxed_str(),
            metadata.into_boxed_slice(),
        );

        let application_id = ApplicationId::from(*Hash::hash_borsh(&application)?);

        let mut handle = self.datastore.handle();

        handle.put(&key::ApplicationMeta::new(application_id), &application)?;

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

        self.install_application(blob_id, size, &(uri.as_str().parse()?), metadata)
    }

    pub async fn install_application_from_url(
        &self,
        url: Url,
        metadata: Vec<u8>,
        expected_hash: Option<Hash>,
    ) -> eyre::Result<ApplicationId> {
        let uri = url.as_str().parse()?;

        let response = reqwest::Client::new().get(url).send().await?;

        let expected_size = response.content_length();

        let (blob_id, size) = self
            .add_blob(
                response
                    .bytes_stream()
                    .map_err(io::Error::other)
                    .into_async_read(),
                expected_size,
                expected_hash,
            )
            .await?;

        self.install_application(blob_id, size, &uri, metadata)
    }

    pub fn uninstall_application(&self, application_id: ApplicationId) -> eyre::Result<()> {
        let application_meta_key = key::ApplicationMeta::new(application_id);
        let mut handle = self.datastore.handle();
        handle.delete(&application_meta_key)?;
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
}
