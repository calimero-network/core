//! Packages: signed `.mpk` releases, installed from the operator's registry or
//! uploaded as a file.
//!
//! A release is verified before the system log records it (`InstallPackage`):
//! the publisher's signature over the manifest (`calimero_bundle::VerifiedBundle`),
//! every artifact's digest, the manifest's `minRuntimeVersion`, the package's
//! pinned signer, and that each service's WASM compiles. The log records the
//! `.mpk`'s hash, so a node that fetches it later checks it is the very release
//! the fleet installed, then verifies it again before running any of it.
//!
//! A registry release is never pushed between nodes: each node that runs it
//! fetches it from the registry itself (`Node::release_wasm`), the one-source
//! rule. An uploaded release has no registry to come from, so a majority of
//! nodes hold it and the rest fetch it from them.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use calimero_app_downloader::source::http::HttpRegistry;
use calimero_app_downloader::{AppRequest, AppSource};
use calimero_app_host::{Abi, MethodKind};
use calimero_bundle::VerifiedBundle;
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::error::ApiError;
use crate::node::Node;
use crate::system::{
    compare_versions, BundleSource, PackageRef, PackageVersion, SystemCmd, SystemState, Visibility,
    OPERATOR,
};
use crate::types::{now_secs, Components};

pub(crate) mod pipelines;

/// The largest `.mpk` a node takes, uploaded or fetched: it is kept as a blob,
/// so the blob limit (v1 allowed 512 MiB; a blob here must reach a
/// majority inside one sealed request).
pub const MAX_MPK_BYTES: usize = crate::node::MAX_BLOB_BYTES;

/// The header naming the account a private install is for, sent with the registry's credential.
pub const INSTALL_ACCOUNT_HEADER: &str = "x-calimero-install-account";

/// How a create or upgrade reaches the release it names, besides the account's own rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    /// The account's own rule ([`SystemState::sees_release`]).
    Account,
    /// A session bound to the package's frontend ([`SystemState::sees_release_from`]).
    Frontend,
}

impl Via {
    fn sees(self, s: &SystemState, v: &PackageVersion, actor: &str) -> bool {
        s.sees_release_from(v, actor, self == Self::Frontend)
    }
}

/// The release a bundle hash came from, as the system log recorded it.
#[derive(Clone, Debug)]
pub struct Release {
    pub at: PackageRef,
    pub version: PackageVersion,
}

/// This node's release, which a manifest's `minRuntimeVersion` must not exceed.
fn runtime_version() -> semver::Version {
    semver::Version::parse(env!("CALIMERO_RELEASE_VERSION"))
        .expect("build.rs sets CALIMERO_RELEASE_VERSION from the workspace release")
}

/// A verified release, ready to record: what `InstallPackage` carries, plus
/// each service's WASM for this node's blob store.
pub struct Verified {
    pub package: String,
    pub signer_id: String,
    pub release: PackageVersion,
    /// Every WASM of the release (services and compute components) by its bundle hash.
    pub wasm: BTreeMap<String, Vec<u8>>,
}

/// One service of a release, digest-checked.
pub struct Service {
    /// `""` for a single-service bundle.
    pub name: String,
    /// The service's pipeline files by archive path.
    pub pipelines: Vec<(String, Vec<u8>)>,
    /// The app component; `None` for a compute-only single-service bundle.
    pub wasm: Option<Vec<u8>>,
    /// Its compute component, when the release ships one.
    pub compute: Option<Vec<u8>>,
}

/// The refusal when a compute-only package's component declares no compute method.
pub const NO_COMPUTE_METHODS: &str = "NO_COMPUTE_METHODS";

/// The refusal when a compute component's methods or caps differ from the main ABI's.
pub const COMPUTE_ABI_MISMATCH: &str = "COMPUTE_ABI_MISMATCH";

