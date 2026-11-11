//! A release's pipelines: compiled and checked at install against the package's own compute
//! component, kept as canonical JSON parts of the release.

use std::collections::{BTreeMap, BTreeSet};

use calimero_app_host::{Abi, MethodKind};
use calimero_pipeline::{canonical_json, check, compile, worst_case_fuel, Caps, Env, SpecError};
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::node::Node;
use crate::system::{compare_versions, ModulePin, PipelineRef};

use super::verify;

/// The refusal when a package's pipelines do not compile or do not fit its compute component.
pub const BAD_PIPELINE: &str = "BAD_PIPELINE";

/// The module a pipeline names its own package's compute component by.
pub const SELF_MODULE: &str = "self";

/// One pipeline of a release, compiled.
pub struct Compiled {
    pub name: String,
    pub canonical: Vec<u8>,
    pub hash: String,
}

impl Compiled {
    fn of(source: &[u8]) -> Result<(calimero_pipeline::Pipeline, Self), Vec<SpecError>> {
        let spec_error = |message: String| SpecError {
            path: String::new(),
            code: "NOT_UTF8",
            message,
        };
        let source = std::str::from_utf8(source).map_err(|e| vec![spec_error(e.to_string())])?;
        let pipeline = compile(source)?;
        let canonical = canonical_json(&pipeline);
        let hash = hex::encode(calimero_identity::sha256(&canonical));
        let name = pipeline.name.clone();
        Ok((
            pipeline,
            Self {
                name,
                canonical,
                hash,
            },
        ))
    }
}

/// What a pipeline's `self` module offers: the package's compute methods under their caps.
pub fn compute_caps(abi: &Abi) -> BTreeMap<String, Caps> {
    abi.methods
        .iter()
        .filter_map(|(name, m)| match &m.kind {
            MethodKind::Compute(c) => Some((
                name.clone(),
                Caps {
                    fuel: c.fuel,
                    memory_mib: c.memory_mib,
                    wall_secs: c.wall_secs,
                },
            )),
            _ => None,
        })
        .collect()
}

fn detail(file: &str, path: &str, code: &str, message: &str) -> Value {
    json!({ "file": file, "path": path, "code": code, "message": message })
}

fn refused(details: Vec<Value>) -> ApiError {
    ApiError::bad_request(BAD_PIPELINE, "the package's pipelines are not valid")
        .with_details(Value::Array(details))
}

/// A module another package offers: the release a reference resolved to and its compute methods.
pub struct Module {
    pub pin: ModulePin,
    pub methods: BTreeMap<String, Caps>,
}

/// The modules other than `self` a pipeline names, by reference as written.
fn module_refs(p: &calimero_pipeline::Pipeline) -> BTreeSet<&str> {
    p.module
        .iter()
        .chain(p.steps.values().filter_map(|s| s.module.as_ref()))
        .map(String::as_str)
        .filter(|m| *m != SELF_MODULE)
        .collect()
}

/// One service's pipeline files (path, source) and the compute methods they may name;
/// `None` when the service ships no compute component.
pub type ServicePipelines = (Vec<(String, Vec<u8>)>, Option<BTreeMap<String, Caps>>);

/// Compile and check every pipeline file of the package against its services' compute
/// components and the cluster's `pools` (name to maximum memory in MiB); every problem of
/// every file is listed, and names are unique across the package.
pub fn register(
    services: &[ServicePipelines],
    pools: &BTreeMap<String, u32>,
    available: &BTreeMap<String, Module>,
) -> Result<Vec<(Compiled, PipelineRef)>, ApiError> {
    let (mut details, mut done, mut names) = (Vec::new(), Vec::new(), BTreeSet::new());
    for (files, methods) in services {
        if files.is_empty() {
            continue;
        }
        let Some(methods) = methods else {
            details.push(detail(
                &files[0].0,
                "",
                "NO_COMPUTE",
                "a package with pipelines must ship a compute component",
            ));
            continue;
        };
        let mut env = Env {
            default_module: Some(SELF_MODULE.to_owned()),
            modules: BTreeMap::from([(SELF_MODULE.to_owned(), methods.clone())]),
            pools: pools.clone(),
        };
        env.modules.extend(
            available
                .iter()
                .map(|(name, m)| (name.clone(), m.methods.clone())),
        );
        for (file, source) in files {
            let from = |errs: Vec<SpecError>| {
                errs.into_iter()
                    .map(|e| detail(file, &e.path, e.code, &e.message))
                    .collect::<Vec<_>>()
            };
            let (pipeline, compiled) = match Compiled::of(source) {
                Ok(ok) => ok,
                Err(errs) => {
                    details.extend(from(errs));
                    continue;
                }
            };
            if let Err(errs) = check(&pipeline, &env) {
                details.extend(from(errs));
                continue;
            }
            if !names.insert(compiled.name.clone()) {
                details.push(detail(
                    file,
                    "name",
                    "DUPLICATE_NAME",
                    &format!(
                        "another pipeline of the package is named `{}`",
                        compiled.name
                    ),
                ));
                continue;
            }
            let modules = module_refs(&pipeline)
                .into_iter()
                .filter_map(|name| Some((name.to_owned(), available.get(name)?.pin.clone())))
                .collect();
            let record = PipelineRef {
                hash: compiled.hash.clone(),
                worst_case_fuel: worst_case_fuel(&pipeline, &env).to_string(),
                modules,
            };
            done.push((compiled, record));
        }
    }
    if details.is_empty() {
        Ok(done)
    } else {
        Err(refused(details))
    }
}

