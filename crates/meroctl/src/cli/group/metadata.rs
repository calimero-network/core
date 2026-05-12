//! `meroctl group metadata ...` — manage generic [`MetadataRecord`]s on a
//! group, a group member, or a group-registered context.

use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::SetMetadataApiRequest;
use clap::{Args, Parser, Subcommand};
use eyre::{bail, Result};

use crate::cli::Environment;

/// Split `k=v` on the first `=`.
fn parse_kv(s: &str) -> Result<(String, String)> {
    match s.split_once('=') {
        Some((k, v)) => Ok((k.to_owned(), v.to_owned())),
        None => bail!("expected `key=value`, got `{s}`"),
    }
}

/// Shared `set` options for the read-modify-write subcommands.
#[derive(Args, Clone, Debug)]
pub struct SetOpts {
    #[clap(long, help = "Set the display name")]
    pub name: Option<String>,

    #[clap(long, conflicts_with = "name", help = "Clear the display name")]
    pub clear_name: bool,

    #[clap(
        long = "set",
        value_name = "KEY=VALUE",
        value_parser = parse_kv,
        help = "Insert/overwrite a data entry (repeatable)"
    )]
    pub set: Vec<(String, String)>,

    #[clap(
        long = "unset",
        value_name = "KEY",
        help = "Remove a data entry (repeatable). Ignored with --replace-data"
    )]
    pub unset: Vec<String>,

    #[clap(
        long,
        help = "Replace the entire data map with the --set pairs instead of merging"
    )]
    pub replace_data: bool,

    #[clap(
        long,
        help = "Requester public key (auto-resolved from node identity if omitted)"
    )]
    pub requester: Option<PublicKey>,
}

impl SetOpts {
    /// Combine the current record with these options into an API request.
    fn into_request(
        self,
        current: calimero_primitives::metadata::MetadataRecord,
    ) -> SetMetadataApiRequest {
        let name = if self.clear_name {
            None
        } else if self.name.is_some() {
            self.name
        } else {
            current.name
        };

        let mut data = if self.replace_data {
            std::collections::BTreeMap::new()
        } else {
            current.data
        };
        for (k, v) in self.set {
            let _ = data.insert(k, v);
        }
        if !self.replace_data {
            for k in &self.unset {
                let _ = data.remove(k);
            }
        }

        SetMetadataApiRequest {
            name,
            data,
            requester: self.requester,
        }
    }
}

#[derive(Debug, Parser)]
#[command(about = "Manage a group's metadata record")]
pub struct MetadataCommand {
    #[command(subcommand)]
    pub subcommand: MetadataSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum MetadataSubCommands {
    #[command(about = "Show the group's metadata record")]
    Get {
        #[clap(name = "GROUP_ID", help = "Hex-encoded group ID")]
        group_id: String,
    },
    #[command(about = "Update the group's metadata record (read-modify-write)")]
    Set {
        #[clap(name = "GROUP_ID", help = "Hex-encoded group ID")]
        group_id: String,
        #[command(flatten)]
        opts: SetOpts,
    },
}

impl MetadataCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        match self.subcommand {
            MetadataSubCommands::Get { group_id } => {
                let response = client.get_group_metadata(&group_id).await?;
                environment.output.write(&response);
            }
            MetadataSubCommands::Set { group_id, opts } => {
                let current = client.get_group_metadata(&group_id).await?.data;
                let response = client
                    .set_group_metadata(&group_id, opts.into_request(current))
                    .await?;
                environment.output.write(&response);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Parser)]
#[command(about = "Manage a group member's metadata record")]
pub struct MemberMetadataCommand {
    #[command(subcommand)]
    pub subcommand: MemberMetadataSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum MemberMetadataSubCommands {
    #[command(about = "Show a member's metadata record")]
    Get {
        #[clap(name = "GROUP_ID", help = "Hex-encoded group ID")]
        group_id: String,
        #[clap(name = "MEMBER", help = "Member public key")]
        member: PublicKey,
    },
    #[command(about = "Update a member's metadata record (read-modify-write)")]
    Set {
        #[clap(name = "GROUP_ID", help = "Hex-encoded group ID")]
        group_id: String,
        #[clap(name = "MEMBER", help = "Member public key")]
        member: PublicKey,
        #[command(flatten)]
        opts: SetOpts,
    },
}

impl MemberMetadataCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        match self.subcommand {
            MemberMetadataSubCommands::Get { group_id, member } => {
                let identity_hex = hex::encode(member.digest());
                let response = client.get_member_metadata(&group_id, &identity_hex).await?;
                environment.output.write(&response);
            }
            MemberMetadataSubCommands::Set {
                group_id,
                member,
                opts,
            } => {
                let identity_hex = hex::encode(member.digest());
                let current = client
                    .get_member_metadata(&group_id, &identity_hex)
                    .await?
                    .data;
                let response = client
                    .set_member_metadata(&group_id, &identity_hex, opts.into_request(current))
                    .await?;
                environment.output.write(&response);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Parser)]
#[command(about = "Manage a group-registered context's metadata record")]
pub struct ContextMetadataCommand {
    #[command(subcommand)]
    pub subcommand: ContextMetadataSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum ContextMetadataSubCommands {
    #[command(about = "Show a context's metadata record")]
    Get {
        #[clap(name = "GROUP_ID", help = "Hex-encoded group ID")]
        group_id: String,
        #[clap(name = "CONTEXT_ID", help = "Context ID")]
        context_id: ContextId,
    },
    #[command(about = "Update a context's metadata record (read-modify-write)")]
    Set {
        #[clap(name = "GROUP_ID", help = "Hex-encoded group ID")]
        group_id: String,
        #[clap(name = "CONTEXT_ID", help = "Context ID")]
        context_id: ContextId,
        #[command(flatten)]
        opts: SetOpts,
    },
}

impl ContextMetadataCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        match self.subcommand {
            ContextMetadataSubCommands::Get {
                group_id,
                context_id,
            } => {
                let response = client
                    .get_context_metadata(&group_id, &context_id.to_string())
                    .await?;
                environment.output.write(&response);
            }
            ContextMetadataSubCommands::Set {
                group_id,
                context_id,
                opts,
            } => {
                let cid = context_id.to_string();
                let current = client.get_context_metadata(&group_id, &cid).await?.data;
                let response = client
                    .set_context_metadata(&group_id, &cid, opts.into_request(current))
                    .await?;
                environment.output.write(&response);
            }
        }
        Ok(())
    }
}
