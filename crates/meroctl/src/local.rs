//! `meroctl local`: a development cluster on this machine and the loop around
//! it. `up` starts `merod local` in the background (the binary on PATH, or the
//! release's container image with `--docker`) and saves it as the node
//! `local`; `dev` builds an app, uploads it, writes its frontend's
//! `.env.development.local`, runs the frontend's dev server and rebuilds on
//! every save; `test` runs the app's tests and scenarios on core's engine;
//! `compute run` runs one compute method alone, as a job node would.

use std::io::Write as _;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use calimero_identity::testing::SoftAuthenticator;
use clap::{Args, Subcommand};
use eyre::{bail, eyre, WrapErr};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::env::{write_private, Env, Home, SavedNode, Trust};

/// The node name `up` saves the cluster as.
pub const NODE: &str = "local";
/// The origin `merod local` trusts passkey ceremonies from, where `dev` runs the frontend.
const DEV_ORIGIN: &str = "http://localhost:5173";
const STATE_FILE: &str = "meroctl-local.json";
const READY_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Args)]
pub struct ClusterArgs {
    /// How many nodes.
    #[arg(long, default_value_t = 3)]
    pub nodes: u16,
    /// Node i listens on base-port + i (127.0.0.1 only).
    #[arg(long, default_value_t = 2500)]
    pub base_port: u16,
    /// Where the cluster keeps its state, kept across restarts.
    #[arg(long, default_value = ".merod-local")]
    pub dir: PathBuf,
    /// Start from empty state.
    #[arg(long)]
    pub fresh: bool,
    /// Run the release's container image instead of `merod` from PATH
    /// (`docker run --network host`: the nodes listen on 127.0.0.1).
    #[arg(long)]
    pub docker: bool,
    /// The image for `--docker`.
    #[arg(long, default_value_t = default_image())]
    pub image: String,
}

fn default_image() -> String {
    format!("ghcr.io/calimero-network/merod:{}", env!("CALIMERO_RELEASE_VERSION"))
}

#[derive(Debug, Subcommand)]
pub enum LocalCmd {
    /// Start a development cluster in the background and save it as the node
    /// `local` (mock attestation, a development storage key: never for real data).
    Up(ClusterArgs),
    /// Stop the cluster `up` started; its state stays for the next `up`.
    Down {
        #[arg(long, default_value = ".merod-local")]
        dir: PathBuf,
    },
    /// Whether the cluster runs, and its nodes.
    Status {
        #[arg(long, default_value = ".merod-local")]
        dir: PathBuf,
    },
    /// Develop an app on the cluster: start it if it is not running, build the
    /// app, upload it, write the frontend's `.env.development.local`, run the
    /// frontend on http://localhost:5173 and rebuild and upload on every save.
    Dev(DevArgs),
    /// The app's tests: `cargo test` of its crate, which runs every
    /// `scenarios/*.json` on core's engine (the apps template's harness).
    Test {
        /// The app (`<dir>/logic`) or its crate.
        #[arg(default_value = ".")]
        dir: PathBuf,
        /// Arguments after `--` go to `cargo test`.
        #[arg(last = true)]
        cargo_args: Vec<String>,
    },
    /// Compute without a cluster.
    #[command(subcommand)]
    Compute(ComputeCmd),
}

#[derive(Debug, Subcommand)]
pub enum ComputeCmd {
    /// Run one compute method of a compute component alone, with the engine,
    /// caps and determinism of a job node and plain files in place of sealed ones.
    #[cfg(feature = "compute")]
    Run(mero_compute::RunArgs),
    /// Not built in: this meroctl was built without the `compute` feature.
    #[cfg(not(feature = "compute"))]
    Run,
}

#[derive(Debug, Args)]
pub struct DevArgs {
    /// The app: a directory holding `logic/` (its crate) and `app/` (its
    /// frontend), as `meroctl app new` lays it out, or the crate itself.
    #[arg(value_name = "DIR", default_value = ".")]
    pub app: PathBuf,
    /// Do not run the frontend's dev server.
    #[arg(long)]
    pub no_frontend: bool,
    #[command(flatten)]
    pub cluster: ClusterArgs,
}

/// What `up` left for `down` and `status`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct State {
    pid: Option<u32>,
    container: Option<String>,
    urls: Vec<String>,
    ops_token: String,
    log: PathBuf,
}

/// A cluster this process started, stopped when it is dropped (`dev`).
pub struct Started {
    dir: PathBuf,
    child: Option<Child>,
}

