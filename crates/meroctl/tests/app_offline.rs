//! `meroctl app` without a node: `pack` (the cases `pack-component.py`'s test
//! held), `key`, `sign`, `inspect`. No network, no node.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

const COMPONENT: &[u8] = b"\0asm\x0d\0\x01\0";

fn meroctl(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_meroctl"))
        .current_dir(cwd)
        .env("CALIMERO_HOME", cwd.join("home"))
        .args(args)
        .output()
        .unwrap()
}

fn ok(out: &Output) -> Value {
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
}

fn untar(mpk: &Path) -> BTreeMap<String, Vec<u8>> {
    let gz = flate2::read::GzDecoder::new(std::fs::File::open(mpk).unwrap());
    let mut tar = tar::Archive::new(gz);
    tar.entries()
        .unwrap()
        .map(|e| {
            let mut e = e.unwrap();
            let name = e.path().unwrap().to_string_lossy().into_owned();
            let mut data = Vec::new();
            let _ = e.read_to_end(&mut data).unwrap();
            (name, data)
        })
        .collect()
}

/// Packs `app.wasm` (unless `with_app` is false) with `extra`, answering the archive or the failure.
fn pack(dir: &Path, extra: &[&str], with_app: bool) -> Result<BTreeMap<String, Vec<u8>>, String> {
    std::fs::write(dir.join("app.wasm"), [COMPONENT, b"app"].concat()).unwrap();
    let mut args = vec!["app", "pack", "--package", "p", "--app-version", "1.0.0", "--out", "out.mpk"];
    if with_app {
        args.extend(["--wasm", "app.wasm"]);
    }
    args.extend(extra);
    let out = meroctl(dir, &args);
    if out.status.success() {
        Ok(untar(&dir.join("out.mpk")))
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

fn manifest(files: &BTreeMap<String, Vec<u8>>) -> Value {
    serde_json::from_slice(&files["manifest.json"]).unwrap()
}

fn names(files: &BTreeMap<String, Vec<u8>>) -> Vec<&str> {
    files.keys().map(String::as_str).collect()
}

fn compute(dir: &Path) {
    std::fs::write(dir.join("compute.component.wasm"), [COMPONENT, b"compute"].concat()).unwrap();
}

#[test]
fn pack_ships_what_it_is_given_and_declares_it() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();

    // Without compute: no compute part. Signed, with the development key.
    let files = pack(d, &[], true).unwrap();
    assert_eq!(names(&files), ["app.wasm", "manifest.json"]);
    let m = manifest(&files);
    assert!(m.get("compute").is_none() && m.get("pipelines").is_none());
    assert_eq!(m["signature"]["algorithm"], "ed25519");
    assert_eq!(m["signerId"], calimero_dev_signer());

    // With compute: declared and carried.
    compute(d);
    let files = pack(d, &["--compute", "compute.component.wasm"], true).unwrap();
    assert_eq!(names(&files), ["app.wasm", "compute.wasm", "manifest.json"]);
    let m = manifest(&files);
    assert_eq!(m["compute"]["path"], "compute.wasm");
    assert_eq!(m["compute"]["size"], files["compute.wasm"].len());

    // Compute only.
    let files = pack(d, &["--compute", "compute.component.wasm"], false).unwrap();
    assert_eq!(names(&files), ["compute.wasm", "manifest.json"]);
    assert!(manifest(&files).get("wasm").is_none());

    // Pipelines, each under pipelines/.
    std::fs::write(d.join("first.yaml"), "name: first\n").unwrap();
    std::fs::write(d.join("second.yaml"), "name: second\n").unwrap();
    let files = pack(
        d,
        &["--compute", "compute.component.wasm", "--pipeline", "first.yaml", "--pipeline", "second.yaml"],
        false,
    )
    .unwrap();
    assert_eq!(
        names(&files),
        ["compute.wasm", "manifest.json", "pipelines/first.yaml", "pipelines/second.yaml"]
    );
    let declared = &manifest(&files)["pipelines"];
    assert_eq!(declared[0]["path"], "pipelines/first.yaml");
    assert_eq!(declared[1]["path"], "pipelines/second.yaml");

    // A frontend build, under frontend/.
    std::fs::create_dir_all(d.join("dist/assets")).unwrap();
    std::fs::write(d.join("dist/index.html"), "<!doctype html><title>x</title>").unwrap();
    std::fs::write(d.join("dist/assets/app.js"), "export {}").unwrap();
    let files = pack(d, &["--frontend", "dist"], true).unwrap();
    assert_eq!(
        names(&files),
        ["app.wasm", "frontend/assets/app.js", "frontend/index.html", "manifest.json"]
    );
    let f = &manifest(&files)["frontend"];
    assert_eq!(f["entry"], "frontend/index.html");
    assert_eq!(f["files"][1]["size"], files["frontend/index.html"].len());

    // Outbound prefixes and who upgrades.
    let files = pack(
        d,
        &[
            "--outbound",
            "https://api.example.com/v1/",
            "--outbound",
            "https://captcha.example/",
            "--upgrades",
            "publisher",
        ],
        true,
    )
    .unwrap();
    let m = manifest(&files);
    assert_eq!(m["outbound"][1], "https://captcha.example/");
    assert_eq!(m["upgrades"], "publisher");
}

fn calimero_dev_signer() -> &'static str {
    calimero_bundle::DEV_SIGNER_ID
}