impl Node {
    /// The modules the files name that `actor` can use: a package release it sees, as written
    /// (`package` for the newest, `package@version` for one), shipping one compute component.
    /// A reference that resolves to none is left out, so `check` answers `UNKNOWN_MODULE`.
    pub async fn resolve_modules(
        &self,
        services: &[ServicePipelines],
        actor: &str,
    ) -> BTreeMap<String, Module> {
        let refs: BTreeSet<String> = services
            .iter()
            .flat_map(|(files, _)| files)
            .filter_map(|(_, source)| Compiled::of(source).ok())
            .flat_map(|(p, _)| {
                module_refs(&p)
                    .into_iter()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect();
        let mut found = BTreeMap::new();
        for reference in refs {
            if let Some(module) = self.resolve_module(&reference, actor).await {
                let _prev = found.insert(reference, module);
            }
        }
        found
    }

    async fn resolve_module(&self, reference: &str, actor: &str) -> Option<Module> {
        let (package, wanted) = match reference.split_once('@') {
            Some((p, v)) => (p, Some(v)),
            None => (reference, None),
        };
        let (version, compute) = self.sys(|s| {
            let record = s.packages.get(package)?;
            let release = match wanted {
                Some(v) => record.versions.get(v).filter(|r| s.sees_release(r, actor)),
                None => s
                    .visible_versions(record, actor)
                    .max_by(|a, b| compare_versions(&a.version, &b.version)),
            }?;
            Some((release.version.clone(), release.compute.get("").cloned()?))
        })?;
        let methods = self.caps_of_component(&compute).await.ok()?;
        Some(Module {
            pin: ModulePin {
                package: package.to_owned(),
                version,
                compute,
            },
            methods: (*methods).clone(),
        })
    }

    /// A registered pipeline's canonical JSON by its hash: from this node's blob store, else
    /// recompiled from the release that registered it, fetched and verified again.
    pub async fn pipeline_json(&self, hash: &str) -> Result<Vec<u8>, ApiError> {
        let stored = self
            .engine
            .get_blob(hash)
            .map_err(|e| ApiError::internal(&e.to_string()))?;
        if let Some(bytes) = stored.filter(|b| hex::encode(calimero_identity::sha256(b)) == hash) {
            return Ok(bytes);
        }
        let release = self
            .sys(|s| {
                s.packages.iter().find_map(|(package, p)| {
                    let v = p
                        .versions
                        .values()
                        .find(|v| v.pipelines.values().any(|r| r.hash == hash))?;
                    Some((package.clone(), v.clone()))
                })
            })
            .ok_or_else(|| ApiError::not_found("no release registers that pipeline"))?;
        let mpk = self.release_mpk(&release.0, &release.1).await?;
        let (_bundle, services) = verify(mpk)?;
        let found = services
            .iter()
            .flat_map(|s| &s.pipelines)
            .filter_map(|(_, source)| Compiled::of(source).ok())
            .find(|(_, c)| c.hash == hash)
            .ok_or_else(|| ApiError::internal("the release lacks the pipeline it registered"))?;
        self.engine
            .put_blob(hash, &found.1.canonical)
            .map_err(|e| ApiError::internal(&e.to_string()))?;
        Ok(found.1.canonical)
    }

    /// The pipelines `version` of `package` registers, each with its canonical JSON, for a
    /// caller who sees the release.
    pub async fn release_pipelines(
        &self,
        refs: &BTreeMap<String, PipelineRef>,
    ) -> Result<Value, ApiError> {
        let mut out = serde_json::Map::new();
        for (name, r) in refs {
            let bytes = self.pipeline_json(&r.hash).await?;
            let pipeline: Value =
                serde_json::from_slice(&bytes).map_err(|e| ApiError::internal(&e.to_string()))?;
            let _prev = out.insert(
                name.clone(),
                json!({ "hash": r.hash, "worstCaseFuel": r.worst_case_fuel, "modules": r.modules, "pipeline": pipeline }),
            );
        }
        Ok(Value::Object(out))
    }
}
