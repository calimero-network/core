pub mod archive;
mod frontend;
#[cfg(feature = "html")]
mod html;
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

/// Represents an artifact within a bundle (WASM, ABI, migration)
///
/// `hash` is what binds the artifact bytes to the manifest signature, so a
/// manifest that omits it is malformed rather than merely unhashed.
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
    /// Primary storefront category, from the registry's closed vocabulary
    /// (`cargo_mero::meta::CATEGORIES`). Optional on the wire so bundles
    /// published before it existed still deserialize, and so an older node
    /// reading a newer bundle is unaffected — this struct has no
    /// `deny_unknown_fields`.
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

/// Whether `url` may be declared under a manifest's `outbound`: `https://`,
/// a host, no credentials, no query or fragment, no whitespace or control
/// characters, at most 2048 bytes. It is a prefix: it covers itself and every
/// URL below it at a `/`, with any query string.
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

/// Most secrets one release declares.
pub const MAX_SECRETS: usize = 16;
/// Most hosts one declared secret may go to.
pub const MAX_SECRET_HOSTS: usize = 8;
/// Most bytes of a declared secret's description.
pub const MAX_SECRET_DESCRIPTION_BYTES: usize = 256;
/// Most bytes of a secret's name.
pub const MAX_SECRET_NAME_BYTES: usize = 64;

/// One secret a release declares (`BundleManifest::secrets`).
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BundleSecret {
    /// The hosts it may be sent to: each the host of one of the release's `outbound`
    /// prefixes, lowercase.
    pub hosts: Vec<String>,
    /// What it is, for whoever supplies it ("NEAR AI Cloud API key").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The app does not work without it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
}

/// Whether `name` may name a secret: `A-Z`, `0-9` and `_`, starting with a letter, at most
/// [`MAX_SECRET_NAME_BYTES`]. The same names `{{secret:NAME}}` takes.
#[must_use]
pub fn is_secret_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SECRET_NAME_BYTES
        && name.starts_with(|c: char| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
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

/// Why a manifest's `secrets` are refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SecretsError {
    #[error("a release declares at most {MAX_SECRETS} secrets")]
    TooMany,
    #[error("secret name {0:?} must be A-Z, 0-9 and _, starting with a letter, at most {MAX_SECRET_NAME_BYTES} bytes")]
    BadName(String),
    #[error("secret {0} names no host")]
    NoHosts(String),
    #[error("secret {0} names more than {MAX_SECRET_HOSTS} hosts")]
    TooManyHosts(String),
    #[error("secret {name} names {host}, which no outbound prefix of the release covers")]
    HostNotDeclared { name: String, host: String },
    #[error("secret {0}'s description is longer than {MAX_SECRET_DESCRIPTION_BYTES} bytes")]
    LongDescription(String),
}

impl SecretsError {
    /// The stable code a node answers with.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        "BAD_SECRETS"
    }
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
    /// The referenced app's methods this release calls as the person its run serves, once
    /// that person grants it (`SystemCmd::SetDelegation`): the referenced app then sees the
    /// person as its caller. Empty: the release calls it as itself only. Shown on the
    /// release's card and on the grant's consent, so the person sees what they allow.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub methods: Vec<String>,
}

