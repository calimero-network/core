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

use calimero_app_downloader::source::http::{HttpRegistry, RegistryStatus};
use calimero_app_downloader::{AppRequest, AppSource};
use calimero_app_host::{Abi, MethodKind};
use calimero_bundle::VerifiedBundle;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::compile::Engine;
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

/// A compile's refusal of `service`'s component ([`Node::compile`]): one of the
/// component itself (`BAD_BUNDLE`, `COMPILE_TIMEOUT`) names the service; one of
/// the node's load (`BUSY`) or size (`TOO_LARGE`) is answered as it is.
fn compile_refusal(e: ApiError, service: &str, compute: bool) -> ApiError {
    if e.code == crate::node::BAD_BUNDLE || e.code == crate::compile::COMPILE_TIMED_OUT {
        of_service(e, service, compute)
    } else {
        e
    }
}

/// Verify a `.mpk`: signature, digests, runtime version, and that it has WASM.
/// Unpacking and hashing a large package takes long enough to stop an async
/// worker, so it runs on the blocking pool.
pub async fn verify(mpk: Arc<[u8]>) -> Result<(VerifiedBundle, Vec<Service>), ApiError> {
    tokio::task::spawn_blocking(move || verify_sync(&mpk))
        .await
        .map_err(|e| ApiError::internal(&e.to_string()))?
}

