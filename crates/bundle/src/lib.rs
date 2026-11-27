//! A signed app package (`.mpk`): the manifest and its schema, RFC 8785 (JCS)
//! canonicalization, Ed25519 signatures and the `did:key` signer id, the archive and its
//! verified contents, frontends and declared secrets. Every producer (`meroctl app sign`,
//! `meroctl-sign`) and consumer (the node's install path) of a package reads it through here.
//!
//! Spec: [What a release carries](https://calimero-network.github.io/core/protocol/applications/#what-a-release-carries),
//! [Packaging & signing](https://calimero-network.github.io/core/build/packaging-signing/).
//! Crate guide: [`crates/bundle/AGENTS.md`](https://github.com/calimero-network/core/blob/master/crates/bundle/AGENTS.md).

pub mod archive;
mod frontend;
#[cfg(feature = "html")]
mod html;
pub mod secrets;
mod signature;
pub mod update;

pub use archive::{
    extract_bundle_manifest, is_bundle_blob, validate_artifact_path, validate_path_component,
    VerifiedBundle, VerifiedContents, VerifiedWasm, MAX_ARCHIVE_BYTES,
};
pub use frontend::{
    mime_type, verify_frontend_file, BundleFrontend, FrontendError, FRONTEND_ROOT,
    MAX_FRONTEND_BYTES,
};
#[cfg(feature = "html")]
pub use html::check_frontend_html;
pub use secrets::{
    is_forbidden_header, is_secret_body_field, is_secret_format, is_secret_header, is_secret_name,
    is_secret_path, path_covers, BundleSecret, SecretPlacement, SecretsError, FORBIDDEN_HEADERS,
    MAX_SECRETS, MAX_SECRET_DESCRIPTION_BYTES, MAX_SECRET_FIELD_BYTES, MAX_SECRET_FORMAT_BYTES,
    MAX_SECRET_NAME_BYTES, MAX_SECRET_PATHS, MAX_SECRET_PATH_BYTES, SECRET_FORMAT_SLOT,
};
pub use signature::{
    canonicalize_manifest, compute_bundle_hash, compute_signing_payload, decode_public_key,
    decode_signature, derive_signer_id_did_key, format_bundle_hash, sign_manifest_json,
    verify_ed25519, verify_manifest_signature, ManifestVerification, DEV_SIGNER_ID,
};

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The node refuses a bundle's `manifest.json` above this size, before any
/// signature check.
pub const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

/// A package declares at most this many pipelines, across all its services.
pub const MAX_PIPELINES_PER_PACKAGE: usize = 16;

/// A pipeline file is at most this large, the pipeline compiler's own source cap.
pub const MAX_PIPELINE_BYTES: u64 = 64 * 1024;

/// `hash` binds the artifact bytes to the manifest signature, so a manifest that omits it is
/// malformed rather than merely unhashed.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleArtifact {
    pub path: String,
    pub hash: String,
    pub size: u64,
}

/// Display metadata for the application
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleMetadata {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Primary storefront category from the registry's closed vocabulary. Optional and not
    /// `deny_unknown_fields`, so older bundles still read and older nodes accept newer ones.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// The app's Markdown guide for agents, the full text of the file its
    /// `guide` metadata key names. The registry, not this crate, owns its format.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guide: Option<String>,
}

/// Declarative interfaces (intents) implemented or required by the application
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleInterfaces {
    #[serde(default)]
    pub exports: Vec<String>,
    #[serde(default)]
    pub uses: Vec<String>,
}

/// External links for the application
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleLinks {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frontend: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
}

/// An absolute http(s) URL with a host and no control characters: what every
/// `links.*` value and `frontendAliases` entry must be before a node stores it.
pub fn is_web_url(url: &str) -> bool {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    let rest = if lower.starts_with("https://") {
        &url[8..]
    } else if lower.starts_with("http://") {
        &url[7..]
    } else {
        return false;
    };
    !rest.is_empty() && !url.chars().any(char::is_control)
}

/// Whether `url` may be declared under `outbound`: `https://`, a host, no credentials, query, fragment,
/// whitespace or control characters, at most 2048 bytes. A prefix: covers every URL below it at a `/`.
#[must_use]
pub fn is_outbound_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    url.len() <= 2048
        && !authority.is_empty()
        && !authority.contains('@')
        && !url.contains(['?', '#'])
        && !url.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// The host of an `outbound` prefix (`https://api.example.com/v1/` -> `api.example.com`),
/// lowercase; `None` for a prefix [`is_outbound_url`] refuses or one with a port.
#[must_use]
pub fn outbound_host(prefix: &str) -> Option<String> {
    if !is_outbound_url(prefix) {
        return None;
    }
    let rest = prefix.strip_prefix("https://")?;
    let authority = rest.split('/').next()?;
    if authority.contains(':') {
        return None;
    }
    Some(authority.to_ascii_lowercase())
}

/// Most apps one release references.
pub const MAX_USES: usize = 16;
/// Most bytes of a reference's name.
pub const MAX_USE_NAME_BYTES: usize = 32;
/// Most bytes of a reference's package or range.
pub const MAX_USE_FIELD_BYTES: usize = 128;
/// Most methods of one referenced app a release calls as the person its run serves.
pub const MAX_USE_METHODS: usize = 64;
/// Most bytes of one of those methods' names.
pub const MAX_USE_METHOD_BYTES: usize = 64;

/// How a release holds an app it references (`BundleManifest::uses`).
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum UseMode {
    /// Created with the referencing app, in its organization, and upgraded with it in one
    /// consent: the two are one unit.
    #[default]
    Create,
    /// An app the deployer already runs: linked, never created or upgraded by the
    /// referencing app; a version outside the range is a warning, not an upgrade.
    Attach,
}

/// One app a release references (`BundleManifest::uses`).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BundleUse {
    /// The referenced package, e.g. `com.calimero.confidential-kv`.
    pub package: String,
    /// The semver requirement its releases must meet (`^0.1`).
    pub range: String,
    #[serde(default)]
    pub mode: UseMode,
    /// The release the app was built against (`calimero.lock`), inside `range`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// That release's ABI, as the app vendored it: `sha256:<64 hex>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abi: Option<String>,
    /// The referenced app's methods this release calls as the person its run serves, once they grant it
    /// (`SystemCmd::SetDelegation`). Empty: it calls as itself only. Shown on the grant's consent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub methods: Vec<String>,
}

