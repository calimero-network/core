//! `cargo mero bundle --check-frontend` and `--frontend-dir`, driven through the real binary.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use flate2::write::GzEncoder;
use flate2::Compression;
use sha2::{Digest, Sha256};

type Files<'a> = &'a [(&'a str, &'a str)];

const INDEX_HTML: &str = r#"<!doctype html><script src="/app.js"></script>"#;

fn cargo_mero(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cargo-mero"))
        .arg("mero")
        .args(args)
        .output()
        .unwrap()
}

fn stdout_json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// A `.mpk` holding a manifest that declares `files` (path, bytes the manifest
/// was built from) and an archive that holds `archived` instead.
fn write_mpk(dir: &Path, files: &[(&str, &str)], archived: &[(&str, &str)]) -> PathBuf {
    let declared: Vec<_> = files
        .iter()
        .map(|(path, body)| {
            serde_json::json!({
                "path": path,
                "hash": hex::encode(Sha256::digest(body.as_bytes())),
                "size": body.len(),
            })
        })
        .collect();
    let manifest = serde_json::json!({
        "version": "1.0",
        "package": "com.example.web",
        "appVersion": "0.1.0",
        "minRuntimeVersion": "0.1.0",
        "frontend": { "entry": "frontend/index.html", "files": declared },
    });

    let mpk = dir.join("app.mpk");
    let mut tar = tar::Builder::new(GzEncoder::new(
        std::fs::File::create(&mpk).unwrap(),
        Compression::default(),
    ));
    let manifest = serde_json::to_vec(&manifest).unwrap();
    let mut entries = vec![("manifest.json", manifest.as_slice())];
    entries.extend(archived.iter().map(|(p, b)| (*p, b.as_bytes())));
    for (path, bytes) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        tar.append_data(&mut header, path, bytes).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap();
    mpk
}

#[test]
fn check_frontend_accepts_a_valid_bundle() {
    let tmp = tempfile::tempdir().unwrap();
    let files = [
        ("frontend/index.html", INDEX_HTML),
        ("frontend/app.js", "1"),
    ];
    let mpk = write_mpk(tmp.path(), &files, &files);

    let out = cargo_mero(&[
        "bundle",
        "--check-frontend",
        mpk.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let summary = stdout_json(&out);
    assert_eq!(summary["ok"], true);
    assert_eq!(summary["entry"], "frontend/index.html");
    assert_eq!(summary["files"], 2);
}

#[test]
fn check_frontend_reports_a_tampered_file_as_json() {
    let tmp = tempfile::tempdir().unwrap();
    let declared = [
        ("frontend/index.html", INDEX_HTML),
        ("frontend/app.js", "1"),
    ];
    let archived = [
        ("frontend/index.html", INDEX_HTML),
        ("frontend/app.js", "2"),
    ];
    let mpk = write_mpk(tmp.path(), &declared, &archived);

    let out = cargo_mero(&[
        "bundle",
        "--check-frontend",
        mpk.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert!(!out.status.success());
    let err = stdout_json(&out);
    assert_eq!(err["code"], "FRONTEND_HASH_MISMATCH");
    assert_eq!(err["path"], "frontend/app.js");
    assert!(
        err["message"].is_string() && err["hint"].is_string(),
        "{err}"
    );
}

fn check_code(files: &[(&str, &str)], archived: &[(&str, &str)]) -> serde_json::Value {
    let tmp = tempfile::tempdir().unwrap();
    let mpk = write_mpk(tmp.path(), files, archived);
    let out = cargo_mero(&[
        "bundle",
        "--check-frontend",
        mpk.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert!(!out.status.success());
    stdout_json(&out)
}

#[test]
fn check_frontend_reports_each_rule_with_its_code() {
    let remote = r#"<script src="https://cdn.example/x.js"></script>"#;
    let index = ("frontend/index.html", INDEX_HTML);
    let cases: [(Files, &str, &str); 5] = [
        (
            &[("frontend/index.html", remote)],
            "FRONTEND_REMOTE_SCRIPT",
            "frontend/index.html",
        ),
        (
            &[index, ("frontend/app.js.map", "{}")],
            "FRONTEND_UNSUPPORTED_TYPE",
            "frontend/app.js.map",
        ),
        (
            &[index, ("frontend/a\\b.js", "1")],
            "FRONTEND_UNSAFE_PATH",
            "frontend/a\\b.js",
        ),
        (
            &[index, index],
            "FRONTEND_DUPLICATE_PATH",
            "frontend/index.html",
        ),
        (&[("frontend/app.js", "1")], "FRONTEND_NO_ENTRY", ""),
    ];
    for (files, code, path) in cases {
        let err = check_code(files, files);
        assert_eq!(err["code"], code, "{err}");
        assert_eq!(err["path"].as_str().unwrap_or(""), path, "{err}");
    }
}

#[test]
fn check_frontend_reports_a_resized_file() {
    let declared = [
        ("frontend/index.html", INDEX_HTML),
        ("frontend/app.js", "1"),
    ];
    let archived = [
        ("frontend/index.html", INDEX_HTML),
        ("frontend/app.js", "12"),
    ];
    assert_eq!(
        check_code(&declared, &archived)["code"],
        "FRONTEND_SIZE_MISMATCH"
    );
}

#[test]
fn check_frontend_reports_a_missing_file_as_a_generic_error() {
    let declared = [
        ("frontend/index.html", INDEX_HTML),
        ("frontend/app.js", "1"),
    ];
    let err = check_code(&declared, &declared[..1]);
    assert_eq!(err["code"], "BUNDLE_ERROR", "{err}");
}
