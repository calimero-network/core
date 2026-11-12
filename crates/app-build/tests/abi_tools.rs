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
        ("swift", Some("KvStoreClient.swift")),
        ("kotlin", Some("KvStoreClient.kt")),
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

/// A wasm module holding only the ABI custom section.
fn wasm_with_abi(abi: &Value) -> Vec<u8> {
    let name = b"calimero:abi";
    let mut section = vec![u8::try_from(name.len()).unwrap()];
    section.extend_from_slice(name);
    section.extend_from_slice(&serde_json::to_vec(abi).unwrap());
    let mut out = b"\0asm\x01\0\0\0".to_vec();
    out.push(0);
    let mut len = section.len();
    loop {
        let byte = u8::try_from(len & 0x7f).unwrap();
        len >>= 7;
        if len == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
    out.extend_from_slice(&section);
    out
}

#[test]
fn a_multi_service_bundle_gets_a_directory_and_a_kotlin_package_per_service() {
    let dir = tempfile::tempdir().unwrap();
    let abi = json!({ "abiVersion": 1, "methods": { "len": { "kind": "view", "params": [], "returns": "u64" } } });
    let wasm = wasm_with_abi(&abi);
    let mut services = Vec::new();
    for name in ["room-directory", "room"] {
        fs::write(dir.path().join(format!("{name}.wasm")), &wasm).unwrap();
        services.push(json!({
            "name": name,
            "wasm": { "path": format!("{name}.wasm"), "hash": "00", "size": wasm.len() },
        }));
    }
    let manifest = json!({
        "version": "1.0", "package": "com.example.rooms", "appVersion": "1.0.0",
        "minRuntimeVersion": "0.0.1", "services": services,
    });
    let manifest_path = dir.path().join("manifest.json");
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let out = dir.path().join("out");
    let status = bin()
        .args(["codegen", "--abi"])
        .arg(&manifest_path)
        .args(["--lang", "kotlin", "--package", "com.example.rooms", "-o"])
        .arg(&out)
        .status()
        .unwrap();
    assert!(status.success());
    let directory = fs::read_to_string(out.join("room-directory/RoomDirectoryClient.kt")).unwrap();
    assert!(directory.contains("\npackage com.example.rooms.roomdirectory\n"));
    let room = fs::read_to_string(out.join("room/RoomClient.kt")).unwrap();
    assert!(room.contains("\npackage com.example.rooms.room\n"));
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

#[test]
fn abi_diff_compares_record_and_enum_shapes() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../app-abi/tests/fixtures/kitchen-sink.abi.json");
    let old: Value = serde_json::from_slice(&fs::read(fixture).unwrap()).unwrap();
    let write = |name: &str, v: &Value| {
        let p = dir.path().join(name);
        fs::write(&p, serde_json::to_vec(v).unwrap()).unwrap();
        p
    };
    let old_path = write("old.json", &old);
    let run = |new: &Value| {
        bin()
            .arg("abi-diff")
            .arg(&old_path)
            .arg(write("new.json", new))
            .output()
            .unwrap()
    };

    // `Item` is a parameter of `everything`: a field callers may leave out is compatible.
    let mut optional = old.clone();
    optional["types"]["Item"]["fields"]
        .as_array_mut()
        .unwrap()
        .push(json!(["rank", "option<u32>"]));
    let out = run(&optional);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout)
        .contains("compatible type Item: optional field `rank` added (option<u32>)"));

    // `Status` sits inside `Item`, which `lookup` returns: a new variant breaks readers.
    let mut variant = old;
    variant["types"]["Status"]["variants"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "name": "Archived" }));
    let out = run(&variant);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout)
        .contains("breaking   type Status: variant `Archived` added: a client that decodes"));
}

#[test]
fn version_prints_the_release() {
    for flag in ["--version", "-V"] {
        let out = bin().arg(flag).output().unwrap();
        assert!(out.status.success(), "{flag}: {out:?}");
        let text = String::from_utf8_lossy(&out.stdout);
        let rest = text
            .strip_prefix("mero-app-build ")
            .unwrap_or_default()
            .trim();
        let (version, build) = rest.split_once(' ').unwrap_or((rest, ""));
        // The release (`[workspace.metadata.workspaces].version`), not the `0.0.0` placeholder,
        // then what it was built from: `(<git describe>, <commit>)`.
        assert!(!version.is_empty() && version != "0.0.0", "{flag}: {text}");
        let build = build
            .strip_prefix('(')
            .and_then(|b| b.strip_suffix(')'))
            .unwrap_or_default();
        let (describe, commit) = build.split_once(", ").unwrap_or_default();
        assert!(!describe.is_empty() && !commit.is_empty(), "{flag}: {text}");
    }
}