/// Whether `name` may name a method a release calls as the person: an ABI method name
/// (`A-Za-z0-9_`, no leading digit, at most [`MAX_USE_METHOD_BYTES`]), never a node-own `__` one.
#[must_use]
pub fn is_use_method(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_USE_METHOD_BYTES
        && !name.starts_with("__")
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Whether `name` may name a reference: `a-z`, `0-9` and `_`, starting with a letter, at
/// most [`MAX_USE_NAME_BYTES`]; the name the app's code knows it by.
#[must_use]
pub fn is_use_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_USE_NAME_BYTES
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Why a manifest's `uses` are refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum UsesError {
    #[error("a release references at most {MAX_USES} apps")]
    TooMany,
    #[error("reference name {0:?} must be a-z, 0-9 and _, starting with a letter, at most {MAX_USE_NAME_BYTES} bytes")]
    BadName(String),
    #[error("reference {0} names no package, or one longer than {MAX_USE_FIELD_BYTES} bytes")]
    BadPackage(String),
    #[error("reference {0} references the release's own package")]
    SelfReference(String),
    #[error("reference {name}'s range {range:?} is not a semver requirement")]
    BadRange { name: String, range: String },
    #[error("reference {name}'s version {version:?} is not a version inside {range}")]
    VersionOutsideRange {
        name: String,
        version: String,
        range: String,
    },
    #[error("reference {0}'s abi must be sha256: and 64 lowercase hex digits")]
    BadAbi(String),
    #[error("reference {0}'s methods must be at most {MAX_USE_METHODS} distinct method names, none starting with __")]
    BadMethods(String),
}

impl UsesError {
    /// The stable code a node answers with.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        "BAD_USES"
    }
}

impl BundleManifest {
    /// Checks `secrets` against the node's install rules ([`secrets`]): each a credential bound
    /// to one host, the host of an `outbound` prefix, and path prefixes there each under an
    /// `outbound` prefix (so a secret goes only where the release already says data goes),
    /// and one place, a header or a body field, no two in the same place.
    pub fn check_secrets(&self) -> Result<(), SecretsError> {
        let Some(secrets) = &self.secrets else {
            return Ok(());
        };
        let hosts: std::collections::BTreeSet<String> = self
            .outbound
            .iter()
            .filter_map(|p| outbound_host(p))
            .collect();
        secrets::check(secrets, &self.outbound, &hosts)
    }

    /// Checks `uses` against the node's install rules: names, count, own-package ban, semver range,
    /// locked version inside it and ABI digest shape.
    pub fn check_uses(&self) -> Result<(), UsesError> {
        let Some(uses) = &self.uses else {
            return Ok(());
        };
        if uses.len() > MAX_USES {
            return Err(UsesError::TooMany);
        }
        for (name, u) in uses {
            if !is_use_name(name) {
                return Err(UsesError::BadName(name.clone()));
            }
            if u.package.is_empty()
                || u.package.len() > MAX_USE_FIELD_BYTES
                || u.package
                    .chars()
                    .any(|c| c.is_control() || c.is_whitespace())
            {
                return Err(UsesError::BadPackage(name.clone()));
            }
            if u.package == self.package {
                return Err(UsesError::SelfReference(name.clone()));
            }
            let bad_range = || UsesError::BadRange {
                name: name.clone(),
                range: u.range.clone(),
            };
            if u.range.len() > MAX_USE_FIELD_BYTES {
                return Err(bad_range());
            }
            let range = semver::VersionReq::parse(&u.range).map_err(|_| bad_range())?;
            if let Some(version) = &u.version {
                let inside = semver::Version::parse(version).is_ok_and(|v| range.matches(&v));
                if !inside {
                    return Err(UsesError::VersionOutsideRange {
                        name: name.clone(),
                        version: version.clone(),
                        range: u.range.clone(),
                    });
                }
            }
            if let Some(abi) = &u.abi {
                let hex = abi.strip_prefix("sha256:").unwrap_or_default();
                if hex.len() != 64 || !hex.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
                    return Err(UsesError::BadAbi(name.clone()));
                }
            }
            let distinct: std::collections::BTreeSet<&String> = u.methods.iter().collect();
            if u.methods.len() > MAX_USE_METHODS
                || distinct.len() != u.methods.len()
                || !u.methods.iter().all(|m| is_use_method(m))
            {
                return Err(UsesError::BadMethods(name.clone()));
            }
        }
        Ok(())
    }
}

/// Deep-link handler declarations (`slug`; absent: the desktop kebab-cases the app name).
/// Sibling of `metadata`, so outside the raw-wasm application-id derivation.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleHandlers {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
}

/// How `calimero-sdk` resolved at build time, read off the resolve graph rather than declared.
/// Sibling of `metadata` so it moves no app id; every field is optional so older bundles and nodes are unaffected.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleBuildInfo {
    /// Where the dependency resolved from: `"git"`, `"registry"`, or `"path"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk_source: Option<String>,

    /// Node version built against: the git `tag=` or the registry crate version. Never a git dependency's
    /// own crate version (the `0.0.0` placeholder); a branch or rev names none, so this stays `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk_version: Option<String>,

    /// The exact git commit the dependency resolved to. `None` for a registry
    /// or path dependency, which have no commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk_rev: Option<String>,
}

/// Ed25519 over the domain-tagged SHA-256 of the canonical manifest (RFC 8785, `signature` excluded).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleSignature {
    /// Signature algorithm identifier. MUST be "ed25519" in v0.
    pub algorithm: String,
    /// Ed25519 public key encoded as base64url (no padding).
    pub public_key: String,
    /// Signature over canonical manifest encoded as base64url (no padding).
    pub signature: String,
    /// Optional ISO 8601 timestamp of signing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signed_at: Option<String>,
}

/// A named service within a multi-service bundle.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleService {
    pub name: String,
    pub wasm: BundleArtifact,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abi: Option<BundleArtifact>,
    /// The service's compute component, the one job nodes run; absent when the
    /// service declares no compute methods. An older node ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compute: Option<BundleArtifact>,
    /// The service's pipelines: YAML files over its compute methods. An older node ignores them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pipelines: Vec<BundleArtifact>,
}

/// Who upgrades a package's instances, as its signed manifest declares; it may only loosen toward owners:
/// once a release declares `owner`, no later version may declare `publisher` (a node refuses it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UpgradeAuthority {
    /// Each instance's owner, or an organization's `MANAGE_APPLICATION` holders there. The default.
    #[default]
    Owner,
    /// The organization the signing key is bound to: its `MANAGE_APPLICATION` holders upgrade every
    /// instance. Until it binds the key (`BindPublisher`), nobody upgrades them.
    Publisher,
}

impl UpgradeAuthority {
    /// Whether this is [`Self::Owner`] (the default, left out when stored).
    #[must_use]
    pub fn is_owner(&self) -> bool {
        *self == Self::Owner
    }
}

/// Whether an instance takes guests (accounts outside its organization), as the manifest declares.
/// Absent means [`Self::Forbidden`]; an older node ignores the field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GuestPolicy {
    /// An instance's listed accounts reach it without joining its organization, where
    /// the organization allows guests too.
    Allowed,
    /// Only the organization's members reach an instance. The default.
    #[default]
    Forbidden,
}

impl GuestPolicy {
    /// The manifest's word for it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Forbidden => "forbidden",
        }
    }
}

/// Where an instance goes (`instanceOrg`): the organization it is created in. Absent means
/// [`Self::Either`]; an older node ignores the field (the signature is over the raw manifest).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InstanceOrg {
    /// Every instance comes with an organization of its own, made with it (its creator the owner);
    /// a request may not name an existing one.
    Standalone,
    /// An instance goes into an organization its creator is already in: the one
    /// named, or their only one. Never makes one.
    Org,
    /// The organization named, else the creator's only one, else (they are in none) a
    /// new one, as [`Self::Standalone`] makes. The default.
    #[default]
    Either,
}

