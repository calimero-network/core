//! `meroctl local`: a development cluster on this machine and the loop around
//! it. `up` starts `merod local` in the background (the release's container
//! image, or with `--local` the `merod` on PATH or in `MEROD`) and saves it as the node
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
    /// Where the cluster keeps its state, kept across restarts (default:
    /// `{home}/meroctl/local`, the same from every directory).
    #[arg(long)]
    pub dir: Option<PathBuf>,
    /// Start from empty state.
    #[arg(long)]
    pub fresh: bool,
    /// Run `merod` from PATH (or `MEROD`) instead of the release's container
    /// image. Setting `MEROD` implies it.
    #[arg(long)]
    pub local: bool,
    /// The release's container image, the default: kept so older scripts
    /// still parse.
    #[arg(long, hide = true, conflicts_with = "local")]
    pub docker: bool,
    /// The image the node runs in (`merod local --bind 0.0.0.0`, its ports
    /// published on 127.0.0.1).
    #[arg(long, default_value_t = default_image())]
    pub image: String,
    /// The package registry the cluster installs from, so `meroctl app install
    /// PACKAGE@VERSION` works on it (empty: none; `app install --path` still does).
    #[arg(long, env = "MEROD_REGISTRY_URL", default_value = DEFAULT_REGISTRY)]
    pub registry_url: String,
}

fn default_image() -> String {
    format!(
        "ghcr.io/calimero-network/merod:{}",
        env!("CALIMERO_RELEASE_VERSION")
    )
}

