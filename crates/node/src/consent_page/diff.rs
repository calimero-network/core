//! The capability sheet an upgrade's consent page shows: what the target
//! release may do that the current one may not, read from the two signed
//! manifests and the ABIs their WASM embeds, never from the app's own page.
//!
//! Every entry is a fact the node checks or enforces elsewhere, so the sheet
//! is what the approver actually lets in:
//!
//! - **outbound prefixes**: what the app may call out to once approved
//!   (`calimero-outbound`);
//! - **frontend origin and aliases**: an added origin is a new place a
//!   sign-in callback is trusted to land (highlighted as `newLoginOrigins`);
//! - **interfaces** the bundle uses and exports;
//! - **methods** (their kind, whether each needs a passkey confirmation, is
//!   public, accepts the system, other apps or only the app's actions, and its
//!   signature), **events** and **timers**, from the `calimero:abi` section each
//!   component embeds ([`crate::component::embedded_abi`]);
//! - the **state version** each side declares (an upgrade is code only: no
//!   migration runs);
//! - SDK and build information, and the hashes that identify each side.

use std::collections::{BTreeMap, BTreeSet};

use calimero_bundle::BundleManifest;
use serde_json::{json, Value};

/// A component's `calimero:abi` JSON, as the build wrote it.
pub type Abi = Value;

/// One side of an upgrade, as the node holds it.
pub struct Side<'a> {
    pub package: &'a str,
    pub version: &'a str,
    /// The service the app runs; `None` for a single-service bundle.
    pub service: Option<&'a str>,
    /// The service WASM's hash: the bundle the app runs.
    pub bundle: &'a str,
    /// `hex(SHA-256)` of the release's `.mpk`.
    pub mpk: &'a str,
    /// `hex` of the signed manifest's hash (`VerifiedBundle::manifest_hash`).
    pub manifest_hash: &'a str,
    /// The publisher's key that signed the manifest (`did:key`).
    pub signer: &'a str,
    pub manifest: &'a BundleManifest,
    /// The ABI the service's component embeds; `None` when it embeds none.
    pub abi: Option<&'a Abi>,
}

/// The sheet for an upgrade `from` -> `to`.
#[must_use]
pub fn capability_sheet(from: &Side<'_>, to: &Side<'_>) -> Value {
    json!({
        "from": side(from),
        "to": side(to),
        "publisher": {
            "signer": to.signer,
            "author": to.manifest.metadata.as_ref().and_then(|m| m.author.clone()),
            "changed": from.signer != to.signer,
        },
        "outbound": set_diff(
            from.manifest.outbound.iter().cloned().collect(),
            to.manifest.outbound.iter().cloned().collect(),
        ),
        "frontend": frontend(from.manifest, to.manifest),
        "interfaces": {
            "uses": set_diff(interfaces(from.manifest, false), interfaces(to.manifest, false)),
            "exports": set_diff(interfaces(from.manifest, true), interfaces(to.manifest, true)),
        },
        "abi": { "from": from.abi.is_some(), "to": to.abi.is_some() },
        "methods": methods(from.abi, to.abi),
        "events": set_diff(events(from.abi), events(to.abi)),
        "timers": timers(from.abi, to.abi),
        "migration": migration(from.abi, to.abi),
        "buildInfo": {
            "from": from.manifest.build_info,
            "to": to.manifest.build_info,
            "changed": serde_json::to_value(&from.manifest.build_info).ok()
                != serde_json::to_value(&to.manifest.build_info).ok(),
        },
        "minRuntimeVersion": {
            "from": from.manifest.min_runtime_version,
            "to": to.manifest.min_runtime_version,
        },
    })
}

fn side(s: &Side<'_>) -> Value {
    json!({
        "package": s.package,
        "version": s.version,
        "service": s.service,
        "bundle": s.bundle,
        "mpk": s.mpk,
        "manifestHash": s.manifest_hash,
        "signer": s.signer,
        "name": s.manifest.metadata.as_ref().map(|m| m.name.clone()),
    })
}

/// `{ added, removed, kept }` from `before` to `after`, each sorted.
fn set_diff(before: BTreeSet<String>, after: BTreeSet<String>) -> Value {
    json!({
        "added": after.difference(&before).collect::<Vec<_>>(),
        "removed": before.difference(&after).collect::<Vec<_>>(),
        "kept": before.intersection(&after).collect::<Vec<_>>(),
    })
}

/// The origin (`scheme://host[:port]`) of a web URL; `None` for anything else.
fn origin(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    if !matches!(u.scheme(), "https" | "http") {
        return None;
    }
    let o = u.origin().ascii_serialization();
    (o != "null").then_some(o)
}

