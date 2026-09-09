//! Bundle verification, manifest extraction, and path validation.
//!
//! Pure functions for working with `.mpk` bundle archives - no
//! `NodeClient` or blob manager state needed.

use std::collections::{BTreeSet, HashMap};
use std::io::{self, Read};
use std::path::{Component, Path};
use std::sync::Arc;

use crate::bundle::{
    verify_manifest_signature, BundleArtifact, BundleManifest, ManifestVerification,
    MAX_MANIFEST_BYTES,
};
use eyre::bail;
use flate2::read::GzDecoder;
use semver::Version;
use sha2::{Digest, Sha256};
use tar::{Archive, Entry};
use tracing::{debug, warn};

/// How far the manifest scan may decompress. It runs before any signature is
/// checked, and the manifest must appear within it or the archive is refused.
const MAX_MANIFEST_SCAN_BYTES: u64 = 8 * 1024 * 1024;

/// How far any one archive walk may decompress. Every read walks the whole
/// archive, so this bounds a bundle's total decompressed size.
pub const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;

/// Bounds total decompressed bytes. `tar` buffers GNU long-name, long-link and
/// pax members with an unbounded read inside `entries()`, before any `Entry` is
/// handed out, so a per-entry limit is unreachable and this has to sit lower.
struct Capped<R> {
    inner: R,
    limit: u64,
    used: u64,
}

impl<R: Read> Read for Capped<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // One byte of slack, so a stream of exactly `limit` bytes still reaches
        // EOF instead of tripping the cap on its final empty read.
        let allowed = self.limit.saturating_add(1).saturating_sub(self.used);
        if allowed == 0 {
            return Err(io::Error::other(format!(
                "bundle archive exceeds the {} byte decompressed cap",
                self.limit
            )));
        }
        let len = buf
            .len()
            .min(usize::try_from(allowed).unwrap_or(usize::MAX));
        let read = self.inner.read(&mut buf[..len])?;
        self.used += read as u64;
        Ok(read)
    }
}

/// A `.mpk`'s tar stream, bounded to `limit` decompressed bytes.
fn bounded_archive(data: &[u8], limit: u64) -> Archive<Capped<GzDecoder<&[u8]>>> {
    Archive::new(Capped {
        inner: GzDecoder::new(data),
        limit,
        used: 0,
    })
}

/// Whether two archive-relative paths name the same entry. A leading `./` is the
/// spelling ordinary tar producers emit and means nothing, but `old/` is a real
/// component, so this is not basename matching: a nested manifest stays distinct.
fn same_archive_path(a: &Path, b: &Path) -> bool {
    fn parts(path: &Path) -> impl Iterator<Item = Component<'_>> {
        path.components()
            .skip_while(|c| matches!(c, Component::CurDir))
    }
    parts(a).eq(parts(b))
}

/// One rule for every archive scan, so they cannot be edited apart. Exact and
/// top-level: a nested `old/manifest.json` can carry an authentic older release.
/// It takes the entry, not a path, so that resolving the path is part of the
/// shared rule: an unresolvable one is a non-match here and must never abort a
/// scan, since [`is_bundle_blob`] can only fail by falling back to raw bytes.
fn is_manifest_entry<R: Read>(entry: &Entry<'_, R>) -> bool {
    entry
        .path()
        .is_ok_and(|path| same_archive_path(&path, Path::new("manifest.json")))
}

/// A second `manifest.json` leaves an unpacker holding one the signature never
/// covered, so the archive is refused rather than resolved by entry order.
fn ensure_single_manifest(bundle_data: &[u8]) -> eyre::Result<()> {
    let mut archive = bounded_archive(bundle_data, MAX_ARCHIVE_BYTES);
    let mut seen = false;
    for entry in archive.entries()? {
        let entry = entry?;
        if !is_manifest_entry(&entry) {
            continue;
        }
        if seen {
            bail!("bundle archive has more than one entry at 'manifest.json'");
        }
        seen = true;
    }
    Ok(())
}

