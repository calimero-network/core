//! `meroctl local status` finds the cluster `local up` started, from any
//! directory: its state lives under the meroctl home, not the working
//! directory. A listener stands in for the cluster's node.

use std::net::TcpListener;
use std::path::Path;
use std::process::Command;

fn status(cwd: &Path, home: &Path) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_meroctl"))
        .current_dir(cwd)
        .env("CALIMERO_HOME", home)
        .args(["local", "status"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

/// What `local up` leaves in a cluster's state directory.
fn running_cluster(dir: &Path, url: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("meroctl-local.json"),
        serde_json::json!({
            "pid": null,
            "container": null,
            "urls": [url],
            "opsToken": "t",
            "log": dir.join("merod.log"),
        })
        .to_string(),
    )
    .unwrap();
}

// The quickstart runs `local up`, then `cd my-app`: `status` and `down` there
// used to look in `my-app/.merod-local`, report `"running": false` and leave
// the cluster up.
#[test]
fn status_finds_the_cluster_from_another_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let node = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", node.local_addr().unwrap());
    running_cluster(&home.join("meroctl/local"), &url);
    let project = tmp.path().join("my-app");
    std::fs::create_dir_all(&project).unwrap();

    let answer = status(&project, &home);
    assert_eq!(answer["running"], true, "{answer}");
    assert_eq!(answer["urls"][0], url.as_str());
}

// A cluster an older meroctl started keeps its state in `./.merod-local`.
#[test]
fn status_still_finds_a_cluster_in_the_old_place() {
    let tmp = tempfile::tempdir().unwrap();
    let node = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", node.local_addr().unwrap());
    running_cluster(&tmp.path().join(".merod-local"), &url);

    let answer = status(tmp.path(), &tmp.path().join("home"));
    assert_eq!(answer["running"], true, "{answer}");
}
