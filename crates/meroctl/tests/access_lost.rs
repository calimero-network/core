//! `meroctl context watch` and `context presence stream` when the node ends
//! the stream because the caller lost access (D81): exit 77, the reason on
//! stderr. In its own test binary: test binaries run one after another, and
//! two local clusters started at once in one binary could take each other's
//! ports (`calimero_node::local::start` uses consecutive ports from a base).
//!
//! Needs `kv_store.wasm` (`KV_STORE_WASM`), as `cli.rs` does.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::Value;

fn kv_wasm() -> PathBuf {
    std::env::var("KV_STORE_WASM").map_or_else(
        |_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/wasm32-unknown-unknown/app-release/kv_store.wasm")
        },
        PathBuf::from,
    )
}

/// A base port for a 3-node local cluster whose nodes' ports (base+1 to
/// base+3, `calimero_node::local::start`) are all free, from 21000-29000:
/// below the OS's ephemeral range (from 32768 on Linux, 49152 on macOS),
/// where `cli.rs` takes its base. nextest runs both binaries at once, so this
/// cluster never takes the ports that one's is about to bind.
fn free_base_port() -> u16 {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos())
        ^ std::process::id();
    for k in 0..2_000_u32 {
        let base = 21_000 + u16::try_from(seed.wrapping_add(k * 4) % 2_000).unwrap() * 4;
        let free = (1..=3).all(|i| std::net::TcpListener::bind(("127.0.0.1", base + i)).is_ok());
        if free {
            return base;
        }
    }
    panic!("no free ports between 21000 and 29000");
}

/// One person's meroctl: their own home (saved nodes, session).
struct Meroctl {
    home: PathBuf,
}

impl Meroctl {
    fn cmd(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_meroctl"));
        let _ = c
            .env("CALIMERO_HOME", &self.home)
            .env_remove("CALIMERO_API")
            .env_remove("CALIMERO_API_KEY")
            .env_remove("CALIMERO_OPS_TOKEN");
        c
    }

    fn run(&self, args: &[&str]) -> Value {
        let out = self.cmd().args(args).output().unwrap();
        assert!(
            out.status.success(),
            "meroctl {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8_lossy(&out.stdout);
        serde_json::from_str(&text).unwrap_or(Value::String(text.into_owned()))
    }
}

fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}

/// `context watch` and `context presence stream` exit 77 (`ACCESS_LOST_EXIT`),
/// not 1 as for any other failure, when the node ends the stream because the
/// caller lost access, and say why on stderr: here an anonymous caller once the
/// context stops being public (`401 UNAUTHENTICATED`).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn streams_exit_77_when_the_caller_loses_access() {
    let dir = tempfile::tempdir().unwrap();
    // `free_base_port` checks only the base, and the nodes take the ports
    // after it; nextest runs other binaries' clusters at the same time. A
    // start that finds a port taken tries other ports, as the node's own
    // test harness does.
    let mut attempt = 0;
    let cluster = loop {
        attempt += 1;
        match calimero_node::local::start(
            3,
            free_base_port(),
            &dir.path().join(format!("cluster-{attempt}")),
            "ops-token",
            vec!["http://localhost:3000".into()],
            None,
        )
        .await
        {
            Ok(c) => break c,
            Err(e) if attempt < 5 => eprintln!("cluster start failed ({e:#}); retrying"),
            Err(e) => panic!("cluster did not start: {e:#}"),
        }
    };
    let url = cluster.urls[0].clone();
    let root = dir.path().to_path_buf();
    tokio::task::spawn_blocking(move || access_lost_scenario(&root, &url))
        .await
        .unwrap();
    cluster.stop().await;
}

fn access_lost_scenario(root: &Path, url: &str) {
    use std::io::BufRead;
    use std::process::Stdio;

    let alice = Meroctl {
        home: root.join("alice"),
    };
    let anon = Meroctl {
        home: root.join("anon"),
    };
    let pk = root.join("alice-passkey.json");
    let _ = alice.run(&["node", "add", "local", url, "--allow-mock"]);
    let _ = anon.run(&["node", "add", "local", url, "--allow-mock"]);
    let _ = alice.run(&["login", "--dev-passkey", path(&pk), "--name", "alice"]);
    let wasm = root.join("kv.wasm");
    std::fs::write(&wasm, std::fs::read(kv_wasm()).unwrap()).unwrap();
    let created = alice.run(&[
        "context",
        "create",
        "--path",
        path(&wasm),
        "--name",
        "notes",
    ]);
    let app = created["appId"].as_str().unwrap().to_owned();
    let _ = alice.run(&["context", "access", &app, "public"]);
    let _ = alice.run(&[
        "call",
        "--context",
        &app,
        "set",
        "--args",
        r#"{"key":"a","value":"1"}"#,
    ]);

    // Both streams are open once each has printed its first line (the
    // event written above; presence's snapshot).
    let mut streams = Vec::new();
    for args in [
        vec!["context", "watch", app.as_str()],
        vec!["context", "presence", "stream", app.as_str()],
    ] {
        let mut child = anon
            .cmd()
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut out = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut first = String::new();
        let _n = out.read_line(&mut first).unwrap();
        assert!(first.contains("\"kind\""), "{}: {first}", args.join(" "));
        // Keep reading what it prints until it exits, so it never writes to
        // a closed pipe.
        let rest = std::thread::spawn(move || out.lines().map_while(Result::ok).count());
        streams.push((args.join(" "), child, rest));
    }

    let _ = alice.run(&["context", "access", &app, "owner"]);

    for (name, mut child, rest) in streams {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if std::time::Instant::now() > deadline {
                let _ = child.kill();
                panic!("meroctl {name} kept running after the caller lost access");
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let mut stderr = String::new();
        let _n =
            std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut stderr).unwrap();
        let _lines = rest.join().unwrap();
        assert_eq!(status.code(), Some(77), "meroctl {name}: {stderr}");
        assert!(
            stderr.contains("access lost: 401 UNAUTHENTICATED (sign in to use this app)"),
            "meroctl {name}: {stderr}"
        );
    }
}