fn verify_sync(mpk: &Arc<[u8]>) -> Result<(VerifiedBundle, Vec<Service>), ApiError> {
    if mpk.len() > MAX_MPK_BYTES {
        return Err(ApiError::new(
            413,
            "TOO_LARGE",
            &format!("a package is limited to {} MiB", MAX_MPK_BYTES >> 20),
        ));
    }
    let bundle = VerifiedBundle::open(Arc::clone(mpk), &runtime_version())
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
        Self::fetch_raw(registry, package, version)
            .await
            .map_err(unreachable_registry)
    }

    async fn fetch_raw(
        registry: HttpRegistry,
        package: &str,
        version: &str,
    ) -> eyre::Result<Option<Arc<[u8]>>> {
        let req = AppRequest {
            application_id: None,
            package,
            version,
            bytecode_id: None,
            context_id: None,
        };
        registry.fetch(&req).await
    }

    /// `package@version` for an install by `actor`, and whether it is public: served without
    /// the registry's credential, it is; else it is fetched with the credential, naming the
    /// installing account (none for the operator), and is private.
    async fn fetch_for_install(
        &self,
        package: &str,
        version: &str,
        actor: &str,
    ) -> Result<(Arc<[u8]>, Visibility), ApiError> {
        // A registry that asks for a sign-in (401, 403) serves no public release.
        match Self::fetch_raw(self.registry(false)?, package, version).await {
            Ok(Some(mpk)) => return Ok((mpk, Visibility::Public)),
            Ok(None) => {}
            Err(e) if asks_sign_in(&e) => {}
            Err(e) => return Err(unreachable_registry(e)),
        }
        let url = self.registry_url()?;
        if url.username().is_empty() && url.password().is_none() {
            return Err(ApiError::not_found(&format!(
                "the registry has no {package}@{version}"
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
            None => Err(not_for_this_account(package, version)),
        }
    }

    /// The organization `actor` installs for: `named`, which it must be a member of, else
    /// its only one (`None`: it is in none, and a private release is then its installers'
    /// alone). The operator installs for none.
    async fn install_org(
        &self,
        actor: &str,
        named: Option<&str>,
    ) -> Result<Option<String>, ApiError> {
        if actor == OPERATOR {
            return Ok(None);
        }
        let _caught_up = self.sys_fresh().await;
        // Below the feature `record_release` drops the org, so none is asked for.
        if !self.sys(|s| s.cluster_brings(crate::system::cluster_version::Feature::PrivateReleases))
        {
            return Ok(None);
        }
        let Some(org) = named else {
            return crate::api::apps::only_org(self, actor).await;
        };
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
    ) -> Result<PackageInstalled, ApiError> {
        self.refuse_undeployable(actor).await?;
        let org = self.install_org(actor, org).await?;
        // Before the fetch, judged by the key the package is pinned to (none
        // yet: unbound, as for a package the actor may not use); `record_release` judges
        // the verified key again.
        let pinned = self
            .sys(|s| {
                s.package_used(package, actor, false)
                    .map(|p| p.signer_id.clone())
            })
            .unwrap_or_default();
        self.refuse_stalled_install(actor, &pinned)?;
        let (mpk, visibility) = self.fetch_for_install(package, version, actor).await?;
        let verified = self
            .verify_release(mpk, BundleSource::Registry, actor, visibility, org)
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

    /// Install an uploaded `.mpk`, private to `org` (named, else the installer's only one;
    /// in none, to its installers).
    pub async fn install_upload(
        &self,
        mpk: Arc<[u8]>,
        actor: &str,
        org: Option<&str>,
    ) -> Result<PackageInstalled, ApiError> {
        self.refuse_undeployable(actor).await?;
        let org = self.install_org(actor, org).await?;
        let verified = self
            .verify_release(
                Arc::clone(&mpk),
                BundleSource::Upload,
                actor,
                Visibility::Private,
                org,
            )
            .await?;
        self.record_release(verified, Some(mpk)).await
    }

    /// Everything about a release that can be checked before the log records
    /// it, including the signer pin `apply` checks again; installed `visibility` for `org`.
    async fn verify_release(
        &self,
        mpk: Arc<[u8]>,
        source: BundleSource,
        actor: &str,
        visibility: Visibility,
        org: Option<String>,
    ) -> Result<Verified, ApiError> {
        let mpk_hash = hex::encode(calimero_identity::sha256(&mpk));
        let mpk_size = mpk.len() as u64;
        let (bundle, wasm) = verify(mpk).await?;
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
                let (prepared, job) = self
                    .compile(Engine::Compute, &job_hash, job)
                    .await
                    .map_err(|e| compile_refusal(e, &service, true))?;
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
            let (main, bytes) = self
                .compile(Engine::App, &hash, bytes)
                .await
                .map_err(|e| compile_refusal(e, &service, false))?;
            let mut job_caps = None;
            if let Some(job) = job {
                let job_hash = hex::encode(calimero_identity::sha256(&job));
                let (prepared, job) = self
                    .compile(Engine::Compute, &job_hash, job)
                    .await
                    .map_err(|e| compile_refusal(e, &service, true))?;
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
        let orgs: BTreeSet<String> = org.into_iter().collect();
        // Judged on the release as this install leaves it: an install again keeps its pins
        // and adds its organization, or makes it public.
        let kept = self.sys(|s| {
            s.packages
                .get(&package)?
                .versions
                .get(&manifest.app_version)
                .cloned()
        });
        let (after, orgs_after, pinned) = match &kept {
            Some(v) => (
                if visibility == Visibility::Public {
                    Some(visibility)
                } else {
                    v.visibility
                },
                v.orgs.union(&orgs).cloned().collect(),
                v.pipelines
                    .values()
                    .flat_map(|r| r.modules.clone())
                    .collect(),
            ),
            None => (Some(visibility), orgs.clone(), BTreeMap::new()),
        };
        // An install again that would leave the release one its installer may not use (an
        // account in no organization, of a release private to some) is refused, as the
        // registry refuses an account it does not serve.
        if let Some(v) = &kept {
            let mut merged = v.clone();
            merged.visibility = after;
            merged.orgs.clone_from(&orgs_after);
            let _new = merged.installers.insert(actor.to_owned());
            if !self.sys(|s| s.sees_release(&merged, actor)) {
                return Err(not_for_this_account(&package, &manifest.app_version));
            }
        }
        let private_for = (after == Some(Visibility::Private)).then_some(&orgs_after);
        let modules = self
            .resolve_modules(
                &service_pipelines.0,
                actor,
                &signer_id,
                private_for,
                &pinned,
            )
            .await;
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
                visibility: Some(visibility),
                orgs,
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
    ) -> Result<PackageInstalled, ApiError> {
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
        Ok(PackageInstalled {
            package: v.package,
            version,
            signer_id: v.signer_id,
            services,
        })
    }

    /// The release `app` runs ([`SystemState::running_release`]): the one its
    /// record names, else the only release its bundle ships in; none when
    /// several share it.
    pub fn release_of_app(&self, app: &crate::types::AppRecord) -> Option<Release> {
        self.sys(|s| {
            let at = s.running_release(app)?;
            let version = s
                .packages
                .get(&at.package)?
                .versions
                .get(&at.version)?
                .clone();
            Some(Release { at, version })
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
        let (_bundle, wasm) = verify(mpk).await?;
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
            // Public: served to anyone, else (the publisher made it private since) with the
            // credential. Private, or recorded before: with it, naming no account. Pinned by the
            // recorded hash below.
            BundleSource::Registry => {
                let public = v.visibility == Some(Visibility::Public);
                let anonymous = if public {
                    match Self::fetch_raw(self.registry(false)?, package, &v.version).await {
                        Ok(found) => found,
                        Err(e) if asks_sign_in(&e) => None,
                        Err(e) => return Err(unreachable_registry(e)),
                    }
                } else {
                    None
                };
                match anonymous {
                    Some(mpk) => Some(mpk),
                    None => Self::fetch_from(self.registry(true)?, package, &v.version).await?,
                }
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

/// `404`: the registry does not serve `package@version` to this account.
fn not_for_this_account(package: &str, version: &str) -> ApiError {
    ApiError::not_found(&format!(
        "the registry has no {package}@{version} for this account; a private package needs this cluster account linked to its Calimero Cloud developer in the console"
    ))
}

/// A registry's `401` or `403`: it serves nothing without a sign-in.
fn asks_sign_in(e: &eyre::Report) -> bool {
    e.downcast_ref::<RegistryStatus>()
        .is_some_and(|r| matches!(r.status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN))
}

/// `503`: the registry did not serve a fetch.
fn unreachable_registry(e: eyre::Report) -> ApiError {
    ApiError::unavailable(&format!("the registry could not be reached: {e:#}"))
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

/// A release installed: its package, version, pinned signer and services (name to bundle hash).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageInstalled {
    pub package: String,
    pub version: String,
    pub signer_id: String,
    pub services: BTreeMap<String, String>,
}