/// `e` naming the release's `service` it refused, and whether it was that service's
/// compute component.
#[must_use]
pub fn of_service(e: ApiError, service: &str, compute: bool) -> ApiError {
    let mut details = serde_json::json!({ "service": service });
    if compute {
        details["compute"] = serde_json::json!(true);
    }
    e.with_details(details)
}

/// Verify a `.mpk`: signature, digests, runtime version, and that it has WASM.
pub fn verify(mpk: Arc<[u8]>) -> Result<(VerifiedBundle, Vec<Service>), ApiError> {
    if mpk.len() > MAX_MPK_BYTES {
        return Err(ApiError::new(
            413,
            "TOO_LARGE",
            &format!("a package is limited to {} MiB", MAX_MPK_BYTES >> 20),
        ));
    }
    let bundle = VerifiedBundle::open(mpk, &runtime_version())
        .map_err(|e| ApiError::bad_request("BAD_PACKAGE", &format!("{e:#}")))?;
    let contents = bundle
        .contents()
        .map_err(|e| ApiError::bad_request("BAD_PACKAGE", &format!("{e:#}")))?;
    if contents.wasm.is_empty() {
        return Err(ApiError::bad_request(
            "BAD_PACKAGE",
            "the package ships no WASM",
        ));
    }
    let wasm = contents
        .wasm
        .into_iter()
        .map(|w| Service {
            name: w.name.unwrap_or_default(),
            wasm: w.bytes.map(|b| b.to_vec()),
            compute: w.compute.map(|c| c.to_vec()),
            pipelines: w
                .pipelines
                .into_iter()
                .map(|(path, bytes)| (path, bytes.to_vec()))
                .collect(),
        })
        .collect();
    Ok((bundle, wasm))
}

/// A service's compute component must run exactly the compute methods its main ABI declares:
/// the same names, each with the same kind and caps (fuel, memory, wall time, result method).
fn compute_abi_agrees(main: &Abi, compute: &Abi) -> Result<(), String> {
    let declared = |abi: &Abi| -> BTreeMap<String, MethodKind> {
        abi.methods
            .iter()
            .filter(|(_, m)| matches!(m.kind, MethodKind::Compute(_)))
            .map(|(name, m)| (name.clone(), m.kind.clone()))
            .collect()
    };
    let (main, compute) = (declared(main), declared(compute));
    if main == compute {
        return Ok(());
    }
    let names = |m: &BTreeMap<String, MethodKind>| m.keys().cloned().collect::<Vec<_>>();
    Err(format!(
        "the compute component's methods {:?} do not match the main ABI's compute methods {:?}, or differ in their caps",
        names(&compute),
        names(&main),
    ))
}

impl Node {
    /// The operator's registry (`--registry-url`), with its credential (the URL's userinfo)
    /// only when `credential`.
    fn registry(&self, credential: bool) -> Result<HttpRegistry, ApiError> {
        let mut url = self.registry_url()?;
        if !credential {
            let _user = url.set_username("");
            let _password = url.set_password(None);
        }
        HttpRegistry::new(url).map_err(|e| ApiError::internal(&format!("{e:#}")))
    }

    fn registry_url(&self) -> Result<url::Url, ApiError> {
        let base = self.config.registry.as_ref().ok_or_else(|| {
            ApiError::bad_request(
                "NO_REGISTRY",
                "this node has no registry configured (--registry-url)",
            )
        })?;
        base.parse()
            .map_err(|e| ApiError::internal(&format!("invalid registry URL: {e}")))
    }

    /// `package@version`'s `.mpk` from `registry`, unverified; `None` when it serves none.
    async fn fetch_from(
        registry: HttpRegistry,
        package: &str,
        version: &str,
    ) -> Result<Option<Arc<[u8]>>, ApiError> {
        let req = AppRequest {
            application_id: None,
            package,
            version,
            bytecode_id: None,
            context_id: None,
        };
        registry.fetch(&req).await.map_err(|e| {
            ApiError::unavailable(&format!("the registry could not be reached: {e:#}"))
        })
    }

