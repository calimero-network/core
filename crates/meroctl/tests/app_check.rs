//! `meroctl app check` offline (`--against FILE.mpk`): releases packed with `app pack`,
//! judged green or red by the node's classifier, with the gate's exit codes. No node.

#![allow(
    clippy::unwrap_used,
    reason = "a test's panic is its report; clippy.toml exempts #[test] fns, not their helpers"
)]

use std::path::Path;
use std::process::{Command, Output};

use serde_json::{json, Value};

const AI: &str = "https://inference.near.ai/v1/";
const ACME: &str = "https://api.acme.com/";

fn meroctl(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_meroctl"))
        .current_dir(cwd)
        .env("CALIMERO_HOME", cwd.join("home"))
        .args(args)
        .output()
        .unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "status {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A component holding only the `calimero:abi` custom section.
fn component(abi: &Value) -> Vec<u8> {
    let name = b"calimero:abi";
    let mut section = vec![u8::try_from(name.len()).unwrap()];
    section.extend_from_slice(name);
    section.extend_from_slice(&serde_json::to_vec(abi).unwrap());
    let mut out = b"\0asm\x0d\0\x01\0".to_vec();
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

/// Packs `package` `version` to `out` with an app component declaring `methods`, plus
/// `extra` flags.
fn pack(d: &Path, package: &str, version: &str, methods: &[&str], extra: &[&str], out: &str) {
    let methods: serde_json::Map<String, Value> = methods
        .iter()
        .map(|m| ((*m).to_owned(), json!({})))
        .collect();
    let wasm = format!("{out}.wasm");
    std::fs::write(
        d.join(&wasm),
        component(&json!({ "state": { "version": 1 }, "methods": methods })),
    )
    .unwrap();
    let mut args = vec![
        "app",
        "pack",
        "--package",
        package,
        "--app-version",
        version,
        "--wasm",
        &wasm,
        "--out",
        out,
    ];
    args.extend(extra);
    let o = meroctl(d, &args);
    assert!(o.status.success(), "{}", text(&o));
}

fn check(d: &Path, candidate: &str, against: &str, extra: &[&str]) -> Output {
    let mut args = vec!["app", "check", "--mpk", candidate, "--against", against];
    args.extend(extra);
    meroctl(d, &args)
}

fn json_of(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", text(out)))
}

#[test]
fn the_same_signer_and_egress_is_green_and_passes_the_gate() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    pack(
        d,
        "com.acme.notes",
        "1.3.0",
        &["get"],
        &["--outbound", AI],
        "v13.mpk",
    );
    pack(
        d,
        "com.acme.notes",
        "1.4.0",
        &["get", "pin", "unpin"],
        &["--outbound", AI],
        "v14.mpk",
    );

    let out = check(d, "v14.mpk", "v13.mpk", &["--fail-on", "red"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let summary = String::from_utf8_lossy(&out.stdout);
    assert!(
        summary.starts_with("com.acme.notes 1.3.0 → 1.4.0\n"),
        "{summary}"
    );
    assert!(
        summary.contains(&format!("  Data can go to: {AI}\n")),
        "{summary}"
    );
    assert!(!summary.to_lowercase().contains("sealed"), "{summary}");
    // Green, but the release sends data out: no policy applies it on its own.
    assert!(!summary.contains("apply it on their own"), "{summary}");
    assert!(
        summary.contains("wait for an owner's or admin's approval (green, but the app or the apps it created can send data out"),
        "{summary}"
    );
    assert!(
        summary.contains("New methods: pin, unpin · removed: none"),
        "{summary}"
    );
    assert!(
        summary.contains("Gate (--fail-on red): passed"),
        "{summary}"
    );

    let out = check(d, "v14.mpk", "v13.mpk", &["--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let v = json_of(&out);
    assert_eq!(v["classification"]["class"], "green");
    assert_eq!(v["classification"]["egress"]["after"], json!([AI]));
    assert!(v["classification"].get("sealedAfter").is_none());
    assert_eq!(v["classification"]["newMethods"], json!(["pin", "unpin"]));
    assert_eq!(v["gate"]["passed"], true);
}

#[test]
fn an_added_destination_is_red_and_fails_the_gate_unless_allowed() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    pack(
        d,
        "com.acme.notes",
        "1.3.0",
        &["get"],
        &["--outbound", AI],
        "v13.mpk",
    );
    pack(
        d,
        "com.acme.notes",
        "1.4.0",
        &["get"],
        &["--outbound", AI, "--outbound", ACME],
        "v14.mpk",
    );

    let out = check(d, "v14.mpk", "v13.mpk", &["--fail-on", "red"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let summary = String::from_utf8_lossy(&out.stdout);
    assert!(
        summary.contains(&format!("  Data can go to: {ACME}, {AI}\n")),
        "{summary}"
    );
    assert!(
        summary.contains(&format!("- it can send data somewhere new: {ACME}")),
        "{summary}"
    );
    assert!(
        summary.contains("Gate (--fail-on red): failed:"),
        "{summary}"
    );

    // Without a gate it only reports; `--allow-egress` is the PR saying so.
    let out = check(d, "v14.mpk", "v13.mpk", &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    for gate in ["red", "egress"] {
        let out = check(
            d,
            "v14.mpk",
            "v13.mpk",
            &["--fail-on", gate, "--allow-egress", ACME, "--json"],
        );
        assert_eq!(out.status.code(), Some(0), "{}", text(&out));
        let v = json_of(&out);
        assert_eq!(v["classification"]["class"], "red");
        assert_eq!(v["classification"]["egress"]["added"], json!([ACME]));
        assert_eq!(v["gate"]["waived"], json!([ACME]));
        assert_eq!(v["gate"]["failing"], json!([]));
    }
}

#[test]
fn another_signer_is_red_and_egress_only_gates_let_it_pass() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let key = meroctl(d, &["app", "signer", "new", "k.json"]);
    assert!(key.status.success(), "{}", text(&key));
    pack(
        d,
        "com.acme.notes",
        "1.3.0",
        &["get"],
        &["--outbound", AI],
        "v13.mpk",
    );
    pack(
        d,
        "com.acme.notes",
        "1.4.0",
        &["get"],
        &["--outbound", AI, "--key", "k.json"],
        "v14.mpk",
    );

    let out = check(d, "v14.mpk", "v13.mpk", &["--fail-on", "red", "--json"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let v = json_of(&out);
    assert_eq!(v["classification"]["signerChanged"], true);
    assert_eq!(v["gate"]["failing"][0]["code"], "signerChanged");
    assert_ne!(v["from"]["signer"], v["to"]["signer"]);

    let out = check(d, "v14.mpk", "v13.mpk", &["--fail-on", "egress"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(String::from_utf8_lossy(&out.stdout).contains("is signed by another key"));
}

#[test]
fn a_member_the_release_creates_is_read_from_its_mpk() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    pack(
        d,
        "com.calimero.ai",
        "1.0.0",
        &["chat"],
        &["--outbound", AI],
        "ai.mpk",
    );
    pack(d, "com.acme.notes", "1.3.0", &["get"], &[], "v13.mpk");
    // 1.4.0 creates `ai` (unpinned: the newest at hand inside ^1).
    std::fs::write(
        d.join("Cargo.toml"),
        "[package]\nname = \"notes\"\nversion = \"1.4.0\"\n\n[package.metadata.calimero]\n\
         package = \"com.acme.notes\"\n\n[package.metadata.calimero.uses]\n\
         ai = { package = \"com.calimero.ai\", range = \"^1\", mode = \"create\" }\n",
    )
    .unwrap();
    std::fs::write(
        d.join("app.wasm"),
        component(&json!({ "state": { "version": 1 }, "methods": { "get": {} } })),
    )
    .unwrap();
    let packed = meroctl(
        d,
        &["app", "pack", ".", "--wasm", "app.wasm", "--out", "v14.mpk"],
    );
    assert!(packed.status.success(), "{}", text(&packed));

    // Not at hand: it cannot judge.
    let out = check(d, "v14.mpk", "v13.mpk", &["--fail-on", "egress"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--uses-mpk"),
        "{}",
        text(&out)
    );

    let out = check(
        d,
        "v14.mpk",
        "v13.mpk",
        &["--fail-on", "egress", "--uses-mpk", "ai.mpk"],
    );
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let summary = String::from_utf8_lossy(&out.stdout);
    assert!(
        summary.contains(&format!(
            "it now creates ai (com.calimero.ai), which sends data to {AI}"
        )),
        "{summary}"
    );
    assert!(
        summary.contains("Members: ai com.calimero.ai 1.0.0 (new)"),
        "{summary}"
    );
    assert!(
        summary.contains(&format!("  Data can go to: {AI}\n")),
        "{summary}"
    );
}

#[test]
fn a_release_with_no_destination_keeps_data_in_calimero() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    pack(d, "com.acme.notes", "1.3.0", &["get"], &[], "v13.mpk");
    pack(d, "com.acme.notes", "1.4.0", &["get"], &[], "v14.mpk");
    let out = check(d, "v14.mpk", "v13.mpk", &["--fail-on", "red"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let summary = String::from_utf8_lossy(&out.stdout);
    assert!(summary.contains("  Data: stays in Calimero\n"), "{summary}");
}

#[test]
fn what_cannot_be_judged_exits_2() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    pack(d, "com.acme.notes", "1.4.0", &["get"], &[], "v14.mpk");
    pack(d, "com.acme.other", "1.0.0", &["get"], &[], "other.mpk");

    let out = check(d, "v14.mpk", "missing.mpk", &[]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    let out = check(d, "v14.mpk", "other.mpk", &[]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(String::from_utf8_lossy(&out.stderr).contains("com.acme.other"));
    let out = check(d, "v14.mpk", "v14.mpk", &["--allow-egress", "http://x/"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
}