impl InstanceOrg {
    /// The manifest's word for it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standalone => "standalone",
            Self::Org => "org",
            Self::Either => "either",
        }
    }
}

/// Where a package's instances may move (`lineage`), as its signed manifest declares. Absent means
/// [`Self::Free`]; an older node ignores the field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Lineage {
    /// An instance moves to any release its upgrader may name. The default.
    #[default]
    Free,
    /// An instance created from, or upgraded into, such a release stays on its package and
    /// signer, never moving to an older release; nothing unpins it.
    Pinned,
}

impl Lineage {
    /// Whether this is [`Self::Free`] (the default, left out when stored).
    #[must_use]
    pub fn is_free(&self) -> bool {
        *self == Self::Free
    }
}

/// Bundle manifest: single-service (`wasm` + optional `abi`, backward compat) or multi-service
/// (`services`); a non-empty `services` takes priority over `wasm`/`abi`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleManifest {
    pub version: String,
    pub package: String,
    pub app_version: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub signer_id: Option<String>,

    pub min_runtime_version: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BundleMetadata>,

    /// Deep-link handler declarations. Sibling of `metadata`, so it does not
    /// participate in the raw-wasm application-id derivation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handlers: Option<BundleHandlers>,

    /// Build provenance: which `calimero-sdk` the wasm was compiled against.
    /// Sibling of `metadata`, so it stays out of application-id derivation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_info: Option<BundleBuildInfo>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interfaces: Option<BundleInterfaces>,

    /// Single-service WASM (backward compat). Ignored when `services` is non-empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasm: Option<BundleArtifact>,
    /// Single-service ABI (backward compat). Ignored when `services` is non-empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abi: Option<BundleArtifact>,
    /// Single-service compute component (see [`BundleService::compute`]). Ignored when
    /// `services` is non-empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compute: Option<BundleArtifact>,
    /// Single-service pipelines (see [`BundleService::pipelines`]). Ignored when `services`
    /// is non-empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pipelines: Vec<BundleArtifact>,

    /// Named services. When present, each app instance names the service it runs.
    /// If empty/absent, the bundle is single-service and uses `wasm`/`abi` above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub services: Option<Vec<BundleService>>,

    /// The app's static web build, served by the node per version. An older node
    /// ignores it and the app still runs from `links.frontend`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frontend: Option<BundleFrontend>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<BundleLinks>,

    /// Other URLs the frontend is served at; never opened, but a node's sign-in page trusts a login callback to them.
    /// A sibling of `links`: the registry refuses any `links` value that is not one URL string.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frontend_aliases: Vec<String>,

    /// The `https://` URL prefixes the app's outbound requests may go under; a request no prefix covers is refused.
    /// Signed and shown at install; a node still sends nothing unless its operator allows the host.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outbound: Vec<String>,

    /// The credentials the app's requests carry, by name ([`BundleSecret`]): each bound to one host and one
    /// header or body field, where the node attaches it; the app never places or reads one. Absent: none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<BTreeMap<String, BundleSecret>>,
    /// The other apps this release calls, by the name its code knows each by ([`BundleUse`]); signed, shown
    /// before install. An older node ignores the field (the signature is over the raw manifest).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uses: Option<BTreeMap<String, BundleUse>>,

    /// Who upgrades the package's instances ([`UpgradeAuthority`]); absent means `Owner`. Signed with the rest;
    /// a value other than `owner` or `publisher` fails to parse, so the bundle is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upgrades: Option<UpgradeAuthority>,

    /// Which organization an instance is created in ([`InstanceOrg`]); absent means `either`. Signed with the
    /// rest; an unknown value fails to parse, so the bundle is refused. An older node ignores the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_org: Option<InstanceOrg>,

    /// Whether an instance takes guests ([`GuestPolicy`]); absent means `forbidden`. Signed with the rest;
    /// an unknown value fails to parse. An older node ignores the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guests: Option<GuestPolicy>,

    /// Where its instances may move ([`Lineage`]); absent means `free`. Signed with the rest;
    /// an unknown value fails to parse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lineage: Option<Lineage>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<BundleSignature>,
}

/// A WASM artifact within a bundle, abstracting over single-service and
/// multi-service layouts so callers never need to check both shapes.
pub struct WasmArtifact<'a> {
    /// Service name. `None` for single-service bundles.
    pub name: Option<&'a str>,
    /// The app component; `None` for a single-service bundle that ships only a compute component.
    pub wasm: Option<&'a BundleArtifact>,
    pub compute: Option<&'a BundleArtifact>,
    pub pipelines: &'a [BundleArtifact],
}

impl BundleManifest {
    /// Empty for single-service bundles.
    pub fn service_names(&self) -> Vec<&str> {
        match &self.services {
            Some(svcs) if !svcs.is_empty() => svcs.iter().map(|s| s.name.as_str()).collect(),
            _ => vec![],
        }
    }

    pub fn is_multi_service(&self) -> bool {
        matches!(&self.services, Some(svcs) if svcs.len() > 1)
    }

