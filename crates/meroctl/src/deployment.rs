//! `meroctl deployment`: a developer's deployments, the apps each offers to
//! other organizations (one installed release per app name), the secret
//! values every instance made from it uses, and the customer organizations
//! that made instances from it. Thin wrappers over `/deployments*`; what
//! changes one is approved with a passkey (`--dev-passkey` on a development
//! cluster, else the node answers `412 CONFIRM_REQUIRED`).

use std::io::{IsTerminal as _, Read as _, Write as _};
use std::path::PathBuf;

use calimero_client::types::{Deployer, Deployment, DeploymentOrgsResponse};
use clap::{Subcommand, ValueEnum};
use eyre::{bail, eyre};
use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::env::{to_json, DevPasskey, Env};

#[derive(Debug, Subcommand)]
pub enum DeploymentCmd {
    /// The deployments this account manages: its own, and those of the
    /// organizations where it holds `MANAGE_APPLICATION`.
    #[command(alias = "ls")]
    List,
    /// One deployment, as anyone may read it: its name, its owner's kind and
    /// name, and the release each app it offers runs.
    Get {
        /// The deployment: its id, or the name of one this account manages.
        deployment: String,
    },
    /// Create a deployment, owned by this account or by an organization
    /// where it holds `MANAGE_APPLICATION`. Approved with a passkey.
    Create {
        /// The name customers see.
        name: String,
        /// `account` (this account), `account:<id>` or `org:<id>`.
        #[arg(long, value_parser = parse_owner)]
        owner: Owner,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// Delete a deployment that offers nothing (withdraw its apps first).
    /// Approved with a passkey.
    #[command(alias = "rm")]
    Delete {
        deployment: String,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// Offer an installed release under an app name: new instances get it,
    /// and every instance made from the deployment is proposed it (its
    /// owner approves). Approved with a passkey.
    Offer {
        deployment: String,
        /// The app's name in the deployment.
        app: String,
        /// The release (installed on the cluster: `meroctl app release`).
        version: String,
        /// The release's package (default: the one the app already offers).
        #[arg(long)]
        package: Option<String>,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// Stop offering an app; refused while an instance made from the
    /// deployment runs it. Approved with a passkey.
    Withdraw {
        deployment: String,
        app: String,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// The deployment's secret values: list (fingerprints, never values),
    /// set, clear.
    #[command(subcommand)]
    Secret(SecretCmd),
    /// The customer organizations with instances made from the deployment:
    /// how many, the releases they run, member counts. Never who.
    Orgs { deployment: String },
    /// The deployment's use, per customer organization (`--by org`): its
    /// instances, the releases they run and its members active in the last
    /// 30 days. The node meters fuel and writes per developer, not per
    /// customer organization (`meroctl account usage`, `GET /orgs/{org}/usage`).
    Usage {
        deployment: String,
        /// What one row is.
        #[arg(long, value_enum)]
        by: UsageBy,
        /// From this day (YYYY-MM-DD). Not supported by the node yet: it
        /// keeps no usage over time per customer organization.
        #[arg(long, value_name = "YYYY-MM-DD")]
        since: Option<String>,
        /// Print CSV, one row per customer organization, instead of JSON.
        #[arg(long)]
        csv: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum SecretCmd {
    /// Each secret the offered releases declare, or the deployment holds a
    /// value of: whether it is set, and its fingerprint, who set it and when.
    /// Never a value.
    #[command(alias = "ls")]
    List { deployment: String },
    /// Set the value every instance made from the deployment uses unless it
    /// set its own. The value is read from stdin, or typed at a prompt that
    /// does not echo it; never from the command line. Approved with a passkey.
    Set {
        deployment: String,
        /// The secret's name (A-Z, 0-9 and _), as a release declares it.
        name: String,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// Remove the deployment's value: its instances fall back to the
    /// operator's. Approved with a passkey.
    #[command(alias = "rm")]
    Clear {
        deployment: String,
        name: String,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum UsageBy {
    /// One row per customer organization.
    Org,
}

/// Who owns a new deployment, as `--owner` names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Owner {
    /// The account the request runs as.
    Me,
    Account(String),
    Org(String),
}

fn parse_owner(s: &str) -> Result<Owner, String> {
    match s.split_once(':') {
        None if s == "account" => Ok(Owner::Me),
        Some(("account", id)) if !id.is_empty() => Ok(Owner::Account(id.to_owned())),
        Some(("org", id)) if !id.is_empty() => Ok(Owner::Org(id.to_owned())),
        _ => Err("`account`, `account:<id>` or `org:<id>`".to_owned()),
    }
}

/// The deployment an argument names: the id or the name of one this account
/// manages (`GET /deployments`), else the argument as an id. A name two of
/// them share is refused, naming their ids.
fn resolve<'d>(arg: &str, managed: &'d [Deployment]) -> eyre::Result<Option<&'d Deployment>> {
    if let Some(d) = managed.iter().find(|d| d.id == arg) {
        return Ok(Some(d));
    }
    let named: Vec<&Deployment> = managed.iter().filter(|d| d.name == arg).collect();
    match named.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(one)),
        many => bail!(
            "{} deployments are named {arg:?}: give one's id ({})",
            many.len(),
            many.iter()
                .map(|d| d.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// The deployments this account manages.
async fn managed(env: &Env) -> eyre::Result<Vec<Deployment>> {
    Ok(env.client.deployments().list().await?.deployments)
}

/// The id of the deployment `arg` names ([`resolve`]).
async fn id_of(env: &Env, arg: &str) -> eyre::Result<String> {
    let list = managed(env).await?;
    Ok(resolve(arg, &list)?.map_or_else(|| arg.to_owned(), |d| d.id.clone()))
}

/// The value of a secret: stdin when it is not a terminal (a pipe or a
/// file; one trailing newline dropped), else typed at a prompt on stderr
/// that does not echo it. Never empty.
pub(crate) fn read_secret(name: &str) -> eyre::Result<Zeroizing<String>> {
    let stdin = std::io::stdin();
    let mut value = Zeroizing::new(String::new());
    if stdin.is_terminal() {
        eprint!("{name} (not echoed): ");
        std::io::stderr().flush()?;
        let _echo = NoEcho::on_stdin()?;
        let _n = stdin.read_line(&mut value)?;
        eprintln!();
    } else {
        let _n = stdin.lock().read_to_string(&mut value)?;
    }
    let len = trimmed_len(&value);
    value.truncate(len);
    if value.is_empty() {
        bail!("no value for {name}: pipe it on stdin, or type it at the prompt");
    }
    Ok(value)
}

/// `value` without one trailing `\n` or `\r\n`.
fn trimmed_len(value: &str) -> usize {
    value
        .strip_suffix("\r\n")
        .or_else(|| value.strip_suffix('\n'))
        .unwrap_or(value)
        .len()
}

/// The terminal's echo, off while this lives.
struct NoEcho {
    #[cfg(unix)]
    saved: libc::termios,
}

impl NoEcho {
    #[cfg(unix)]
    fn on_stdin() -> eyre::Result<Self> {
        // SAFETY: `termios` is plain data; tcgetattr fills it in for fd 0, a
        // terminal (checked by the caller), and the result is checked.
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut saved) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut quiet = saved;
        quiet.c_lflag &= !libc::ECHO;
        quiet.c_lflag |= libc::ECHONL;
        // SAFETY: a copy of the settings just read, with echo off.
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &quiet) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Self { saved })
    }

    #[cfg(not(unix))]
    fn on_stdin() -> eyre::Result<Self> {
        bail!("a prompt that does not echo needs a Unix terminal here: pipe the value on stdin")
    }
}

#[cfg(unix)]
impl Drop for NoEcho {
    fn drop(&mut self) {
        // SAFETY: restores the settings `on_stdin` read from the same fd.
        let _ = unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.saved) };
    }
}

/// `deployment usage --by org --csv`: one row per customer organization.
fn usage_csv(orgs: &DeploymentOrgsResponse) -> String {
    let mut out = String::from(
        "org_id,name,instances,members,active_members_30d,releases,first_created_at\n",
    );
    for o in &orgs.orgs {
        let releases: Vec<String> = o
            .apps
            .iter()
            .map(|a| format!("{}@{}", a.app, a.version))
            .collect();
        let row = [
            csv_field(&o.org_id),
            csv_field(o.name.as_deref().unwrap_or_default()),
            o.instances.to_string(),
            o.member_count.to_string(),
            o.active_members30d.to_string(),
            csv_field(&releases.join(" ")),
            csv_field(&o.first_created_at),
        ];
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out
}

/// A CSV field (RFC 4180): quoted when it holds a comma, quote or line
/// break, and with a leading `=`, `+`, `-` or `@` (an organization's name is
/// its creator's text) prefixed with `'` so a spreadsheet never runs it.
fn csv_field(s: &str) -> String {
    let s = if s.starts_with(['=', '+', '-', '@']) {
        format!("'{s}")
    } else {
        s.to_owned()
    };
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

pub async fn run(cmd: DeploymentCmd, env: &Env) -> eyre::Result<Value> {
    let deployments = env.client.deployments();
    Ok(match cmd {
        DeploymentCmd::List => json!({ "deployments": managed(env).await? }),
        DeploymentCmd::Get { deployment } => {
            // `GET /deployments/{id}` is public: without a session, the id.
            let id = if env.api_key.is_some() || env.home.session(&env.url).is_some() {
                id_of(env, &deployment).await?
            } else {
                deployment
            };
            to_json(&deployments.get(&id).await?)?
        }
        DeploymentCmd::Create {
            name,
            owner,
            dev_passkey,
        } => {
            let owner = match owner {
                Owner::Me => Deployer::Account {
                    account: env.client.auth().me().await?.account_id,
                },
                Owner::Account(account) => Deployer::Account { account },
                Owner::Org(org) => Deployer::Org { org },
            };
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = deployments.create(&name, owner, pk.get()).await;
            pk.save()?;
            to_json(&out?)?
        }
        DeploymentCmd::Delete {
            deployment,
            dev_passkey,
        } => {
            let id = id_of(env, &deployment).await?;
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = deployments.delete(&id, pk.get()).await;
            pk.save()?;
            to_json(&out?)?
        }
        DeploymentCmd::Offer {
            deployment,
            app,
            version,
            package,
            dev_passkey,
        } => {
            let list = managed(env).await?;
            let found = resolve(&deployment, &list)?;
            let id = found.map_or_else(|| deployment.clone(), |d| d.id.clone());
            let package = match package {
                Some(p) => p,
                None => found
                    .and_then(|d| d.apps.iter().find(|a| a.app == app))
                    .map(|a| a.package.clone())
                    .ok_or_else(|| {
                        eyre!("{deployment} offers no app {app:?} yet: name its package with --package")
                    })?,
            };
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = deployments
                .offer(&id, &app, &package, &version, pk.get())
                .await;
            pk.save()?;
            to_json(&out?)?
        }
        DeploymentCmd::Withdraw {
            deployment,
            app,
            dev_passkey,
        } => {
            let id = id_of(env, &deployment).await?;
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = deployments.withdraw(&id, &app, pk.get()).await;
            pk.save()?;
            to_json(&out?)?
        }
        DeploymentCmd::Secret(SecretCmd::List { deployment }) => {
            to_json(&deployments.secrets(&id_of(env, &deployment).await?).await?)?
        }
        DeploymentCmd::Secret(SecretCmd::Set {
            deployment,
            name,
            dev_passkey,
        }) => {
            let id = id_of(env, &deployment).await?;
            // Loaded before the value is asked for: a wrong path fails first.
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let value = read_secret(&name)?;
            let out = deployments.secret_set(&id, &name, &value, pk.get()).await;
            pk.save()?;
            to_json(&out?)?
        }
        DeploymentCmd::Secret(SecretCmd::Clear {
            deployment,
            name,
            dev_passkey,
        }) => {
            let id = id_of(env, &deployment).await?;
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = deployments.secret_clear(&id, &name, pk.get()).await;
            pk.save()?;
            to_json(&out?)?
        }
        DeploymentCmd::Orgs { deployment } => {
            to_json(&deployments.orgs(&id_of(env, &deployment).await?).await?)?
        }
        DeploymentCmd::Usage {
            deployment,
            by: UsageBy::Org,
            since,
            csv,
        } => {
            if let Some(since) = since {
                bail!(
                    "--since {since}: not supported by the node yet. It keeps no usage over time \
                     per customer organization: `GET /deployments/{{id}}/orgs` is today's \
                     counts, and fuel and writes are metered per developer for the current \
                     day only (`meroctl account usage`, `meroctl raw get /orgs/ORG/usage`)"
                );
            }
            let orgs = deployments.orgs(&id_of(env, &deployment).await?).await?;
            if csv {
                print!("{}", usage_csv(&orgs));
                if orgs.outside_orgs > 0 {
                    eprintln!(
                        "note: {} more instances are in personal organizations, counted only",
                        orgs.outside_orgs
                    );
                }
                return Ok(Value::Null);
            }
            json!({ "deployment": orgs.deployment, "by": "org", "orgs": orgs.orgs, "outsideOrgs": orgs.outside_orgs })
        }
    })
}

#[cfg(test)]
mod tests {
    use calimero_client::types::{
        DeploymentAppsItem, DeploymentOrgsResponseOrgsItem, DeploymentOrgsResponseOrgsItemAppsItem,
    };
    use serde_json::Map;

    use super::*;

    fn deployment(id: &str, name: &str) -> Deployment {
        Deployment {
            id: id.to_owned(),
            host: None,
            name: name.to_owned(),
            owner: Deployer::Org {
                org: "acme".to_owned(),
            },
            apps: vec![DeploymentAppsItem {
                app: "notes".to_owned(),
                package: "com.acme.notes".to_owned(),
                version: "1.3.0".to_owned(),
                ..DeploymentAppsItem::default()
            }],
            created_by: "a".to_owned(),
            created_at: "2026-11-01T00:00:00Z".to_owned(),
            extra: Map::new(),
        }
    }

    #[test]
    fn an_owner_is_this_account_another_account_or_an_org() {
        assert_eq!(parse_owner("account"), Ok(Owner::Me));
        assert_eq!(parse_owner("account:abc"), Ok(Owner::Account("abc".into())));
        assert_eq!(parse_owner("org:acme"), Ok(Owner::Org("acme".into())));
        for bad in ["org", "org:", "account:", "team:x", "acme"] {
            assert!(parse_owner(bad).is_err(), "{bad}");
        }
    }

    /// An id wins over a name; a name two deployments share is refused with
    /// their ids; anything else is taken as an id.
    #[test]
    fn a_deployment_is_named_by_its_id_or_its_name() {
        let list = vec![
            deployment("d1", "acme-apps"),
            deployment("d2", "beta"),
            deployment("d3", "beta"),
            deployment("d4", "d1"),
        ];
        assert_eq!(resolve("acme-apps", &list).unwrap().unwrap().id, "d1");
        assert_eq!(resolve("d2", &list).unwrap().unwrap().id, "d2");
        assert_eq!(resolve("d1", &list).unwrap().unwrap().id, "d1");
        assert!(resolve("unknown", &list).unwrap().is_none());
        let shared = resolve("beta", &list).unwrap_err().to_string();
        assert!(shared.contains("d2, d3"), "{shared}");
    }

    #[test]
    fn a_piped_value_loses_one_trailing_newline_only() {
        assert_eq!(trimmed_len("sk-1\n"), 4);
        assert_eq!(trimmed_len("sk-1\r\n"), 4);
        assert_eq!(trimmed_len("sk-1\n\n"), 5);
        assert_eq!(trimmed_len("sk-1"), 4);
    }

    /// One row per customer organization; a name with a comma or quote is
    /// quoted, and one a spreadsheet would run as a formula is defused.
    #[test]
    fn usage_csv_has_a_row_per_customer_org() {
        let orgs = DeploymentOrgsResponse {
            deployment: "d1".to_owned(),
            orgs: vec![
                DeploymentOrgsResponseOrgsItem {
                    org_id: "o1".to_owned(),
                    name: Some("Globex, \"Inc\"".to_owned()),
                    member_count: 14,
                    active_members30d: 9,
                    instances: 2,
                    apps: vec![
                        DeploymentOrgsResponseOrgsItemAppsItem {
                            app: "notes".to_owned(),
                            version: "1.3.0".to_owned(),
                            ..Default::default()
                        },
                        DeploymentOrgsResponseOrgsItemAppsItem {
                            app: "notes".to_owned(),
                            version: "1.4.0".to_owned(),
                            ..Default::default()
                        },
                    ],
                    first_created_at: "2026-11-02T09:12:44Z".to_owned(),
                    ..Default::default()
                },
                DeploymentOrgsResponseOrgsItem {
                    org_id: "o2".to_owned(),
                    name: Some("=HYPERLINK(\"x\")".to_owned()),
                    ..Default::default()
                },
            ],
            outside_orgs: 3,
            ..Default::default()
        };
        let csv = usage_csv(&orgs);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(
            lines,
            [
                "org_id,name,instances,members,active_members_30d,releases,first_created_at",
                "o1,\"Globex, \"\"Inc\"\"\",2,14,9,notes@1.3.0 notes@1.4.0,2026-11-02T09:12:44Z",
                "o2,\"'=HYPERLINK(\"\"x\"\")\",0,0,0,,",
            ]
        );
    }
}
