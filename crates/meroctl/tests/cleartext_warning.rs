//! Runs the meroctl binary against node URLs that fail fast and checks whether it warns,
//! on stderr, that credentials would cross the network unencrypted.

use std::io::Read as _;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const RUN_TIMEOUT: Duration = Duration::from_secs(20); // the warning precedes any network I/O
const WARNING: &str = "plain http";

fn stderr_of(args: &[&str]) -> String {
    let home = tempfile::tempdir().expect("temp home");
    let mut child = Command::new(env!("CARGO_BIN_EXE_meroctl"))
        .args(args)
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to run meroctl");

    let deadline = Instant::now() + RUN_TIMEOUT;
    while child.try_wait().expect("wait on meroctl").is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ignored = child.kill();
    let _ignored = child.wait();

    let mut stderr = String::new();
    let _ignored = child
        .stderr
        .take()
        .expect("piped stderr")
        .read_to_string(&mut stderr);
    stderr
}

#[test]
fn api_flag_with_remote_http_warns() {
    let stderr = stderr_of(&["--api", "http://node.invalid:2528", "app", "ls"]);
    assert_eq!(stderr.matches(WARNING).count(), 1, "stderr:\n{stderr}");
}

#[test]
fn node_add_with_remote_http_warns() {
    let stderr = stderr_of(&["node", "add", "lan", "http://node.invalid:2528"]);
    assert_eq!(stderr.matches(WARNING).count(), 1, "stderr:\n{stderr}");
}

#[test]
fn loopback_http_and_remote_https_do_not_warn() {
    for api in [
        "http://127.0.0.1:1",
        "http://localhost:1",
        "http://[::1]:1",
        "https://node.invalid:2528",
    ] {
        let stderr = stderr_of(&["--api", api, "app", "ls"]);
        assert!(!stderr.contains(WARNING), "{api} warned; stderr:\n{stderr}");
    }
}

#[test]
fn warning_omits_credentials_embedded_in_the_url() {
    let stderr = stderr_of(&[
        "--api",
        "http://user:hunter2@node.invalid:2528",
        "app",
        "ls",
    ]);
    assert!(stderr.contains(WARNING), "stderr:\n{stderr}");
    assert!(!stderr.contains("hunter2"), "stderr:\n{stderr}");
}