    /// One `WasmArtifact` per service, or one with `name: None` for a single-service bundle.
    pub fn wasm_artifacts(&self) -> Vec<WasmArtifact<'_>> {
        match &self.services {
            Some(svcs) if !svcs.is_empty() => svcs
                .iter()
                .map(|s| WasmArtifact {
                    name: Some(&s.name),
                    wasm: Some(&s.wasm),
                    compute: s.compute.as_ref(),
                    pipelines: &s.pipelines,
                })
                .collect(),
            _ if self.wasm.is_some() || self.compute.is_some() => vec![WasmArtifact {
                name: None,
                wasm: self.wasm.as_ref(),
                compute: self.compute.as_ref(),
                pipelines: &self.pipelines,
            }],
            _ => Vec::new(),
        }
    }

    /// Every artifact the manifest declares, paired with its manifest field. The
    /// `..`-free destructure stops compiling until a new field is classified.
    pub fn artifacts(&self) -> Vec<(String, &BundleArtifact)> {
        let Self {
            wasm,
            abi,
            compute,
            pipelines,
            services,
            frontend,
            version: _,
            package: _,
            app_version: _,
            signer_id: _,
            min_runtime_version: _,
            metadata: _,
            handlers: _,
            build_info: _,
            interfaces: _,
            links: _,
            frontend_aliases: _,
            outbound: _,
            secrets: _,
            uses: _,
            upgrades: _,
            instance_org: _,
            guests: _,
            lineage: _,
            signature: _,
        } = self;

        let mut artifacts = Vec::new();
        if let Some(wasm) = wasm {
            artifacts.push(("wasm".to_owned(), wasm));
        }
        if let Some(abi) = abi {
            artifacts.push(("abi".to_owned(), abi));
        }
        if let Some(compute) = compute {
            artifacts.push(("compute".to_owned(), compute));
        }
        for (i, pipeline) in pipelines.iter().enumerate() {
            artifacts.push((format!("pipelines[{i}]"), pipeline));
        }
        for (i, service) in services.iter().flatten().enumerate() {
            artifacts.push((format!("services[{i}].wasm"), &service.wasm));
            if let Some(abi) = &service.abi {
                artifacts.push((format!("services[{i}].abi"), abi));
            }
            if let Some(compute) = &service.compute {
                artifacts.push((format!("services[{i}].compute"), compute));
            }
            for (j, pipeline) in service.pipelines.iter().enumerate() {
                artifacts.push((format!("services[{i}].pipelines[{j}]"), pipeline));
            }
        }
        for (i, file) in frontend.iter().flat_map(|f| &f.files).enumerate() {
            artifacts.push((format!("frontend.files[{i}]"), file));
        }
        artifacts
    }

    /// The display metadata a node stores at install: `package`, `version`, `metadata.*`, `links.*` and
    /// `frontendAliases` as a flat JSON object.
    pub fn to_metadata_json(&self) -> eyre::Result<Vec<u8>> {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "package".into(),
            serde_json::Value::String(self.package.clone()),
        );
        obj.insert(
            "version".into(),
            serde_json::Value::String(self.app_version.clone()),
        );

        if let Some(ref m) = self.metadata {
            obj.insert("name".into(), serde_json::Value::String(m.name.clone()));
            if let Some(ref v) = m.description {
                obj.insert("description".into(), serde_json::Value::String(v.clone()));
            }
            if let Some(ref v) = m.author {
                obj.insert("author".into(), serde_json::Value::String(v.clone()));
            }
            if let Some(ref v) = m.icon {
                obj.insert("icon".into(), serde_json::Value::String(v.clone()));
            }
            if !m.tags.is_empty() {
                obj.insert(
                    "tags".into(),
                    serde_json::Value::Array(
                        m.tags
                            .iter()
                            .map(|t| serde_json::Value::String(t.clone()))
                            .collect(),
                    ),
                );
            }
            if let Some(ref v) = m.license {
                obj.insert("license".into(), serde_json::Value::String(v.clone()));
            }
            if let Some(ref v) = m.guide {
                obj.insert("guide".into(), serde_json::Value::String(v.clone()));
            }
        }

        if let Some(ref l) = self.links {
            let mut links_obj = serde_json::Map::new();
            let links = [
                ("frontend", &l.frontend),
                ("github", &l.github),
                ("docs", &l.docs),
            ];
            for (key, value) in links {
                if let Some(v) = value.as_deref().filter(|v| is_web_url(v)) {
                    links_obj.insert(key.into(), serde_json::Value::String(v.to_owned()));
                }
            }
            if !links_obj.is_empty() {
                obj.insert("links".into(), serde_json::Value::Object(links_obj));
            }
        }

        let aliases: Vec<serde_json::Value> = self
            .frontend_aliases
            .iter()
            .filter(|v| is_web_url(v))
            .map(|v| serde_json::Value::String(v.clone()))
            .collect();
        if !aliases.is_empty() {
            obj.insert("frontendAliases".into(), serde_json::Value::Array(aliases));
        }

        let outbound: Vec<serde_json::Value> = self
            .outbound
            .iter()
            .filter(|v| is_outbound_url(v))
            .map(|v| serde_json::Value::String(v.clone()))
            .collect();
        if !outbound.is_empty() {
            obj.insert("outbound".into(), serde_json::Value::Array(outbound));
        }

        Ok(serde_json::to_vec(&serde_json::Value::Object(obj))?)
    }
}

#[cfg(test)]
mod tests {
    use calimero_primitives::application::ApplicationId;
    use serde_json::Value;

    use super::*;

    fn with_secrets(outbound: &[&str], secrets: Value) -> BundleManifest {
        serde_json::from_value(serde_json::json!({
            "version": "1.0", "package": "com.example.s", "appVersion": "1.0.0",
            "minRuntimeVersion": "0.1.0", "outbound": outbound, "secrets": secrets,
        }))
        .unwrap()
    }

    fn with_uses(uses: serde_json::Value) -> BundleManifest {
        serde_json::from_value(serde_json::json!({
            "version": "1.0", "package": "com.example.notes", "appVersion": "1.0.0",
            "minRuntimeVersion": "0.1.0", "uses": uses,
        }))
        .unwrap()
    }

