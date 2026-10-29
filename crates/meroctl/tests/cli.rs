//! `meroctl` against a local cluster (`merod local`'s launcher, in process):
//! every command group, through the binary, as a user and as the operator.
//!
//! Needs `kv_store.wasm`: `cargo build -p kv-store --target wasm32-unknown-unknown --profile app-release`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::{json, Value};

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
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

/// A signed `.mpk` of `wasm`.
fn mpk(package: &str, version: &str, wasm: &[u8], key: &ed25519_dalek::SigningKey) -> Vec<u8> {
    use sha2::Digest;
    let manifest = calimero_bundle::BundleManifest {
        version: "1.0".to_owned(),
        package: package.to_owned(),
        app_version: version.to_owned(),
        signer_id: Some(calimero_bundle::derive_signer_id_did_key(
            key.verifying_key().as_bytes(),
        )),
        min_runtime_version: "0.1.0".to_owned(),
        metadata: None,
        handlers: None,
        build_info: None,
        interfaces: None,
        wasm: Some(calimero_bundle::BundleArtifact {
            path: "app.wasm".to_owned(),
            hash: hex::encode(sha2::Sha256::digest(wasm)),
            size: wasm.len() as u64,
        }),
        abi: None,
        links: None,
        frontend: None,
        frontend_aliases: Vec::new(),
        outbound: Vec::new(),
        services: None,
        signature: None,
    };
    let mut manifest = serde_json::to_value(&manifest).unwrap();
    calimero_bundle::sign_manifest_json(&mut manifest, key).unwrap();
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::default(),
    ));
    for (name, bytes) in [
        ("manifest.json", serde_json::to_vec(&manifest).unwrap()),
        ("app.wasm", wasm.to_vec()),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_path(name).unwrap();
        header.set_size(bytes.len() as u64);
        header.set_cksum();
        tar.append(&header, bytes.as_slice()).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

/// `wasm` with a custom section, so it hashes differently and still runs.
fn variant(wasm: &[u8], tag: &str) -> Vec<u8> {
    let mut out = wasm.to_vec();
    out.push(0);
    out.push(u8::try_from(1 + tag.len()).unwrap());
    out.push(u8::try_from(tag.len()).unwrap());
    out.extend_from_slice(tag.as_bytes());
    out
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

    fn try_run(&self, args: &[&str]) -> Result<Value, String> {
        let out = self.cmd().args(args).output().unwrap();
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).into_owned());
        }
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text.into_owned())))
    }

    fn run(&self, args: &[&str]) -> Value {
        self.try_run(args)
            .unwrap_or_else(|e| panic!("meroctl {}: {e}", args.join(" ")))
    }
}

fn s(v: &Value) -> String {
    v.as_str()
        .unwrap_or_else(|| panic!("not a string: {v}"))
        .to_owned()
}

fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn every_command_group_works_against_a_cluster() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = calimero_node::local::start(
        3,
        free_base_port(),
        &dir.path().join("cluster"),
        "ops-token",
        vec!["http://localhost:3000".into()],
        None,
    )
    .await
    .unwrap();
    let url = cluster.urls[0].clone();
    let root = dir.path().to_path_buf();

    // meroctl blocks, so it runs off the runtime the cluster serves on.
    tokio::task::spawn_blocking(move || scenario(&root, &url))
        .await
        .unwrap();
    cluster.stop().await;
}