/// Whether `name` may name a method a release calls as the person: an ABI method name
/// (`A-Za-z0-9_`, not starting with a digit, at most [`MAX_USE_METHOD_BYTES`]), never one of
/// the node's own (`__`).
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
    /// Checks `secrets` against the rules a node enforces at install: valid names, at most
    /// [`MAX_SECRETS`], each with 1 to [`MAX_SECRET_HOSTS`] hosts, every host the host of one
    /// of the release's `outbound` prefixes (so a secret goes only where the release already
    /// says data goes), and a short description.
    ///
    /// # Errors
    ///
    /// The first rule a declared secret breaks.
    pub fn check_secrets(&self) -> Result<(), SecretsError> {
        let Some(secrets) = &self.secrets else {
            return Ok(());
        };
        if secrets.len() > MAX_SECRETS {
            return Err(SecretsError::TooMany);
        }
        let hosts: std::collections::BTreeSet<String> = self
            .outbound
            .iter()
            .filter_map(|p| outbound_host(p))
            .collect();
        for (name, secret) in secrets {
            if !is_secret_name(name) {
                return Err(SecretsError::BadName(name.clone()));
            }
            if secret.hosts.is_empty() {
                return Err(SecretsError::NoHosts(name.clone()));
            }
            if secret.hosts.len() > MAX_SECRET_HOSTS {
                return Err(SecretsError::TooManyHosts(name.clone()));
            }
            if secret.description.len() > MAX_SECRET_DESCRIPTION_BYTES {
                return Err(SecretsError::LongDescription(name.clone()));
            }
            for host in &secret.hosts {
                if !hosts.contains(host) {
                    return Err(SecretsError::HostNotDeclared {
                        name: name.clone(),
                        host: host.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Checks `uses` against the rules a node enforces at install: valid names, at most
    /// [`MAX_USES`], a package other than the release's own, a semver range, a locked
    /// version inside it, and an ABI digest of the right shape.
    ///
    /// # Errors
    ///
    /// The first rule a reference breaks.
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

/// Deep-link handler declarations for the application.
///
/// Lets an app declare its own deep-link `slug` (e.g. `mero-chat`) so links
/// like `https://links.calimero.network/<slug>/join` and
/// `calimero://<slug>/join` resolve to the right app. When absent, the desktop
/// falls back to kebab-casing the app name.
///
/// This block is a sibling of `metadata` in the manifest, NOT nested inside it,
/// so it is deliberately outside the raw-wasm application-id derivation
/// (`hash(bytecode, size, source, metadata)`).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleHandlers {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
}

/// How the bundle's `calimero-sdk` dependency resolved at build time — i.e.
/// which node release the wasm was actually compiled against.
///
/// Distinct from `min_runtime_version`, which is a hand-declared floor ("refuse
/// to run below this") and defaults to `0.1.0` for anyone who never sets it.
/// This block is derived, not declared: a bundling tool reads it off the
/// resolve graph, so it cannot drift from the bytecode it ships beside.
///
/// A sibling of `metadata`, like `handlers`, and for the same reason: stamping
/// it must not move any app's raw-wasm application id
/// (`hash(bytecode, size, source, metadata)`).
///
/// Every field is optional on the wire, and the struct has no
/// `deny_unknown_fields`: bundles published before this existed carry no
/// `buildInfo` at all, and an older node reading a newer bundle is unaffected.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleBuildInfo {
    /// Where the dependency resolved from: `"git"`, `"registry"`, or `"path"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk_source: Option<String>,

    /// The node version the wasm was built against, when the resolution names
    /// one: the git `tag=`, or the crate version for a registry dependency.
    ///
    /// ⚠️ NEVER the git dependency's own crate version. Core's in-git
    /// `[workspace.package] version` is the literal `0.0.0` placeholder that
    /// cargo-workspaces rewrites only when publishing, so every git-resolved
    /// `calimero-sdk` reports `0.0.0` — a tagged checkout of rc.34 included.
    /// A branch or rev dependency names no version at all; both leave this
    /// `None` rather than publish a number that is not a release. `sdk_rev`
    /// still identifies the build exactly in that case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk_version: Option<String>,

    /// The exact git commit the dependency resolved to. `None` for a registry
    /// or path dependency, which have no commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk_rev: Option<String>,
}

/// Cryptographic signature of the manifest
///
/// The signature is computed over the SHA-256 hash of the canonical manifest bytes (RFC 8785 JCS) with the `signature` field excluded.
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

/// Who upgrades the instances of a package, as its signed manifest declares
/// (`upgrades`), where the cluster leaves app upgrades to owners
/// (`--app-upgrades owner`; where the operator holds them, it alone does).
///
/// The declaration may only loosen toward owners: once a release declares
/// `owner`, no later version of the package may declare `publisher` (a node
/// refuses to install it), and no instance is upgraded from an `owner`
/// release to a `publisher` one. Whoever accepted an instance as theirs to
/// upgrade never loses that to a later release.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UpgradeAuthority {
    /// Each instance's owner, or, for an organization's instance, whoever
    /// holds `MANAGE_APPLICATION` there. The default when the manifest
    /// leaves `upgrades` out.
    #[default]
    Owner,
    /// The organization the package's signing key is bound to: its `MANAGE_APPLICATION` holders upgrade every
    /// instance, whoever owns it. Until an organization has bound the key
    /// (`BindPublisher`), nobody upgrades them.
    Publisher,
}

impl UpgradeAuthority {
    /// Whether this is [`Self::Owner`] (the default, left out when stored).
    #[must_use]
    pub fn is_owner(&self) -> bool {
        *self == Self::Owner
    }
}

/// Whether an instance of a package takes guests, as its signed manifest declares
/// (`guests`): accounts listed on one instance that are not members of its organization.
///
/// Absent means [`Self::Forbidden`], so a package from before never takes guests it did
/// not declare. A node older than the rule ignores the field.
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

/// Where an instance of a package goes, as its signed manifest declares
/// (`instanceOrg`): the organization it is created in.
///
/// Absent means [`Self::Either`]. A node older than the rule ignores the field
/// (the signature is over the raw manifest).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InstanceOrg {
    /// Every instance comes with an organization of its own, made with it (its
    /// creator the owner); a request may not name an existing one. Its people are
    /// invited into that organization.
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

/// Bundle manifest describing the contents of a bundle archive.
///
/// Supports two formats:
/// - **Single-service** (backward compat): `wasm` + optional `abi` fields
/// - **Multi-service**: `services` array with named WASM modules
///
/// If `services` is present and non-empty, it takes priority over `wasm`/`abi`.
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

    /// Other URLs the same frontend is served at, besides `links.frontend`.
    /// Never opened: `links.frontend` stays the one place clients load the
    /// app. A node's sign-in page also trusts a login callback to any of their
    /// origins, so an app that moves domain lists its old origin here until
    /// every client has moved, then drops it.
    ///
    /// A sibling of `links`, not inside it: the registry refuses any `links`
    /// value that is not a single URL string, and clients iterate `links` as
    /// one URL each. An older node or registry ignores an unknown top-level
    /// field, and a bundle's id is `(package, signer)`, so no app moves.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frontend_aliases: Vec<String>,

    /// The `https://` URL prefixes the app's runs may send an outbound request
    /// under (a run's outgoing HTTP): `https://api.example.com/v1/` covers every
    /// path below it, with any query string. Signed with the rest, shown at
    /// install, and kept on the release record, where the node reads it: a request
    /// no prefix covers is refused, and an app that lists none never gets a client.
    /// A node still sends nothing unless its operator turned outbound requests on
    /// and allows the URL's host.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outbound: Vec<String>,

    /// The secrets the app's outbound requests name (`{{secret:NAME}}`), by name: the
    /// hosts each may go to (each the host of an `outbound` prefix) and what it is, so
    /// whoever deploys the app knows what to supply ([`BundleSecret`]). Signed with the
    /// rest. A node that keeps it fills only a declared name, and only into a request to
    /// one of its hosts; absent, the node fills any secret its operator bound to the
    /// package, as before. An older node ignores the field (the signature is over the
    /// raw manifest).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<BTreeMap<String, BundleSecret>>,
    /// The other apps this release calls, by the name its code knows each by
    /// ([`BundleUse`]): the package, the semver range it works with, the release it was
    /// built against and that release's ABI, and whether the reference is created with
    /// the app and upgraded with it (`create`) or an app the deployer already runs
    /// (`attach`). Signed with the rest, shown before install and on the upgrade consent
    /// page. An older node ignores the field (the signature is over the raw manifest).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uses: Option<BTreeMap<String, BundleUse>>,

    /// Who upgrades the package's instances ([`UpgradeAuthority`]); absent
    /// means [`UpgradeAuthority::Owner`]. Signed with the rest; a value other
    /// than `owner` or `publisher` fails to parse, so the bundle is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upgrades: Option<UpgradeAuthority>,

    /// Which organization an instance is created in ([`InstanceOrg`]): one of its own
    /// (`standalone`), one its creator is in (`org`), or either (`either`, the default
    /// when absent). Signed with the rest; any other value fails to parse, so the
    /// bundle is refused. An older node ignores the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_org: Option<InstanceOrg>,

    /// Whether an instance takes guests ([`GuestPolicy`]): `allowed`, or `forbidden` (the
    /// default when absent). Signed with the rest; any other value fails to parse. An
    /// older node ignores the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guests: Option<GuestPolicy>,

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
    /// Returns the list of service names in this bundle.
    /// For single-service bundles, returns an empty vec (no named services).
    pub fn service_names(&self) -> Vec<&str> {
        match &self.services {
            Some(svcs) if !svcs.is_empty() => svcs.iter().map(|s| s.name.as_str()).collect(),
            _ => vec![],
        }
    }

    /// Returns true if this is a multi-service bundle.
    pub fn is_multi_service(&self) -> bool {
        matches!(&self.services, Some(svcs) if svcs.len() > 1)
    }

    /// Iterate all WASM artifacts uniformly, regardless of single/multi-service.
    ///
    /// - Multi-service (`services` non-empty): yields one `WasmArtifact` per service.
    /// - Single-service: yields one `WasmArtifact` with `name: None` from the
    ///   top-level `wasm` field.
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

    /// Serialize the manifest's display metadata to JSON bytes for storage.
    ///
    /// Extracts `package`, `version`, `metadata.*`, `links.*` and
    /// `frontendAliases` into a flat JSON object. This replaces the ~70-line inline blocks that were
    /// copy-pasted across the three bundle install paths.
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

    /// A secret goes only where the release already says data goes: each host is an
    /// `outbound` prefix's host; names, counts and descriptions are bounded.
    #[test]
    fn declared_secrets_name_only_hosts_the_release_sends_to() {
        let out = ["https://cloud-api.near.ai/v1/chat/completions"];
        let ok = with_secrets(
            &out,
            serde_json::json!({ "NEAR_AI_API_KEY": {
                "hosts": ["cloud-api.near.ai"], "description": "NEAR AI key", "required": true } }),
        );
        assert_eq!(ok.check_secrets(), Ok(()));
        let s = &ok.secrets.as_ref().unwrap()["NEAR_AI_API_KEY"];
        assert!(s.required);
        // Signed as written: it reads back the same.
        let again: BundleManifest =
            serde_json::from_value(serde_json::to_value(&ok).unwrap()).unwrap();
        assert_eq!(again.secrets, ok.secrets);

        let err = |secrets| with_secrets(&out, secrets).check_secrets().unwrap_err();
        assert!(matches!(
            err(serde_json::json!({ "K": { "hosts": ["evil.example"] } })),
            SecretsError::HostNotDeclared { .. }
        ));
        assert!(matches!(
            err(serde_json::json!({ "lower": { "hosts": ["cloud-api.near.ai"] } })),
            SecretsError::BadName(_)
        ));
        assert!(matches!(
            err(serde_json::json!({ "K": { "hosts": [] } })),
            SecretsError::NoHosts(_)
        ));
        let many: serde_json::Map<String, Value> = (0..=MAX_SECRETS)
            .map(|i| {
                (
                    format!("K{i}"),
                    serde_json::json!({ "hosts": ["cloud-api.near.ai"] }),
                )
            })
            .collect();
        assert_eq!(err(Value::Object(many)), SecretsError::TooMany);
        let long = "x".repeat(MAX_SECRET_DESCRIPTION_BYTES + 1);
        assert!(matches!(
            err(
                serde_json::json!({ "K": { "hosts": ["cloud-api.near.ai"], "description": long } })
            ),
            SecretsError::LongDescription(_)
        ));
        // An unknown key in a secret is refused when the manifest is read.
        let unknown: Result<BundleManifest, _> = serde_json::from_value(serde_json::json!({
            "version": "1.0", "package": "p", "appVersion": "1.0.0", "minRuntimeVersion": "0.1.0",
            "secrets": { "K": { "hosts": [], "value": "leaked" } },
        }));
        assert!(unknown.is_err());
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

    /// A minimal manifest JSON with the given optional `handlers` block spliced
    /// in verbatim. Everything else is held constant so the only difference
    /// between two produced manifests is the presence of `handlers`.
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

    /// `upgrades` says who upgrades a package's instances: absent (every
    /// manifest from before) is the owner and is written back absent, an
    /// unknown value is refused, and the field is signed with the rest.
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

    /// `instanceOrg` says where a package's instances go: absent (every manifest
    /// from before) is `either` and is written back absent, an unknown value is
    /// refused, and the field is signed with the rest.
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

    /// `guests` says whether an instance takes guests: absent (every manifest from
    /// before) is `forbidden` and is written back absent, an unknown value is refused, and
    /// the field is signed with the rest.
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
    fn handlers_defaults_to_none_when_absent() {
        let manifest: BundleManifest = serde_json::from_str(&manifest_json(""))
            .expect("manifest without handlers should deserialize");

        assert!(
            manifest.handlers.is_none(),
            "handlers must default to None when the block is absent"
        );
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

    /// Every bundle published before `buildInfo` existed has none. Reading one
    /// must stay a plain `None`, not an error — this is the whole reason the
    /// field is optional.
    #[test]
    fn build_info_defaults_to_none_when_absent() {
        let manifest: BundleManifest = serde_json::from_str(&manifest_json(""))
            .expect("manifest without buildInfo should deserialize");

        assert!(manifest.build_info.is_none());
    }

    /// A manifest that carries no `buildInfo` must serialize without the key
    /// at all: an emitted `"buildInfo": null` would change the canonical bytes
    /// every existing signature was computed over.
    #[test]
    fn absent_build_info_is_not_serialized() {
        let manifest: BundleManifest = serde_json::from_str(&manifest_json("")).unwrap();

        let json = serde_json::to_string(&manifest).expect("serialize");

        assert!(
            !json.contains("buildInfo"),
            "absent buildInfo must not appear in the serialized manifest: {json}"
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

    /// A manifest without a guide must serialize exactly as it did before the
    /// field existed: an emitted `"guide": null` would change every signed payload.
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
