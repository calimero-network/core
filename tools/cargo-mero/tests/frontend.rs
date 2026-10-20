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
    assert_eq!(summary["frontend"]["entry"], "frontend/index.html");
    assert_eq!(summary["frontend"]["files"], 2);
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

fn demo_manifest() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/demo-app/Cargo.toml")
}

fn write_build(dir: &Path, files: &[(&str, &str)]) {
    for (path, body) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
}

fn bundle_with_frontend(build: &Path, extra: &[&str]) -> Output {
    let mut args = vec!["bundle", "--dev", "--format", "json", "--manifest-path"];
    let manifest = demo_manifest();
    args.push(manifest.to_str().unwrap());
    args.extend(["--frontend-dir", build.to_str().unwrap()]);
    args.extend(extra);
    cargo_mero(&args)
}

#[test]
fn an_invalid_frontend_fails_before_the_wasm_build() {
    let tmp = tempfile::tempdir().unwrap();
    // This crate cannot compile, so reaching the build would give another error.
    let app = tmp.path().join("app");
    write_build(
        &app,
        &[
            (
                "Cargo.toml",
                "[package]\nname = \"broken\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
                 [package.metadata.calimero]\npackage = \"com.example.broken\"\nicon = \"default\"\n[workspace]\n",
            ),
            ("src/lib.rs", "compile_error!(\"build ran\");"),
        ],
    );
    let build = tmp.path().join("web");
    write_build(
        &build,
        &[(
            "index.html",
            r#"<script src="https://cdn.example/x.js"></script>"#,
        )],
    );

    let out = cargo_mero(&[
        "bundle",
        "--dev",
        "--format",
        "json",
        "--manifest-path",
        app.join("Cargo.toml").to_str().unwrap(),
        "--frontend-dir",
        build.to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    let err = stdout_json(&out);
    assert_eq!(err["code"], "FRONTEND_REMOTE_SCRIPT");
    assert_eq!(err["path"], "frontend/index.html");
}

#[test]
fn a_build_without_an_index_fails_with_no_entry() {
    let tmp = tempfile::tempdir().unwrap();
    write_build(tmp.path(), &[("app.js", "1")]);
    assert_eq!(
        stdout_json(&bundle_with_frontend(tmp.path(), &[]))["code"],
        "FRONTEND_NO_ENTRY"
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_in_the_frontend_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    write_build(tmp.path(), &[("index.html", INDEX_HTML)]);
    std::os::unix::fs::symlink("/etc/hosts", tmp.path().join("hosts.txt")).unwrap();

    let err = stdout_json(&bundle_with_frontend(tmp.path(), &[]));
    assert_eq!(err["code"], "FRONTEND_UNSAFE_PATH", "{err}");
    assert_eq!(err["path"], "frontend/hosts.txt");
}

#[test]
fn a_valid_frontend_is_packed_and_passes_the_check() {
    let tmp = tempfile::tempdir().unwrap();
    let build = tmp.path().join("web");
    write_build(
        &build,
        &[
            ("index.html", INDEX_HTML),
            ("app.js", "1"),
            ("app.js.map", "{}"),
            ("assets/logo.svg", "<svg/>"),
        ],
    );
    let mpk = tmp.path().join("out.mpk");

    let out = bundle_with_frontend(&build, &["--output", mpk.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let summary: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).lines().last().unwrap()).unwrap();
    assert_eq!(summary["ok"], true);
    assert_eq!(summary["frontend"]["files"], 3);
    assert_eq!(summary["output"], mpk.to_str().unwrap());

    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(
        std::fs::File::open(&mpk).unwrap(),
    ));
    let mut names: Vec<_> = archive
        .entries()
        .unwrap()
        .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
        .filter(|p| p.starts_with("frontend/"))
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "frontend/app.js",
            "frontend/assets/logo.svg",
            "frontend/index.html"
        ]
    );

    let check = cargo_mero(&["bundle", "--check-frontend", mpk.to_str().unwrap()]);
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
}
