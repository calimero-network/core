//! `calimero-cloud` against a local cluster (`merod-v2 local`'s launcher, in process).
//!
//! Needs `kv_store.wasm`: `cargo build -p kv-store --target wasm32-unknown-unknown --profile app-release`.

use std::path::{Path, PathBuf};
use std::process::Command;

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

fn free_base_port() -> u16 {
    // Three consecutive ports after the one the OS hands out.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

struct Cli<'a> {
    url: &'a str,
    home: &'a Path,
}

impl Cli<'_> {
    fn run(&self, args: &[&str]) -> Result<Value, String> {
        let out = Command::new(env!("CARGO_BIN_EXE_calimero-cloud"))
            .env("XDG_CONFIG_HOME", self.home)
            .env_remove("CALIMERO_API_KEY")
            .args(["--url", self.url, "--allow-mock"])
            .args(args)
            .output()
            .unwrap();
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).into_owned());
        }
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text.into_owned())))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn deploy_call_view_and_operate_from_the_command_line() {
    let dir = tempfile::tempdir().unwrap();
    let base = free_base_port();
    let cluster = calimero_cloud_node::local::start(
        3,
        base,
        &dir.path().join("cluster"),
        "ops-token",
        vec!["http://localhost:3000".into()],
        None,
    )
    .await
    .unwrap();
    let url = cluster.urls[0].clone();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let passkey = dir.path().join("dev-passkey.json");
    let wasm = kv_wasm();
    let (url2, home2, passkey2, wasm2) = (url.clone(), home.clone(), passkey.clone(), wasm.clone());

    // The CLI blocks, so it runs off the runtime the cluster serves on.
    tokio::task::spawn_blocking(move || {
        let cli = Cli {
            url: &url2,
            home: &home2,
        };
        let pk = passkey2.to_str().unwrap();

        // A real-only client refuses the dev cluster's mock quote.
        let refused = Command::new(env!("CARGO_BIN_EXE_calimero-cloud"))
            .env("XDG_CONFIG_HOME", &home2)
            .args(["--url", &url2, "whoami"])
            .output()
            .unwrap();
        assert!(!refused.status.success());
        assert!(String::from_utf8_lossy(&refused.stderr).contains("mock quote refused"));

        // Sign up with a new file passkey, then sign in with it again.
        let first = cli
            .run(&["login", "--dev-passkey", pk, "--name", "dev"])
            .unwrap();
        let again = cli.run(&["login", "--dev-passkey", pk]).unwrap();
        assert_eq!(first["accountId"], again["accountId"]);
        assert_eq!(
            cli.run(&["whoami"]).unwrap()["accountId"],
            first["accountId"]
        );

        let deployed = cli
            .run(&[
                "deploy",
                wasm2.to_str().unwrap(),
                "--name",
                "kv",
                "--confirm",
                "clear",
            ])
            .unwrap();
        let app = deployed["appId"].as_str().unwrap().to_owned();
        let call = cli
            .run(&["call", &app, "set", r#"{"key":"a","value":"1"}"#])
            .unwrap();
        assert!(call["index"].as_u64().unwrap() > 0);
        assert_eq!(
            cli.run(&["view", &app, "get", r#"{"key":"a"}"#]).unwrap()["result"],
            "1"
        );
        let listed = cli.run(&["app", "list"]).unwrap();
        assert!(listed.to_string().contains(&app));

        // An API key made from the session works on its own.
        let key = cli.run(&["apikey", "create", "--app", &app]).unwrap();
        let key = key["key"].as_str().unwrap().to_owned();
        let out = Command::new(env!("CARGO_BIN_EXE_calimero-cloud"))
            .env("XDG_CONFIG_HOME", dir.path().join("empty"))
            .args(["--url", &url2, "--allow-mock", "--api-key", &key])
            .args(["view", &app, "get", r#"{"key":"a"}"#])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        // Events as JSON lines.
        let ev = Command::new(env!("CARGO_BIN_EXE_calimero-cloud"))
            .env("XDG_CONFIG_HOME", &home2)
            .args([
                "--url",
                &url2,
                "--allow-mock",
                "events",
                &app,
                "--limit",
                "1",
            ])
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&ev.stdout).contains("\"kind\""));

        // Ops: counts, and a refused call without the token.
        assert!(cli
            .run(&["ops", "status"])
            .unwrap_err()
            .contains("--ops-token"));
        let status = cli
            .run(&["--ops-token", "ops-token", "ops", "status"])
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(status["apps"], 1);

        // Logout forgets the session.
        let _ = cli.run(&["logout"]).unwrap();
        assert!(cli.run(&["whoami"]).is_err());
    })
    .await
    .unwrap();
    let _ = (url, home, passkey, wasm);
    cluster.stop().await;
}