impl Drop for Started {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = Command::new("kill")
                .args(["-INT", &child.id().to_string()])
                .stderr(Stdio::null())
                .status();
            let _ = child.wait();
        }
        let _ = stop(&self.dir);
    }
}

fn urls(nodes: u16, base_port: u16) -> Vec<String> {
    (1..=nodes)
        .map(|i| format!("http://127.0.0.1:{}/", base_port + i))
        .collect()
}

fn listening(url: &str) -> bool {
    let addr = url.trim_start_matches("http://").trim_end_matches('/');
    TcpStream::connect_timeout(&addr.parse().expect("127.0.0.1:port"), Duration::from_millis(300))
        .is_ok()
}

fn read_state(dir: &Path) -> Option<State> {
    serde_json::from_str(&std::fs::read_to_string(dir.join(STATE_FILE)).ok()?).ok()
}

fn alive(state: &State) -> bool {
    state.urls.iter().all(|u| listening(u))
}

/// Start the cluster and wait until every node listens; with `own`, it is
/// stopped when the answer is dropped.
pub fn up(a: &ClusterArgs, home: &Home, own: bool) -> eyre::Result<(Value, Option<Started>)> {
    if a.nodes == 0 {
        bail!("--nodes must be at least 1");
    }
    if let Some(state) = read_state(&a.dir).filter(alive) {
        if a.fresh {
            bail!("a cluster is running from {}: `meroctl local down` first", a.dir.display());
        }
        save_node(home, &state.urls[0])?;
        return Ok((describe(&a.dir, &state, true), None));
    }
    let urls = urls(a.nodes, a.base_port);
    if let Some(busy) = urls.iter().find(|u| listening(u)) {
        bail!("{busy} is taken by another process: stop it, or pass --base-port");
    }
    std::fs::create_dir_all(&a.dir)?;
    let dir = std::path::absolute(&a.dir)?;
    let token = hex::encode(calimero_identity::random::<16>());
    let log = dir.join("merod.log");
    let mut local = vec![
        "local".to_owned(),
        "--nodes".into(),
        a.nodes.to_string(),
        "--base-port".into(),
        a.base_port.to_string(),
        "--ops-token".into(),
        token.clone(),
    ];
    if a.fresh {
        local.push("--fresh".into());
    }
    let out = std::fs::File::create(&log)?;
    let (pid, container, child) = if a.docker {
        // The image's merod writes as uid 10001 under /data, the mounted dir.
        local.extend(["--dir".into(), "/data/cluster".into()]);
        let name = format!("meroctl-local-{}", a.base_port);
        let _ = Command::new("docker")
            .args(["rm", "-f", &name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let mut cmd = Command::new("docker");
        let _ = cmd
            .args(["run", "-d", "--name", &name, "--network", "host", "-v"])
            .arg(format!("{}:/data", dir.display()))
            .arg(&a.image)
            .args(&local);
        let status = cmd
            .stdout(Stdio::null())
            .status()
            .wrap_err("running docker (is it installed and on PATH?)")?;
        if !status.success() {
            bail!("docker run {} failed ({status})", a.image);
        }
        // Follow the container's output into the log, as merod's own would be.
        let _ = Command::new("docker")
            .args(["logs", "-f", &name])
            .stdout(out.try_clone()?)
            .stderr(out)
            .spawn();
        (None, Some(name), None)
    } else {
        local.extend(["--dir".into(), dir.join("cluster").display().to_string()]);
        let merod = std::env::var_os("MEROD").unwrap_or_else(|| "merod".into());
        let child = Command::new(&merod)
            .args(&local)
            .stdin(Stdio::null())
            .stdout(out.try_clone()?)
            .stderr(out)
            .spawn()
            .wrap_err_with(|| {
                format!(
                    "starting {} (put merod on PATH, set MEROD, or pass --docker)",
                    merod.to_string_lossy()
                )
            })?;
        (Some(child.id()), None, Some(child))
    };
    let state = State {
        pid,
        container,
        urls: urls.clone(),
        ops_token: token,
        log: log.clone(),
    };
    write_private(&dir.join(STATE_FILE), &serde_json::to_string_pretty(&state)?)?;
    // Only a caller that owns the cluster (`dev`) stops it when done.
    let started = own.then(|| Started {
        dir: a.dir.clone(),
        child,
    });
    let deadline = Instant::now() + READY_TIMEOUT;
    // `merod local` prints its ops token once every node has started.
    let printed_ready = || std::fs::read_to_string(&log).is_ok_and(|t| t.contains("ops token"));
    while !(alive(&state) && printed_ready()) {
        if Instant::now() > deadline {
            bail!("the cluster did not start in {READY_TIMEOUT:?}; its log: {}", log.display());
        }
        if let Some(pid) = state.pid {
            if !Path::new(&format!("/proc/{pid}")).exists() && cfg!(target_os = "linux") {
                bail!("merod local exited; its log: {}\n{}", log.display(), tail(&log));
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    save_node(home, &urls[0])?;
    let answer = describe(&a.dir, &state, true);
    Ok((answer, started))
}

fn tail(log: &Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let lines: Vec<_> = text.lines().collect();
    lines[lines.len().saturating_sub(20)..].join("\n")
}

/// Save the cluster as the node `local` (mock quotes trusted), the default when none is.
fn save_node(home: &Home, url: &str) -> eyre::Result<()> {
    let mut nodes = home.nodes();
    let _ = nodes.nodes.insert(
        NODE.to_owned(),
        SavedNode {
            url: url.to_owned(),
            allow_mock: true,
        },
    );
    if nodes.default.is_none() {
        nodes.default = Some(NODE.to_owned());
    }
    home.save_nodes(&nodes)
}

fn describe(dir: &Path, state: &State, running: bool) -> Value {
    json!({
        "running": running,
        "node": NODE,
        "urls": state.urls,
        "opsToken": state.ops_token,
        "dir": dir,
        "log": state.log,
        "docker": state.container,
    })
}

/// Stop what `up` started from `dir`; nothing to stop is not an error.
pub fn stop(dir: &Path) -> eyre::Result<Value> {
    let Some(state) = read_state(dir) else {
        return Ok(json!({ "running": false }));
    };
    if let Some(pid) = state.pid {
        let _ = Command::new("kill")
            .args(["-INT", &pid.to_string()])
            .stderr(Stdio::null())
            .status();
    }
    if let Some(name) = &state.container {
        let _ = Command::new("docker")
            .args(["stop", name])
            .stdout(Stdio::null())
            .status();
        let _ = Command::new("docker")
            .args(["rm", name])
            .stdout(Stdio::null())
            .status();
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while state.urls.iter().any(|u| listening(u)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
    std::fs::remove_file(dir.join(STATE_FILE))?;
    Ok(json!({ "running": false, "stopped": state.urls }))
}

pub async fn run(cmd: LocalCmd, home: Home) -> eyre::Result<Value> {
    match cmd {
        LocalCmd::Up(a) => Ok(up(&a, &home, false)?.0),
        LocalCmd::Down { dir } => stop(&dir),
        LocalCmd::Status { dir } => Ok(match read_state(&dir) {
            Some(state) => {
                let running = alive(&state);
                describe(&dir, &state, running)
            }
            None => json!({ "running": false }),
        }),
        LocalCmd::Dev(a) => dev(a, home).await,
        LocalCmd::Test { dir, cargo_args } => {
            let krate = crate_dir(&dir);
            let status = Command::new("cargo")
                .arg("test")
                .arg("--manifest-path")
                .arg(krate.join("Cargo.toml"))
                .args(cargo_args)
                .status()
                .wrap_err("running cargo")?;
            if !status.success() {
                std::process::exit(status.code().unwrap_or(1));
            }
            Ok(Value::Null)
        }
        #[cfg(feature = "compute")]
        LocalCmd::Compute(ComputeCmd::Run(args)) => {
            let code = mero_compute::main_run(args);
            if code != std::process::ExitCode::SUCCESS {
                std::process::exit(1);
            }
            Ok(Value::Null)
        }
        #[cfg(not(feature = "compute"))]
        LocalCmd::Compute(ComputeCmd::Run) => {
            bail!("this meroctl was built without the `compute` feature")
        }
    }
}

/// The app's crate: `<dir>/logic` when there is one, else `dir`.
fn crate_dir(dir: &Path) -> PathBuf {
    let logic = dir.join("logic");
    if logic.join("Cargo.toml").is_file() {
        logic
    } else {
        dir.to_owned()
    }
}

/// The newest modification under the crate's sources and manifest.
fn sources_changed_at(krate: &Path) -> Option<SystemTime> {
    let mut newest = None;
    let mut stack = vec![krate.join("src"), krate.join("Cargo.toml")];
    while let Some(p) = stack.pop() {
        let Ok(meta) = std::fs::metadata(&p) else {
            continue;
        };
        if meta.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&p) {
                stack.extend(entries.flatten().map(|e| e.path()));
            }
        } else if let Ok(m) = meta.modified() {
            newest = newest.max(Some(m));
        }
    }
    newest
}

async fn dev(a: DevArgs, home: Home) -> eyre::Result<Value> {
    let krate = crate_dir(&a.app);
    if !krate.join("Cargo.toml").is_file() {
        bail!("{} holds no crate (no Cargo.toml, nor logic/Cargo.toml)", a.app.display());
    }
    let frontend = Some(a.app.join("app")).filter(|f| !a.no_frontend && f.join("package.json").is_file());

    let (cluster, _owned) = up(&a.cluster, &home, true)?;
    let url = cluster["urls"][0]
        .as_str()
        .ok_or_else(|| eyre!("no node"))?
        .to_owned();
    eprintln!("• cluster: {} (log: {})", url, cluster["log"].as_str().unwrap_or_default());

    // A deployer account of its own: a raw bundle is visible to every account
    // on a development cluster, so the person signing in to the frontend
    // creates their instance from it.
    let env = Env::connect(
        Home { dir: home.dir.clone() },
        url.clone(),
        Trust {
            allow_mock: true,
            ..Trust::default()
        },
        None,
        None,
    )?;
    let mut passkey = SoftAuthenticator::new("localhost", DEV_ORIGIN);
    // A fresh cluster's system group may still be electing: retry a little.
    let mut tries = 0;
    loop {
        match env.client.sign_up(&mut passkey, "deployer").await {
            Ok(_) => break,
            Err(e) if tries < 15 => {
                tries += 1;
                eprintln!("• waiting for the cluster ({e})");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(e) => return Err(e.into()),
        }
    }

    let wasm = std::path::absolute(krate.join(crate::make::APP_FILE))?;
    let nodes: Vec<String> = cluster["urls"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default();
    let target = Deploy {
        krate: krate.clone(),
        wasm,
        frontend: frontend.clone(),
        nodes,
    };
    let first = target.deploy(&env).await?;

    let mut vite = match &frontend {
        Some(f) => {
            eprintln!("• frontend: {DEV_ORIGIN}/ (Ctrl-C stops it and the cluster)");
            Some(
                tokio::process::Command::new("pnpm")
                    .arg("--dir")
                    .arg(f)
                    .args(["dev", "--port", "5173", "--strictPort"])
                    .kill_on_drop(true)
                    .spawn()
                    .wrap_err("running pnpm (install the frontend's dependencies first: pnpm install)")?,
            )
        }
        None => None,
    };

    let mut seen = sources_changed_at(&krate);
    let mut last = first;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            status = async { match &mut vite { Some(v) => v.wait().await, None => std::future::pending().await } } => {
                eprintln!("• the frontend's dev server exited ({})", status.map(|s| s.to_string()).unwrap_or_default());
                break;
            }
            () = tokio::time::sleep(Duration::from_millis(700)) => {
                let now = sources_changed_at(&krate);
                if now != seen {
                    seen = now;
                    match target.deploy(&env).await {
                        Ok(b) => last = b,
                        Err(e) => eprintln!("• build or upload failed: {e:#}"),
                    }
                }
            }
        }
    }
    if let Some(v) = &mut vite {
        let _ = v.kill().await;
    }
    Ok(json!({ "bundle": last }))
}

/// What `dev` builds, where it uploads it, and the frontend it points at it.
struct Deploy {
    krate: PathBuf,
    wasm: PathBuf,
    frontend: Option<PathBuf>,
    nodes: Vec<String>,
}

impl Deploy {
    /// Build, upload as a raw bundle, and write the frontend's
    /// `.env.development.local` (Vite restarts on it); answers the bundle hash.
    async fn deploy(&self, env: &Env) -> eyre::Result<String> {
        eprintln!("• building {}", self.krate.display());
        let (k, w) = (self.krate.clone(), self.wasm.clone());
        tokio::task::spawn_blocking(move || -> eyre::Result<()> {
            let built = calimero_app_build::build_components(&k, &k.join("target"))?;
            Ok(std::fs::write(&w, built.app.component)?)
        })
        .await??;
        let answer = crate::app::install_file(env, &self.wasm).await?;
        let bundle = answer["bundle"].as_str().unwrap_or_default().to_owned();
        eprintln!("• uploaded bundle {bundle}");
        if let Some(f) = &self.frontend {
            let path = f.join(".env.development.local");
            let mut file = std::fs::File::create(&path)?;
            writeln!(
                file,
                "# Written by `meroctl local dev`; regenerated on every upload.\nVITE_CALIMERO_TARGET=local\nVITE_CALIMERO_NODES={}\nVITE_CALIMERO_BUNDLE={bundle}",
                self.nodes.join(",")
            )?;
            eprintln!("• wrote {}", path.display());
        }
        Ok(bundle)
    }
}
