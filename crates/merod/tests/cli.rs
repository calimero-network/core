//! `merod init`, `config`, `run`, `inspect` and `attest`, through the binary: v1's
//! command line over the node.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Output};
use std::time::{Duration, Instant};

fn merod(home: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_merod"));
    let _ = c.env("CALIMERO_HOME", home).env("RUST_LOG", "warn");
    for (k, _) in std::env::vars() {
        if k.starts_with("MEROD_") || k.starts_with("MERO_TEE_") {
            let _ = c.env_remove(k);
        }
    }
    c
}

/// `[workspace.metadata.workspaces].version` of the root `Cargo.toml`, read
/// here independently of the build script that feeds `RELEASE_VERSION`.
fn workspace_release() -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    let text = std::fs::read_to_string(root).unwrap();
    let section = text
        .split("[workspace.metadata.workspaces]")
        .nth(1)
        .expect("the root Cargo.toml has [workspace.metadata.workspaces]");
    let line = section
        .lines()
        .find(|l| l.trim_start().starts_with("version"))
        .expect("[workspace.metadata.workspaces] names a version");
    line.split('"').nth(1).unwrap().to_owned()
}

fn ok(out: Output) -> String {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn health(port: u16) -> Option<String> {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    write!(
        s,
        "GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out.contains("200 OK").then_some(out)
}

/// Stop a node the way a terminal does (SIGINT), and wait for it.
fn interrupt(mut child: Child) {
    let _ = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if child.try_wait().unwrap().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    panic!("merod did not stop on SIGINT");
}

#[test]
fn init_config_run_and_inspect_a_node() {
    let home = tempfile::tempdir().unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let url = format!("http://127.0.0.1:{port}");
    let listen = format!("127.0.0.1:{port}");
    let peer = format!("1={url}");

    let written = ok(merod(home.path())
        .args([
            "--node",
            "n1",
            "init",
            "--node-id",
            "1",
            "--public-url",
            &url,
        ])
        .args([
            "--listen",
            &listen,
            "--peers",
            &peer,
            "--bootstrap",
            "--dev",
        ])
        .args(["--replicas", "1", "--app-upgrades", "operator"])
        .output()
        .unwrap());
    let file = home.path().join("n1").join("config.toml");
    assert_eq!(written.trim(), file.to_str().unwrap());
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("node-id = 1"), "{text}");
    assert!(text.contains("app-upgrades = \"operator\""), "{text}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "the configuration is its owner's only");
    }

    // A second init does not overwrite it; `config` changes it.
    let again = merod(home.path())
        .args([
            "--node",
            "n1",
            "init",
            "--node-id",
            "2",
            "--public-url",
            &url,
        ])
        .output()
        .unwrap();
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("config"));
    let changed = ok(merod(home.path())
        .args(["--node", "n1", "config", "--cluster", "test-cluster"])
        .output()
        .unwrap());
    assert!(changed.contains("cluster = \"test-cluster\""), "{changed}");
    assert!(changed.contains("node-id = 1"), "the rest stays: {changed}");

    // A refused configuration says what is missing.
    let missing = merod(home.path())
        .args(["--node", "nobody", "run"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&missing.stderr).contains("node's id"));

    // `run` starts the node from the file.
    let child = merod(home.path())
        .args(["--node", "n1", "run"])
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let answer = loop {
        if let Some(a) = health(port) {
            break a;
        }
        assert!(Instant::now() < deadline, "merod never answered /health");
        std::thread::sleep(Duration::from_millis(200));
    };
    assert!(answer.contains("\"node\":1"), "{answer}");
    interrupt(child);

    // `inspect` reads the stopped node's store from its data directory.
    let inspected = ok(merod(home.path())
        .args(["--node", "n1", "inspect"])
        .output()
        .unwrap());
    assert!(inspected.contains("groups"), "{inspected}");
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// `GET path` on 127.0.0.1:port: the status line and the body.
fn get(port: u16, path: &str) -> Option<(String, String)> {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    let (head, body) = out.split_once("\r\n\r\n")?;
    Some((head.lines().next()?.to_owned(), body.to_owned()))
}

fn verify(
    doc: &calimero_client::attest::release::ReleaseAttestation,
    nonce: &[u8; 32],
    allow_mock: bool,
) -> eyre::Result<calimero_client::attest::release::VerifiedRelease> {
    let policy = calimero_client::Policy {
        allow_mock,
        ..calimero_client::Policy::default()
    };
    tokio::runtime::Runtime::new().unwrap().block_on(
        calimero_client::attest::release::verify_release(doc, nonce, &policy),
    )
}

/// A release attestation needs no node, no store and no KMS: a boot script
/// runs it before the KMS has released anything.
#[test]
fn attest_prints_a_release_attestation_bound_to_the_nonce() {
    let home = tempfile::tempdir().unwrap();
    let nonce = [9u8; 32];
    let out = ok(merod(home.path())
        .args([
            "attest",
            "--dev",
            "--nonce",
            &hex::encode(nonce),
            "--tee-release",
            "2.4.0",
            "--tee-profile",
            "locked-read-only",
        ])
        .output()
        .unwrap());
    let doc: calimero_client::attest::release::ReleaseAttestation =
        serde_json::from_str(&out).unwrap();
    assert!(doc.mock);
    // The release this build is, never the workspace placeholder `0.0.0`
    // every crate's `CARGO_PKG_VERSION` is: a verifier pins the claim's
    // version to the release it trusts.
    assert_eq!(doc.claim.merod_version, calimero_node::RELEASE_VERSION);
    assert_ne!(doc.claim.merod_version, "0.0.0");
    assert_eq!(doc.claim.merod_version, workspace_release());
    assert_eq!(doc.claim.tee_release.as_deref(), Some("2.4.0"));
    assert_eq!(doc.claim.tee_profile.as_deref(), Some("locked-read-only"));
    assert_eq!(doc.claim.identity, None);
    // The digest of the binary that made it.
    let binary = std::fs::read(env!("CARGO_BIN_EXE_merod")).unwrap();
    assert_eq!(
        doc.claim.merod_sha256,
        Some(hex::encode(calimero_identity::sha256(&binary)))
    );

    let verified = verify(&doc, &nonce, true).unwrap();
    assert_eq!(verified.claim, doc.claim);
    let err = verify(&doc, &[8; 32], true).unwrap_err();
    assert!(err.to_string().contains("nonce mismatch"), "{err}");
    let err = verify(&doc, &nonce, false).unwrap_err();
    assert!(err.to_string().contains("mock quote refused"), "{err}");

    // `--out`, and the configured node's `--dev` and release.
    let _ = ok(merod(home.path())
        .args([
            "--node",
            "n1",
            "init",
            "--node-id",
            "1",
            "--public-url",
            "http://127.0.0.1:1",
            "--dev",
            "--tee-release",
            "2.5.0",
        ])
        .output()
        .unwrap());
    let file = home.path().join("attestation.json");
    let _ = ok(merod(home.path())
        .args(["--node", "n1", "attest", "--out"])
        .arg(&file)
        .output()
        .unwrap());
    let doc: calimero_client::attest::release::ReleaseAttestation =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert!(doc.mock);
    assert_eq!(doc.claim.tee_release.as_deref(), Some("2.5.0"));
    let nonce: [u8; 32] = hex::decode(&doc.nonce).unwrap().try_into().unwrap();
    let _verified = verify(&doc, &nonce, true).unwrap();

    // An identity to bind must exist: it is never created here.
    let missing = home.path().join("no-identity");
    let out = merod(home.path())
        .args(["attest", "--dev", "--identity"])
        .arg(&missing)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(!missing.exists());
    let out = merod(home.path())
        .args(["attest", "--dev", "--nonce", "abcd"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}

/// `--serve`: a fresh quote per caller nonce, until stopped.
#[test]
fn attest_serve_answers_each_nonce() {
    let home = tempfile::tempdir().unwrap();
    let port = free_port();
    let child = merod(home.path())
        .args(["attest", "--dev", "--serve", &format!("127.0.0.1:{port}")])
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let nonce = [3u8; 32];
    let path = format!("/attestation/release?nonce={}", hex::encode(nonce));
    let (status, body) = loop {
        if let Some(answer) = get(port, &path) {
            break answer;
        }
        assert!(
            Instant::now() < deadline,
            "merod attest --serve did not answer"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(status.contains("200"), "{status} {body}");
    let doc: calimero_client::attest::release::ReleaseAttestation =
        serde_json::from_str(&body).unwrap();
    let _verified = verify(&doc, &nonce, true).unwrap();
    assert!(verify(&doc, &[4; 32], true).is_err());

    let (status, _) = get(port, "/attestation/release?nonce=00").unwrap();
    assert!(status.contains("400"), "{status}");
    let (status, _) = get(port, "/attestation").unwrap();
    assert!(status.contains("404"), "{status}");
    interrupt(child);
}
