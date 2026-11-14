//! `meroctl dev`: v1's developer workflow. `new` starts a project from an app
//! of the apps repository; `start` installs a build, creates a context from it
//! and (with `--watch`) keeps pushing it on every save.

use std::path::{Path, PathBuf};
use std::process::Command;

use calimero_client::types::AppCreateRequest;
use clap::Subcommand;

use eyre::{bail, WrapErr};
use serde_json::{json, Value};

use crate::env::{json_arg, Env};

/// Where `dev new` takes its templates from: every component app there is one.
pub const APPS_REPO: &str = "https://github.com/calimero-network/apps";

/// What a project keeps of the apps repository besides its app: the shared
/// frontend packages (`@calimero-apps/connect`, `passkey`, `test-setup`, and
/// `merod-local`, which `pnpm dev:local` runs) and `scripts/` (its
/// `component-build.sh` builds the contract and writes `res/abi.json`, and
/// `install-meroctl.sh` pins meroctl). Cone mode adds the root files (`package.json`,
/// `pnpm-workspace.yaml` with its catalogs, `pnpm-lock.yaml`,
/// `tsconfig.base.json`, `Cargo.toml` with the core tag), so the app's
/// `catalog:` and `workspace:*` dependencies and relative paths resolve as
/// they do upstream.
const SHARED: [&str; 2] = ["packages", "scripts"];

#[derive(Debug, Subcommand)]
pub enum DevCmd {
    /// `meroctl app new`, under its old name.
    #[command(hide = true)]
    New {
        dir: PathBuf,
        /// The app under `apps/` to start from.
        #[arg(long, default_value = "kv-store")]
        template: String,
        /// The branch or tag of the apps repository.
        #[arg(long = "ref", default_value = "main")]
        git_ref: String,
        /// The apps repository to clone.
        #[arg(long, default_value = APPS_REPO, hide = true)]
        repo: String,
        /// Skip `git init` in the new project.
        #[arg(long)]
        no_git: bool,
    },
    /// Install a build (`.mpk`, or a development `.wasm`), create a context
    /// from it, and with `--watch` push every save to it.
    Start {
        #[arg(long)]
        path: PathBuf,
        /// The context's name.
        #[arg(long, default_value = "dev")]
        name: String,
        /// Arguments for the app's `init`, JSON.
        #[arg(long, short)]
        params: Option<String>,
        #[arg(long, short)]
        watch: bool,
        /// The owner's passkey file that approves each update.
        #[arg(long)]
        dev_passkey: PathBuf,
    },
    /// Push a build to its contexts on every save (as `meroctl app watch`).
    Watch {
        #[arg(long)]
        path: PathBuf,
        #[arg(long = "context", short)]
        contexts: Vec<String>,
        #[arg(long)]
        dev_passkey: PathBuf,
        #[arg(long)]
        once: bool,
        #[arg(long, default_value_t = 500)]
        interval_ms: u64,
    },
}

pub async fn run(cmd: DevCmd, env: &Env) -> eyre::Result<()> {
    match cmd {
        DevCmd::New { .. } => unreachable!("`dev new` runs before connecting (main)"),
        DevCmd::Start {
            path,
            name,
            params,
            watch,
            dev_passkey,
        } => {
            let installed = crate::app::install_file(env, &path).await?;
            let text = |k: &str| installed[k].as_str().map(str::to_owned);
            let mut body = AppCreateRequest {
                name: Some(name),
                init_args: params.as_deref().map(json_arg).transpose()?,
                ..AppCreateRequest::default()
            };
            match text("bundle") {
                Some(b) => body.bundle = Some(b),
                None => {
                    body.package = text("package");
                    body.version = text("version");
                }
            }
            let created = env.client.apps().create(body).await?;
            println!("{}", serde_json::to_string(&created)?);
            if watch {
                let app = created.app_id;
                // The build is installed already: the first push is the next save.
                crate::app::watch(env, &path, &[app], &dev_passkey, false, 500, false).await?;
            }
            Ok(())
        }
        DevCmd::Watch {
            path,
            contexts,
            dev_passkey,
            once,
            interval_ms,
        } => crate::app::watch(env, &path, &contexts, &dev_passkey, once, interval_ms, true).await,
    }
}