#[test]
fn help_prints_usage_and_succeeds() {
    for args in [
        &["--help"][..],
        &["-h"],
        &["codegen", "--help"],
        &["codegen", "-h"],
        &["codegen", "--abi", "x.json", "--help"],
        &["abi-diff", "--help"],
    ] {
        let out = bin().args(args).output().unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            text.starts_with("usage: mero-app-build"),
            "{args:?}: {text}"
        );
        assert!(out.stderr.is_empty(), "{args:?}: {out:?}");
    }
}

#[test]
fn codegen_writes_a_kotlin_client_for_the_serialization_plugin() {
    let dir = tempfile::tempdir().unwrap();
    for (plugin, declares) in [(false, true), (true, false)] {
        let out = dir.path().join(plugin.to_string());
        let mut cmd = bin();
        cmd.args(["codegen", "--abi"])
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../app-abi/tests/fixtures/kitchen-sink.abi.json"),
            )
            .args(["--lang", "kotlin", "--no-openapi", "-o"])
            .arg(&out);
        if plugin {
            let _ = cmd.arg("--kotlin-serialization-plugin");
        }
        assert!(cmd.status().unwrap().success(), "plugin {plugin}");
        let text = fs::read_to_string(out.join("KitchenSinkClient.kt")).unwrap();
        assert!(text.contains("@Serializable(with = Item.Companion.Wire::class)"));
        // With the plugin, the plugin writes `serializer()`.
        assert_eq!(
            text.contains("fun serializer(): KSerializer<Item> = Wire"),
            declares,
            "plugin {plugin}"
        );
    }
}

/// `<root>/logic/Cargo.toml` naming the crate `kv-store`, and its ABI at `logic/res/abi.json`.
fn crate_with_abi(root: &Path, manifest: &str) -> PathBuf {
    let res = root.join("logic/res");
    fs::create_dir_all(&res).unwrap();
    fs::write(root.join("logic/Cargo.toml"), manifest).unwrap();
    let abi = res.join("abi.json");
    let _ = fs::copy(kv_fixture(), &abi).unwrap();
    abi
}

#[test]
fn codegen_names_a_bare_abi_json_after_its_crate() {
    let dir = tempfile::tempdir().unwrap();
    let abi = crate_with_abi(
        dir.path(),
        "[package]\nname = \"kv-store\"\nversion = \"0.0.1\"\n",
    );
    let out = dir.path().join("out");
    let status = bin()
        .args(["codegen", "--abi"])
        .arg(&abi)
        .args(["--lang", "ts", "-o"])
        .arg(&out)
        .status()
        .unwrap();
    assert!(status.success());
    let text = fs::read_to_string(out.join("client.ts")).unwrap();
    assert!(text.contains("export class KvStoreClient<"), "{text}");
    assert!(!text.contains("AbiClient"), "{text}");
}

#[test]
fn codegen_asks_for_a_name_when_a_bare_abi_json_has_no_crate() {
    let dir = tempfile::tempdir().unwrap();
    let abi = crate_with_abi(dir.path(), "[workspace]\nmembers = []\n");
    let out = bin()
        .args(["codegen", "--abi"])
        .arg(&abi)
        .args(["--lang", "ts", "-o"])
        .arg(dir.path().join("out"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--name"), "{stderr}");
    assert!(!dir.path().join("out/client.ts").exists());

    let named = bin()
        .args(["codegen", "--abi"])
        .arg(&abi)
        .args(["--lang", "ts", "--name", "kv-store", "-o"])
        .arg(dir.path().join("out"))
        .status()
        .unwrap();
    assert!(named.success());
}

#[test]
fn codegen_no_openapi_writes_the_client_alone() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    let status = bin()
        .args(["codegen", "--abi"])
        .arg(kv_fixture())
        .args(["--lang", "ts", "--no-openapi", "-o"])
        .arg(&out)
        .status()
        .unwrap();
    assert!(status.success());
    let files: Vec<_> = fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(files, ["client.ts"]);

    let nothing = bin()
        .args(["codegen", "--abi"])
        .arg(kv_fixture())
        .args(["--lang", "openapi", "--no-openapi", "-o"])
        .arg(&out)
        .output()
        .unwrap();
    assert!(!nothing.status.success());
}

#[test]
fn codegen_runs_in_process() {
    use calimero_app_build::codegen::{Codegen, Lang};

    let dir = tempfile::tempdir().unwrap();
    let mut job = Codegen::new(kv_fixture(), Lang::TypeScript, dir.path().join("out"));
    job.openapi = false;
    let written = job.run().unwrap();
    assert_eq!(written, [dir.path().join("out/client.ts")]);
    let text = fs::read_to_string(&written[0]).unwrap();
    assert!(text.contains("export class KvStoreClient<"), "{text}");
}