#[test]
fn pack_refuses_what_the_node_would_refuse() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    // Neither an app nor a compute component.
    assert!(pack(d, &[], false).unwrap_err().contains("--wasm, --compute"));
    // Not a component.
    std::fs::write(d.join("bad.wasm"), "not wasm at all").unwrap();
    assert!(pack(d, &["--compute", "bad.wasm"], true).unwrap_err().contains("not a WASM component"));
    // Two pipelines with one name.
    for sub in ["a", "b"] {
        std::fs::create_dir_all(d.join(sub)).unwrap();
        std::fs::write(d.join(sub).join("p.yaml"), "name: p\n").unwrap();
    }
    assert!(pack(d, &["--pipeline", "a/p.yaml", "--pipeline", "b/p.yaml"], true)
        .unwrap_err()
        .contains("same name"));
    // Frontend files the node refuses, and a build without its entry.
    for (name, code) in [("bad name.html", "FRONTEND_UNSAFE_PATH"), ("tool.exe", "FRONTEND_UNSUPPORTED_TYPE")] {
        let dist = d.join(format!("dist-{code}"));
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(dist.join("index.html"), "<p>").unwrap();
        std::fs::write(dist.join(name), "x").unwrap();
        let err = pack(d, &["--frontend", dist.to_str().unwrap()], true).unwrap_err();
        assert!(err.contains(code), "{name}: {err}");
    }
    std::fs::create_dir_all(d.join("noentry")).unwrap();
    std::fs::write(d.join("noentry/app.js"), "x").unwrap();
    assert!(pack(d, &["--frontend", "noentry"], true).unwrap_err().contains("FRONTEND_NO_ENTRY"));
    // A bad outbound prefix.
    assert!(pack(d, &["--outbound", "http://insecure.example/"], true)
        .unwrap_err()
        .contains("--outbound"));
}