    /// `package@version` for an install by `actor`, and whether it is public: served without
    /// the registry's credential, it is; else it is fetched with the credential, naming the
    /// installing account (none for the operator), and is private. An account installs a
    /// private release only for an organization (`org`).
    async fn fetch_for_install(
        &self,
        package: &str,
        version: &str,
        actor: &str,
        org: Option<&String>,
    ) -> Result<(Arc<[u8]>, Visibility), ApiError> {
        if let Some(mpk) = Self::fetch_from(self.registry(false)?, package, version).await? {
            return Ok((mpk, Visibility::Public));
        }
        let url = self.registry_url()?;
        if url.username().is_empty() && url.password().is_none() {
            return Err(ApiError::not_found(&format!(
                "the registry has no {package}@{version}"
            )));
        }
        if actor != OPERATOR && org.is_none() {
            return Err(crate::api::apps::no_organization(&format!(
                "the registry serves no public {package}@{version}, and a private one is installed for an organization you are in"
            )));
        }
        let mut registry = self.registry(true)?;
        if actor != OPERATOR {
            registry = registry
                .with_header(INSTALL_ACCOUNT_HEADER, actor)
                .map_err(|e| ApiError::internal(&format!("{e:#}")))?;
        }
        match Self::fetch_from(registry, package, version).await? {
            Some(mpk) => Ok((mpk, Visibility::Private)),
            None => Err(ApiError::not_found(&format!(
                "the registry has no {package}@{version} for this account; a private package needs this cluster account linked to its Calimero Cloud developer in the console"
            ))),
        }
    }

    /// The organization `actor` installs for: `named`, which it must be a member of, else
    /// its only one (`None`: it is in none). The operator installs for none.
    async fn install_org(
        &self,
        actor: &str,
        named: Option<&str>,
    ) -> Result<Option<String>, ApiError> {
        if actor == OPERATOR {
            return Ok(None);
        }
        let Some(org) = named else {
            return crate::api::apps::only_org(self, actor).await;
        };
        let _caught_up = self.sys_fresh().await;
        if !self.sys(|s| s.member_of(org, actor)) {
            return Err(ApiError::not_found("no such organization"));
        }
        Ok(Some(org.to_owned()))
    }

    /// Install `package@version` from the registry for `org` (named, else the installer's
    /// only one), public or private as the registry serves it.
    pub async fn install_from_registry(
        &self,
        package: &str,
        version: &str,
        actor: &str,
        org: Option<&str>,
    ) -> Result<Value, ApiError> {
        self.refuse_undeployable(actor).await?;
        let org = self.install_org(actor, org).await?;
        // Before the fetch, judged by the key the package is pinned to (none
        // yet: unbound); `record_release` judges the verified key again.
        let pinned = self
            .sys(|s| s.packages.get(package).map(|p| p.signer_id.clone()))
            .unwrap_or_default();
        self.refuse_stalled_install(actor, &pinned)?;
        let (mpk, visibility) = self
            .fetch_for_install(package, version, actor, org.as_ref())
            .await?;
        let mut verified = self
            .verify_release(mpk, BundleSource::Registry, actor)
            .await?;
        if verified.package != package || verified.release.version != version {
            return Err(ApiError::bad_request(
                "BAD_PACKAGE",
                &format!(
                    "asked the registry for {package}@{version}, it served {}@{}",
                    verified.package, verified.release.version
                ),
            ));
        }
        verified.release.visibility = Some(visibility);
        verified.release.orgs = org.into_iter().collect();
        self.record_release(verified, None).await
    }

    /// Refused `409 RELEASE_DEADLINE` before any fetch or verification when
    /// `actor` may install nothing past a release deadline
    /// ([`crate::system::SystemState::install_stalled`]); `InstallPackage`
    /// asks again at apply.
    fn refuse_stalled_install(&self, actor: &str, signer: &str) -> Result<(), ApiError> {
        match self.sys(|s| s.install_stalled(actor, signer, now_secs())) {
            Some(stall) => Err(crate::node::stalled(&stall)),
            None => Ok(()),
        }
    }

