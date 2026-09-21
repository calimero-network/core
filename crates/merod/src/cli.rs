use std::path::{Path, PathBuf};

use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use const_format::concatcp;
use eyre::Result as EyreResult;

use crate::defaults;

mod account;
mod admin_creds;
mod auth;
mod auth_mode;
mod config;
mod init;
mod kms;
mod run;
mod tee;
mod validation;

use account::AccountCommand;
use auth::AuthCommand;
use config::ConfigCommand;
use init::InitCommand;
use kms::KmsCommand;
use run::RunCommand;
use tee::TeeCommand;

pub const EXAMPLES: &str = concat!(
    r"
  # Initialize node
  $ merod --node node1 init --server-port 2428 --swarm-port 2528

  # Initialize node with a custom home directory data
  $ mkdir data
  $ merod --home data/ --node node1 init

  # Configure an existing node (key=value; use TOML paths).
  # Quote the argument in zsh so [ ] are not globbed:
  $ merod --node node1 config ",
    "\"",
    r"server.listen=['/ip4/127.0.0.2/tcp/3000', '/ip6/::1/tcp/3000']",
    "\"",
    r"

  # Run a node
  $ merod --node node1 run
",
);

#[derive(Debug, Parser)]
#[command(
    author,
    version = const_format::formatcp!(
        "{} (build {}) (commit {}) (rustc {})",
        env!("MEROD_VERSION"),
        env!("MEROD_BUILD"),
        env!("MEROD_COMMIT"),
        env!("MEROD_RUSTC_VERSION")
    ),
    about,
    long_about = None
)]
#[command(after_help = concatcp!(
    "Environment variables:\n",
    "  CALIMERO_HOME    Directory for config and data\n\n",
    "Examples:",
    EXAMPLES
))]
pub struct RootCommand {
    #[command(flatten)]
    pub args: RootArgs,

    #[command(subcommand)]
    pub action: SubCommands,
}

#[derive(Debug, Subcommand)]
pub enum SubCommands {
    Account(AccountCommand),
    Auth(AuthCommand),
    Config(ConfigCommand),
    Init(InitCommand),
    Kms(KmsCommand),
    #[command(alias = "up")]
    Run(RunCommand),
    Tee(TeeCommand),
}

#[derive(Debug, Parser)]
pub struct RootArgs {
    /// Directory for config and data
    #[arg(long, value_name = "PATH", default_value_t = defaults::default_node_dir())]
    #[arg(env = "CALIMERO_HOME", hide_env_values = true)]
    pub home: Utf8PathBuf,

    /// Name of node. Required by everything that reads a node's config or
    /// store; omit it for the `account` subcommands that sign offline.
    ///
    /// Optional because several commands here touch no node at all. `account
    /// warrant` and `account login-statement` are pure functions of their
    /// flags, and `account sign-cert`/`revoke-proof`/`sign-with-root` reach the
    /// account root from `--from <PHRASE>` without opening anything. Demanding
    /// a name they never read meant naming a node that need not exist — and a
    /// caller who obliged with a real one could reasonably think the command
    /// had consulted it.
    #[arg(short = 'n', long = "node", value_name = "NAME")]
    pub node_name: Option<Utf8PathBuf>,
}

impl RootArgs {
    /// The home directory of the node this command operates on.
    ///
    /// The single place `--node` becomes a path, so a command that needs a node
    /// fails on the missing name rather than on whatever it found at
    /// `$CALIMERO_HOME/` — `home` has a default, so joining an absent name
    /// would silently address the parent of every node home.
    pub fn node_home(&self) -> EyreResult<Utf8PathBuf> {
        let name = self.node_name.as_ref().ok_or_else(|| {
            eyre::eyre!("--node <NAME> is required: this command operates on one node's home")
        })?;

        Ok(self.home.join(name))
    }
}

/// Resolve a path from config against the node home: relative paths (the
/// default, e.g. the `auth` storage dir) live under the node home; absolute
/// paths are honored as-is. The single source of truth for this rule —
/// `init`, `run`, and `auth set-admin` must all resolve identically or they
/// operate on different databases.
pub(crate) fn resolve_node_relative_path(node_home: &Path, path: PathBuf) -> PathBuf {
    if path.is_relative() {
        node_home.join(path)
    } else {
        path
    }
}

impl RootCommand {
    pub async fn run(self) -> EyreResult<()> {
        match self.action {
            SubCommands::Account(account) => account.run(&self.args).await,
            SubCommands::Auth(auth) => auth.run(&self.args).await,
            SubCommands::Config(config) => config.run(&self.args).await,
            SubCommands::Init(init) => init.run(self.args).await,
            SubCommands::Kms(kms) => kms.run(&self.args).await,
            SubCommands::Run(run) => run.run(self.args).await,
            SubCommands::Tee(tee) => tee.run(&self.args).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{RootArgs, RootCommand};

    /// The offline signers must parse with no `--node`.
    ///
    /// They were unreachable without one: `--node` was required, so the
    /// cold-storage path — a root on paper and no node anywhere — had to name a
    /// node that need not exist, and a bogus name was accepted precisely
    /// because nothing read it. The e2e harness carried a `find … -name
    /// config.toml` helper for exactly this, hunting a home for commands that
    /// open none.
    #[test]
    fn offline_account_commands_parse_without_a_node() {
        for args in [
            vec![
                "merod",
                "account",
                "sign-with-root",
                "--domain",
                "mdma.account-login",
                "--payload",
                "6e6f6e6365",
                "--from",
                "phrase.txt",
            ],
            vec![
                "merod",
                "account",
                "warrant",
                "--context",
                "11111111111111111111111111111111",
                "--executor",
                &"b".repeat(64),
                "--method",
                "set",
                "--args",
                "{}",
                "--nonce",
                "1",
                "--valid-for",
                "300",
                "--device-secret",
                &"c".repeat(64),
                "--credential",
                &"d".repeat(64),
            ],
        ] {
            let parsed = RootCommand::try_parse_from(&args);
            assert!(
                parsed.is_ok(),
                "`{}` must parse with no --node: it opens no store\n{}",
                args[2],
                parsed.err().map(|e| e.to_string()).unwrap_or_default(),
            );
        }
    }

    /// And `--node` must still be demanded by anything that reads a node.
    ///
    /// Optional at parse time is not optional in effect: `home` carries a
    /// default, so a missing name that reached `join` would address the parent
    /// of every node home rather than one node.
    #[test]
    fn node_home_refuses_a_missing_node_name() {
        let args = RootArgs {
            home: camino::Utf8PathBuf::from("/tmp/calimero"),
            node_name: None,
        };

        let err = args
            .node_home()
            .expect_err("no node name can yield no node home");
        assert!(
            err.to_string().contains("--node"),
            "the error must name the missing flag, got: {err}",
        );

        let named = RootArgs {
            home: camino::Utf8PathBuf::from("/tmp/calimero"),
            node_name: Some(camino::Utf8PathBuf::from("node1")),
        };
        assert_eq!(
            named.node_home().expect("a named node resolves"),
            camino::Utf8PathBuf::from("/tmp/calimero/node1"),
        );
    }
}