#[test]
fn a_key_signs_and_inspect_verifies_what_pack_wrote() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    let _ = ok(&meroctl(d, &["app", "key", "new", "k.json"]));
    let id = meroctl(d, &["app", "key", "id", "k.json"]);
    let signer = String::from_utf8(ok_raw(&id)).unwrap().trim().to_owned();
    assert!(signer.starts_with("did:key:z"), "{signer}");

    let _ = pack(d, &["--key", "k.json"], true).unwrap();
    let inspected = ok(&meroctl(d, &["app", "inspect", "out.mpk"]));
    assert_eq!(inspected["verified"], true);
    assert_eq!(inspected["signerId"], signer);
    assert_eq!(inspected["manifest"]["package"], "p");

    // A byte changed after signing is caught.
    let files = untar(&d.join("out.mpk"));
    let mut m = manifest(&files);
    m["appVersion"] = "9.9.9".into();
    let tampered = d.join("tampered.mpk");
    let gz = flate2::write::GzEncoder::new(std::fs::File::create(&tampered).unwrap(), flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);
    for (name, data) in [("manifest.json", serde_json::to_vec(&m).unwrap()), ("app.wasm", files["app.wasm"].clone())] {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        tar.append_data(&mut h, name, data.as_slice()).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap();
    assert!(!meroctl(d, &["app", "inspect", "tampered.mpk"]).status.success());

    // `sign` signs a manifest in place; without --key or --dev it refuses.
    std::fs::write(
        d.join("manifest.json"),
        r#"{"version":"1.0","package":"p","appVersion":"1.0.0","minRuntimeVersion":"0.1.0"}"#,
    )
    .unwrap();
    assert!(!meroctl(d, &["app", "sign", "manifest.json"]).status.success());
    let _ = ok(&meroctl(d, &["app", "sign", "manifest.json", "--key", "k.json"]));
    let signed: Value = serde_json::from_slice(&std::fs::read(d.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(signed["signerId"], signer);
}

fn ok_raw(out: &Output) -> Vec<u8> {
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    out.stdout.clone()
}

#[test]
fn pack_reads_the_crate_metadata_of_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    std::fs::create_dir_all(d.join("logic")).unwrap();
    std::fs::write(
        d.join("logic/Cargo.toml"),
        "[package]\nname = \"x\"\nversion = \"0.3.1\"\n\n[package.metadata.calimero]\npackage = \"com.example.x\"\nmin-runtime-version = \"1.0.0-rc.20\"\nfrontend-dir = \"../app/dist\"\n",
    )
    .unwrap();
    std::fs::write(d.join("app.wasm"), [COMPONENT, b"app"].concat()).unwrap();
    // No frontend built yet: packed without one, with a note.
    let out = meroctl(d, &["app", "pack", "logic", "--wasm", "app.wasm"]);
    let answer = ok(&out);
    assert!(String::from_utf8_lossy(&out.stderr).contains("no frontend packed"));
    assert_eq!(answer["package"], "com.example.x");
    assert_eq!(answer["appVersion"], "0.3.1");
    let files = untar(&d.join("logic/dist/com.example.x-0.3.1.mpk"));
    assert_eq!(manifest(&files)["minRuntimeVersion"], "1.0.0-rc.20");
    // Built: packed from frontend-dir.
    std::fs::create_dir_all(d.join("app/dist")).unwrap();
    std::fs::write(d.join("app/dist/index.html"), "<!doctype html><title>x</title>").unwrap();
    let _ = ok(&meroctl(d, &["app", "pack", "logic", "--wasm", "app.wasm", "--out", "with.mpk"]));
    assert!(untar(&d.join("with.mpk")).contains_key("frontend/index.html"));
}

/// `publish` sends the signed manifest's own fields and the whole `.mpk` as hex,
/// with the key as a bearer token; a refusal names the registry's code.
#[tokio::test(flavor = "multi_thread")]
async fn publish_pushes_the_bundle_to_the_registry() {
    use std::sync::{Arc, Mutex};

    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::post;
    use axum::Json;

    type Seen = Arc<Mutex<Option<(String, Value)>>>;
    async fn push(
        State(seen): State<Seen>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        let auth = headers["authorization"].to_str().unwrap().to_owned();
        let refused = body["appVersion"] == "0.0.1";
        *seen.lock().unwrap() = Some((auth, body));
        if refused {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "version_not_allowed", "message": "New version (0.0.1) must be greater than latest (1.0.0)."})),
            );
        }
        (StatusCode::CREATED, Json(serde_json::json!({"package": "p", "version": "1.0.0"})))
    }
    let seen: Seen = Arc::default();
    let app = axum::Router::new()
        .route("/api/v2/bundles/push", post(push))
        .with_state(Arc::clone(&seen));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let _server = tokio::spawn(async move { axum::serve(listener, app).await });

    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().to_owned();
    let run = |args: Vec<String>| {
        let d = d.clone();
        tokio::task::spawn_blocking(move || {
            meroctl(&d, &args.iter().map(String::as_str).collect::<Vec<_>>())
        })
    };
    let _ = pack(&d, &[], true).unwrap();
    let out = run(vec![
        "app".into(), "publish".into(), "out.mpk".into(), "--registry".into(), url.clone(),
        "--api-key".into(), "secret".into(), "--public".into(),
    ])
    .await
    .unwrap();
    assert_eq!(ok(&out)["version"], "1.0.0");
    let (auth, body) = seen.lock().unwrap().clone().unwrap();
    assert_eq!(auth, "Bearer secret");
    assert_eq!(body["package"], "p");
    assert_eq!(body["signature"]["algorithm"], "ed25519");
    assert_eq!(body["_visibility"], "public");
    assert_eq!(body["migrations"], serde_json::json!([]));
    assert_eq!(hex::decode(body["_binary"].as_str().unwrap()).unwrap(), std::fs::read(d.join("out.mpk")).unwrap());

    // A refusal comes back with the registry's code.
    let out = run(vec![
        "app".into(), "pack".into(), "--wasm".into(), "app.wasm".into(), "--package".into(), "p".into(),
        "--app-version".into(), "0.0.1".into(), "--out".into(), "old.mpk".into(),
    ])
    .await
    .unwrap();
    let _ = ok(&out);
    let out = run(vec![
        "app".into(), "publish".into(), "old.mpk".into(), "--registry".into(), url.clone(),
        "--api-key".into(), "secret".into(),
    ])
    .await
    .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("version_not_allowed"));

    // Never an API key over plain http to anything but loopback.
    let out = run(vec![
        "app".into(), "publish".into(), "out.mpk".into(), "--registry".into(), "http://registry.example".into(),
        "--api-key".into(), "secret".into(),
    ])
    .await
    .unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("https://"));
}