    /// Refused `403 DEPLOY_NOT_GRANTED` before any fetch, verification or
    /// store when `actor` may not install apps here
    /// ([`crate::system::SystemState::may_deploy`]), judged on fresh state
    /// before it refuses; `InstallPackage` and `PutBundle` ask again at
    /// apply, at their own time.
    pub async fn refuse_undeployable(&self, actor: &str) -> Result<(), ApiError> {
        let may = || self.sys(|s| s.may_deploy(actor, now_secs()));
        if may() {
            return Ok(());
        }
        let _caught_up = self.sys_fresh().await;
        if may() {
            return Ok(());
        }
        Err(crate::node::not_a_deployer())
    }

    /// Install an uploaded `.mpk`, private to `org` (named, else the installer's only one).
    pub async fn install_upload(
        &self,
        mpk: Arc<[u8]>,
        actor: &str,
        org: Option<&str>,
    ) -> Result<Value, ApiError> {
        self.refuse_undeployable(actor).await?;
        let org = self.install_org(actor, org).await?;
        if actor != OPERATOR && org.is_none() {
            return Err(crate::api::apps::no_organization(
                "an upload is private to an organization you are in",
            ));
        }
        let mut verified = self
            .verify_release(Arc::clone(&mpk), BundleSource::Upload, actor)
            .await?;
        verified.release.visibility = Some(Visibility::Private);
        verified.release.orgs = org.into_iter().collect();
        self.record_release(verified, Some(mpk)).await
    }

