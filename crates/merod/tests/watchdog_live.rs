//! The watchdogs against a real node, because the unit tests cover the watches in
//! isolation and every bug so far has been in the wiring between them.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Debug builds start slowly on a loaded CI runner, so these are generous: they
/// bound a hang, they do not measure latency.
const STORE_OPEN_TIMEOUT: Duration = Duration::from_secs(90);
const EXIT_TIMEOUT: Duration = Duration::from_secs(45);

/// Kills the node even when an assertion unwinds.
struct Node(Child);

impl Drop for Node {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
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

fn init(home: &Path, node: &str) {
    let out = Command::new(env!("CARGO_BIN_EXE_merod"))
        .args([
            "--home".as_ref(),
            home.as_os_str(),
            "--node".as_ref(),
            node.as_ref(),
        ])
        .args([
            "init",
            "--server-port",
            &free_port().to_string(),
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
}

/// Runs the node and waits until its store is open, so a test that follows is
/// acting on a node that is actually up.
fn run(home: &Path, node: &str) -> Node {
    let child = Command::new(env!("CARGO_BIN_EXE_merod"))
        .args([
            "--home".as_ref(),
            home.as_os_str(),
            "--node".as_ref(),
            node.as_ref(),
        ])
        .arg("run")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn merod run");
    let node_process = Node(child);

    let marker = home.join(node).join("data").join("CURRENT");
    let deadline = Instant::now() + STORE_OPEN_TIMEOUT;
    while Instant::now() < deadline {
        if marker.exists() {
            std::thread::sleep(Duration::from_secs(2));
            return node_process;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("node never opened its store at {}", marker.display());
}

fn wait_for_exit(node: &mut Node, what: &str) -> Duration {
    let started = Instant::now();
    while started.elapsed() < EXIT_TIMEOUT {
        match node.0.try_wait().expect("try_wait") {
            Some(_) => return started.elapsed(),
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    panic!("node was still running {EXIT_TIMEOUT:?} after {what}");
}

/// `SIGKILL` on the parent runs no code, so the closed pipe is the only thing that
/// can report it. Closing the write end here is that same event.
#[test]
fn the_node_stops_when_the_pipe_to_its_parent_closes() {
    let home = scratch("parent");
    init(&home, "n1");

    let (reader, writer) = std::io::pipe().expect("pipe");
    let mut child = Command::new(env!("CARGO_BIN_EXE_merod"))
        .args([
            "--home".as_ref(),
            home.as_os_str(),
            "--node".as_ref(),
            "n1".as_ref(),
        ])
        .args(["run", "--exit-on-eof", "0"])
        .stdin(Stdio::from(reader))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn merod run");

    let marker = home.join("n1").join("data").join("CURRENT");
    let deadline = Instant::now() + STORE_OPEN_TIMEOUT;
    while Instant::now() < deadline && !marker.exists() {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(marker.exists(), "node never opened its store");
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "node stopped before the pipe closed"
    );

    drop(writer);

    let mut node = Node(child);
    let took = wait_for_exit(&mut node, "the pipe closed");
    println!("node exited {took:?} after its parent's pipe closed");
    let _ = std::fs::remove_dir_all(&home);
}

/// `rm -rf` removes a name, not a file, so nothing tells the node - it has to
/// notice that the directory it holds is no longer the one at its path.
#[test]
fn the_node_stops_when_its_data_directory_is_deleted() {
    let home = scratch("deleted");
    init(&home, "n1");
    let mut node = run(&home, "n1");

    std::fs::remove_dir_all(&home).expect("delete the home under the node");

    let took = wait_for_exit(&mut node, "its data directory was deleted");
    println!("node exited {took:?} after its data directory was deleted");
}

/// The watch must not stop a node whose directory is merely busy.
#[test]
fn the_node_keeps_running_while_its_data_directory_is_intact() {
    let home = scratch("intact");
    init(&home, "n1");
    let mut node = run(&home, "n1");

    std::fs::write(home.join("n1").join("data").join("scratch-file"), b"busy").expect("write");
    std::thread::sleep(Duration::from_secs(12));

    assert!(
        node.0.try_wait().expect("try_wait").is_none(),
        "a node whose directory is untouched must keep running"
    );
    let _ = std::fs::remove_dir_all(&home);
}
