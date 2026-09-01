use calimero_primitives::alias::Alias;
use calimero_primitives::identity::DeviceId;
use clap::Parser;
use eyre::{eyre, Result, WrapErr};

use crate::cli::Environment;
use crate::output::{ErrorLine, WarnLine};

#[derive(Copy, Clone, Debug, Parser)]
#[command(about = "Manage device aliases")]
pub struct DeviceAliasCommand {
    #[command(subcommand)]
    pub command: DeviceAliasSubcommand,
}

#[derive(Copy, Clone, Debug, Parser)]
pub enum DeviceAliasSubcommand {
    #[command(about = "Add new alias for a device", aliases = ["new", "create"])]
    Add {
        #[arg(help = "Name for the alias")]
        alias: Alias<DeviceId>,

        #[arg(help = "The device to create an alias for")]
        device_id: DeviceId,

        #[arg(long, short, help = "Force overwrite if alias already exists")]
        force: bool,
    },

    #[command(about = "Remove a device alias", aliases = ["rm", "del", "delete"])]
    Remove {
        #[arg(help = "Name of the alias to remove")]
        alias: Alias<DeviceId>,
    },

    #[command(about = "Resolve the alias to a device")]
    Get {
        #[arg(help = "Name of the alias to look up")]
        alias: Alias<DeviceId>,
    },

    #[command(about = "List all device aliases", alias = "ls")]
    List,
}

impl DeviceAliasCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.command {
            DeviceAliasSubcommand::Add {
                alias,
                device_id,
                force,
            } => {
                let client = environment.client()?.clone();

                let lookup_result = client.lookup_alias(alias, None).await?;
                if let Some(existing_device) = lookup_result.data.value {
                    if existing_device == device_id {
                        environment.output.write(&WarnLine(&format!(
                            "Alias '{alias}' already points to '{device_id}'. Doing nothing."
                        )));
                        return Ok(());
                    }

                    if !force {
                        environment.output.write(&ErrorLine(&format!(
                            "Alias '{alias}' already exists and points to '{existing_device}'. Use --force to overwrite."
                        )));
                        return Ok(());
                    }
                    environment.output.write(&WarnLine(&format!(
                        "Overwriting existing alias '{alias}' from '{existing_device}' to '{device_id}'"
                    )));

                    let _ignored = client
                        .delete_alias(alias, None)
                        .await
                        .wrap_err("Failed to delete existing alias")?;
                }

                let res = client
                    .create_alias_generic(alias, None, device_id)
                    .await
                    .map_err(|e| eyre!("Failed to create alias: {}", e))?;
                environment.output.write(&res);
            }

            DeviceAliasSubcommand::Remove { alias } => {
                let client = environment.client()?.clone();
                let res = client.delete_alias(alias, None).await?;

                environment.output.write(&res);
            }
            DeviceAliasSubcommand::Get { alias } => {
                let client = environment.client()?.clone();
                let res = client.lookup_alias(alias, None).await?;

                environment.output.write(&res);
            }
            DeviceAliasSubcommand::List => {
                let client = environment.client()?.clone();
                let res = client.list_aliases::<DeviceId>(None).await?;

                environment.output.write(&res);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn device_alias_add_parses_alias_device_id_and_force() {
        let cmd = DeviceAliasCommand::try_parse_from([
            "alias",
            "add",
            "laptop",
            &DeviceId::from([0x11; 32]).to_string(),
            "--force",
        ])
        .unwrap();

        match cmd.command {
            DeviceAliasSubcommand::Add {
                alias,
                device_id,
                force,
            } => {
                assert_eq!(alias.as_str(), "laptop");
                assert_eq!(device_id, DeviceId::from([0x11; 32]));
                assert!(force);
            }
            _ => panic!("expected Add"),
        }
    }

    #[test]
    fn device_alias_get_and_remove_and_list_parse() {
        assert!(matches!(
            DeviceAliasCommand::try_parse_from(["alias", "get", "laptop"])
                .unwrap()
                .command,
            DeviceAliasSubcommand::Get { .. }
        ));
        assert!(matches!(
            DeviceAliasCommand::try_parse_from(["alias", "remove", "laptop"])
                .unwrap()
                .command,
            DeviceAliasSubcommand::Remove { .. }
        ));
        assert!(matches!(
            DeviceAliasCommand::try_parse_from(["alias", "ls"])
                .unwrap()
                .command,
            DeviceAliasSubcommand::List
        ));
    }
}