/// Validates that a string is safe for use as a filesystem path component.
///
/// This prevents path traversal attacks where malicious bundle manifests could
/// write files outside the intended `applications` directory.
pub fn validate_path_component(value: &str, field_name: &str) -> eyre::Result<()> {
    if value.contains("..") {
        bail!("{} contains path traversal sequence '..'", field_name);
    }
    if value.contains('/') || value.contains('\\') {
        bail!("{} contains directory separator", field_name);
    }
    if value.contains('\0') {
        bail!("{} contains null byte", field_name);
    }
    if value.len() >= 2 && value.as_bytes().get(1) == Some(&b':') {
        bail!("{} appears to be an absolute path", field_name);
    }
    Ok(())
}

/// Validates that an artifact path is safe for use as a relative filesystem path.
///
/// Unlike `validate_path_component`, this allows subdirectories (forward slashes)
/// but still prevents path traversal attacks.
pub fn validate_artifact_path(value: &str, field_name: &str) -> eyre::Result<()> {
    if value.is_empty() {
        bail!("{} is empty", field_name);
    }
    if value.contains('\0') {
        bail!("{} contains null byte", field_name);
    }
    if value.contains('\\') {
        bail!("{} contains backslash (use forward slashes)", field_name);
    }
    if value.starts_with('/') {
        bail!("{} is an absolute path", field_name);
    }
    if value.as_bytes().get(1) == Some(&b':') {
        bail!("{} appears to be an absolute Windows path", field_name);
    }
    if value.split('/').any(|c| c == "..") {
        bail!("{} contains path traversal component '..'", field_name);
    }
    Ok(())
}

/// Extract and parse bundle manifest from bundle archive data.
/// Returns both the raw JSON value (for signature verification) and the typed manifest.
pub fn extract_bundle_manifest(
    bundle_data: &[u8],
) -> eyre::Result<(serde_json::Value, BundleManifest)> {
    let mut archive = bounded_archive(bundle_data, MAX_MANIFEST_SCAN_BYTES);

    for entry in archive.entries()? {
        let entry = entry?;

        if is_manifest_entry(&entry) {
            let mut manifest_bytes = Vec::new();
            let _ = entry
                .take(MAX_MANIFEST_BYTES + 1)
                .read_to_end(&mut manifest_bytes)?;
            if manifest_bytes.len() as u64 > MAX_MANIFEST_BYTES {
                bail!("bundle manifest.json exceeds the {MAX_MANIFEST_BYTES} byte cap");
            }

            let manifest_json: serde_json::Value = serde_json::from_slice(&manifest_bytes)
                .map_err(|e| eyre::eyre!("failed to parse manifest.json as JSON: {}", e))?;

            let manifest: BundleManifest = serde_json::from_value(manifest_json.clone())
                .map_err(|e| eyre::eyre!("failed to parse manifest.json: {}", e))?;

            if manifest.package.is_empty() {
                bail!("bundle manifest 'package' field is empty");
            }
            if manifest.app_version.is_empty() {
                bail!("bundle manifest 'appVersion' field is empty");
            }

            validate_path_component(&manifest.package, "package")?;
            validate_path_component(&manifest.app_version, "appVersion")?;

            for (field, artifact) in manifest.artifacts() {
                validate_artifact_path(&artifact.path, &format!("{field}.path"))?;
            }

            let current_runtime_version = Version::parse(env!("CALIMERO_RELEASE_VERSION"))
                .map_err(|e| eyre::eyre!("failed to parse current runtime version: {}", e))?;
            let min_runtime_version =
                Version::parse(&manifest.min_runtime_version).map_err(|e| {
                    eyre::eyre!(
                        "invalid minRuntimeVersion '{}': {}",
                        manifest.min_runtime_version,
                        e
                    )
                })?;

            if min_runtime_version > current_runtime_version {
                bail!(
                    "bundle requires runtime version {} but current runtime is {}",
                    min_runtime_version,
                    current_runtime_version
                );
            }

            return Ok((manifest_json, manifest));
        }
    }

    bail!("manifest.json not found in bundle")
}