    /// A secret is a credential bound to one host the release sends to, path prefixes there
    /// under its `outbound` prefixes, and one place: a header (with a format) or a body
    /// field; names, counts, paths, places and descriptions are bounded, and two secrets
    /// never share a place on a path.
    #[test]
    fn declared_secrets_are_bound_to_one_host_and_one_place() {
        let out = [
            "https://api.openai.com/v1/",
            "https://challenges.cloudflare.com/turnstile/v0/siteverify",
        ];
        let ok = with_secrets(
            &out,
            serde_json::json!({
                "OPENAI_API_KEY": { "host": "api.openai.com", "paths": ["/v1/"], "header": "authorization",
                    "format": "Bearer {}", "description": "OpenAI key", "required": true },
                "TURNSTILE_SECRET": { "host": "challenges.cloudflare.com", "paths": ["/turnstile/v0/siteverify"], "bodyField": "secret" },
            }),
        );
        assert_eq!(ok.check_secrets(), Ok(()));
        let secrets = ok.secrets.as_ref().unwrap();
        assert!(secrets["OPENAI_API_KEY"].required);
        assert_eq!(
            secrets["OPENAI_API_KEY"].placement(),
            Some(SecretPlacement::Header {
                name: "authorization",
                format: "Bearer {}"
            })
        );
        assert_eq!(
            secrets["TURNSTILE_SECRET"].placement(),
            Some(SecretPlacement::BodyField("secret"))
        );
        // Signed as written: it reads back the same.
        let again: BundleManifest =
            serde_json::from_value(serde_json::to_value(&ok).unwrap()).unwrap();
        assert_eq!(again.secrets, ok.secrets);

        let err = |secrets| with_secrets(&out, secrets).check_secrets().unwrap_err();
        let header = |h: &str| serde_json::json!({ "host": "api.openai.com", "paths": ["/v1/"], "header": h });
        assert!(matches!(
            err(
                serde_json::json!({ "K": { "host": "evil.example", "paths": ["/"], "header": "authorization" } })
            ),
            SecretsError::HostNotDeclared { .. }
        ));
        assert!(matches!(
            err(serde_json::json!({ "lower": header("authorization") })),
            SecretsError::BadName(_)
        ));
        // Exactly one place: neither, both, or a format with no header are refused.
        for no_place in [
            serde_json::json!({ "host": "api.openai.com", "paths": ["/v1/"] }),
            serde_json::json!({ "host": "api.openai.com", "paths": ["/v1/"], "header": "x-key", "bodyField": "key" }),
            serde_json::json!({ "host": "api.openai.com", "paths": ["/v1/"], "bodyField": "key", "format": "{}" }),
        ] {
            assert!(matches!(
                err(serde_json::json!({ "K": no_place })),
                SecretsError::NoPlacement(_)
            ));
        }
        for bad in [
            "Authorization",
            "cookie",
            "host",
            "proxy-x",
            "content-type",
            "",
        ] {
            assert!(
                matches!(
                    err(serde_json::json!({ "K": header(bad) })),
                    SecretsError::BadHeader { .. }
                ),
                "{bad:?}"
            );
        }
        assert!(matches!(
            err(
                serde_json::json!({ "K": { "host": "api.openai.com", "paths": ["/v1/"], "header": "authorization",
                "format": "Bearer {}{}" } })
            ),
            SecretsError::BadFormat(_)
        ));
        assert!(matches!(
            err(
                serde_json::json!({ "K": { "host": "api.openai.com", "paths": ["/v1/"], "bodyField": "a b" } })
            ),
            SecretsError::BadBodyField { .. }
        ));
        assert!(matches!(
            err(serde_json::json!({ "A": header("x-key"), "B": header("x-key") })),
            SecretsError::SamePlace { .. }
        ));
        // The same place on one host twice only on paths neither shares.
        let on = |paths: Value| serde_json::json!({ "host": "api.openai.com", "header": "x-key", "paths": paths });
        assert_eq!(
            with_secrets(&out, serde_json::json!({ "A": on(serde_json::json!(["/v1/a"])), "B": on(serde_json::json!(["/v1/b"])) }))
                .check_secrets(),
            Ok(())
        );
        assert!(matches!(
            err(
                serde_json::json!({ "A": on(serde_json::json!(["/v1/"])), "B": on(serde_json::json!(["/v1/b"])) })
            ),
            SecretsError::SamePlace { .. }
        ));
        // Paths: required, bounded, well formed, and each under an `outbound` prefix.
        for bad in [
            serde_json::json!([]),
            serde_json::json!(["v1"]),
            serde_json::json!(["/v1/../admin"]),
            serde_json::json!(["/v1?x"]),
            serde_json::json!((0..=MAX_SECRET_PATHS)
                .map(|i| format!("/v1/{i}"))
                .collect::<Vec<_>>()),
        ] {
            assert!(
                matches!(
                    err(serde_json::json!({ "K": on(bad.clone()) })),
                    SecretsError::BadPaths(_)
                ),
                "{bad}"
            );
        }
        for outside in ["/", "/v2/", "/v1"] {
            assert!(
                matches!(
                    err(serde_json::json!({ "K": on(serde_json::json!([outside])) })),
                    SecretsError::PathNotDeclared { .. }
                ),
                "{outside}"
            );
        }
        let no_paths: Result<BundleManifest, _> = serde_json::from_value(serde_json::json!({
            "version": "1.0", "package": "p", "appVersion": "1.0.0", "minRuntimeVersion": "0.1.0",
            "secrets": { "K": { "host": "api.openai.com", "header": "x-key" } },
        }));
        assert!(no_paths.is_err(), "paths is required");
        let many: serde_json::Map<String, Value> = (0..=MAX_SECRETS)
            .map(|i| (format!("K{i}"), header(&format!("x-key-{i}"))))
            .collect();
        assert_eq!(err(Value::Object(many)), SecretsError::TooMany);
        let long = "x".repeat(MAX_SECRET_DESCRIPTION_BYTES + 1);
        assert!(matches!(
            err(
                serde_json::json!({ "K": { "host": "api.openai.com", "paths": ["/v1/"], "header": "x-key",
                "description": long } })
            ),
            SecretsError::LongDescription(_)
        ));
        // The earlier shape (`hosts`, filled wherever the app wrote `{{secret:NAME}}`), or
        // an unknown key, is refused when the manifest is read.
        for refused in [
            serde_json::json!({ "K": { "hosts": ["api.openai.com"] } }),
            serde_json::json!({ "K": { "host": "api.openai.com", "paths": ["/v1/"], "header": "x-key", "value": "leaked" } }),
        ] {
            let read: Result<BundleManifest, _> = serde_json::from_value(serde_json::json!({
                "version": "1.0", "package": "p", "appVersion": "1.0.0", "minRuntimeVersion": "0.1.0",
                "secrets": refused,
            }));
            assert!(read.is_err());
        }
        // No secrets: nothing to check, and nothing written.
        let none = with_secrets(&out, Value::Null);
        assert_eq!(none.check_secrets(), Ok(()));
        assert!(serde_json::to_value(&none)
            .unwrap()
            .get("secrets")
            .is_none());
    }

    #[test]
    fn an_outbound_prefixs_host_is_its_lowercase_authority() {
        assert_eq!(
            outbound_host("https://API.Example.com/v1/").as_deref(),
            Some("api.example.com")
        );
        assert_eq!(
            outbound_host("https://api.example.com").as_deref(),
            Some("api.example.com")
        );
        assert_eq!(outbound_host("https://api.example.com:8443/"), None);
        assert_eq!(outbound_host("http://api.example.com/"), None);
        assert!(is_secret_name("NEAR_AI_API_KEY"));
        assert!(!is_secret_name("1KEY") && !is_secret_name("key") && !is_secret_name(""));
    }

    /// A release names the apps it calls: the package, a semver range, the locked
    /// release inside it and its ABI digest; `create` unless it says `attach`.
    #[test]
    fn a_release_declares_the_apps_it_uses() {
        let abi = format!("sha256:{}", "a".repeat(64));
        let ok = with_uses(serde_json::json!({
            "kv": { "package": "com.calimero.confidential-kv", "range": "^0.1", "version": "0.1.3", "abi": abi },
            "calendar": { "package": "com.example.calendar", "range": ">=2, <3", "mode": "attach" },
        }));
        assert_eq!(ok.check_uses(), Ok(()));
        let uses = ok.uses.as_ref().unwrap();
        assert_eq!(uses["kv"].mode, UseMode::Create);
        assert_eq!(uses["calendar"].mode, UseMode::Attach);
        let again: BundleManifest =
            serde_json::from_value(serde_json::to_value(&ok).unwrap()).unwrap();
        assert_eq!(again.uses, ok.uses);

        let err = |u| with_uses(u).check_uses().unwrap_err();
        let kv = |extra: serde_json::Value| {
            let mut u =
                serde_json::json!({ "package": "com.calimero.confidential-kv", "range": "^0.1" });
            u.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            serde_json::json!({ "kv": u })
        };
        assert!(matches!(
            err(kv(serde_json::json!({ "version": "0.2.0" }))),
            UsesError::VersionOutsideRange { .. }
        ));
        assert!(matches!(
            err(kv(serde_json::json!({ "range": "not a range" }))),
            UsesError::BadRange { .. }
        ));
        assert!(matches!(
            err(kv(serde_json::json!({ "abi": "sha256:XYZ" }))),
            UsesError::BadAbi(_)
        ));
        assert!(matches!(
            err(kv(serde_json::json!({ "package": "com.example.notes" }))),
            UsesError::SelfReference(_)
        ));
        // The methods it calls as the person: method names, distinct, none of the node's.
        let calls = with_uses(serde_json::json!({
            "kv": { "package": "com.calimero.confidential-kv", "range": "^0.1",
                    "methods": ["get", "set_if", "listEvents"] },
        }));
        assert_eq!(calls.check_uses(), Ok(()));
        assert_eq!(calls.uses.as_ref().unwrap()["kv"].methods.len(), 3);
        for methods in [
            serde_json::json!(["__secret_set"]),
            serde_json::json!(["get", "get"]),
            serde_json::json!(["9lives"]),
            serde_json::json!(["has space"]),
        ] {
            assert!(
                matches!(
                    err(kv(serde_json::json!({ "methods": methods }))),
                    UsesError::BadMethods(_)
                ),
                "{methods}"
            );
        }
        assert!(serde_json::to_value(&ok).unwrap()["uses"]["kv"]
            .get("methods")
            .is_none());
        assert!(matches!(
            err(kv(serde_json::json!({ "package": "" }))),
            UsesError::BadPackage(_)
        ));
        assert!(matches!(
            err(serde_json::json!({ "Kv": { "package": "p", "range": "^1" } })),
            UsesError::BadName(_)
        ));
        let many: serde_json::Map<String, serde_json::Value> = (0..=MAX_USES)
            .map(|i| {
                (
                    format!("a{i}"),
                    serde_json::json!({ "package": format!("p{i}"), "range": "^1" }),
                )
            })
            .collect();
        assert_eq!(err(serde_json::Value::Object(many)), UsesError::TooMany);
        // A mode other than create or attach, or an unknown key, does not read.
        let bad: Result<BundleManifest, _> = serde_json::from_value(serde_json::json!({
            "version": "1.0", "package": "p", "appVersion": "1.0.0", "minRuntimeVersion": "0.1.0",
            "uses": { "kv": { "package": "q", "range": "^1", "mode": "borrow" } },
        }));
        assert!(bad.is_err());
        // None: nothing to check, nothing written.
        let none = with_uses(serde_json::Value::Null);
        assert_eq!(none.check_uses(), Ok(()));
        assert!(serde_json::to_value(&none).unwrap().get("uses").is_none());
    }

