//! Application installation and uninstallation.

use std::io::{self, ErrorKind};
use std::sync::Arc;

use super::bundle;
use calimero_primitives::application::{ApplicationId, ApplicationSource};
use calimero_primitives::blobs::BlobId;
use calimero_primitives::content_hash::ContentHash;
use calimero_primitives::hash::Hash;
use calimero_store::{key, types};
use camino::Utf8PathBuf;
use eyre::bail;
use futures_util::{io::Cursor, TryStreamExt};
use reqwest::Url;
use tokio::fs::File;
use tokio_util::compat::TokioAsyncReadCompatExt;
use tracing::{debug, trace};

use crate::client::NodeClient;

const MAX_ERROR_BODY_LEN: usize = 256;

/// Maximum redirects to follow when downloading an application.
const MAX_INSTALL_REDIRECTS: usize = 10;

/// Total request timeout (connect + transfer) for a URL install. Generous so
/// large `.mpk` bundles over slow links still succeed, but bounded so a
/// malicious/slow host can't hold the install open indefinitely (slowloris).
const INSTALL_TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Connect timeout for a URL install.
const INSTALL_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Maximum size of an application artifact fetched from a URL. Bounds memory so
/// a hostile host can't OOM the node with an unbounded (or Content-Length-lying)
/// body. Generous headroom for real bundles.
const MAX_INSTALL_BYTES: u64 = 512 * 1024 * 1024;

/// Whether a URL's host must be refused as an SSRF target: a literal
/// loopback / private / link-local / unspecified IP, or `localhost`. Applied
/// here at fetch time so it also covers redirect hops, which the
/// request-validation layer cannot see.
///
/// SYNC(#3053): mirrors `calimero-server-primitives::validation::url_host_is_blocked`
/// (the two crates do not share a dependency). Keep the blocked ranges in step
/// with that copy until both are deduplicated into a shared crate (#3053).
///
/// Uses the typed `Url::host()` rather than string parsing so IPv6 literals
/// (including bracketed forms) are classified by the URL parser, not by manual
/// bracket stripping.
fn host_is_blocked(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => ip_is_blocked(std::net::IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => ip_is_blocked(std::net::IpAddr::V6(ip)),
        Some(url::Host::Domain(domain)) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            domain == "localhost" || domain.ends_with(".localhost")
        }
        None => true, // no host → refuse
    }
}