    /// Everything about a release that can be checked before the log records
    /// it, including the signer pin `apply` checks again.
    async fn verify_release(
        &self,
        mpk: Arc<[u8]>,
        source: BundleSource,
        actor: &str,
    ) -> Result<Verified, ApiError> {
        let mpk_hash = hex::encode(calimero_identity::sha256(&mpk));
        let mpk_size = mpk.len() as u64;
        let (bundle, wasm) = verify(mpk)?;
        crate::consent_page::remember_release_facts(self, &mpk_hash, &bundle, &wasm);
        let manifest = bundle.manifest();
        let package = manifest.package.clone();
        let signer_id = bundle.signer_id().to_owned();
        // The dev key's seed is public: its signature proves no publisher, so
        // only a node that takes unsigned bundles takes a release it signed.
        if signer_id == calimero_bundle::DEV_SIGNER_ID && !self.config.allow_unsigned_bundles {
            return Err(ApiError::new(
                403,
                "DEV_SIGNED_PACKAGE",
                &format!(
                    "{package} is signed with the public development key (`meroctl app pack` without --key); \
                     this node takes it only with --dev or --allow-unsigned-bundles"
                ),
            ));
        }
        if let Some(pinned) = self.sys(|s| s.packages.get(&package).map(|p| p.signer_id.clone())) {
            if pinned != signer_id {
                return Err(ApiError::forbidden(&format!(
                    "{package} is published under {pinned}; this release is signed by {signer_id}"
                )));
            }
        }
        manifest
            .check_secrets()
            .map_err(|e| ApiError::bad_request(e.code(), &e.to_string()))?;
        manifest
            .check_uses()
            .map_err(|e| ApiError::bad_request(e.code(), &e.to_string()))?;
        let upgrades = manifest.upgrades.unwrap_or_default();
        let in_order = self.sys(|s| {
            s.packages
                .get(&package)
                .is_none_or(|p| p.upgrades_in_order(&manifest.app_version, upgrades))
        });
        if !in_order {
            return Err(ApiError::conflict(
                "a release may not give its package's upgrades back to the publisher once a lower one gave them to owners",
            ));
        }
        let mut services = BTreeMap::new();
        let mut compute = BTreeMap::new();
        let mut wasm_by_hash = BTreeMap::new();
        let mut part_sizes = BTreeMap::new();
        let mut only_compute = false;
        let mut service_pipelines: pipelines::ServicePipelines = (Vec::new(), None);
        for Service {
            name: service,
            wasm: bytes,
            compute: job,
            pipelines: files,
        } in wasm
        {
            if !files.is_empty() && !service.is_empty() {
                return Err(ApiError::new(
                    422,
                    "PIPELINES_NEED_ONE_SERVICE",
                    "pipelines run on a package with one service",
                ));
            }
            let Some(bytes) = bytes else {
                only_compute = true;
                let job = job.ok_or_else(|| {
                    ApiError::bad_request("BAD_PACKAGE", "the package ships no WASM")
                })?;
                let job_hash = hex::encode(calimero_identity::sha256(&job));
                let prepared = {
                    use calimero_app_host::Engine as _;
                    self.compute_engine()
                        .prepare(&job_hash, &job)
                        .map_err(|e| {
                            ApiError::bad_request(crate::node::BAD_BUNDLE, &e.to_string())
                                .with_details(
                                    serde_json::json!({ "service": service, "compute": true }),
                                )
                        })?
                };
                if !prepared
                    .abi
                    .methods
                    .values()
                    .any(|m| matches!(m.kind, MethodKind::Compute(_)))
                {
                    return Err(ApiError::bad_request(
                        NO_COMPUTE_METHODS,
                        "a package with only a compute component must declare at least one compute method",
                    ));
                }
                if !files.is_empty() {
                    service_pipelines = (files, Some(pipelines::compute_caps(&prepared.abi)));
                }
                let _prev = compute.insert(service, job_hash.clone());
                let _prev = part_sizes.insert(job_hash.clone(), job.len() as u64);
                let _prev = wasm_by_hash.insert(job_hash, job);
                continue;
            };
            let hash = hex::encode(calimero_identity::sha256(&bytes));
            // Refuse what no app could run before anything is recorded: a core
            // module (an SDK 0.12 service) is `400 UNSUPPORTED_BUNDLE`, a component
            // must pass the engine's prepare.
            if crate::types::Executor::of(&bytes) != crate::types::Executor::Component {
                return Err(of_service(ApiError::unsupported_bundle(), &service, false));
            }
            let main = {
                use calimero_app_host::Engine as _;
                self.app_engine()
                    .prepare(&hash, &bytes)
                    .map_err(|e| ApiError::bad_request(crate::node::BAD_BUNDLE, &e.to_string()))?
            };
            let mut job_caps = None;
            if let Some(job) = job {
                let job_hash = hex::encode(calimero_identity::sha256(&job));
                let prepared = {
                    use calimero_app_host::Engine as _;
                    self.compute_engine()
                        .prepare(&job_hash, &job)
                        .map_err(|e| {
                            of_service(
                                ApiError::bad_request(crate::node::BAD_BUNDLE, &e.to_string()),
                                &service,
                                true,
                            )
                        })?
                };
                compute_abi_agrees(&main.abi, &prepared.abi).map_err(|why| {
                    of_service(
                        ApiError::bad_request(COMPUTE_ABI_MISMATCH, &why),
                        &service,
                        false,
                    )
                })?;
                job_caps = Some(pipelines::compute_caps(&prepared.abi));
                let _prev = compute.insert(service.clone(), job_hash.clone());
                let _prev = part_sizes.insert(job_hash.clone(), job.len() as u64);
                let _prev = wasm_by_hash.insert(job_hash, job);
            }
            if !files.is_empty() {
                service_pipelines = (files, job_caps);
            }
            let _prev = services.insert(service, hash.clone());
            let _prev = part_sizes.insert(hash.clone(), bytes.len() as u64);
            let _prev = wasm_by_hash.insert(hash, bytes);
        }
        let shared = self.sys(|s| {
            s.packages.iter().any(|(name, p)| {
                *name != package
                    && p.signer_id != signer_id
                    && p.versions.values().any(|v| {
                        v.services
                            .values()
                            .chain(v.compute.values())
                            .any(|h| part_sizes.contains_key(h))
                    })
            })
        });
        if shared {
            return Err(ApiError::conflict(crate::system::SHIPS_IN_OTHER_PACKAGE));
        }
        let pools = self.pool_ceilings();
        let modules = self.resolve_modules(&service_pipelines.0, actor).await;
        let mut pipeline_refs = BTreeMap::new();
        for (compiled, record) in pipelines::register(&service_pipelines, &pools, &modules)? {
            let _prev = wasm_by_hash.insert(compiled.hash, compiled.canonical);
            let _prev = pipeline_refs.insert(compiled.name, record);
        }
        let metadata = manifest
            .metadata
            .as_ref()
            .and_then(|m| serde_json::to_value(m).ok())
            .unwrap_or(Value::Null);
        Ok(Verified {
            package,
            signer_id,
            release: PackageVersion {
                version: manifest.app_version.clone(),
                mpk: mpk_hash,
                mpk_size,
                source,
                services,
                compute,
                pipelines: pipeline_refs,
                part_sizes,
                metadata,
                outbound: manifest.outbound.clone(),
                secrets: manifest.secrets.clone().unwrap_or_default(),
                uses: manifest
                    .uses
                    .iter()
                    .flatten()
                    .map(|(name, u)| (name.clone(), u.into()))
                    .collect(),
                installed_by: actor.to_owned(),
                installed_at: now_secs(),
                installers: BTreeSet::new(),
                upgrades,
                components: if only_compute {
                    Components::ComputeOnly
                } else {
                    Components::App
                },
                min_runtime_version: Some(manifest.min_runtime_version.clone()),
                // Every release says, absent being `either`.
                instance_org: Some(manifest.instance_org.unwrap_or_default()),
                guests: Some(manifest.guests.unwrap_or_default()),
                visibility: None,
                orgs: BTreeSet::new(),
            },
            wasm: wasm_by_hash,
        })
    }

