//! A release's pipelines: compiled and checked at install against the package's own compute
//! component, kept as canonical JSON parts of the release.

use std::collections::{BTreeMap, BTreeSet};

use calimero_app_host::{Abi, MethodKind};
use calimero_pipeline::{canonical_json, check, compile, worst_case_fuel, Caps, Env, SpecError};
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::node::Node;
use crate::system::PipelineRef;

use super::verify;

/// The refusal when a package's pipelines do not compile or do not fit its compute component.
pub const BAD_PIPELINE: &str = "BAD_PIPELINE";

/// The module a pipeline names its own package's compute component by.
const SELF_MODULE: &str = "self";

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

/// One service's pipeline files (path, source) and the compute methods they may name;
/// `None` when the service ships no compute component.
pub type ServicePipelines = (Vec<(String, Vec<u8>)>, Option<BTreeMap<String, Caps>>);

/// Compile and check every pipeline file of the package against its services' compute
/// components and the cluster's `pools` (name to maximum memory in MiB); every problem of
/// every file is listed, and names are unique across the package.
pub fn register(
    services: &[ServicePipelines],
    pools: &BTreeMap<String, u32>,
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
        let env = Env {
            default_module: Some(SELF_MODULE.to_owned()),
            modules: BTreeMap::from([(SELF_MODULE.to_owned(), methods.clone())]),
            pools: pools.clone(),
        };
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
            let record = PipelineRef {
                hash: compiled.hash.clone(),
                worst_case_fuel: worst_case_fuel(&pipeline, &env).to_string(),
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
                json!({ "hash": r.hash, "worstCaseFuel": r.worst_case_fuel, "pipeline": pipeline }),
            );
        }
        Ok(Value::Object(out))
    }
}