fn ip_is_blocked(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            // 100.64.0.0/10 (RFC 6598, CGNAT). SYNC(#3053).
            let is_cgnat = o[0] == 100 && (o[1] & 0xc0) == 0x40;
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || is_cgnat
        }
        std::net::IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() {
                return true;
            }
            if let Some(v4) = v6.to_ipv4_mapped() {
                return ip_is_blocked(std::net::IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            // fc00::/7 (unique-local) or fe80::/10 (link-local).
            (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

/// Build an HTTP client that refuses to connect to non-public addresses, on the
/// initial request and on every redirect hop (closes redirect-to-private SSRF).
///
/// Note: a public hostname whose DNS resolves to a private address (DNS
/// rebinding) is not caught here — that needs a custom resolver and is tracked
/// as a follow-up. The literal-IP and `localhost` cases, including via
/// redirect, are covered.
fn ssrf_guarded_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(INSTALL_CONNECT_TIMEOUT)
        .timeout(INSTALL_TOTAL_TIMEOUT)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if !matches!(attempt.url().scheme(), "http" | "https") {
                attempt.error("redirect to a non-http(s) scheme is blocked")
            } else if host_is_blocked(attempt.url()) {
                attempt.error("redirect to a non-public address is blocked")
            } else if attempt.previous().len() >= MAX_INSTALL_REDIRECTS {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
}

/// Read an HTTP response body into memory, enforcing a hard byte cap so a
/// missing or lying `Content-Length` can't grow the buffer without bound.
async fn read_body_capped(mut response: reqwest::Response, max: u64) -> eyre::Result<Vec<u8>> {
    let mut buf = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        // `> max` means a body of exactly `max` bytes is accepted (`max` is an
        // inclusive limit) while anything larger is rejected before it is
        // buffered — the running total never exceeds `max`.
        if buf.len() as u64 + chunk.len() as u64 > max {
            bail!("application artifact exceeded size limit of {max} bytes");
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

impl NodeClient {
    pub fn install_raw_wasm(
        &self,
        blob_id: &BlobId,
        size: u64,
        source: &ApplicationSource,
        metadata: Vec<u8>,
        package: &str,
        version: &str,
    ) -> eyre::Result<ApplicationId> {
        let application = types::ApplicationMeta::new(
            key::BlobMeta::new(*blob_id),
            size,
            source.to_string().into_boxed_str(),
            metadata.into_boxed_slice(),
            key::BlobMeta::new(BlobId::from([0; 32])),
            types::PackageInfo {
                package: package.to_owned().into_boxed_str(),
                version: version.to_owned().into_boxed_str(),
                signer_id: String::new().into_boxed_str(),
                // Raw wasm has no manifest and no guaranteed ABI.
                state_version: 0,
            },
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

    fn install_bundle_application(
        &self,
        blob_id: &BlobId,
        size: u64,
        source: &ApplicationSource,
        metadata: Vec<u8>,
        info: types::PackageInfo,
        services: Vec<types::ServiceMeta>,
    ) -> eyre::Result<ApplicationId> {
        let mut application = types::ApplicationMeta::new(
            key::BlobMeta::new(*blob_id),
            size,
            source.to_string().into_boxed_str(),
            metadata.into_boxed_slice(),
            key::BlobMeta::new(BlobId::from([0; 32])),
            info,
        );
        application.services = services;

        let application_id =
            ApplicationId::for_bundle(&application.package, &application.signer_id)?;

        // Bundle ids are version-stable (hash(package, signer)), so a new
        // version overwrites the row in place. The row is a download-cache
        // pointer ("latest fetched"); what a context executes is decided by
        // its per-context binding (activation marker / group app_key), so no
        // displaced-blob breadcrumb is needed.
        let mut handle = self.datastore.handle();
        let key = key::ApplicationMeta::new(application_id);
        handle.put(&key, &application)?;
        Ok(application_id)
    }

    /// Install a `.mpk`. The signature is mandatory and every wasm artifact is
    /// digest-checked before any bytes are stored.
    async fn install_bundle(
        &self,
        bundle_data: Arc<[u8]>,
        blob_id: &BlobId,
        stored_size: u64,
        source: &ApplicationSource,
    ) -> eyre::Result<ApplicationId> {
        // Every artifact, including the unnamed single-service one that gets no
        // blob of its own, so substituted bytes are refused before first execution.
        let (verified, wasm) = tokio::task::spawn_blocking(move || -> eyre::Result<_> {
            let verified = bundle::VerifiedBundle::open(bundle_data)?;
            let wasm = verified.all_wasm()?;
            Ok((verified, wasm))
        })
        .await??;

        // Otherwise the application row is written with no bytecode and the
        // bundle only reads as malformed at its first execution.
        if wasm.is_empty() {
            bail!("bundle manifest declares no wasm artifact: expected a top-level 'wasm' or a non-empty 'services' list");
        }

        let manifest = verified.manifest();
        let package = &manifest.package;
        let version = &manifest.app_version;

        let mut services = Vec::new();
        let mut max_state_version = 0;
        for artifact in &wasm {
            // Before the name guard: an unnamed single-service artifact still
            // carries a state version.
            if let Some(schema) =
                calimero_wasm_abi::embed::read_embedded_state_schema(&artifact.bytes)
            {
                max_state_version = max_state_version.max(schema.state_version_or_default());
            }
            let Some(name) = &artifact.name else { continue };
            let (svc_blob_id, _svc_size) = self
                .add_blob(
                    Cursor::new(&artifact.bytes[..]),
                    Some(artifact.bytes.len() as u64),
                    None,
                )
                .await?;
            services.push(types::ServiceMeta {
                name: name.as_str().into(),
                bytecode: key::BlobMeta::new(svc_blob_id),
                compiled: key::BlobMeta::new(BlobId::from([0; 32])),
            });
        }

        self.install_bundle_application(
            blob_id,
            stored_size,
            source,
            manifest.to_metadata_json()?,
            types::PackageInfo {
                package: package.as_str().into(),
                version: version.as_str().into(),
                signer_id: verified.signer_id().into(),
                state_version: max_state_version,
            },
            services,
        )
    }

    pub async fn install_application_from_path(
        &self,
        path: Utf8PathBuf,
        metadata: Vec<u8>,
        package: Option<String>,
        version: Option<String>,
    ) -> eyre::Result<ApplicationId> {
        let metadata_len = metadata.len();
        debug!(
            path = %path,
            metadata_len,
            "install_application_from_path started"
        );

        let path = match path.canonicalize_utf8() {
            Ok(canonicalized) => canonicalized,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                bail!("application file not found at {}", path);
            }
            Err(err) => return Err(err.into()),
        };
        trace!(path = %path, "application path canonicalized");

        // Detect bundle vs single WASM
        if bundle::is_bundle_archive(&path) {
            return self.install_bundle_from_path(path, metadata).await;
        }

        // For non-bundle installations, use provided package/version or defaults
        let package = package.as_deref().unwrap_or("unknown");
        let version = version.as_deref().unwrap_or("0.0.0");

        // Existing single WASM installation path
        let file = match File::open(&path).await {
            Ok(file) => file,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                bail!("application file not found at {}", path);
            }
            Err(err) => return Err(err.into()),
        };
        trace!(path = %path, "application file opened");

        let expected_size = file.metadata().await?.len();
        debug!(
            path = %path,
            expected_size,
            "install_application_from_path discovered file size"
        );

        let (blob_id, size) = self
            .add_blob(file.compat(), Some(expected_size), None)
            .await?;
        debug!(
            %blob_id,
            expected_size,
            stored_size = size,
            "application blob added via add_blob"
        );

        let Ok(uri) = Url::from_file_path(path) else {
            bail!("non-absolute path")
        };

        self.install_raw_wasm(
            &blob_id,
            size,
            &uri.as_str().parse()?,
            metadata,
            package,
            version,
        )
    }

    pub async fn install_application_from_url(
        &self,
        url: Url,
        metadata: Vec<u8>,
        expected_content_hash: Option<&ContentHash>,
    ) -> eyre::Result<ApplicationId> {
        let uri = url.as_str().parse()?;

        // SSRF guard: refuse non-http(s) schemes and literal non-public hosts
        // before connecting; the client also re-checks every redirect hop.
        if !matches!(url.scheme(), "http" | "https") {
            bail!(
                "unsupported URL scheme '{}'; only http and https are allowed",
                url.scheme()
            );
        }
        if host_is_blocked(&url) {
            bail!("refusing to fetch from a non-public address: {}", url);
        }

        let response = ssrf_guarded_client()?.get(url.clone()).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<unreadable body>".to_owned());
            // Truncate to avoid leaking sensitive data that may appear in error responses.
            let truncated: String = body.chars().take(MAX_ERROR_BODY_LEN).collect();
            eyre::bail!(
                "Registry returned HTTP {} for {}: {}",
                status,
                url,
                truncated
            );
        }

        let expected_size = response.content_length();

        // Reject up front when the server declares a body larger than we allow.
        if let Some(len) = expected_size {
            if len > MAX_INSTALL_BYTES {
                bail!(
                    "application artifact too large: {len} bytes exceeds limit of {MAX_INSTALL_BYTES}"
                );
            }
        }

        // Check if URL indicates a bundle archive (.mpk - Mero Package Kit)
        let is_bundle = url.path().ends_with(".mpk");

        if is_bundle {
            // Read with a hard cap so a missing/lying Content-Length can't grow
            // the buffer without bound.
            let bundle_data: Arc<[u8]> =
                read_body_capped(response, MAX_INSTALL_BYTES).await?.into();

            let cursor = Cursor::new(&bundle_data[..]);
            let (bundle_blob_id, stored_size) = self
                .add_blob(
                    cursor,
                    Some(bundle_data.len() as u64),
                    expected_content_hash,
                )
                .await?;

            debug!(
                %bundle_blob_id,
                bundle_size = bundle_data.len(),
                stored_size,
                "bundle downloaded and stored as blob"
            );

            return self
                .install_bundle(bundle_data, &bundle_blob_id, stored_size, &uri)
                .await;
        }

        // Single WASM installation (existing behavior)
        // For non-bundle installations, use defaults (package/version are not part of ApplicationId)
        let package = "unknown";
        let version = "0.0.0";

        let (blob_id, size) = self
            .add_blob(
                response
                    .bytes_stream()
                    .map_err(io::Error::other)
                    .into_async_read(),
                expected_size,
                expected_content_hash,
            )
            .await?;

        self.install_raw_wasm(&blob_id, size, &uri, metadata, package, version)
    }

    /// Install a bundle archive (.mpk - Mero Package Kit) containing WASM, ABI, and migrations
    /// Note: metadata parameter is ignored for bundles - metadata is always extracted from manifest
    async fn install_bundle_from_path(
        &self,
        path: Utf8PathBuf,
        _metadata: Vec<u8>,
    ) -> eyre::Result<ApplicationId> {
        debug!(path = %path, "install_bundle_from_path started");

        let bundle_data: Arc<[u8]> = tokio::fs::read(&path).await?.into();

        let cursor = Cursor::new(&bundle_data[..]);
        let (bundle_blob_id, stored_size) = self
            .add_blob(cursor, Some(bundle_data.len() as u64), None)
            .await?;

        debug!(
            %bundle_blob_id,
            bundle_size = bundle_data.len(),
            stored_size,
            "bundle stored as blob"
        );

        let Ok(uri) = Url::from_file_path(&path) else {
            bail!("non-absolute path")
        };

        self.install_bundle(
            bundle_data,
            &bundle_blob_id,
            stored_size,
            &uri.as_str().parse()?,
        )
        .await
    }

    // Bundle verification, manifest extraction, and path validation functions
    // are in the `bundle` submodule. The methods below delegate for backward
    // compatibility with external callers that use `NodeClient::method()`.

    /// Check if a blob contains a bundle archive.
    /// Delegates to [`bundle::is_bundle_blob`].
    pub fn is_bundle_blob(blob_bytes: &[u8]) -> bool {
        bundle::is_bundle_blob(blob_bytes)
    }

    /// Install an application from a bundle blob that's already in the blobstore.
    /// This is used when a bundle blob is received via blob sharing or discovery.
    pub async fn install_application_from_bundle_blob(
        &self,
        blob_id: &BlobId,
        source: &ApplicationSource,
    ) -> eyre::Result<ApplicationId> {
        debug!(%blob_id, "install_application_from_bundle_blob started");

        let Some(bundle_bytes) = self.get_blob_bytes(blob_id, None).await? else {
            bail!("bundle blob not found");
        };

        let stored_size = bundle_bytes.len() as u64;

        self.install_bundle(bundle_bytes, blob_id, stored_size, source)
            .await
    }

    pub fn uninstall_application(&self, application_id: &ApplicationId) -> eyre::Result<()> {
        let mut handle = self.datastore.handle();
        handle.delete(&key::ApplicationMeta::new(*application_id))?;
        Ok(())
    }

    // Query and management functions (list_applications, list_packages,
    // list_versions, get_latest_version, update_compiled_app) are in the
    // `query` submodule.

    /// Install application by package and version
    pub async fn install_by_package_version(
        &self,
        _package: &str,
        _version: &str,
        source: &ApplicationSource,
        metadata: Vec<u8>,
    ) -> eyre::Result<ApplicationId> {
        // For now, we'll use the source URL to download the application
        // In a real implementation, you might want to resolve the package/version to a URL
        let url = source.to_string().parse()?;
        self.install_application_from_url(url, metadata, None).await
    }
}

#[cfg(test)]
mod ssrf_tests {
    use reqwest::Url;

    use super::host_is_blocked;

    #[test]
    fn blocks_non_public_hosts() {
        for bad in [
            "http://169.254.169.254/latest/meta-data/",
            "http://127.0.0.1/",
            "http://localhost/",
            "http://10.0.0.1/",
            "http://192.168.0.1/",
            "http://[::1]/",
            "http://[fc00::1]/",
            "http://[::ffff:127.0.0.1]/",
        ] {
            assert!(
                host_is_blocked(&Url::parse(bad).unwrap()),
                "{bad} must be blocked",
            );
        }
    }

    #[test]
    fn allows_public_hosts() {
        for ok in ["https://registry.example.com/", "http://93.184.216.34/"] {
            assert!(
                !host_is_blocked(&Url::parse(ok).unwrap()),
                "{ok} must be allowed",
            );
        }
    }
}