#[derive(Debug, Subcommand)]
pub enum LocalCmd {
    /// Start a development cluster in the background and save it as the node
    /// `local` (mock attestation, a development storage key: never for real data).
    Up(ClusterArgs),
    /// Stop the cluster `up` started; its state stays for the next `up`.
    Down {
        /// The cluster's state directory (default: the one `up` uses).
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Whether the cluster runs, and its nodes.
    Status {
        /// The cluster's state directory (default: the one `up` uses).
        #[arg(long)]
        dir: Option<PathBuf>,
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
    TcpStream::connect_timeout(
        &addr.parse().expect("127.0.0.1:port"),
        Duration::from_millis(300),
    )
    .is_ok()
}

/// Where a cluster keeps its state when `--dir` names none: under the meroctl
/// home, so `up` in one directory and `down` in another find the same cluster.
fn default_dir(home: &Home) -> PathBuf {
    home.dir.join("meroctl").join("local")
}

/// The state directory `down` and `status` act on: `--dir`, else the default;
/// failing that, a cluster an older meroctl started in `./.merod-local`.
fn found_dir(home: &Home, dir: Option<PathBuf>) -> PathBuf {
    dir.unwrap_or_else(|| {
        let default = default_dir(home);
        let legacy = PathBuf::from(".merod-local");
        if read_state(&default).is_none() && read_state(&legacy).is_some() {
            legacy
        } else {
            default
        }
    })
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
    let state_dir = a.dir.clone().unwrap_or_else(|| default_dir(home));
    if let Some(state) = read_state(&state_dir).filter(alive) {
        if a.fresh {
            bail!(
                "a cluster is running from {}: `meroctl local down` first",
                state_dir.display()
            );
        }
        save_node(home, &state.urls[0])?;
        return Ok((describe(&state_dir, &state, true), None));
    }
    let urls = urls(a.nodes, a.base_port);
    if let Some(busy) = urls.iter().find(|u| listening(u)) {
        bail!("{busy} is taken by another process: stop it, or pass --base-port");
    }
    std::fs::create_dir_all(&state_dir)?;
    let dir = std::path::absolute(&state_dir)?;
    let token = hex::encode(calimero_identity::random::<16>());
    let log = dir.join("merod.log");
    let mut local = merod_local_args(a, &token);
    let out = std::fs::File::create(&log)?;
    // The release's image unless `--local` (or MEROD) asks for a `merod` binary:
    // only Docker is needed, on Linux and macOS alike.
    let docker = !a.local && std::env::var_os("MEROD").is_none();
    if docker {
        docker_ready()?;
    }
    let (pid, container, child) = if docker {
        // The image's merod writes as uid 10001 under /data, a named volume; it
        // listens on 0.0.0.0 inside the container and the ports are published on
        // loopback only, so this works on macOS and Linux alike.
        local.extend([
            "--dir".into(),
            "/data/cluster".into(),
            "--bind".into(),
            "0.0.0.0".into(),
        ]);
        let name = format!("meroctl-local-{}", a.base_port);
        let _ = Command::new("docker")
            .args(["rm", "-f", &name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let first = a.base_port + 1;
        let last = a.base_port + a.nodes;
        let mut cmd = Command::new("docker");
        let _ = cmd
            .args(["run", "-d", "--name", &name, "-p"])
            .arg(format!("127.0.0.1:{first}-{last}:{first}-{last}"))
            // A named volume: the image's user (uid 10001) cannot write a
            // host directory owned by someone else.
            .arg("-v")
            .arg(format!("{name}:/data"))
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
                    "starting {} (with --local, put merod on PATH or set MEROD; without it the node runs in Docker)",
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
    write_private(
        &dir.join(STATE_FILE),
        &serde_json::to_string_pretty(&state)?,
    )?;
    // Only a caller that owns the cluster (`dev`) stops it when done.
    let started = own.then(|| Started {
        dir: state_dir.clone(),
        child,
    });
    let deadline = Instant::now() + READY_TIMEOUT;
    // `merod local` prints its ops token once every node has started.
    let printed_ready = || std::fs::read_to_string(&log).is_ok_and(|t| t.contains("ops token"));
    while !(alive(&state) && printed_ready()) {
        if Instant::now() > deadline {
            bail!(
                "the cluster did not start in {READY_TIMEOUT:?}; its log: {}",
                log.display()
            );
        }
        if let Some(pid) = state.pid {
            if !Path::new(&format!("/proc/{pid}")).exists() && cfg!(target_os = "linux") {
                bail!(
                    "merod local exited; its log: {}\n{}",
                    log.display(),
                    tail(&log)
                );
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    save_node(home, &urls[0])?;
    let answer = describe(&state_dir, &state, true);
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
        LocalCmd::Down { dir } => stop(&found_dir(&home, dir)),
        LocalCmd::Status { dir } => {
            let dir = found_dir(&home, dir);
            Ok(match read_state(&dir) {
                Some(state) => {
                    let running = alive(&state);
                    describe(&dir, &state, running)
                }
                None => json!({ "running": false }),
            })
        }
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
        bail!(
            "{} holds no crate (no Cargo.toml, nor logic/Cargo.toml)",
            a.app.display()
        );
    }
    let frontend =
        Some(a.app.join("app")).filter(|f| !a.no_frontend && f.join("package.json").is_file());

    let (cluster, _owned) = up(&a.cluster, &home, true)?;
    let url = cluster["urls"][0]
        .as_str()
        .ok_or_else(|| eyre!("no node"))?
        .to_owned();
    eprintln!(
        "• cluster: {} (log: {})",
        url,
        cluster["log"].as_str().unwrap_or_default()
    );

    // A deployer account of its own: a raw bundle is visible to every account
    // on a development cluster, so the person signing in to the frontend
    // creates their instance from it.
    let env = Env::connect(
        Home {
            dir: home.dir.clone(),
        },
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

    // The build's output stays in the target dir, not beside the sources.
    let wasm = std::path::absolute(crate::make::target_dir(&krate).join(crate::make::APP_FILE))?;
    let nodes: Vec<String> = cluster["urls"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
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
                    .wrap_err(
                        "running pnpm (install the frontend's dependencies first: pnpm install)",
                    )?,
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
            let built = calimero_app_build::build_components(&k, &crate::make::target_dir(&k))?;
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

/// The public app registry, which `app publish` publishes to.
const DEFAULT_REGISTRY: &str = "https://apps.calimero.network";

/// `merod local`'s arguments for `up`, before where it keeps its state and
/// what it binds (those differ between the image and a binary).
fn merod_local_args(a: &ClusterArgs, token: &str) -> Vec<String> {
    let mut local = vec![
        "local".to_owned(),
        "--nodes".into(),
        a.nodes.to_string(),
        "--base-port".into(),
        a.base_port.to_string(),
        "--ops-token".into(),
        token.to_owned(),
    ];
    if a.fresh {
        local.push("--fresh".into());
    }
    if !a.registry_url.is_empty() {
        local.extend(["--registry-url".into(), a.registry_url.clone()]);
    }
    local
}

/// Docker, which runs the node by default: installed, and its daemon up.
fn docker_ready() -> eyre::Result<()> {
    let out = Command::new("docker")
        .args(["info", "--format", "{{.ServerVersion}}"])
        .stdin(Stdio::null())
        .output();
    match out {
        Err(_) => bail!(
            "the development cluster runs in Docker, which is not installed \
             (https://docs.docker.com/get-docker/); or pass --local to run merod from PATH"
        ),
        Ok(o) if !o.status.success() => bail!(
            "Docker is installed but not running: start Docker Desktop or the docker \
             service; or pass --local to run merod from PATH\n{}",
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Ok(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        cluster: ClusterArgs,
    }

    fn args(argv: &[&str]) -> Vec<String> {
        let cli = Cli::try_parse_from(std::iter::once("up").chain(argv.iter().copied())).unwrap();
        merod_local_args(&cli.cluster, "t")
    }

    // `meroctl app install PACKAGE@VERSION` on a fresh `local up` answered
    // `400 NO_REGISTRY`: the cluster was started without a registry.
    #[test]
    fn the_cluster_installs_from_the_public_registry_unless_told_otherwise() {
        let default = args(&[]);
        let at = default.iter().position(|a| a == "--registry-url").unwrap();
        assert_eq!(default[at + 1], DEFAULT_REGISTRY);
        let other = args(&["--registry-url", "http://127.0.0.1:8080"]);
        assert!(other
            .windows(2)
            .any(|w| w == ["--registry-url", "http://127.0.0.1:8080"]));
        assert!(!args(&["--registry-url", ""]).contains(&"--registry-url".to_owned()));
    }
}
