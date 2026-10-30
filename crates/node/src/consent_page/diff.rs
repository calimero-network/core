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
//! - **public methods** (with whether each is a view, an xcall entry point,
//!   an event handler, destructive), **events** and **timers**;
//! - the **migration** the leader would run (`services::plan_from_abis`);
//! - SDK and build information, and the hashes that identify each side.

use std::collections::{BTreeMap, BTreeSet};

use calimero_bundle::BundleManifest;
use calimero_wasm_abi::schema::{Manifest as Abi, MethodIntent};
use serde_json::{json, Value};

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
    /// The ABI the service's WASM embeds; `None` when it embeds none.
    pub abi: Option<&'a Abi>,
}

/// The sheet for an upgrade `from` -> `to`, with the migration `plan` the
/// leader would run (`Ok(None)`: code only; `Err`: refused, and why).
#[must_use]
pub fn capability_sheet(
    from: &Side<'_>,
    to: &Side<'_>,
    plan: &Result<Option<String>, String>,
) -> Value {
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
        "migration": migration(from.abi, to.abi, plan),
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

/// What a method is, as far as an approver cares.
fn method_facts(m: &calimero_wasm_abi::schema::Method) -> Value {
    json!({
        "kind": if m.intent == MethodIntent::ReadOnly { "view" } else { "call" },
        "xcall": m.xcall_callable,
        "handler": m.handler,
        "destructive": m.destructive,
    })
}

fn method_map(abi: Option<&Abi>) -> BTreeMap<String, Value> {
    abi.map(|a| {
        a.methods
            .iter()
            .map(|m| (m.name.clone(), method_facts(m)))
            .collect()
    })
    .unwrap_or_default()
}

/// Methods added (with what they are), removed, and those whose kind or
/// flags changed. Unknown (`null`) unless both sides embed an ABI.
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
    abi.map(|a| a.events.iter().map(|e| e.name.clone()).collect())
        .unwrap_or_default()
}

fn timer_map(abi: Option<&Abi>) -> BTreeMap<String, u64> {
    abi.map(|a| {
        a.methods
            .iter()
            .filter_map(|m| Some((m.name.clone(), m.tee_every_secs.filter(|s| *s > 0)?)))
            .collect()
    })
    .unwrap_or_default()
}

/// Timers (`#[app::tee(every)]`) added, removed, or with a new period.
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

fn migration(from: Option<&Abi>, to: Option<&Abi>, plan: &Result<Option<String>, String>) -> Value {
    json!({
        "runs": matches!(plan, Ok(Some(_))),
        "export": plan.as_ref().ok().cloned().flatten(),
        "refused": plan.as_ref().err(),
        "fromStateVersion": from.map(Abi::state_version_or_default),
        "toStateVersion": to.map(Abi::state_version_or_default),
    })
}

#[cfg(test)]
mod tests {
    use calimero_wasm_abi::schema::{Event, Method};

    use super::*;

    fn manifest(v: Value) -> BundleManifest {
        serde_json::from_value(v).unwrap()
    }

    fn method(name: &str, view: bool) -> Method {
        Method {
            name: name.to_owned(),
            intent: if view {
                MethodIntent::ReadOnly
            } else {
                MethodIntent::Mutating
            },
            ..Method::default()
        }
    }

    fn event(name: &str) -> Event {
        Event {
            name: name.to_owned(),
            payload: None,
            doc: None,
        }
    }

    /// The fixture pair: 1.0.0 calls one host and has one alias; 2.0.0 calls
    /// a second host, moves its frontend, adds an alias, a destructive method,
    /// an event and a timer, and migrates its state from version 1.
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
        let mut a = Abi::new();
        a.methods = vec![
            method("get", true),
            method("set", false),
            method("old", false),
        ];
        a.events = vec![event("Changed")];
        let mut b = Abi::new();
        b.state_version = Some(2);
        b.methods = vec![
            method("get", true),
            Method {
                destructive: true,
                ..method("set", false)
            },
            Method {
                destructive: true,
                ..method("wipe", false)
            },
            Method {
                tee_every_secs: Some(60),
                ..method("tick", false)
            },
        ];
        b.events = vec![event("Changed"), event("Wiped")];
        fn side<'a>(
            version: &'a str,
            manifest: &'a BundleManifest,
            abi: Option<&'a Abi>,
        ) -> Side<'a> {
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
        let sheet = capability_sheet(
            &side("1.0.0", &from, Some(&a)),
            &side("2.0.0", &to, Some(&b)),
            &Ok(Some("migrate_from_1".to_owned())),
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
        assert_eq!(
            sheet["methods"]["added"],
            json!([
                { "name": "tick", "kind": "call", "xcall": false, "handler": false, "destructive": false },
                { "name": "wipe", "kind": "call", "xcall": false, "handler": false, "destructive": true },
            ])
        );
        assert_eq!(sheet["methods"]["removed"], json!(["old"]));
        assert_eq!(sheet["methods"]["changed"][0]["name"], "set");
        assert_eq!(sheet["methods"]["changed"][0]["to"]["destructive"], true);
        assert_eq!(sheet["events"]["added"], json!(["Wiped"]));
        assert_eq!(
            sheet["timers"]["added"],
            json!([{ "name": "tick", "everySecs": 60 }])
        );
        assert_eq!(sheet["migration"]["runs"], true);
        assert_eq!(sheet["migration"]["export"], "migrate_from_1");
        assert_eq!(sheet["migration"]["toStateVersion"], 2);
        assert_eq!(sheet["buildInfo"]["changed"], true);
        assert_eq!(sheet["publisher"]["changed"], false);
    }

    #[test]
    fn without_an_abi_on_either_side_methods_are_unknown_not_empty() {
        let m = manifest(json!({
            "version": "1.0", "package": "p", "appVersion": "1.0.0", "minRuntimeVersion": "0.1.0",
        }));
        let s = Side {
            package: "p",
            version: "1.0.0",
            service: None,
            bundle: "w",
            mpk: "m",
            manifest_hash: "h",
            signer: "s",
            manifest: &m,
            abi: None,
        };
        let sheet = capability_sheet(&s, &s, &Ok(None));
        assert_eq!(sheet["methods"], Value::Null);
        assert_eq!(sheet["abi"], json!({ "from": false, "to": false }));
        assert_eq!(sheet["migration"]["runs"], false);
    }
}