    /// A minimal manifest JSON with the given `handlers` block spliced in verbatim; the rest is constant.
    fn manifest_json(handlers_block: &str) -> String {
        format!(
            r#"{{
                "version": "1.0",
                "package": "com.example.deeplink",
                "appVersion": "1.0.0",
                "minRuntimeVersion": "0.1.0",
                "metadata": {{
                    "name": "Mero Chat"
                }},
                {handlers_block}
                "wasm": {{ "path": "app.wasm", "hash": "00", "size": 10 }}
            }}"#
        )
    }

    /// Like `manifest_json`, but splices the given fragment into the
    /// `metadata` object instead of the top-level `handlers` block.
    fn manifest_json_with_metadata(metadata_fragment: &str) -> String {
        format!(
            r#"{{
                "version": "1.0",
                "package": "com.example.deeplink",
                "appVersion": "1.0.0",
                "minRuntimeVersion": "0.1.0",
                "metadata": {{
                    {metadata_fragment}
                    "name": "Mero Chat"
                }},
                "wasm": {{ "path": "app.wasm", "hash": "00", "size": 10 }}
            }}"#
        )
    }

    fn stored_links(links_block: &str) -> serde_json::Value {
        let manifest: BundleManifest =
            serde_json::from_str(&manifest_json(&format!(r#""links": {links_block},"#)))
                .expect("manifest with links should deserialize");
        let meta: serde_json::Value =
            serde_json::from_slice(&manifest.to_metadata_json().unwrap()).unwrap();
        meta.get("links")
            .cloned()
            .unwrap_or(serde_json::Value::Null)
    }

    fn stored_meta(top_level_block: &str) -> serde_json::Value {
        let manifest: BundleManifest = serde_json::from_str(&manifest_json(top_level_block))
            .expect("manifest should deserialize");
        serde_json::from_slice(&manifest.to_metadata_json().unwrap()).unwrap()
    }

    /// A node's sign-in page reads the aliases from what an install stores, so
    /// they have to survive into it - and only as web URLs, like `links`.
    #[test]
    fn stores_web_frontend_aliases() {
        let meta = stored_meta(
            r#""frontendAliases": ["https://old.example/", "javascript:alert(1)", "http://localhost:5173"],"#,
        );
        assert_eq!(
            meta["frontendAliases"],
            serde_json::json!(["https://old.example/", "http://localhost:5173"])
        );
    }

    /// An app without aliases stores the same bytes it always did.
    #[test]
    fn no_frontend_aliases_stores_nothing() {
        let meta = stored_meta("");
        assert!(meta.get("frontendAliases").is_none());
        let only_unsafe = stored_meta(r#""frontendAliases": ["data:text/html,x"],"#);
        assert!(only_unsafe.get("frontendAliases").is_none());
    }

    #[test]
    fn frontend_aliases_round_trip_and_stay_off_the_wire_when_empty() {
        let manifest: BundleManifest = serde_json::from_str(&manifest_json(
            r#""frontendAliases": ["https://old.example"],"#,
        ))
        .unwrap();
        assert_eq!(manifest.frontend_aliases, vec!["https://old.example"]);

        let without: BundleManifest = serde_json::from_str(&manifest_json("")).unwrap();
        assert!(without.frontend_aliases.is_empty());
        let wire = serde_json::to_value(&without).unwrap();
        assert!(wire.get("frontendAliases").is_none());
    }

    #[test]
    fn stores_web_links() {
        let links = stored_links(
            r#"{ "frontend": "https://app.example/", "github": "http://github.com/x", "docs": "HTTPS://docs.example" }"#,
        );
        assert_eq!(links["frontend"], "https://app.example/");
        assert_eq!(links["github"], "http://github.com/x");
        assert_eq!(links["docs"], "HTTPS://docs.example");
    }

    #[test]
    fn drops_links_that_are_not_web_urls() {
        let links = stored_links(
            r#"{ "frontend": "javascript:alert(1)//", "github": " https://x.example", "docs": "data:text/html,x" }"#,
        );
        assert!(links.is_null(), "no link should survive, got {links}");

        let links = stored_links(
            r#"{ "frontend": "JavaScript:alert(1)", "github": "https://github.com/x" }"#,
        );
        assert!(links.get("frontend").is_none());
        assert_eq!(links["github"], "https://github.com/x");
    }

    #[test]
    fn outbound_urls_are_https_with_a_host_and_reach_stored_metadata() {
        assert!(is_outbound_url(
            "https://challenges.cloudflare.com/turnstile/v0/siteverify"
        ));
        assert!(!is_outbound_url("http://challenges.cloudflare.com/x"));
        assert!(!is_outbound_url("https://"));
        assert!(!is_outbound_url("https://user:pw@host.example/x"));
        assert!(!is_outbound_url("https://host.example/a b"));
        assert!(is_outbound_url("https://api.example.com"));
        assert!(!is_outbound_url("https://api.example.com/v1/?key=1"));
        assert!(!is_outbound_url("https://api.example.com/v1/#top"));
        let meta = stored_meta(
            r#""outbound": ["https://api.example.com/v1/check", "http://insecure.example/x"],"#,
        );
        assert_eq!(
            meta["outbound"],
            serde_json::json!(["https://api.example.com/v1/check"])
        );
        assert!(stored_meta("").get("outbound").is_none());
    }

    /// `upgrades` absent (every older manifest) is the owner and written back absent; an unknown value
    /// is refused, and the field is signed.
    #[test]
    fn upgrades_is_owner_unless_the_manifest_says_publisher() {
        let parse = |block: &str| serde_json::from_str::<BundleManifest>(&manifest_json(block));
        let none = parse("").unwrap();
        assert_eq!(none.upgrades, None);
        assert!(serde_json::to_value(&none)
            .unwrap()
            .get("upgrades")
            .is_none());
        assert_eq!(
            parse(r#""upgrades": "publisher","#).unwrap().upgrades,
            Some(UpgradeAuthority::Publisher)
        );
        assert_eq!(
            parse(r#""upgrades": "owner","#).unwrap().upgrades,
            Some(UpgradeAuthority::Owner)
        );
        assert!(parse(r#""upgrades": "everyone","#).is_err());

        let key = ed25519_dalek::SigningKey::from_bytes(&[3; 32]);
        let mut signed: serde_json::Value =
            serde_json::from_str(&manifest_json(r#""upgrades": "owner","#)).unwrap();
        let _sig = sign_manifest_json(&mut signed, &key).unwrap();
        assert!(verify_manifest_signature(&signed).is_ok());
        signed["upgrades"] = serde_json::json!("publisher");
        assert!(
            verify_manifest_signature(&signed).is_err(),
            "a changed declaration breaks the signature"
        );
    }

    /// `instanceOrg` absent (every older manifest) is `either` and written back absent; an unknown value
    /// is refused, and the field is signed.
    #[test]
    fn instance_org_is_either_unless_the_manifest_says_otherwise() {
        let parse = |block: &str| serde_json::from_str::<BundleManifest>(&manifest_json(block));
        let none = parse("").unwrap();
        assert_eq!(none.instance_org, None);
        assert_eq!(none.instance_org.unwrap_or_default(), InstanceOrg::Either);
        assert!(serde_json::to_value(&none)
            .unwrap()
            .get("instanceOrg")
            .is_none());
        for (word, mode) in [
            ("standalone", InstanceOrg::Standalone),
            ("org", InstanceOrg::Org),
            ("either", InstanceOrg::Either),
        ] {
            let m = parse(&format!(r#""instanceOrg": "{word}","#)).unwrap();
            assert_eq!(m.instance_org, Some(mode));
            assert_eq!(mode.as_str(), word);
        }
        assert!(parse(r#""instanceOrg": "personal","#).is_err());

        let key = ed25519_dalek::SigningKey::from_bytes(&[3; 32]);
        let mut signed: serde_json::Value =
            serde_json::from_str(&manifest_json(r#""instanceOrg": "standalone","#)).unwrap();
        let _sig = sign_manifest_json(&mut signed, &key).unwrap();
        assert!(verify_manifest_signature(&signed).is_ok());
        signed["instanceOrg"] = serde_json::json!("either");
        assert!(
            verify_manifest_signature(&signed).is_err(),
            "a changed declaration breaks the signature"
        );
    }

    /// `guests` absent (every older manifest) is `forbidden` and written back absent; an unknown value
    /// is refused, and the field is signed.
    #[test]
    fn guests_are_forbidden_unless_the_manifest_allows_them() {
        let parse = |block: &str| serde_json::from_str::<BundleManifest>(&manifest_json(block));
        let none = parse("").unwrap();
        assert_eq!(none.guests, None);
        assert_eq!(none.guests.unwrap_or_default(), GuestPolicy::Forbidden);
        assert!(serde_json::to_value(&none).unwrap().get("guests").is_none());
        assert_eq!(
            parse(r#""guests": "allowed","#).unwrap().guests,
            Some(GuestPolicy::Allowed)
        );
        assert_eq!(GuestPolicy::Allowed.as_str(), "allowed");
        assert!(parse(r#""guests": "anyone","#).is_err());
        let key = ed25519_dalek::SigningKey::from_bytes(&[3; 32]);
        let mut signed: serde_json::Value =
            serde_json::from_str(&manifest_json(r#""guests": "forbidden","#)).unwrap();
        let _sig = sign_manifest_json(&mut signed, &key).unwrap();
        assert!(verify_manifest_signature(&signed).is_ok());
        signed["guests"] = serde_json::json!("allowed");
        assert!(verify_manifest_signature(&signed).is_err());
    }

    /// `lineage` absent (every older manifest) is `free` and written back absent; an unknown value
    /// is refused, and the field is signed.
    #[test]
    fn lineage_is_free_unless_the_manifest_pins_it() {
        let parse = |block: &str| serde_json::from_str::<BundleManifest>(&manifest_json(block));
        let none = parse("").unwrap();
        assert_eq!(none.lineage, None);
        assert_eq!(none.lineage.unwrap_or_default(), Lineage::Free);
        assert!(serde_json::to_value(&none)
            .unwrap()
            .get("lineage")
            .is_none());
        for (word, lineage) in [("pinned", Lineage::Pinned), ("free", Lineage::Free)] {
            assert_eq!(
                parse(&format!(r#""lineage": "{word}","#)).unwrap().lineage,
                Some(lineage)
            );
        }
        assert!(parse(r#""lineage": "locked","#).is_err());
        let key = ed25519_dalek::SigningKey::from_bytes(&[3; 32]);
        let mut signed: serde_json::Value =
            serde_json::from_str(&manifest_json(r#""lineage": "pinned","#)).unwrap();
        let _sig = sign_manifest_json(&mut signed, &key).unwrap();
        assert!(verify_manifest_signature(&signed).is_ok());
        signed["lineage"] = serde_json::json!("free");
        assert!(verify_manifest_signature(&signed).is_err());
    }

    #[test]
    fn is_web_url_needs_a_scheme_and_a_host() {
        assert!(is_web_url("https://a"));
        assert!(is_web_url("http://localhost:5173/"));
        assert!(!is_web_url("https://"));
        assert!(!is_web_url("//evil.example"));
        assert!(!is_web_url("/relative"));
        assert!(!is_web_url(""));
        assert!(!is_web_url("https://a\n.example"));
        assert!(!is_web_url("vbscript:x"));
    }

    #[test]
    fn deserializes_handlers_slug() {
        let manifest: BundleManifest =
            serde_json::from_str(&manifest_json(r#""handlers": { "slug": "mero-chat" },"#))
                .expect("manifest with handlers should deserialize");

        let handlers = manifest.handlers.expect("handlers should be present");
        assert_eq!(handlers.slug.as_deref(), Some("mero-chat"));
    }

    #[test]
    fn handlers_absent_block_and_empty_slug_deserialize() {
        // An empty `handlers: {}` block yields Some(handlers) with slug == None
        // thanks to the field-level `#[serde(default)]`.
        let manifest: BundleManifest =
            serde_json::from_str(&manifest_json(r#""handlers": {},"#)).unwrap();
        let handlers = manifest.handlers.expect("empty handlers block present");
        assert!(handlers.slug.is_none());
    }

    /// The deep-link `handlers` field is a sibling of `metadata`, so it must NOT
    /// leak into the display metadata JSON a bundle install stores.
    #[test]
    fn handlers_does_not_affect_metadata() {
        let with_handlers: BundleManifest =
            serde_json::from_str(&manifest_json(r#""handlers": { "slug": "mero-chat" },"#))
                .unwrap();
        let without_handlers: BundleManifest = serde_json::from_str(&manifest_json("")).unwrap();

        let meta_with = with_handlers.to_metadata_json().unwrap();
        let meta_without = without_handlers.to_metadata_json().unwrap();
        assert_eq!(
            meta_with, meta_without,
            "handlers must not change the display metadata bytes"
        );
    }

    /// `buildInfo` is a sibling of `metadata` for the same reason `handlers`
    /// is: every published app would change identity if it were nested inside.
    #[test]
    fn build_info_does_not_affect_metadata() {
        let with_build_info: BundleManifest = serde_json::from_str(&manifest_json(
            r#""buildInfo": { "sdkSource": "git", "sdkVersion": "0.11.0-rc.34" },"#,
        ))
        .unwrap();
        let without: BundleManifest = serde_json::from_str(&manifest_json("")).unwrap();

        assert_eq!(
            with_build_info.to_metadata_json().unwrap(),
            without.to_metadata_json().unwrap(),
            "buildInfo must not change the display metadata bytes"
        );
    }

    #[test]
    fn deserializes_build_info() {
        let manifest: BundleManifest = serde_json::from_str(&manifest_json(
            r#""buildInfo": {
                "sdkSource": "git",
                "sdkVersion": "0.11.0-rc.34",
                "sdkRev": "6c6fb4ab4fe02500ab1262c643f52dcc6d6278bf"
            },"#,
        ))
        .expect("manifest with buildInfo should deserialize");

        let build_info = manifest.build_info.expect("buildInfo should be present");
        assert_eq!(build_info.sdk_source.as_deref(), Some("git"));
        assert_eq!(build_info.sdk_version.as_deref(), Some("0.11.0-rc.34"));
        assert_eq!(
            build_info.sdk_rev.as_deref(),
            Some("6c6fb4ab4fe02500ab1262c643f52dcc6d6278bf")
        );
    }

    #[test]
    fn author_does_not_shift_the_bundle_app_id() {
        let with: BundleManifest =
            serde_json::from_str(&manifest_json_with_metadata(r#""author": "Acme","#))
                .expect("with author");
        let without: BundleManifest =
            serde_json::from_str(&manifest_json_with_metadata("")).expect("without author");
        // Bundle ids derive from (package, signerId), never from metadata.
        assert_eq!(
            ApplicationId::for_bundle(&with.package, "did:key:z6MkExample").expect("with"),
            ApplicationId::for_bundle(&without.package, "did:key:z6MkExample").expect("without"),
        );
    }

    /// Every absent optional field (guide, buildInfo, handlers) must serialize as before it existed:
    /// an emitted null would change every signed payload.
    #[test]
    fn absent_guide_leaves_the_manifest_bytes_unchanged() {
        let manifest: BundleManifest = serde_json::from_str(&manifest_json("")).unwrap();

        assert_eq!(
            serde_json::to_string(&manifest).unwrap(),
            r#"{"version":"1.0","package":"com.example.deeplink","appVersion":"1.0.0","minRuntimeVersion":"0.1.0","metadata":{"name":"Mero Chat","tags":[]},"wasm":{"path":"app.wasm","hash":"00","size":10}}"#
        );
    }

    // Value equality, not bytes: the workspace enables serde_json's
    // `preserve_order`, so the stored key order depends on the build.
    #[test]
    fn absent_guide_leaves_the_stored_metadata_unchanged() {
        let manifest: BundleManifest = serde_json::from_str(&manifest_json("")).unwrap();

        let stored: serde_json::Value =
            serde_json::from_slice(&manifest.to_metadata_json().unwrap()).unwrap();
        assert_eq!(
            stored,
            serde_json::json!({
                "package": "com.example.deeplink",
                "version": "1.0.0",
                "name": "Mero Chat"
            })
        );
    }

    #[test]
    fn frontend_files_are_artifacts_the_node_path_checks() {
        let manifest: BundleManifest = serde_json::from_str(&manifest_json(
            r#""frontend": {
                "entry": "frontend/index.html",
                "files": [
                    { "path": "frontend/index.html", "hash": "00", "size": 1 },
                    { "path": "frontend/assets/app.js", "hash": "00", "size": 1 }
                ]
            },"#,
        ))
        .unwrap();

        let fields: Vec<_> = manifest
            .artifacts()
            .into_iter()
            .map(|(field, artifact)| (field, artifact.path.clone()))
            .collect();
        assert_eq!(
            fields,
            [
                ("wasm".to_owned(), "app.wasm".to_owned()),
                (
                    "frontend.files[0]".to_owned(),
                    "frontend/index.html".to_owned()
                ),
                (
                    "frontend.files[1]".to_owned(),
                    "frontend/assets/app.js".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn compute_components_are_artifacts_the_node_path_checks() {
        let manifest: BundleManifest = serde_json::from_str(&manifest_json(
            r#""compute": { "path": "job.wasm", "hash": "00", "size": 1 },"#,
        ))
        .unwrap();
        let fields: Vec<_> = manifest.artifacts().into_iter().map(|(f, _)| f).collect();
        assert_eq!(fields, ["wasm", "compute"]);

        let services: BundleManifest = serde_json::from_str(
            r#"{
                "version": "1.0", "package": "p", "appVersion": "1.0.0", "minRuntimeVersion": "0.1.0",
                "services": [
                    { "name": "a", "wasm": { "path": "a.wasm", "hash": "00", "size": 1 },
                      "compute": { "path": "a.job.wasm", "hash": "00", "size": 1 } }
                ]
            }"#,
        )
        .unwrap();
        let fields: Vec<_> = services.artifacts().into_iter().map(|(f, _)| f).collect();
        assert_eq!(fields, ["services[0].wasm", "services[0].compute"]);
    }

    #[test]
    fn guide_rides_in_the_manifest_and_the_stored_metadata() {
        let guide = "## Overview\nA chat app.\n\n## Procedures\n### Send a message\nCall `send`.\n";
        let mut manifest: BundleManifest = serde_json::from_str(&manifest_json("")).unwrap();
        manifest.metadata.as_mut().expect("metadata").guide = Some(guide.to_owned());

        let json = serde_json::to_value(&manifest).unwrap();
        assert_eq!(json["metadata"]["guide"], guide);
        let reparsed: BundleManifest = serde_json::from_value(json).unwrap();
        assert_eq!(
            reparsed.metadata.expect("metadata").guide.as_deref(),
            Some(guide)
        );

        let stored: serde_json::Value =
            serde_json::from_slice(&manifest.to_metadata_json().unwrap()).unwrap();
        assert_eq!(stored["guide"], guide);
    }
}