    /// Keep a verified release here (and, for an upload, on a majority of
    /// nodes), then record it.
    async fn record_release(
        &self,
        mut v: Verified,
        upload: Option<Arc<[u8]>>,
    ) -> Result<Value, ApiError> {
        // An older node drops `uses`: they are recorded once every node keeps them, and an
        // install of the same release after that fills them in. The release installs either
        // way, as it did before they were kept.
        if !v.release.uses.is_empty()
            && self
                .require_features(&[crate::system::cluster_version::Feature::DeploymentUsedSecrets])
                .await
                .is_err()
        {
            v.release.uses.clear();
        }
        // Likewise its instance mode: an older node drops it; the API then reads it from
        // the `.mpk` (`api::apps::instance_org_of`).
        if self
            .require_features(&[crate::system::cluster_version::Feature::OrdinaryOrgs])
            .await
            .is_err()
        {
            v.release.instance_org = None;
        }
        // And whether it takes guests: an older node drops it, and apply takes a release
        // without it as forbidding them.
        if self
            .require_features(&[crate::system::cluster_version::Feature::Guests])
            .await
            .is_err()
        {
            v.release.guests = None;
        }
        // And who may use it: an older node drops both, so the release keeps the rule from
        // before; a later install the registry serves to anyone makes it public.
        if self
            .require_features(&[crate::system::cluster_version::Feature::PrivateReleases])
            .await
            .is_err()
        {
            v.release.visibility = None;
            v.release.orgs.clear();
        }
        self.refuse_stalled_install(&v.release.installed_by, &v.signer_id)?;
        // An older node drops `components` and would apply the release as an app's, and drops
        // `pipelines` and would hold none.
        // And drops `secrets`, filling any secret its operator bound to the package.
        if !v.release.components.is_app()
            || !v.release.pipelines.is_empty()
            || !v.release.secrets.is_empty()
        {
            self.require_upgraded_nodes().await?;
        }
        for (hash, bytes) in &v.wasm {
            self.disk()
                .put_blob(hash, bytes)
                .map_err(|e| ApiError::internal(&e.to_string()))?;
        }
        if let Some(mpk) = upload {
            self.store_blob(&mpk, None).await?;
        }
        info!(
            package = %v.package,
            version = %v.release.version,
            signer = %v.signer_id,
            "package release verified"
        );
        let services = v.release.services.clone();
        let version = v.release.version.clone();
        let _ok = self
            .sys_write(&SystemCmd::InstallPackage {
                package: v.package.clone(),
                signer_id: v.signer_id.clone(),
                release: v.release,
            })
            .await?;
        Ok(json!({
            "package": v.package,
            "version": version,
            "signerId": v.signer_id,
            "services": services,
        }))
    }

