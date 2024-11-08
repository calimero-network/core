use calimero_primitives::hash::Hash;
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use eyre::Result;
use owo_colors::OwoColorize;
use url::Url;

use crate::Node;

/// Manage applications
#[derive(Debug, Parser)]
pub struct ApplicationCommand {
    #[command(subcommand)]
    command: ApplicationSubcommand,
}

#[derive(Debug, Subcommand)]
enum ApplicationSubcommand {
    /// List installed applications
    Ls,
    /// Install an application
    Install {
        #[command(subcommand)]
        resource: Resource,
    },
}

#[derive(Debug, Subcommand)]
enum Resource {
    /// Install an application from a URL
    Url {
        /// The URL to download the application from
        url: Url,
        /// The hash of the application (bs58 encoded)
        hash: Option<Hash>,
        /// Metadata to associate with the application
        metadata: Option<String>,
    },
    /// Install an application from a file
    File {
        /// The file path to the application
        path: Utf8PathBuf,
        /// Metadata to associate with the application
        metadata: Option<String>,
    },
}

impl ApplicationCommand {
    pub async fn run(self, node: &Node) -> Result<()> {
        let ind = ">>".blue();
        match self.command {
            ApplicationSubcommand::Install { resource } => {
                let application_id = match resource {
                    Resource::Url {
                        url,
                        hash,
                        metadata,
                    } => {
                        println!("{ind} Downloading application..");
                        node.ctx_manager
                            .install_application_from_url(
                                url,
                                metadata
                                    .map(|x| x.as_bytes().to_owned())
                                    .unwrap_or_default(),
                                hash,
                            )
                            .await?
                    }
                    Resource::File { path, metadata } => {
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
                    "{ind} {c1:44} | {c2:44} | Installed | Source",
                    c1 = "Application ID",
                    c2 = "Blob ID",
                );
                for application in node.ctx_manager.list_installed_applications()? {
                    let entry = format!(
                        "{c1:44} | {c2:44} | {c3:9} | {c4}",
                        c1 = application.id,
                        c2 = application.blob,
                        c3 = if node.ctx_manager.has_blob_available(application.blob)? {
                            "Yes"
                        } else {
                            "No"
                        },
                        c4 = application.source
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
