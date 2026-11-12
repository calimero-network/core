//! Golden files: each `tests/fixtures/<app>.abi.json` (the `calimero:abi` of an example app
//! under `apps/`, or a crafted one covering every type) against its OpenAPI document and
//! typed clients (TypeScript, Python, Swift, Kotlin) in `tests/golden/`.
//! `UPDATE_GOLDEN=1 cargo test -p calimero-app-abi` rewrites them; review the diff.

use std::fs;
use std::path::{Path, PathBuf};

use calimero_app_abi::codegen::{kotlin, python, swift, typescript, Target};
use calimero_app_abi::openapi::{document, Info};
use calimero_app_abi::AppAbi;
use serde_json::Value;

const APPS: &[&str] = &["kv-store", "rooms", "permissions-showcase", "kitchen-sink"];

fn dir(sub: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(sub)
}

fn check(path: &Path, actual: &str, stale: &mut Vec<String>) {
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        fs::write(path, actual).unwrap();
        return;
    }
    let expected = fs::read_to_string(path).unwrap_or_default();
    if expected != actual {
        stale.push(path.display().to_string());
    }
}

#[test]
fn generated_documents_and_clients_match_the_golden_files() {
    let mut stale = Vec::new();
    for app in APPS {
        let json: Value = serde_json::from_slice(
            &fs::read(dir("fixtures").join(format!("{app}.abi.json"))).unwrap(),
        )
        .unwrap();
        let abi = AppAbi::from_json(&json).unwrap();
        let doc = document(
            &abi,
            &Info {
                title: (*app).to_owned(),
                version: "1.0.0".to_owned(),
                app: None,
            },
        );
        let doc = format!("{}\n", serde_json::to_string_pretty(&doc).unwrap());
        // The keys are sorted whatever serde_json's features (`openapi::sorted`).
        assert!(
            doc.find("\"components\"") < doc.find("\"info\""),
            "{app}: keys not sorted"
        );
        let golden = dir("golden");
        check(
            &golden.join(format!("{app}.openapi.json")),
            &doc,
            &mut stale,
        );
        let target = Target { name: app };
        check(
            &golden.join(format!("{app}.ts")),
            &typescript(&abi, &target),
            &mut stale,
        );
        check(
            &golden.join(format!("{app}.py")),
            &python(&abi, &target),
            &mut stale,
        );
        check(
            &golden.join(format!("{app}.swift")),
            &swift(&abi, &target),
            &mut stale,
        );
        check(
            &golden.join(format!("{app}.kt")),
            &kotlin(&abi, &target, None),
            &mut stale,
        );
    }
    assert!(
        stale.is_empty(),
        "generated output differs from {stale:?}; rerun with UPDATE_GOLDEN=1 and review the diff"
    );
}

/// Every `$ref` in a document names a component it defines.
#[test]
fn every_reference_resolves() {
    fn walk(v: &Value, doc: &Value, missing: &mut Vec<String>) {
        match v {
            Value::Object(map) => {
                if let Some(Value::String(r)) = map.get("$ref") {
                    let pointer = r.trim_start_matches('#');
                    if doc.pointer(pointer).is_none() {
                        missing.push(r.clone());
                    }
                }
                map.values().for_each(|v| walk(v, doc, missing));
            }
            Value::Array(items) => items.iter().for_each(|v| walk(v, doc, missing)),
            _ => {}
        }
    }
    for app in APPS {
        let json: Value = serde_json::from_slice(
            &fs::read(dir("fixtures").join(format!("{app}.abi.json"))).unwrap(),
        )
        .unwrap();
        let abi = AppAbi::from_json(&json).unwrap();
        let doc = document(
            &abi,
            &Info {
                title: (*app).to_owned(),
                version: "1".to_owned(),
                app: Some("app-1".to_owned()),
            },
        );
        let mut missing = Vec::new();
        walk(&doc, &doc, &mut missing);
        assert!(missing.is_empty(), "{app}: unresolved {missing:?}");
        // One operation per client method, named after it.
        let ops: Vec<&str> = doc["paths"]
            .as_object()
            .unwrap()
            .values()
            .filter_map(|p| p.get("post"))
            .map(|op| op["operationId"].as_str().unwrap())
            .collect();
        let methods: Vec<&str> = abi.client_methods().map(|(n, _)| n.as_str()).collect();
        assert_eq!(ops, methods, "{app}");
    }
}
