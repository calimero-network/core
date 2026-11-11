//! `mero-app-build codegen` and `mero-app-build abi-diff` on ABI files (no guest build).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mero-app-build"))
}

fn kv_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../app-abi/tests/fixtures/kv-store.abi.json")
}

#[test]
fn codegen_writes_the_document_and_a_client() {
    let dir = tempfile::tempdir().unwrap();
    for (lang, client) in [
        ("ts", Some("client.ts")),
        ("python", Some("client.py")),
        ("openapi", None),
    ] {
        let out = dir.path().join(lang);
        let status = bin()
            .args(["codegen", "--abi"])
            .arg(kv_fixture())
            .args(["--lang", lang, "-o"])
            .arg(&out)
            .status()
            .unwrap();
        assert!(status.success(), "{lang}");
        let doc: Value =
            serde_json::from_slice(&fs::read(out.join("openapi.json")).unwrap()).unwrap();
        assert_eq!(doc["openapi"], "3.1.0");
        assert_eq!(
            doc["paths"]["/apps/{app}/methods/set"]["post"]["operationId"],
            "set"
        );
        match client {
            Some(file) => {
                let text = fs::read_to_string(out.join(file)).unwrap();
                // Named after the file: `kv-store.abi.json` → `KvStoreClient`.
                assert!(text.contains("KvStoreClient"), "{lang}: {text}");
            }
            None => assert_eq!(fs::read_dir(&out).unwrap().count(), 1),
        }
    }
}

#[test]
fn codegen_refuses_an_unknown_language() {
    let out = bin()
        .args(["codegen", "--abi"])
        .arg(kv_fixture())
        .args(["--lang", "cobol"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown --lang"));
}

#[test]
fn abi_diff_exits_non_zero_on_a_breaking_change() {
    let dir = tempfile::tempdir().unwrap();
    let old: Value = serde_json::from_slice(&fs::read(kv_fixture()).unwrap()).unwrap();
    let write = |name: &str, v: &Value| {
        let p = dir.path().join(name);
        fs::write(&p, serde_json::to_vec(v).unwrap()).unwrap();
        p
    };
    let old_path = write("old.json", &old);

    let same = bin()
        .arg("abi-diff")
        .arg(&old_path)
        .arg(&old_path)
        .output()
        .unwrap();
    assert!(same.status.success());
    assert_eq!(String::from_utf8_lossy(&same.stdout), "no changes\n");

    let mut added = old.clone();
    added["methods"]["count"] = json!({ "kind": "view", "params": [], "returns": "u32" });
    let added_path = write("added.json", &added);
    let compatible = bin()
        .arg("abi-diff")
        .arg(&old_path)
        .arg(&added_path)
        .output()
        .unwrap();
    assert!(compatible.status.success());
    assert!(String::from_utf8_lossy(&compatible.stdout).contains("method count: added (view)"));

    let mut broken = old.clone();
    broken["methods"]["set"]["kind"] = json!("view");
    let broken_path = write("broken.json", &broken);
    let out = bin()
        .arg("abi-diff")
        .arg(&old_path)
        .arg(&broken_path)
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["breaking"], true);
    assert_eq!(report["changes"][0]["message"], "kind call -> view");
}