/// Every origin a sign-in callback is trusted to land on for this release:
/// its frontend link's and each alias's.
fn login_origins(m: &BundleManifest) -> BTreeSet<String> {
    m.links
        .as_ref()
        .and_then(|l| l.frontend.as_deref())
        .into_iter()
        .chain(m.frontend_aliases.iter().map(String::as_str))
        .filter_map(origin)
        .collect()
}

fn frontend(from: &BundleManifest, to: &BundleManifest) -> Value {
    let link = |m: &BundleManifest| m.links.as_ref().and_then(|l| l.frontend.clone());
    let aliases = set_diff(
        from.frontend_aliases.iter().cloned().collect(),
        to.frontend_aliases.iter().cloned().collect(),
    );
    let (before, after) = (login_origins(from), login_origins(to));
    json!({
        "from": link(from),
        "to": link(to),
        "changed": link(from) != link(to),
        "aliases": aliases,
        "newLoginOrigins": after.difference(&before).collect::<Vec<_>>(),
        "droppedLoginOrigins": before.difference(&after).collect::<Vec<_>>(),
        "ships": { "from": from.frontend.is_some(), "to": to.frontend.is_some() },
    })
}

fn interfaces(m: &BundleManifest, exports: bool) -> BTreeSet<String> {
    m.interfaces
        .as_ref()
        .map(|i| {
            if exports {
                i.exports.iter().cloned().collect()
            } else {
                i.uses.iter().cloned().collect()
            }
        })
        .unwrap_or_default()
}

/// The methods a component's ABI declares, by (camelCase) name.
fn declared(abi: Option<&Abi>) -> impl Iterator<Item = (&String, &Value)> {
    abi.and_then(|a| a["methods"].as_object())
        .into_iter()
        .flatten()
}

/// What a method is, as far as an approver cares: every attribute the node
/// enforces from the ABI (`calimero_app_host::Abi`), and its signature.
fn method_facts(m: &Value) -> Value {
    let flag = |key: &str| m[key].as_bool().unwrap_or(false);
    json!({
        "kind": m["kind"],
        "confirm": flag("confirm"),
        "public": flag("public"),
        "rate": m["rate"],
        "allowSystem": flag("allowSystem"),
        "allowApps": m.get("allowApps").cloned().unwrap_or_else(|| json!([])),
        "allowAction": flag("allowAction"),
        "params": m.get("params").cloned().unwrap_or_else(|| json!([])),
        "returns": m["returns"],
    })
}

fn method_map(abi: Option<&Abi>) -> BTreeMap<String, Value> {
    declared(abi)
        .map(|(name, m)| (name.clone(), method_facts(m)))
        .collect()
}

/// Methods added (with what they are), removed, and those whose attributes or
/// signature changed. Unknown (`null`) unless both sides embed an ABI.
fn methods(from: Option<&Abi>, to: Option<&Abi>) -> Value {
    if from.is_none() || to.is_none() {
        return Value::Null;
    }
    let (before, after) = (method_map(from), method_map(to));
    let added: Vec<Value> = after
        .iter()
        .filter(|(n, _)| !before.contains_key(*n))
        .map(|(n, f)| with_name(n, f))
        .collect();
    let removed: Vec<&String> = before.keys().filter(|n| !after.contains_key(*n)).collect();
    let changed: Vec<Value> = after
        .iter()
        .filter_map(|(n, f)| {
            let was = before.get(n)?;
            (was != f).then(|| json!({ "name": n, "from": was, "to": f }))
        })
        .collect();
    json!({ "added": added, "removed": removed, "changed": changed })
}

fn with_name(name: &str, facts: &Value) -> Value {
    let mut v = facts.clone();
    if let Some(o) = v.as_object_mut() {
        let _prev = o.insert("name".into(), json!(name));
    }
    v
}

fn events(abi: Option<&Abi>) -> BTreeSet<String> {
    abi.and_then(|a| a["events"].as_object())
        .map(|e| e.keys().cloned().collect())
        .unwrap_or_default()
}

/// A timer's `every` (`"<n>s"`, `"<n>m"` or `"<n>h"`) in seconds.
fn every_secs(every: &str) -> Option<u64> {
    let unit = match every.chars().last()? {
        's' => 1,
        'm' => 60,
        'h' => 3_600,
        _ => return None,
    };
    every[..every.len() - 1]
        .parse::<u64>()
        .ok()?
        .checked_mul(unit)
}

