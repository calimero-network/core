use std::io;
use std::sync::Arc;

use calimero_primitives::application::{
    Application, ApplicationBlob, ApplicationId, ApplicationSource,
};
use calimero_primitives::blobs::BlobId;
use calimero_primitives::hash::Hash;
use calimero_store::{key, types};
use camino::Utf8PathBuf;
use eyre::bail;
use eyre::OptionExt;
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

        let key = key::ApplicationMeta::new(*application_id);

        let Some(application) = handle.get(&key)? else {
            return Ok(None);
        };

        let application = Application::new(
            *application_id,
            ApplicationBlob {
                bytecode: application.bytecode.blob_id(),
                compiled: application.compiled.blob_id(),
            },
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

        let Some(bytes) = self
            .get_blob_bytes(&application.bytecode.blob_id(), None)
            .await?
        else {
            bail!("fatal: application points to dangling blob");
        };

        Ok(Some(bytes))
    }

    pub fn has_application(&self, application_id: &ApplicationId) -> eyre::Result<bool> {
        let handle = self.datastore.handle();

        let key = key::ApplicationMeta::new(*application_id);

        if let Some(application) = handle.get(&key)? {
            return self.has_blob(&application.bytecode.blob_id());
        }

        Ok(false)
    }

    pub fn install_application(
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

        let application_id = {
            let components = (
                application.bytecode,
                application.size,
                &application.source,
                &application.metadata,
            );

            ApplicationId::from(*Hash::hash_borsh(&components)?)
        };

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

        self.install_application(&blob_id, size, &uri.as_str().parse()?, metadata)
    }

    pub async fn install_application_from_url(
        &self,
        url: Url,
        metadata: Vec<u8>,
        expected_hash: Option<&Hash>,
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

        self.install_application(&blob_id, size, &uri, metadata)
    }

    pub fn uninstall_application(&self, application_id: &ApplicationId) -> eyre::Result<()> {
        let mut handle = self.datastore.handle();

        let key = key::ApplicationMeta::new(*application_id);

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

    pub fn update_compiled_app(
        &self,
        application_id: &ApplicationId,
        compiled_blob_id: &BlobId,
    ) -> eyre::Result<()> {
        let mut handle = self.datastore.handle();

        let key = key::ApplicationMeta::new(*application_id);

        let Some(mut application) = handle.get(&key)? else {
            bail!("application not found");
        };

        application.compiled = key::BlobMeta::new(*compiled_blob_id);

        handle.put(&key, &application)?;

        Ok(())
    }

    pub async fn install_application_from_manifest(
        &self,
        manifest: serde_json::Value,
    ) -> eyre::Result<ApplicationId> {
        // Derive canonical id from manifest fields (ignore app.id)
        // Expect: manifest.app { name, namespace, developer_pubkey }
        let app = manifest.get("app").ok_or_eyre("manifest.app missing")?;

        let name = app
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_eyre("manifest.app.name missing")?
            .trim()
            .to_lowercase();
        let namespace = app
            .get("namespace")
            .and_then(|v| v.as_str())
            .ok_or_eyre("manifest.app.namespace missing")?
            .trim()
            .to_lowercase();
        let developer = app
            .get("developer_pubkey")
            .and_then(|v| v.as_str())
            .ok_or_eyre("manifest.app.developer_pubkey missing")?
            .trim();

        let canonical = format!("{namespace}.{name}:{developer}");
        let application_id = ApplicationId::from(*Hash::new(canonical.as_bytes()).as_ref());

        // Pick artifact: prefer wasm target node
        let artifacts = manifest
            .get("artifacts")
            .and_then(|v| v.as_array())
            .ok_or_eyre("manifest.artifacts missing or invalid")?;

        let artifact = artifacts
            .iter()
            .find(|a| {
                a.get("type").and_then(|v| v.as_str()) == Some("wasm")
                    && a.get("target").and_then(|v| v.as_str()) == Some("node")
            })
            .or_else(|| {
                artifacts
                    .iter()
                    .find(|a| a.get("type").and_then(|v| v.as_str()) == Some("wasm"))
            })
            .ok_or_eyre("no suitable wasm artifact in manifest")?;

        let size = artifact.get("size").and_then(|v| v.as_u64());
        let mirrors = artifact
            .get("mirrors")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
            .unwrap_or_default();
        let cid = artifact.get("cid").and_then(|v| v.as_str());
        let path = artifact.get("path").and_then(|v| v.as_str());

        // Source selection precedence: local path (dev) > https mirror > cid via gateway
        let (blob_id, size, uri) = if let Some(path_str) = path {
            let path_buf = Utf8PathBuf::from(path_str);
            let path_buf = path_buf.canonicalize_utf8()?;
            let file = File::open(&path_buf).await?;
            let file_size = file.metadata().await?.len();
            let (blob_id, size) = self.add_blob(file.compat(), Some(file_size), None).await?;
            let Ok(uri) = Url::from_file_path(&path_buf) else {
                bail!("non-absolute path")
            };
            (blob_id, size, uri)
        } else {
            let url = mirrors
                .into_iter()
                .find(|u| u.starts_with("https://") || u.starts_with("http://localhost"))
                .map(|s| s.to_string())
                .or_else(|| cid.map(|c| format!("https://ipfs.io/ipfs/{c}")))
                .ok_or_eyre("no artifact path, url or cid available")?;

            let uri = Url::parse(&url)?;
            let response = reqwest::Client::new().get(uri.clone()).send().await?;
            let expected_size = response.content_length().or(size);
            let (blob_id, size) = self
                .add_blob(
                    response
                        .bytes_stream()
                        .map_err(io::Error::other)
                        .into_async_read(),
                    expected_size,
                    None,
                )
                .await?;
            (blob_id, size, uri)
        };

        // Build metadata from manifest (store compact JSON)
        let metadata = serde_json::to_vec(&manifest)?;

        self.install_application_with_id(
            &application_id,
            &blob_id,
            size,
            &uri.as_str().parse()?,
            metadata,
        )
    }

    pub fn install_application_with_id(
        &self,
        application_id: &ApplicationId,
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

        let mut handle = self.datastore.handle();

        let key = key::ApplicationMeta::new(*application_id);

        handle.put(&key, &application)?;

        Ok(*application_id)
    }

    pub async fn install_application_from_url_with_id(
        &self,
        application_id: &ApplicationId,
        url: Url,
        metadata: Vec<u8>,
        expected_hash: Option<&Hash>,
    ) -> eyre::Result<ApplicationId> {
        let uri: ApplicationSource = url.as_str().parse()?;

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

        self.install_application_with_id(application_id, &blob_id, size, &uri, metadata)
    }

    pub async fn install_application_from_path_with_id(
        &self,
        application_id: &ApplicationId,
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

        self.install_application_with_id(
            application_id,
            &blob_id,
            size,
            &uri.as_str().parse()?,
            metadata,
        )
    }

    pub async fn update_application_from_url(
        &self,
        application_id: &ApplicationId,
        url: Url,
        metadata: Vec<u8>,
        expected_hash: Option<&Hash>,
    ) -> eyre::Result<()> {
        let uri: ApplicationSource = url.as_str().parse()?;

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

        let mut handle = self.datastore.handle();
        let key = key::ApplicationMeta::new(*application_id);
        let Some(mut application) = handle.get(&key)? else {
            bail!("application not found")
        };

        application.bytecode = key::BlobMeta::new(blob_id);
        application.size = size;
        application.source = uri.to_string().into_boxed_str();
        application.metadata = metadata.into_boxed_slice();
        application.compiled = key::BlobMeta::new(BlobId::from([0; 32]));

        handle.put(&key, &application)?;
        Ok(())
    }

    pub async fn update_application_from_path(
        &self,
        application_id: &ApplicationId,
        path: Utf8PathBuf,
        metadata: Vec<u8>,
    ) -> eyre::Result<()> {
        let path = path.canonicalize_utf8()?;
        let file = File::open(&path).await?;
        let expected_size = file.metadata().await?.len();
        let (blob_id, size) = self
            .add_blob(file.compat(), Some(expected_size), None)
            .await?;

        let Ok(uri) = Url::from_file_path(path) else {
            bail!("non-absolute path")
        };

        let mut handle = self.datastore.handle();
        let key = key::ApplicationMeta::new(*application_id);
        let Some(mut application) = handle.get(&key)? else {
            bail!("application not found")
        };

        application.bytecode = key::BlobMeta::new(blob_id);
        application.size = size;
        application.source = uri.as_str().to_string().into_boxed_str();
        application.metadata = metadata.into_boxed_slice();
        application.compiled = key::BlobMeta::new(BlobId::from([0; 32]));

        handle.put(&key, &application)?;
        Ok(())
    }
}
