use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use eyre::Result;
use owo_colors::OwoColorize;

use crate::Node;

#[derive(Debug, Parser)]
pub struct ApplicationCommand {
    #[command(subcommand)]
    command: ApplicationSubcommand,
}

#[derive(Debug, Subcommand)]
enum ApplicationSubcommand {
    Install {
        #[arg(value_enum)]
        type_: InstallType,
        resource: String,
        metadata: Option<String>,
    },
    Ls,
}

#[derive(Debug, clap::ValueEnum, Clone)]
enum InstallType {
    Url,
    File,
}

impl ApplicationCommand {
    pub async fn run(self, node: &Node) -> Result<()> {
        let ind = ">>".blue();
        match self.command {
            ApplicationSubcommand::Install {
                type_,
                resource,
                metadata,
            } => {
                let application_id = match type_ {
                    InstallType::Url => {
                        let url = resource.parse()?;
                        println!("{ind} Downloading application..");
                        node.ctx_manager
                            .install_application_from_url(url, vec![])
                            .await?
                    }
                    InstallType::File => {
                        let path = Utf8PathBuf::from(resource);
                        if let Ok(application_id) = node
                            .ctx_manager
                            .install_application_from_path(
                                path,
                                metadata
                                    .map(|x| x.as_bytes().to_owned())
                                    .unwrap_or_default(),
                            )
                            .await
                        {
                            application_id
                        } else {
                            println!("{ind} Failed to install application from path");
                            eyre::bail!("Failed to install application from path");
                        }
                    }
                };
                println!("{ind} Installed application: {application_id}");
            }
            ApplicationSubcommand::Ls => {
                println!(
                    "{ind} {c1:44} | {c2:44} | Source",
                    c1 = "Application ID",
                    c2 = "Blob ID",
                );
                for application in node.ctx_manager.list_installed_applications()? {
                    let entry = format!(
                        "{c1:44} | {c2:44} | {c3}",
                        c1 = application.id,
                        c2 = application.blob,
                        c3 = application.source
                    );
                    for line in entry.lines() {
                        println!("{ind} {}", line.cyan());
                    }
                }
            }
        }
        Ok(())
    }
}
