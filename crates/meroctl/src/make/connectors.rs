//! `[package.metadata.calimero] connectors = [NAME]`: `app pack` merges each listed
//! dependency's `[package.metadata.calimero.connector]` (`outbound`, `secrets`) into the
//! app's own, so the signed manifest carries them as if the app had written them.
//! Spec: [Connectors](https://calimero-network.github.io/core/build/packaging-signing/#connectors)

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use calimero_app_build::TARGET;
use eyre::{bail, eyre, WrapErr};
use serde_json::{Map, Value};

use super::CrateMeta;

const CONNECTOR_KEYS: &[&str] = &["outbound", "secrets"]; // what a connector table declares
const THE_APP: &str = "the app"; // who declared a secret the app's own table binds

/// Adds what each connector `meta` lists declares to `outbound` and `secrets` (the app's
/// own), printing each prefix and binding it adds and the crate it came from.
pub(super) fn merge(
    meta: &CrateMeta,
    outbound: &mut Vec<String>,
    secrets: &mut Map<String, Value>,
) -> eyre::Result<()> {
    let names = listed(meta)?;
    if names.is_empty() {
        return Ok(());
    }
    let dependencies = dependencies(&meta.dir)?;
    let mut declared_by: BTreeMap<String, &str> =
        secrets.keys().map(|k| (k.clone(), THE_APP)).collect();
    for name in &names {
        let Some(metadata) = dependencies.get(name) else {
            bail!("connectors: {name} is not a dependency of the app (its [dependencies], for {TARGET})");
        };
        let Some(connector) = metadata
            .pointer("/calimero/connector")
            .and_then(Value::as_object)
        else {
            bail!("connectors: {name} declares no [package.metadata.calimero.connector]");
        };
        if let Some(key) = connector
            .keys()
            .find(|k| !CONNECTOR_KEYS.contains(&k.as_str()))
        {
            bail!("connectors: {name}'s connector declares {key}; it takes outbound and secrets");
        }
        for url in strings(connector.get("outbound"))
            .ok_or_else(|| eyre!("connectors: {name}'s outbound is a list of prefixes"))?
        {
            if !outbound.contains(&url) {
                eprintln!("{name}: outbound {url}");
                outbound.push(url);
            }
        }
        let theirs = match connector.get("secrets") {
            None => Map::new(),
            Some(Value::Object(s)) => s.clone(),
            Some(_) => bail!("connectors: {name}'s secrets is a table of NAME = binding"),
        };
        for (secret, binding) in theirs {
            match secrets.get(&secret) {
                Some(ours) if *ours == binding => {}
                Some(_) => bail!(
                    "connectors: secret {secret}: {name} binds it differently from {}",
                    declared_by[&secret]
                ),
                None => {
                    eprintln!("{name}: secret {secret} {binding}");
                    let _ = declared_by.insert(secret.clone(), name);
                    let _ = secrets.insert(secret, binding);
                }
            }
        }
    }
    Ok(())
}

/// The crate names `connectors` lists.
fn listed(meta: &CrateMeta) -> eyre::Result<Vec<String>> {
    match meta.table.get("connectors") {
        None => Ok(Vec::new()),
        Some(list) => list
            .as_array()
            .and_then(|l| l.iter().map(|n| n.as_str().map(str::to_owned)).collect())
            .ok_or_else(|| eyre!("connectors is a list of crate names")),
    }
}

fn strings(list: Option<&Value>) -> Option<Vec<String>> {
    let Some(list) = list else {
        return Some(Vec::new());
    };
    list.as_array()?
        .iter()
        .map(|v| v.as_str().map(str::to_owned))
        .collect()
}

/// Each normal dependency of the crate at `dir` (of every member, for a virtual workspace)
/// that it builds with for the app's target, by package name, with its `[package.metadata]`.
fn dependencies(dir: &Path) -> eyre::Result<BTreeMap<String, Value>> {
    let manifest = dir.join("Cargo.toml");
    let out = Command::new("cargo")
        .args([
            "metadata",
            "--format-version",
            "1",
            "--filter-platform",
            TARGET,
        ])
        .arg("--manifest-path")
        .arg(&manifest)
        .output()
        .wrap_err("running cargo metadata")?;
    if !out.status.success() {
        bail!(
            "cargo metadata for {}: {}",
            manifest.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let doc: Value = serde_json::from_slice(&out.stdout).wrap_err("reading cargo metadata")?;
    let packages: BTreeMap<&str, &Value> = doc["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| Some((p["id"].as_str()?, p)))
        .collect();
    let manifest = canonical(&manifest);
    let mut apps: Vec<&str> = packages
        .iter()
        .filter(|(_, p)| {
            p["manifest_path"].as_str().map(|m| canonical(Path::new(m))) == Some(manifest.clone())
        })
        .map(|(id, _)| *id)
        .collect();
    if apps.is_empty() {
        apps = doc["workspace_members"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
    }
    let mut dependencies = BTreeMap::new();
    for node in doc["resolve"]["nodes"].as_array().into_iter().flatten() {
        if !node["id"].as_str().is_some_and(|id| apps.contains(&id)) {
            continue;
        }
        for dep in node["deps"].as_array().into_iter().flatten() {
            let normal = dep["dep_kinds"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|k| k["kind"].is_null());
            let Some(package) = dep["pkg"].as_str().and_then(|id| packages.get(id)) else {
                continue;
            };
            if let (true, Some(name)) = (normal, package["name"].as_str()) {
                let _ = dependencies.insert(name.to_owned(), package["metadata"].clone());
            }
        }
    }
    Ok(dependencies)
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}