/// Whether a blob is a bundle archive: a tar entry at `manifest.json`.
///
/// Same walk, same bound and the same [`is_manifest_entry`] predicate as
/// [`extract_bundle_manifest`], since this is the only signature gate on the
/// execution-time read: where the two disagree, one path installs a verified
/// bundle the other serves as raw bytes. The one asymmetry left is an entry that
/// will not decode at all, which ends this walk as `false` where the verified
/// read raises it; neither reaches a manifest sitting behind it.
/// [`VerifiedBundle::open`] is stricter still, refusing a duplicated manifest;
/// stricter here would instead mean serving the archive as raw bytes.
pub fn is_bundle_blob(blob_bytes: &[u8]) -> bool {
    let mut archive = bounded_archive(blob_bytes, MAX_MANIFEST_SCAN_BYTES);

    let entries = match archive.entries() {
        Ok(entries) => entries,
        Err(e) => {
            warn!(
                "Failed to read tar archive entries (possible corruption): {}",
                e
            );
            return false;
        }
    };

    for (i, entry) in entries.enumerate() {
        match entry {
            Ok(entry) => {
                // Existential: one hit is decisive, so stopping here cannot
                // change the answer the way the old entry-count cap could.
                if is_manifest_entry(&entry) {
                    return true;
                }
            }
            // A corrupt entry stops this walk and `extract_bundle_manifest`
            // alike, so neither path reaches a manifest hidden behind it.
            Err(e) => {
                warn!("Failed to read tar entry {}: {}", i, e);
                break;
            }
        }
    }

    false
}

/// Read the `wanted` paths out of a bundle archive in memory (no extraction to
/// disk), in one walk. Paths are manifest-relative and matched whole against the
/// archive under the same [`same_archive_path`] rule the manifest scan uses: a
/// basename or first-hit match would let a decoy entry pass the digest check for
/// the entry an unpacker later writes. Paths the archive does not hold are simply
/// absent from the result. A path that will not decode is a non-match rather than
/// an abort, as everywhere else.
///
/// One walk for every path, not one per path: skipping an entry still
/// decompresses it, so a walk per artifact would multiply the archive's whole
/// decompressed size by the number of artifacts.
fn extract_bundle_files<'a>(
    bundle_data: &[u8],
    wanted: &BTreeSet<&'a str>,
) -> eyre::Result<HashMap<&'a str, Arc<[u8]>>> {
    let mut archive = bounded_archive(bundle_data, MAX_ARCHIVE_BYTES);
    let mut found = HashMap::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let Ok(entry_path) = entry.path() else {
            continue;
        };
        // Every match, not the first: a manifest may spell one path two ways,
        // and each declaration carries its own digest.
        let matched = wanted
            .iter()
            .copied()
            .filter(|w| same_archive_path(Path::new(w), &entry_path))
            .collect::<Vec<_>>();
        if matched.is_empty() {
            continue;
        }
        let mut bytes = Vec::new();
        let _ = entry.read_to_end(&mut bytes)?;
        let bytes: Arc<[u8]> = Arc::from(bytes);
        for path in matched {
            if found.insert(path, Arc::clone(&bytes)).is_some() {
                bail!("bundle archive has more than one entry at '{path}'");
            }
        }
    }
    Ok(found)
}

/// One artifact out of a completed walk. Several services may name one path, so
/// the bytes are shared rather than copied per service.
fn take_artifact(found: &HashMap<&str, Arc<[u8]>>, path: &str) -> eyre::Result<Arc<[u8]>> {
    found.get(path).map(Arc::clone).ok_or_else(|| {
        eyre::eyre!("bundle artifact '{path}' named in the manifest is missing from the archive")
    })
}

