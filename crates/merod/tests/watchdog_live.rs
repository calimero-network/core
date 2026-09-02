//! The watchdogs against a real node, because the unit tests cover the watches in
//! isolation and every bug so far has been in the wiring between them.
//!
//! `cargo test -p merod --test watchdog_live -- --include-ignored`
//!
//! Ignored by default: they need an environment where a node can shut down at all.
//! On a runner without netlink permissions, `if-watch` (used by QUIC and mDNS alike)
//! retries its failure in a tight loop that starves the runtime, and the node then
//! ignores every stop including `SIGTERM` - which `the_node_stops_on_sigterm` is
//! here to distinguish from a fault in the watchdogs themselves.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Debug builds start slowly on a loaded CI runner, so these are generous: they
/// bound a hang, they do not measure latency.
const READY_TIMEOUT: Duration = Duration::from_secs(120);
const EXIT_TIMEOUT: Duration = Duration::from_secs(90);

/// Kills the node even when an assertion unwinds, and keeps its log so a timeout
/// can say what the node was doing rather than only that it was doing something.
struct Node {
    child: Child,
    log: PathBuf,
}

impl Node {
    /// Outside the node's home, since a test deletes that. Keyed on the test's own
    /// tag, not the node name, which several tests share.
    fn log_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("merod-live-{}-{tag}.log", std::process::id()))
    }

    fn tail_all(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn tail(&self) -> String {
        let body = std::fs::read_to_string(&self.log).unwrap_or_default();
        let lines: Vec<&str> = body.lines().rev().take(25).collect();
        lines.into_iter().rev().collect::<Vec<_>>().join("\n")
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.log);
    }
}

fn scratch(tag: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!("merod-live-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).expect("scratch home");
    home
}

/// Asked of the OS rather than hardcoded, so parallel jobs on one runner cannot
/// collide on a port.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port()
}

/// The node only reaches the select that watches for a stop once every subsystem
/// is up, so an open store is not enough to say it is listening.
const READY_LINE: &str = "Node started successfully";

