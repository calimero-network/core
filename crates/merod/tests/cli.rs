//! `merod init`, `config`, `run` and `inspect`, through the binary: v1's
//! command line over the release-v2 node.

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