    /// The release `app` runs: the one its record names
    /// (`AppRecord::release`), else, for a record from before, the release
    /// its bundle came out of ([`Self::release_of`]).
    pub fn release_of_app(&self, app: &crate::types::AppRecord) -> Option<Release> {
        let Some(at) = &app.release else {
            return self.release_of(&app.bundle);
        };
        self.sys(|s| {
            let version = s
                .packages
                .get(&at.package)?
                .versions
                .get(&at.version)?
                .clone();
            Some(Release {
                at: at.clone(),
                version,
            })
        })
    }

    /// The release a bundle hash belongs to, if it came out of a package: the
    /// first recorded, so a guess when releases share the WASM.
    pub fn release_of(&self, hash: &str) -> Option<Release> {
        self.sys(|s| {
            let at = s.bundles.get(hash)?.package.clone()?;
            let version = s
                .packages
                .get(&at.package)?
                .versions
                .get(&at.version)?
                .clone();
            Some(Release { at, version })
        })
    }

    /// A packaged bundle's WASM this node does not hold yet: the release's
    /// `.mpk` from where it lives (the registry, or a peer for an upload),
    /// checked against the hash the log recorded and verified again.
    pub async fn release_wasm(&self, hash: &str, r: &Release) -> Result<Vec<u8>, ApiError> {
        let mpk = self.release_mpk(&r.at.package, &r.version).await?;
        let (_bundle, wasm) = verify(mpk)?;
        let service = r.at.service.clone().unwrap_or_default();
        let bytes = wasm
            .into_iter()
            .find(|s| s.name == service)
            .and_then(|s| if r.at.compute { s.compute } else { s.wasm })
            .ok_or_else(|| {
                ApiError::internal("the release lacks the service it was recorded with")
            })?;
        if hex::encode(calimero_identity::sha256(&bytes)) != hash {
            return Err(ApiError::internal(
                "the release's WASM is not the recorded one",
            ));
        }
        self.disk()
            .put_blob(hash, &bytes)
            .map_err(|e| ApiError::internal(&e.to_string()))?;
        info!(package = %r.at.package, version = %r.at.version, "fetched and verified a release");
        Ok(bytes)
    }

    /// A release's `.mpk` from where it lives (the registry, or this node's
    /// blob store or a peer for an upload), refused unless its hash is the one
    /// the log recorded. Not verified yet: the caller opens it.
    pub async fn release_mpk(
        &self,
        package: &str,
        v: &PackageVersion,
    ) -> Result<Arc<[u8]>, ApiError> {
        let mpk: Arc<[u8]> = match v.source {
            // Public: served to anyone. Private, or recorded before: with the credential,
            // naming no account, pinned by the recorded hash below.
            BundleSource::Registry => {
                let public = v.visibility == Some(Visibility::Public);
                Self::fetch_from(self.registry(!public)?, package, &v.version)
                    .await?
                    .ok_or_else(|| {
                        ApiError::not_found(&format!("the registry has no {package}@{}", v.version))
                    })?
            }
            BundleSource::Upload => match self.engine.get_blob(&v.mpk) {
                Ok(Some(b)) => b.into(),
                _ => self.fetch_blob(&v.mpk).await?.into(),
            },
        };
        if hex::encode(calimero_identity::sha256(&mpk)) != v.mpk {
            warn!(
                %package,
                version = %v.version,
                "the fetched release is not the one the fleet installed"
            );
            return Err(ApiError::unavailable(&format!(
                "{package}@{} as fetched differs from the release the fleet installed",
                v.version
            )));
        }
        Ok(mpk)
    }