/// One digest-checked wasm artifact out of a [`VerifiedBundle`], the owned
/// counterpart to the manifest's [`crate::bundle::WasmArtifact`].
pub struct VerifiedWasm {
    /// Service name. `None` for single-service bundles.
    pub name: Option<String>,
    pub bytes: Arc<[u8]>,
}

/// A bundle whose manifest signature has been verified. The only way to obtain
/// artifact bytes: every accessor checks the artifact digest against the signed
/// manifest first, so unverified bytes have no representation.
pub struct VerifiedBundle {
    data: Arc<[u8]>,
    manifest: BundleManifest,
    signer_id: String,
}

impl VerifiedBundle {
    /// Signature is mandatory. There is no unsigned constructor.
    pub fn open(data: Arc<[u8]>) -> eyre::Result<Self> {
        let (manifest_json, manifest) = extract_bundle_manifest(&data)?;
        let ManifestVerification { signer_id, .. } = verify_manifest_signature(&manifest_json)?;
        // After the signature, not during the scan: reaching the end of a large
        // archive needs the archive cap, which the pre-auth scan must not spend.
        ensure_single_manifest(&data)?;
        debug!(%signer_id, package = %manifest.package, "bundle manifest signature verified");
        Ok(Self {
            data,
            manifest,
            signer_id,
        })
    }

    pub fn manifest(&self) -> &BundleManifest {
        &self.manifest
    }

    pub fn signer_id(&self) -> &str {
        &self.signer_id
    }

    /// Wasm bytes for a service (`None` selects a single-service bundle's
    /// top-level `wasm`), checked against the manifest digest.
    pub fn wasm(&self, service: Option<&str>) -> eyre::Result<Arc<[u8]>> {
        let artifact = self.wasm_artifact(service)?;
        let path = artifact.path.as_str();
        let found = extract_bundle_files(&self.data, &BTreeSet::from([path]))?;
        let bytes = take_artifact(&found, path)?;
        verify_artifact_digest(artifact, &bytes)?;
        Ok(bytes)
    }

    /// Every declared wasm artifact, digest-checked, paired with its service
    /// name. One walk covers every path, and a path named by several services
    /// is read once and shared.
    pub fn all_wasm(&self) -> eyre::Result<Vec<VerifiedWasm>> {
        let artifacts = self.manifest.wasm_artifacts();
        let wanted = artifacts
            .iter()
            .map(|a| a.wasm.path.as_str())
            .collect::<BTreeSet<_>>();
        let found = extract_bundle_files(&self.data, &wanted)?;

        let mut all = Vec::new();
        for artifact in &artifacts {
            let bytes = take_artifact(&found, &artifact.wasm.path)?;
            // Per artifact, never per read: two services may declare different
            // digests for one path, and sharing the bytes must not share the check.
            verify_artifact_digest(artifact.wasm, &bytes)?;
            all.push(VerifiedWasm {
                name: artifact.name.map(str::to_owned),
                bytes,
            });
        }
        Ok(all)
    }

    fn wasm_artifact(&self, service: Option<&str>) -> eyre::Result<&BundleArtifact> {
        match service {
            Some(name) => self
                .manifest
                .wasm_artifacts()
                .into_iter()
                .find(|a| a.name == Some(name))
                .map(|a| a.wasm)
                .ok_or_else(|| eyre::eyre!("service '{}' not found in bundle manifest", name)),
            None => self
                .manifest
                .wasm
                .as_ref()
                .ok_or_else(|| eyre::eyre!("bundle manifest declares no top-level wasm")),
        }
    }
}

/// Compare an artifact's bytes against the digest the publisher signed. Hex case
/// carries no information, so a case-differing manifest is still a match.
fn verify_artifact_digest(artifact: &BundleArtifact, bytes: &[u8]) -> eyre::Result<()> {
    let actual = hex::encode(Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(&artifact.hash) {
        bail!(
            "bundle artifact '{}' failed its integrity check: manifest hash {}, actual {}",
            artifact.path,
            artifact.hash,
            actual
        );
    }
    Ok(())
}
