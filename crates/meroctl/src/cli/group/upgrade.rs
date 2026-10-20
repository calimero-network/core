use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::{
    ActivateGroupUpgradeApiRequest, ActivateGroupUpgradeApiResponseData,
    RetryGroupUpgradeApiRequest, UpgradeGroupApiRequest,
};
use clap::{Parser, Subcommand};
use eyre::{bail, Result};

use crate::cli::Environment;

#[derive(Debug, Parser)]
#[command(about = "Manage group upgrades")]
pub struct UpgradeCommand {
    #[command(subcommand)]
    pub subcommand: UpgradeSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum UpgradeSubCommands {
    #[command(about = "Trigger an upgrade for a group")]
    Trigger(TriggerUpgradeCommand),
    #[command(about = "Get the current upgrade status of a group")]
    Status(UpgradeStatusCommand),
    #[command(about = "Retry a failed group upgrade")]
    Retry(RetryUpgradeCommand),
    #[command(about = "Switch this node's contexts of a namespace to its stored target release")]
    Activate(ActivateUpgradeCommand),
}

impl UpgradeCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.subcommand {
            UpgradeSubCommands::Trigger(cmd) => cmd.run(environment).await,
            UpgradeSubCommands::Status(cmd) => cmd.run(environment).await,
            UpgradeSubCommands::Retry(cmd) => cmd.run(environment).await,
            UpgradeSubCommands::Activate(cmd) => cmd.run(environment).await,
        }
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Trigger an upgrade for a group to a new application version")]
pub struct TriggerUpgradeCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(long, help = "Target application ID to upgrade to")]
    pub target_application_id: ApplicationId,

    #[clap(
        long,
        help = "Emit a cascade upgrade that fans out to every descendant subgroup whose \
                current bytecode_id matches the signed group's bytecode_id. Default: false."
    )]
    pub cascade: bool,

    #[clap(
        long,
        help = "Proceed code-only when the target build has no embedded ABI (asserts the \
                update is layout-compatible). Without this, a missing target ABI refuses \
                the upgrade. Default: false."
    )]
    pub force_code_only: bool,

    #[clap(
        long,
        help = "Publish without first trialling the migration on this node's contexts. \
                Default: false."
    )]
    pub skip_trial: bool,
}

impl TriggerUpgradeCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = UpgradeGroupApiRequest {
            target_application_id: self.target_application_id,
            cascade: self.cascade,
            force_code_only: self.force_code_only,
            skip_trial: self.skip_trial,
        };

        let client = environment.client()?;
        let response = client.upgrade_group(&self.group_id, request).await?;

        environment.output.write(&response);

        Ok(())
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Get the current upgrade status of a group")]
pub struct UpgradeStatusCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,
}

impl UpgradeStatusCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client.get_group_upgrade_status(&self.group_id).await?;

        environment.output.write(&response);

        Ok(())
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Retry a failed group upgrade")]
pub struct RetryUpgradeCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,
}

impl RetryUpgradeCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = RetryGroupUpgradeApiRequest {};

        let client = environment.client()?;
        let response = client.retry_group_upgrade(&self.group_id, request).await?;

        environment.output.write(&response);

        Ok(())
    }
}

#[derive(Clone, Debug, Parser)]
#[command(
    about = "Switch this node's contexts of a namespace to its stored target release and \
             replay the deltas held for them; the release must already be installed"
)]
pub struct ActivateUpgradeCommand {
    #[clap(name = "NAMESPACE_ID", help = "The hex-encoded namespace ID")]
    pub namespace_id: String,

    #[clap(
        long,
        help = "Refuse unless the namespace's stored target is this application ID"
    )]
    pub expect_application_id: Option<ApplicationId>,

    #[clap(
        long,
        help = "Refuse unless the namespace's stored target release is this version"
    )]
    pub expect_version: Option<String>,
}

impl ActivateUpgradeCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = ActivateGroupUpgradeApiRequest {
            expected_application_id: self.expect_application_id,
            expected_version: self.expect_version,
        };

        let client = environment.client()?;
        let response = client
            .activate_group_upgrade(&self.namespace_id, request)
            .await?;

        environment.output.write(&response);

        ensure_all_switched(&response.data)
    }
}

/// A non-zero exit when any context did not switch, so a script notices.
fn ensure_all_switched(data: &ActivateGroupUpgradeApiResponseData) -> Result<()> {
    if data.failed_contexts.is_empty() {
        return Ok(());
    }
    bail!(
        "{} context(s) did not switch; running this again retries only those",
        data.failed_contexts.len()
    )
}

#[cfg(test)]
mod tests {
    use calimero_primitives::context::ContextId;
    use calimero_server_primitives::admin::ActivateGroupUpgradeFailedContext;

    use super::*;

    fn activate(args: &[&str]) -> ActivateUpgradeCommand {
        let argv = ["upgrade", "activate"]
            .into_iter()
            .chain(args.iter().copied());
        match UpgradeCommand::try_parse_from(argv).unwrap().subcommand {
            UpgradeSubCommands::Activate(cmd) => cmd,
            other => panic!("expected activate, got {other:?}"),
        }
    }

    #[test]
    fn activate_takes_a_namespace_and_an_optional_expected_application() {
        let app = ApplicationId::from([0xAA; 32]);

        let bare = activate(&["ns1"]);
        assert_eq!(bare.namespace_id, "ns1");
        assert_eq!(bare.expect_application_id, None);

        let app_arg = app.to_string();
        let pinned = activate(&["ns1", "--expect-application-id", &app_arg]);
        assert_eq!(pinned.expect_application_id, Some(app));

        let versioned = activate(&["ns1", "--expect-version", "2.0.0"]);
        assert_eq!(versioned.expect_version.as_deref(), Some("2.0.0"));
    }

    #[test]
    fn activate_fails_when_any_context_did_not_switch() {
        let data = |failed_contexts| ActivateGroupUpgradeApiResponseData {
            target_application_id: ApplicationId::from([0xAA; 32]),
            switched_contexts: 1,
            replayed_deltas: 0,
            failed_contexts,
        };

        assert!(ensure_all_switched(&data(Vec::new())).is_ok());
        let failed = ActivateGroupUpgradeFailedContext {
            context_id: ContextId::from([0x21; 32]),
            error: "migration trapped".to_owned(),
            retryable: false,
        };
        assert!(ensure_all_switched(&data(vec![failed])).is_err());
    }

    #[test]
    fn activate_requires_a_namespace() {
        assert!(UpgradeCommand::try_parse_from(["upgrade", "activate"]).is_err());
    }
}