#[allow(clippy::too_many_lines)]
fn scenario(root: &Path, url: &str) {
    let alice = Meroctl {
        home: root.join("alice"),
    };
    let pk = root.join("alice-passkey.json");
    let pk = path(&pk);
    let kv = std::fs::read(kv_wasm()).unwrap();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
    let write = |name: &str, bytes: &[u8]| {
        let p = root.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    };
    let kv_1_0 = write(
        "kv-1.0.0.mpk",
        &mpk("com.test.kv", "1.0.0", &kv, &publisher),
    );
    let kv_1_1 = write(
        "kv-1.1.0.mpk",
        &mpk("com.test.kv", "1.1.0", &variant(&kv, "v11"), &publisher),
    );

    // Saved nodes. A client that trusts only real quotes refuses the dev cluster.
    let refused = alice.try_run(&["--api", url, "whoami"]).unwrap_err();
    assert!(refused.contains("mock quote refused"), "{refused}");
    let nodes = alice.run(&["node", "add", "local", url, "--allow-mock"]);
    assert_eq!(nodes["default"], "local");
    assert_eq!(alice.run(&["node", "list"])["nodes"]["local"]["url"], url);

    // Sign up with a new file passkey, then sign in with it again.
    let first = alice.run(&["login", "--dev-passkey", pk, "--name", "alice"]);
    let again = alice.run(&["login", "--dev-passkey", pk]);
    assert_eq!(first["accountId"], again["accountId"]);
    let me = alice.run(&["whoami"]);
    assert_eq!(me["accountId"], first["accountId"]);
    let alice_id = s(&me["accountId"]);

    // account: devices, labels, usage.
    let devices = alice.run(&["account", "device", "list"]);
    let device = s(&devices["devices"][0]["id"]);
    let _ = alice.run(&["account", "device", "label", &device, "laptop"]);
    assert_eq!(
        alice.run(&["account", "device", "list"])["devices"][0]["label"],
        "laptop"
    );
    assert!(alice.run(&["account", "usage"]).is_object());

    // app: v1's applications are signed packages.
    let installed = alice.run(&["app", "install", "--path", path(&kv_1_0)]);
    assert_eq!(installed["package"], "com.test.kv");
    assert!(alice
        .run(&["app", "list"])
        .to_string()
        .contains("com.test.kv"));
    let _ = alice.run(&["app", "install", "--path", path(&kv_1_1)]);
    let versions = alice.run(&["app", "versions", "com.test.kv"]);
    assert_eq!(versions["versions"].as_array().unwrap().len(), 2);
    assert_eq!(
        alice.run(&["app", "get-latest-version", "com.test.kv"])["latest"],
        "1.1.0"
    );

    // context: an instance of an application, named by an alias.
    let created = alice.run(&[
        "context",
        "create",
        "--package",
        "com.test.kv",
        "--version",
        "1.0.0",
        "--name",
        "notes",
        "--alias",
        "notes",
    ]);
    let notes = s(&created["appId"]);
    let call = alice.run(&[
        "call",
        "--context",
        "notes",
        "set",
        "--args",
        r#"{"key":"a","value":"1"}"#,
    ]);
    assert!(call["index"].as_u64().unwrap() > 0, "{call}");
    let view = alice.run(&[
        "call",
        "--context",
        "notes",
        "get",
        "--args",
        r#"{"key":"a"}"#,
        "--view",
    ]);
    assert_eq!(view["result"], "1");
    assert!(alice.run(&["context", "list"]).to_string().contains(&notes));
    assert_eq!(alice.run(&["context", "get", "notes"])["appId"], notes);
    assert_eq!(
        alice.run(&["context", "alias", "get", "notes"])["appId"],
        notes
    );
    let _ = alice.run(&["context", "use", "notes"]);
    assert_eq!(alice.run(&["context", "alias", "get"])["appId"], notes);
    let _ = alice.run(&["context", "metadata", "notes", r#"{"colour":"red"}"#]);
    assert_eq!(
        alice.run(&["context", "get", "notes"])["metadata"]["colour"],
        "red"
    );
    assert!(alice.run(&["context", "storage", "notes"]).is_object());
    let events = alice
        .cmd()
        .args(["context", "watch", "notes", "--limit", "1"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&events.stdout).contains("\"kind\""));

    // Presence: ephemeral, per device.
    let _ = alice.run(&[
        "context", "presence", "publish", "notes", "--seq", "1", "--state", "0a0b",
    ]);
    assert!(alice
        .run(&["context", "presence", "list", "notes"])
        .to_string()
        .contains("0a0b"));

    // blob: upload, list, info, download, delete.
    let file = write("note.txt", b"hello blob");
    let up = alice.run(&["blob", "upload", path(&file), "--context", "notes"]);
    let blob = s(up.get("blobId").or_else(|| up.get("id")).unwrap());
    assert!(alice
        .run(&["blob", "list", "--context", "notes"])
        .to_string()
        .contains(&blob));
    assert!(alice
        .run(&["blob", "info", &blob, "--context", "notes"])
        .is_object());
    let out = root.join("note.out");
    let _ = alice.run(&[
        "blob",
        "download",
        &blob,
        "--context",
        "notes",
        "--output",
        path(&out),
    ]);
    assert_eq!(std::fs::read(&out).unwrap(), b"hello blob");
    let _ = alice.run(&["blob", "delete", &blob, "--context", "notes"]);

    // context update: to the next release, approved with the owner's passkey.
    let _ = alice.run(&[
        "context",
        "update",
        "notes",
        "--version",
        "1.1.0",
        "--dev-passkey",
        pk,
    ]);
    assert!(alice
        .run(&["context", "list", "--package", "com.test.kv"])
        .to_string()
        .contains("1.1.0"));
    let view = alice.run(&[
        "call",
        "--context",
        "notes",
        "get",
        "--args",
        r#"{"key":"a"}"#,
        "--view",
    ]);
    assert_eq!(view["result"], "1", "the context keeps its state");

    // dev: a raw .wasm context, and a rebuilt .wasm pushed to it.
    let raw_wasm = write("kv.wasm", &kv);
    let raw = alice.run(&[
        "context",
        "create",
        "--path",
        path(&raw_wasm),
        "--name",
        "raw",
    ]);
    let raw = s(&raw["appId"]);
    let rebuilt = write("kv-rebuilt.wasm", &variant(&kv, "rebuilt"));
    let before = alice.run(&["context", "get", &raw])["bundle"].clone();
    let out = alice
        .cmd()
        .args([
            "dev",
            "watch",
            "--path",
            path(&rebuilt),
            "--context",
            &raw,
            "--dev-passkey",
            pk,
            "--once",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let pushed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(pushed["upgraded"], json!([raw]), "{pushed}");
    assert_ne!(alice.run(&["context", "get", &raw])["bundle"], before);

    // account api-key: a scoped key works on its own.
    let key = alice.run(&["account", "api-key", "create", "--context", "notes"]);
    let key = s(&key["key"]);
    let service = Meroctl {
        home: root.join("service"),
    };
    let _ = service.run(&["node", "add", "local", url, "--allow-mock"]);
    let view = service.run(&[
        "--api-key",
        &key,
        "call",
        "--context",
        &notes,
        "get",
        "--args",
        r#"{"key":"a"}"#,
        "--view",
    ]);
    assert_eq!(view["result"], "1");

    // namespace and group: an organization, its teams, members, metadata.
    let org = alice.run(&["namespace", "create", "acme"]);
    let org = s(&org["orgId"]);
    assert!(alice.run(&["namespace", "list"]).to_string().contains(&org));
    let team = alice.run(&[
        "namespace",
        "create-group",
        &org,
        "eng",
        "--join",
        "restricted",
    ]);
    let team = s(&team["teamId"]);
    assert!(alice
        .run(&["namespace", "groups", &org])
        .to_string()
        .contains(&team));
    let _ = alice.run(&["namespace", "metadata", "set", &org, r#"{"plan":"team"}"#]);
    assert_eq!(
        alice.run(&["namespace", "metadata", "get", &org])["metadata"]["plan"],
        "team"
    );
    let _ = alice.run(&["group", "metadata", &org, &team, r#"{"colour":"blue"}"#]);
    let sub = alice.run(&[
        "namespace",
        "create-group",
        &org,
        "infra",
        "--parent",
        &team,
    ]);
    let sub = s(&sub["teamId"]);
    assert!(alice
        .run(&["group", "subgroups", &org, &team])
        .to_string()
        .contains(&sub));
    let _ = alice.run(&["group", "reparent", &org, &sub, "--top"]);
    assert!(!alice
        .run(&["group", "subgroups", &org, &team])
        .to_string()
        .contains(&sub));

    // A second person joins with an invitation.
    let bob = Meroctl {
        home: root.join("bob"),
    };
    let bob_pk = root.join("bob-passkey.json");
    let _ = bob.run(&["node", "add", "local", url, "--allow-mock"]);
    let bob_id =
        s(&bob.run(&["login", "--dev-passkey", path(&bob_pk), "--name", "bob"])["accountId"]);
    let token = s(&alice.run(&["namespace", "invite", &org, "--role", "member"])["token"]);
    let _ = bob.run(&["namespace", "join", &token]);
    let members = alice.run(&["namespace", "members", "list", &org]);
    assert!(members.to_string().contains(&bob_id), "{members}");
    let _ = alice.run(&[
        "group",
        "members",
        "add",
        &org,
        &team,
        &bob_id,
        "--role",
        "read-only",
    ]);
    assert!(alice
        .run(&["group", "members", "list", &org, &team])
        .to_string()
        .contains(&bob_id));
    let _ = alice.run(&["group", "members", "remove", &org, &team, &bob_id]);
    let _ = alice.run(&[
        "namespace",
        "member-metadata",
        &org,
        &bob_id,
        r#"{"title":"eng"}"#,
    ]);
    let _ = bob.run(&["namespace", "leave", &org]);
    assert!(!alice
        .run(&["namespace", "members", "list", &org])
        .to_string()
        .contains(&bob_id));

    // group upgrade: the organization's contexts to a release, followed to the end.
    let _ = alice.run(&[
        "context",
        "create",
        "--package",
        "com.test.kv",
        "--version",
        "1.0.0",
        "--name",
        "team-kv",
        "--namespace",
        &org,
    ]);
    let started = alice.run(&[
        "group",
        "upgrade",
        "trigger",
        &org,
        "--package",
        "com.test.kv",
        "--version",
        "1.1.0",
        "--dev-passkey",
        pk,
    ]);
    let run = s(&started["runId"]);
    let mut finished = Value::Null;
    for _ in 0..200 {
        finished = alice.run(&["group", "upgrade", "status", &org, &run]);
        if finished["finished"] == json!(true) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(finished["finished"], json!(true), "{finished}");
    assert_eq!(
        finished["done"].as_array().map(Vec::len),
        Some(1),
        "{finished}"
    );

    // ops: the operator's commands, only with the token.
    let no_token = alice.try_run(&["ops", "status"]).unwrap_err();
    assert!(no_token.contains("--ops-token"), "{no_token}");
    let ops = |args: &[&str]| {
        let mut a = vec!["--ops-token", "ops-token", "ops"];
        a.extend_from_slice(args);
        alice.run(&a)
    };
    assert_eq!(ops(&["status"])["apps"], 3);
    let metrics = ops(&["metrics"]);
    assert!(
        metrics.as_str().is_some_and(|m| m.contains("calimero_")),
        "{metrics}"
    );
    // The operator lists only what it installed, never an account's.
    assert!(!ops(&["packages"]).to_string().contains("com.test.kv"));
    let ops_pkg = write("ops.mpk", &mpk("com.test.ops", "1.0.0", &kv, &publisher));
    assert_eq!(
        ops(&["install", "--path", path(&ops_pkg)])["package"],
        "com.test.ops"
    );
    assert!(ops(&["packages"]).to_string().contains("com.test.ops"));
    let _ = ops(&["uninstall", "com.test.ops"]);
    assert!(!ops(&["packages"]).to_string().contains("com.test.ops"));
    // This deployment left app upgrades to owners: the operator's is refused.
    let refused = alice
        .try_run(&[
            "--ops-token",
            "ops-token",
            "ops",
            "upgrade",
            &notes,
            "--version",
            "1.0.0",
        ])
        .unwrap_err();
    assert!(refused.contains("owner"), "{refused}");
    assert!(ops(&["quotas"]).is_object());
    // A node no context lives on, and one that does not exist.
    let unknown = alice
        .try_run(&["--ops-token", "ops-token", "ops", "remove-node", "99"])
        .unwrap_err();
    assert!(
        unknown.contains("no such node") || unknown.contains("NOT_LEADER"),
        "{unknown}"
    );

    // network, attest.
    assert_eq!(alice.run(&["network", "status"])["ok"], true);
    let proven = alice.run(&["attest", "--context", "notes"]);
    assert_eq!(
        proven["receiptKeys"].as_object().map(serde_json::Map::len),
        Some(3)
    );
    assert_eq!(proven["governance"]["appUpgrades"], "owner");

    // Clean up: an organization goes with its contexts; logout forgets the session.
    // Without a file passkey the request still goes out: the node asks for the
    // owner's approval, which on a real cluster is given in the console.
    let me = alice.run(&["whoami"])["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let asked = alice
        .try_run(&["context", "transfer", "notes", &me])
        .unwrap_err();
    assert!(asked.contains("CONFIRM_REQUIRED"), "{asked}");
    let asked = alice.try_run(&["namespace", "delete", &org]).unwrap_err();
    assert!(asked.contains("CONFIRM_REQUIRED"), "{asked}");
    let _ = alice.run(&["namespace", "delete", &org, "--dev-passkey", pk]);
    let _ = alice.run(&["context", "delete", &raw]);
    let _ = alice.run(&["context", "alias", "remove", "notes"]);
    let _ = alice.run(&["logout"]);
    assert!(alice.try_run(&["whoami"]).is_err());
    let _ = alice.run(&["node", "remove", "local"]);
    assert!(alice
        .try_run(&["whoami"])
        .unwrap_err()
        .contains("which node"));
    let _ = alice_id;
}
