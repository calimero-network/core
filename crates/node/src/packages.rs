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
    compare_versions, BundleSource, PackageRef, PackageVersion, SystemCmd, SystemState,
};
use crate::types::{now_secs, Components};

pub(crate) mod pipelines;

/// The largest `.mpk` a node takes, uploaded or fetched: it is kept as a blob,
/// so the blob limit (v1 allowed 512 MiB; a blob here must reach a
/// majority inside one sealed request).
pub const MAX_MPK_BYTES: usize = crate::node::MAX_BLOB_BYTES;

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
    fn registry(&self) -> Result<HttpRegistry, ApiError> {
        let base = self.config.registry.as_ref().ok_or_else(|| {
            ApiError::bad_request(
                "NO_REGISTRY",
                "this node has no registry configured (--registry-url)",
            )
        })?;
        let url = base
            .parse()
            .map_err(|e| ApiError::internal(&format!("invalid registry URL: {e}")))?;
        HttpRegistry::new(url).map_err(|e| ApiError::internal(&format!("{e:#}")))
    }

    /// `package@version`'s `.mpk` from the registry, unverified.
    async fn fetch_from_registry(
        &self,
        package: &str,
        version: &str,
    ) -> Result<Arc<[u8]>, ApiError> {
        let req = AppRequest {
            application_id: None,
            package,
            version,
            bytecode_id: None,
            context_id: None,
        };
        match self.registry()?.fetch(&req).await {
            Ok(Some(bytes)) => Ok(bytes),
            Ok(None) => Err(ApiError::not_found(&format!(
                "the registry has no {package}@{version}"
            ))),
            Err(e) => Err(ApiError::unavailable(&format!(
                "the registry could not be reached: {e:#}"
            ))),
        }
    }

    /// Install `package@version` from the registry.
    pub async fn install_from_registry(
        &self,
        package: &str,
        version: &str,
        actor: &str,
    ) -> Result<Value, ApiError> {
        self.refuse_undeployable(actor).await?;
        // Before the fetch, judged by the key the package is pinned to (none
        // yet: unbound); `record_release` judges the verified key again.
        let pinned = self
            .sys(|s| s.packages.get(package).map(|p| p.signer_id.clone()))
            .unwrap_or_default();
        self.refuse_stalled_install(actor, &pinned)?;
        let mpk = self.fetch_from_registry(package, version).await?;
        let verified = self
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

    /// Install an uploaded `.mpk`.
    pub async fn install_upload(&self, mpk: Arc<[u8]>, actor: &str) -> Result<Value, ApiError> {
        self.refuse_undeployable(actor).await?;
        let verified = self
            .verify_release(Arc::clone(&mpk), BundleSource::Upload, actor)
            .await?;
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
        let manifest = bundle.manifest();
        let package = manifest.package.clone();
        let signer_id = bundle.signer_id().to_owned();
        if let Some(pinned) = self.sys(|s| s.packages.get(&package).map(|p| p.signer_id.clone())) {
            if pinned != signer_id {
                return Err(ApiError::forbidden(&format!(
                    "{package} is published under {pinned}; this release is signed by {signer_id}"
                )));
            }
        }
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
        let mut service_pipelines = Vec::new();
        for Service {
            name: service,
            wasm: bytes,
            compute: job,
            pipelines: files,
        } in wasm
        {
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
                service_pipelines.push((files, Some(pipelines::compute_caps(&prepared.abi))));
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
                return Err(ApiError::unsupported_bundle()
                    .with_details(serde_json::json!({ "service": service })));
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
                            ApiError::bad_request(crate::node::BAD_BUNDLE, &e.to_string())
                                .with_details(
                                    serde_json::json!({ "service": service, "compute": true }),
                                )
                        })?
                };
                compute_abi_agrees(&main.abi, &prepared.abi).map_err(|why| {
                    ApiError::bad_request(COMPUTE_ABI_MISMATCH, &why)
                        .with_details(serde_json::json!({ "service": service }))
                })?;
                job_caps = Some(pipelines::compute_caps(&prepared.abi));
                let _prev = compute.insert(service.clone(), job_hash.clone());
                let _prev = part_sizes.insert(job_hash.clone(), job.len() as u64);
                let _prev = wasm_by_hash.insert(job_hash, job);
            }
            service_pipelines.push((files, job_caps));
            let _prev = services.insert(service, hash.clone());
            let _prev = part_sizes.insert(hash.clone(), bytes.len() as u64);
            let _prev = wasm_by_hash.insert(hash, bytes);
        }
        let pools = self.pool_ceilings();
        let modules = self.resolve_modules(&service_pipelines, actor).await;
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
                installed_by: actor.to_owned(),
                installed_at: now_secs(),
                installers: BTreeSet::new(),
                upgrades,
                components: if only_compute {
                    Components::ComputeOnly
                } else {
                    Components::App
                },
            },
            wasm: wasm_by_hash,
        })
    }

    /// Keep a verified release here (and, for an upload, on a majority of
    /// nodes), then record it.
    async fn record_release(
        &self,
        v: Verified,
        upload: Option<Arc<[u8]>>,
    ) -> Result<Value, ApiError> {
        self.refuse_stalled_install(&v.release.installed_by, &v.signer_id)?;
        // An older node drops `components` and would apply the release as an app's, and drops
        // `pipelines` and would hold none.
        if !v.release.components.is_app() || !v.release.pipelines.is_empty() {
            self.require_upgraded_nodes().await?;
        }
        for (hash, bytes) in &v.wasm {
            self.engine
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
        self.engine
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
            BundleSource::Registry => self.fetch_from_registry(package, &v.version).await?,
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
    /// the release from the registry first if the fleet lacks it. A release
    /// `actor` may not see (`SystemState::sees_release`) counts as lacking:
    /// installing it from the registry makes it theirs too, and one the
    /// registry does not have answers as a missing release does. `frontend`:
    /// the caller came through `package`'s own frontend (a session bound to
    /// it), which is public by name, so every release of it is theirs to run.
    pub async fn resolve_release(
        &self,
        package: &str,
        version: Option<&str>,
        service: Option<&str>,
        actor: &str,
        frontend: bool,
    ) -> Result<String, ApiError> {
        let (bundle, _compute, _release) = self
            .resolve_release_parts(package, version, service, actor, frontend)
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
        actor: &str,
        frontend: bool,
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
                        .filter(|v| frontend || s.sees_release(v, actor))
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
                .filter(|v| frontend || s.sees_release(v, actor))
                .map(|v| (v.app_parts().clone(), v.compute.clone()))
        };
        let (services, compute) = match self.sys(seen) {
            Some(s) => s,
            None => {
                let _ = self.install_from_registry(package, &version, actor).await?;
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
        found.ok_or_else(|| {
            let names: Vec<&String> = services.keys().collect();
            ApiError::bad_request(
                "NO_SUCH_SERVICE",
                &if key.is_empty() {
                    format!("{package}@{version} has several services; name one of {names:?}")
                } else {
                    format!("{package}@{version} has no service {key}")
                },
            )
        })
    }
}