fn wait_until_ready(node: &Node) {
    let deadline = Instant::now() + READY_TIMEOUT;
    while Instant::now() < deadline {
        if node.tail_all().contains(READY_LINE) {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!(
        "node never logged {READY_LINE:?} within {READY_TIMEOUT:?}. Its output:\n{}",
        node.tail()
    );
}

fn init(home: &Path, node: &str) {
    let _ = init_returning_port(home, node);
}

/// [`init`], reporting the server port it picked, for a test that wants to talk
/// to the node rather than only watch it exit.
fn init_returning_port(home: &Path, node: &str) -> u16 {
    let server_port = free_port();
    let out = Command::new(env!("CARGO_BIN_EXE_merod"))
        .args([
            "--home".as_ref(),
            home.as_os_str(),
            "--node".as_ref(),
            node.as_ref(),
        ])
        .args([
            "init",
            // A sandboxed runner has no netlink socket, and mDNS retries that
            // failure in a tight loop that floods the log and starves the node.
            "--no-mdns",
            "--server-port",
            &server_port.to_string(),
            "--swarm-port",
            &free_port().to_string(),
        ])
        .output()
        .expect("run merod init");
    assert!(
        out.status.success(),
        "init: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    server_port
}

/// Runs the node and waits until its store is open, so a test that follows is
/// acting on a node that is actually up.
fn run(home: &Path, node: &str, tag: &str) -> Node {
    let log = Node::log_path(tag);
    let sink = std::fs::File::create(&log).expect("log file");
    let child = Command::new(env!("CARGO_BIN_EXE_merod"))
        .args([
            "--home".as_ref(),
            home.as_os_str(),
            "--node".as_ref(),
            node.as_ref(),
        ])
        .arg("run")
        .env("RUST_LOG", "info")
        .stdout(Stdio::from(sink.try_clone().expect("clone log")))
        .stderr(Stdio::from(sink))
        .spawn()
        .expect("spawn merod run");
    let node_process = Node { child, log };
    wait_until_ready(&node_process);
    node_process
}

fn wait_for_exit(node: &mut Node, what: &str) -> Duration {
    let started = Instant::now();
    while started.elapsed() < EXIT_TIMEOUT {
        match node.child.try_wait().expect("try_wait") {
            Some(_) => return started.elapsed(),
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    panic!(
        "node was still running {EXIT_TIMEOUT:?} after {what}. Its last output:\n{}",
        node.tail()
    );
}

/// `SIGKILL` on the parent runs no code, so the closed pipe is the only thing that
/// can report it. Closing the write end here is that same event.
///
/// Unix-only because the mechanism is: `--exit-on-eof` takes a `RawFd` and merod
/// refuses the flag outright off unix. `the_node_stops_when_its_stdin_closes`
/// below is the portable form of the same guarantee.
#[cfg(unix)]
#[test]
#[ignore = "drives a real node; needs an environment with netlink"]
fn the_node_stops_when_the_pipe_to_its_parent_closes() {
    let home = scratch("parent");
    init(&home, "n1");

    let (reader, writer) = std::io::pipe().expect("pipe");
    let log = Node::log_path("parent");
    let sink = std::fs::File::create(&log).expect("log file");
    let child = Command::new(env!("CARGO_BIN_EXE_merod"))
        .args([
            "--home".as_ref(),
            home.as_os_str(),
            "--node".as_ref(),
            "n1".as_ref(),
        ])
        .args(["run", "--exit-on-eof", "0"])
        .env("RUST_LOG", "info")
        .stdin(Stdio::from(reader))
        .stdout(Stdio::from(sink.try_clone().expect("clone log")))
        .stderr(Stdio::from(sink))
        .spawn()
        .expect("spawn merod run");
    let mut node = Node { child, log };

    wait_until_ready(&node);
    assert!(
        node.child.try_wait().expect("try_wait").is_none(),
        "node stopped before the pipe closed"
    );

    drop(writer);

    let took = wait_for_exit(&mut node, "the pipe closed");
    println!("node exited {took:?} after its parent's pipe closed");
    let _ = std::fs::remove_dir_all(&home);
}

/// The whole point of a Windows lane: a node that starts, serves, and stops.
///
/// `/admin-api/health` is unauthenticated and answers "alive" only when
/// `store.ping()` succeeds, so a 200 here is RocksDB opening and answering a
/// read on this platform — not merely a process that has not exited yet. That
/// distinction matters: every Windows check before this one was a build-time
/// property, and "the binary links" says nothing about whether the datastore
/// works.
///
/// Deliberately portable. It is the only test here that asserts the node does
/// something rather than that it stops doing it.
#[test]
#[ignore = "drives a real node; needs an environment with netlink"]
fn the_node_serves_health_and_then_stops() {
    use std::io::{Read as _, Write as _};

    let home = scratch("health");
    let port = init_returning_port(&home, "n1");

    let log = Node::log_path("health");
    let sink = std::fs::File::create(&log).expect("log file");
    let child = Command::new(env!("CARGO_BIN_EXE_merod"))
        .args([
            "--home".as_ref(),
            home.as_os_str(),
            "--node".as_ref(),
            "n1".as_ref(),
        ])
        .args(["run", "--exit-on-stdin-close"])
        .env("RUST_LOG", "info")
        .stdin(Stdio::piped())
        .stdout(Stdio::from(sink.try_clone().expect("clone log")))
        .stderr(Stdio::from(sink))
        .spawn()
        .expect("spawn merod run");
    let mut node = Node { child, log };
    wait_until_ready(&node);

    // Raw HTTP rather than a client crate: one request, and a test dependency
    // that only this line would justify is not worth the build time on a
    // Windows runner.
    let mut sock = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap_or_else(|e| {
        panic!("the node logged ready but nothing is listening on {port}: {e}")
    });
    sock.set_read_timeout(Some(Duration::from_secs(30)))
        .expect("read timeout");
    sock.write_all(
        b"GET /admin-api/health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    )
    .expect("send the health request");

    let mut response = String::new();
    let _ = sock.read_to_string(&mut response);

    assert!(
        response.starts_with("HTTP/1.1 200"),
        "health did not answer 200 — the datastore ping is what this reports on.\nResponse:\n{response}\nNode log:\n{}",
        node.tail()
    );
    assert!(
        response.contains("alive"),
        "health answered 200 without reporting alive: {response}"
    );

    // And it still stops cleanly afterwards, which on Windows is the only
    // graceful stop there is.
    drop(node.child.stdin.take().expect("child stdin"));
    let took = wait_for_exit(&mut node, "stdin closed");
    println!("node served health, then exited {took:?} after its stdin closed");
    let _ = std::fs::remove_dir_all(&home);
}

/// The portable stop, and on Windows the only one: no `SIGTERM`, and `taskkill`
/// without `/F` cannot reach a console process with no window. A supervisor
/// closing merod's stdin has to be enough on its own.
///
/// Deliberately not `#[cfg(unix)]`, unlike the pipe test above: this is exactly
/// the path that has no unix machinery behind it, so restricting it to unix
/// would exercise everything except the case it exists for.
#[test]
#[ignore = "drives a real node; needs an environment with netlink"]
fn the_node_stops_when_its_stdin_closes() {
    let home = scratch("stdin");
    init(&home, "n1");

    let log = Node::log_path("stdin");
    let sink = std::fs::File::create(&log).expect("log file");
    let child = Command::new(env!("CARGO_BIN_EXE_merod"))
        .args([
            "--home".as_ref(),
            home.as_os_str(),
            "--node".as_ref(),
            "n1".as_ref(),
        ])
        .args(["run", "--exit-on-stdin-close"])
        .env("RUST_LOG", "info")
        .stdin(Stdio::piped())
        .stdout(Stdio::from(sink.try_clone().expect("clone log")))
        .stderr(Stdio::from(sink))
        .spawn()
        .expect("spawn merod run");
    let mut node = Node { child, log };

    wait_until_ready(&node);
    assert!(
        node.child.try_wait().expect("try_wait").is_none(),
        "node stopped before its stdin closed"
    );

    // The whole event: the supervisor lets go of the write end.
    drop(node.child.stdin.take().expect("child stdin"));

    let took = wait_for_exit(&mut node, "stdin closed");
    println!("node exited {took:?} after its stdin closed");
    let _ = std::fs::remove_dir_all(&home);
}

/// `rm -rf` removes a name, not a file, so nothing tells the node - it has to
/// notice that the directory it holds is no longer the one at its path.
/// Unix-only because `data_dir_replaced` identifies a directory by `(dev, ino)`.
/// Windows has an analogue in `GetFileInformationByHandle`, but until merod uses
/// it there is no watch there to test — and asserting this on Windows would fail
/// for a reason that is a known gap rather than a regression.
#[cfg(unix)]
#[test]
#[ignore = "drives a real node; needs an environment with netlink"]
fn the_node_stops_when_its_data_directory_is_deleted() {
    let home = scratch("deleted");
    init(&home, "n1");
    let mut node = run(&home, "n1", "deleted");

    std::fs::remove_dir_all(&home).expect("delete the home under the node");

    let took = wait_for_exit(&mut node, "its data directory was deleted");
    println!("node exited {took:?} after its data directory was deleted");
}

/// The watch must not stop a node whose directory is merely busy.
///
/// Runs everywhere. On Windows there is no data-directory watch to mis-fire, so
/// this degrades into "a node left alone keeps running" — still worth asserting,
/// since a node that dies on its own is exactly what a smoke test should catch.
#[test]
#[ignore = "drives a real node; needs an environment with netlink"]
fn the_node_keeps_running_while_its_data_directory_is_intact() {
    let home = scratch("intact");
    init(&home, "n1");
    let mut node = run(&home, "n1", "intact");

    std::fs::write(home.join("n1").join("data").join("scratch-file"), b"busy").expect("write");
    std::thread::sleep(Duration::from_secs(12));

    assert!(
        node.child.try_wait().expect("try_wait").is_none(),
        "a node whose directory is untouched must keep running"
    );
    let _ = std::fs::remove_dir_all(&home);
}

/// The control: if this fails too, the node cannot shut down in this environment
/// at all, and the two watchdogs above are not what broke.
///
/// Unix-only: there is no `SIGTERM` on Windows, which is the whole reason
/// `--exit-on-stdin-close` exists.
#[cfg(unix)]
#[test]
#[ignore = "drives a real node; needs an environment with netlink"]
fn the_node_stops_on_sigterm() {
    let home = scratch("sigterm");
    init(&home, "n1");
    let mut node = run(&home, "n1", "sigterm");

    let killed = Command::new("kill")
        .args(["-TERM", &node.child.id().to_string()])
        .status()
        .expect("send SIGTERM");
    assert!(killed.success(), "could not signal the node");

    let took = wait_for_exit(&mut node, "SIGTERM");
    println!("node exited {took:?} after SIGTERM");
    let _ = std::fs::remove_dir_all(&home);
}