fn timer_map(abi: Option<&Abi>) -> BTreeMap<String, u64> {
    declared(abi)
        .filter(|(_, m)| m["kind"] == "timer")
        .filter_map(|(name, m)| Some((name.clone(), every_secs(m["every"].as_str()?)?)))
        .collect()
}

/// Timers (`#[app::every]`) added, removed, or with a new period.
fn timers(from: Option<&Abi>, to: Option<&Abi>) -> Value {
    let (before, after) = (timer_map(from), timer_map(to));
    let entry = |n: &String, s: &u64| json!({ "name": n, "everySecs": s });
    json!({
        "added": after.iter().filter(|(n, _)| !before.contains_key(*n)).map(|(n, s)| entry(n, s)).collect::<Vec<_>>(),
        "removed": before.keys().filter(|n| !after.contains_key(*n)).collect::<Vec<_>>(),
        "changed": after.iter().filter_map(|(n, s)| {
            let was = before.get(n)?;
            (was != s).then(|| json!({ "name": n, "fromSecs": was, "toSecs": s }))
        }).collect::<Vec<_>>(),
    })
}

/// The state version each side declares (`#[app::state(version = N)]`). An
/// upgrade is code only: no migration runs, whatever the versions.
fn migration(from: Option<&Abi>, to: Option<&Abi>) -> Value {
    let version = |abi: Option<&Abi>| abi.and_then(|a| a["state"]["version"].as_u64());
    json!({
        "fromStateVersion": version(from),
        "toStateVersion": version(to),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(v: Value) -> BundleManifest {
        serde_json::from_value(v).unwrap()
    }

    fn method(kind: &str) -> Value {
        json!({ "kind": kind, "confirm": false, "public": false, "allowSystem": false,
                "params": [], "returns": null })
    }

    fn side<'a>(version: &'a str, manifest: &'a BundleManifest, abi: Option<&'a Abi>) -> Side<'a> {
        Side {
            package: "com.example.notes",
            version,
            service: None,
            bundle: "wasm",
            mpk: "mpk",
            manifest_hash: "hash",
            signer: "did:key:z1",
            manifest,
            abi,
        }
    }

    /// The fixture pair: 1.0.0 calls one host and has one alias; 2.0.0 calls
    /// a second host, moves its frontend, adds an alias, a confirmed method, a
    /// public view, an event and a timer, changes a timer's period, drops a
    /// method, and declares its state at version 2.
    #[test]
    fn the_sheet_lists_what_the_target_adds_and_drops() {
        let from = manifest(json!({
            "version": "1.0", "package": "com.example.notes", "appVersion": "1.0.0",
            "minRuntimeVersion": "0.1.0",
            "metadata": { "name": "Notes", "author": "Example" },
            "interfaces": { "uses": ["calimero:blobs"], "exports": [] },
            "links": { "frontend": "https://notes.example" },
            "frontendAliases": ["https://old.notes.example/app"],
            "outbound": ["https://api.example/v1/"],
            "buildInfo": { "sdkVersion": "0.12.0" },
        }));
        let to = manifest(json!({
            "version": "1.0", "package": "com.example.notes", "appVersion": "2.0.0",
            "minRuntimeVersion": "0.1.0",
            "metadata": { "name": "Notes", "author": "Example" },
            "interfaces": { "uses": ["calimero:blobs", "calimero:search"], "exports": ["notes:share"] },
            "links": { "frontend": "https://app.notes.example" },
            "frontendAliases": ["https://old.notes.example/app", "https://evil-looking.example"],
            "outbound": ["https://api.example/v1/", "https://metrics.example/"],
            "buildInfo": { "sdkVersion": "0.13.0" },
        }));
        let a = json!({
            "abiVersion": 1,
            "state": { "fields": {}, "search": {}, "version": 1 },
            "methods": {
                "init": method("init"),
                "get": method("view"),
                "set": method("call"),
                "old": method("call"),
                "sweep": { "kind": "timer", "every": "30s", "allowSystem": true },
            },
            "events": { "Changed": { "id": "string" } },
        });
        let b = json!({
            "abiVersion": 1,
            "state": { "fields": {}, "search": {}, "version": 2 },
            "methods": {
                "init": method("init"),
                "get": method("view"),
                "set": { "kind": "call", "confirm": true, "allowApps": ["*"], "params": [["id", "string"]] },
                "wipe": { "kind": "call", "confirm": true },
                "count": { "kind": "view", "public": true, "rate": "10/s", "returns": "u32" },
                "tick": { "kind": "timer", "every": "1m", "allowSystem": true },
                "sweep": { "kind": "timer", "every": "1h", "allowSystem": true },
            },
            "events": { "Changed": { "id": "string" }, "Wiped": null },
            "world": "calimero:app@0.5.1",
        });
        let sheet = capability_sheet(
            &side("1.0.0", &from, Some(&a)),
            &side("2.0.0", &to, Some(&b)),
        );
        assert_eq!(
            sheet["outbound"]["added"],
            json!(["https://metrics.example/"])
        );
        assert_eq!(sheet["outbound"]["removed"], json!([]));
        assert_eq!(
            sheet["frontend"]["aliases"]["added"],
            json!(["https://evil-looking.example"])
        );
        assert_eq!(
            sheet["frontend"]["newLoginOrigins"],
            json!(["https://app.notes.example", "https://evil-looking.example"])
        );
        assert_eq!(
            sheet["frontend"]["droppedLoginOrigins"],
            json!(["https://notes.example"])
        );
        assert_eq!(
            sheet["interfaces"]["uses"]["added"],
            json!(["calimero:search"])
        );
        assert_eq!(
            sheet["interfaces"]["exports"]["added"],
            json!(["notes:share"])
        );
        assert_eq!(sheet["abi"], json!({ "from": true, "to": true }));
        let facts = |name: &str,
                     kind: &str,
                     confirm: bool,
                     public: bool,
                     rate: Value,
                     system: bool,
                     returns: Value| {
            json!({
                "name": name, "kind": kind, "confirm": confirm, "public": public, "rate": rate,
                "allowSystem": system, "allowApps": [], "allowAction": false,
                "params": [], "returns": returns,
            })
        };
        assert_eq!(
            sheet["methods"]["added"],
            json!([
                facts(
                    "count",
                    "view",
                    false,
                    true,
                    json!("10/s"),
                    false,
                    json!("u32")
                ),
                facts(
                    "tick",
                    "timer",
                    false,
                    false,
                    Value::Null,
                    true,
                    Value::Null
                ),
                facts("wipe", "call", true, false, Value::Null, false, Value::Null),
            ])
        );
        assert_eq!(sheet["methods"]["removed"], json!(["old"]));
        let changed = &sheet["methods"]["changed"];
        assert_eq!(changed.as_array().unwrap().len(), 1, "{changed}");
        assert_eq!(changed[0]["name"], "set");
        assert_eq!(changed[0]["from"]["confirm"], false);
        assert_eq!(changed[0]["to"]["confirm"], true);
        assert_eq!(changed[0]["to"]["allowApps"], json!(["*"]));
        assert_eq!(changed[0]["to"]["params"], json!([["id", "string"]]));
        assert_eq!(
            sheet["events"],
            json!({ "added": ["Wiped"], "removed": [], "kept": ["Changed"] })
        );
        assert_eq!(
            sheet["timers"],
            json!({
                "added": [{ "name": "tick", "everySecs": 60 }],
                "removed": [],
                "changed": [{ "name": "sweep", "fromSecs": 30, "toSecs": 3600 }],
            })
        );
        assert_eq!(
            sheet["migration"],
            json!({ "fromStateVersion": 1, "toStateVersion": 2 })
        );
        assert_eq!(sheet["buildInfo"]["changed"], true);
        assert_eq!(sheet["publisher"]["changed"], false);
    }

    #[test]
    fn without_an_abi_on_either_side_methods_are_unknown_not_empty() {
        let m = manifest(json!({
            "version": "1.0", "package": "p", "appVersion": "1.0.0", "minRuntimeVersion": "0.1.0",
        }));
        let abi = json!({ "abiVersion": 1, "methods": { "get": method("view") } });
        for (from, to) in [(None, None), (Some(&abi), None), (None, Some(&abi))] {
            let sheet = capability_sheet(&side("1.0.0", &m, from), &side("2.0.0", &m, to));
            assert_eq!(sheet["methods"], Value::Null);
            assert_eq!(
                sheet["abi"],
                json!({ "from": from.is_some(), "to": to.is_some() })
            );
        }
        let sheet = capability_sheet(&side("1.0.0", &m, None), &side("2.0.0", &m, None));
        assert_eq!(
            sheet["migration"],
            json!({ "fromStateVersion": null, "toStateVersion": null })
        );
    }

    #[test]
    fn timer_periods_read_as_seconds() {
        assert_eq!(every_secs("30s"), Some(30));
        assert_eq!(every_secs("2m"), Some(120));
        assert_eq!(every_secs("1h"), Some(3_600));
        for bad in ["", "h", "1d", "-1s", "1.5m", "99999999999999999999h"] {
            assert_eq!(every_secs(bad), None, "{bad}");
        }
    }
}