/// `meroctl dev new`: a sparse, shallow clone of `repo` at `git_ref` holding
/// `apps/<template>`, [`SHARED`] and the root files, with its history dropped.
/// Nothing is rewritten, so the project builds and runs with the commands the
/// app's README gives (`pnpm install`, `pnpm dev:local <template>`).
pub fn new(
    dir: &Path,
    template: &str,
    git_ref: &str,
    repo: &str,
    git_init: bool,
) -> eyre::Result<Value> {
    if template.is_empty()
        || !template
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("--template must be the name of an app under apps/ (letters, digits, - and _)");
    }
    if dir.exists() && std::fs::read_dir(dir)?.next().is_some() {
        bail!("{} exists and is not empty", dir.display());
    }
    let created = !dir.exists();
    let out = scaffold(dir, template, git_ref, repo, git_init);
    if out.is_err() {
        // Leave nothing half-made behind: only what this call created, or the
        // contents of the empty directory it was given.
        if created {
            let _ = std::fs::remove_dir_all(dir);
        } else if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                let _ = if p.is_dir() {
                    std::fs::remove_dir_all(&p)
                } else {
                    std::fs::remove_file(&p)
                };
            }
        }
    }
    out
}

fn scaffold(
    dir: &Path,
    template: &str,
    git_ref: &str,
    repo: &str,
    git_init: bool,
) -> eyre::Result<Value> {
    let app = format!("apps/{template}");
    let dir_s = dir
        .to_str()
        .ok_or_else(|| eyre::eyre!("DIR must be UTF-8"))?;
    git(
        None,
        &[
            "clone",
            "--quiet",
            "--depth=1",
            "--filter=blob:none",
            "--sparse",
            "--no-tags",
            "--branch",
            git_ref,
            repo,
            dir_s,
        ],
    )
    .wrap_err_with(|| format!("cloning {repo} at {git_ref}"))?;
    let mut set = vec!["sparse-checkout", "set", "--cone", app.as_str()];
    set.extend(SHARED);
    git(Some(dir), &set)?;
    if !dir.join(&app).join("logic/Cargo.toml").is_file() {
        bail!("{repo} at {git_ref} has no app {app} (no {app}/logic/Cargo.toml)");
    }
    let commit = git_output(dir, &["rev-parse", "HEAD"])?;
    std::fs::remove_dir_all(dir.join(".git"))?;
    std::fs::write(
        dir.join("README.md"),
        readme(template, repo, git_ref, &commit),
    )?;
    if git_init {
        git(Some(dir), &["init", "--quiet"])?;
        // One commit of what the template gave, so the project's own changes
        // show as changes. Without a git identity there is none to make it
        // with: the files stay untracked, as `git init` leaves them.
        let message = format!("Start from {repo} {app} ({commit})");
        let committed = Command::new("git")
            .current_dir(dir)
            .args(["add", "-A"])
            .status()
            .is_ok_and(|s| s.success())
            && Command::new("git")
                .current_dir(dir)
                .args(["commit", "--quiet", "-m", &message])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
        if !committed {
            eprintln!("note: no first commit (set git's user.name and user.email to get one)");
        }
    }
    eprintln!(
        "\nNext:\n  cd {dir_s}\n  rustup target add wasm32-wasip2\n  \
         meroctl local dev {app}    # a local cluster, build, deploy, the frontend on http://localhost:5173/\n\n\
         {app}/README.md has the rest."
    );
    Ok(json!({
        "path": dir_s,
        "app": app,
        "from": { "repo": repo, "ref": git_ref, "commit": commit },
    }))
}

fn readme(template: &str, repo: &str, git_ref: &str, commit: &str) -> String {
    format!(
        "# {template}\n\n\
         Started with `meroctl dev new` from [`apps/{template}`]({repo}/tree/{commit}/apps/{template}) \
         ({git_ref}, {commit}).\n\n\
         The app is in `apps/{template}`: its contract in `logic/`, its frontend in `app/`; \
         its README says how to run and test it. `packages/` holds the shared frontend \
         packages it uses, `scripts/component-build.sh` builds the contract and writes its \
         `res/abi.json`, and the root `pnpm-workspace.yaml` and `Cargo.toml` pin the \
         versions, as in the apps repository.\n\n\
         ```bash\n\
         rustup target add wasm32-wasip2\n\
         meroctl local dev apps/{template}\n\
         ```\n"
    )
}

fn git(cwd: Option<&Path>, args: &[&str]) -> eyre::Result<()> {
    let mut cmd = Command::new("git");
    if let Some(cwd) = cwd {
        let _ = cmd.current_dir(cwd);
    }
    let status = cmd
        .args(args)
        .status()
        .wrap_err("running git (is it installed and on PATH?)")?;
    if !status.success() {
        bail!("git {} failed ({status})", args.join(" "));
    }
    Ok(())
}

fn git_output(cwd: &Path, args: &[&str]) -> eyre::Result<String> {
    let out = Command::new("git").current_dir(cwd).args(args).output()?;
    if !out.status.success() {
        bail!("git {} failed ({})", args.join(" "), out.status);
    }
    Ok(String::from_utf8(out.stdout)?.trim().to_owned())
}