    /// The bundle hash `package@version` (service `service`) runs, installing
    /// the release from the registry first, for `org`, if the fleet lacks it. A release
    /// `actor` may not see (`SystemState::sees_release`) counts as lacking:
    /// installing it from the registry makes it theirs too, and one the
    /// registry does not have answers as a missing release does. `via`: how the caller
    /// reaches it besides its account's own rule.
    pub async fn resolve_release(
        &self,
        package: &str,
        version: Option<&str>,
        service: Option<&str>,
        (actor, org): (&str, Option<&str>),
        via: Via,
    ) -> Result<String, ApiError> {
        let (bundle, _compute, _release) = self
            .resolve_release_parts(package, version, service, (actor, org), via)
            .await?;
        Ok(bundle)
    }

    /// [`Self::resolve_release`], with the compute component the same release ships for
    /// the service (`None` when it ships none) and the release itself (the newest
    /// visible one when `version` is `None`).
    pub async fn resolve_release_parts(
        &self,
        package: &str,
        version: Option<&str>,
        service: Option<&str>,
        (actor, org): (&str, Option<&str>),
        via: Via,
    ) -> Result<(String, Option<String>, PackageRef), ApiError> {
        // No version: the latest installed release `actor` sees. A registry
        // install always names its version.
        let version = match version {
            Some(v) => v.to_owned(),
            None => self
                .sys(|s| {
                    let p = s.packages.get(package)?;
                    p.versions
                        .values()
                        .filter(|v| via.sees(s, v, actor))
                        .max_by(|a, b| compare_versions(&a.version, &b.version))
                        .map(|v| v.version.clone())
                })
                .ok_or_else(|| {
                    ApiError::bad_request(
                        "VERSION_REQUIRED",
                        &format!("{package} is not installed; name the version to install"),
                    )
                })?,
        };
        let seen = |s: &SystemState| {
            s.packages
                .get(package)
                .and_then(|p| p.versions.get(&version))
                .filter(|v| via.sees(s, v, actor))
                .map(|v| (v.app_parts().clone(), v.compute.clone()))
        };
        let (services, compute) = match self.sys(seen) {
            Some(s) => s,
            None => {
                let _ = self
                    .install_from_registry(package, &version, actor, org)
                    .await?;
                self.sys(seen).ok_or_else(|| {
                    ApiError::unavailable("the release was installed but is not visible yet")
                })?
            }
        };
        let key = service.unwrap_or_default();
        let release = PackageRef {
            package: package.to_owned(),
            version: version.clone(),
            service: (!key.is_empty()).then(|| key.to_owned()),
            compute: false,
        };
        let found = services
            .get(key)
            .cloned()
            .map(|b| (b, compute.get(key).cloned(), release));
        found.ok_or_else(|| no_such_service(package, &version, key, &services))
    }
}

/// `400 NO_SUCH_SERVICE`: `key` names none of `services`, or is empty and the
/// release has several.
pub(crate) fn no_such_service(
    package: &str,
    version: &str,
    key: &str,
    services: &BTreeMap<String, String>,
) -> ApiError {
    let names: Vec<&String> = services.keys().collect();
    ApiError::bad_request(
        "NO_SUCH_SERVICE",
        &if key.is_empty() {
            format!("{package}@{version} has several services; name one of {names:?}")
        } else {
            format!("{package}@{version} has no service {key}")
        },
    )
}
