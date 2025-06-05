use calimero_node_primitives::client::NodeClient;
use calimero_primitives::hash::Hash;
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use eyre::Result as EyreResult;
use owo_colors::OwoColorize;
use url::Url;

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
    /// Get application details
    Get {
        /// The application ID
        application_id: String,
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
    pub async fn run(self, node_client: &NodeClient) -> EyreResult<()> {
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
                        node_client
                            .install_application_from_url(
                                url,
                                metadata
                                    .map(|x| x.as_bytes().to_owned())
                                    .unwrap_or_default(),
                                hash.as_ref(),
                            )
                            .await?
                    }
                    Resource::File { path, metadata } => {
                        if let Ok(application_id) = node_client
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
                for application in node_client.list_applications()? {
                    let entry = format!(
                        "{c1:44} | {c2:44} | {c3:9} | {c4}",
                        c1 = application.id,
                        c2 = application.blob.bytecode,
                        c3 = if node_client.has_blob(&application.blob.bytecode)? {
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
            ApplicationSubcommand::Get { application_id } => {
                let Ok(application_id) = application_id.parse() else {
                    println!("{ind} Failed to parse application ID");
                    eyre::bail!("Failed to parse application ID");
                };

                if let Some(application) = node_client.get_application(&application_id)? {
                    println!("{ind} Application ID: {}", application.id);
                    println!("{ind} Blob ID: {}", application.blob.bytecode);
                    println!("{ind} Source: {}", application.source);
                    println!("{ind} Metadata: {:?}", application.metadata);
                } else {
                    println!("{ind} Application not found");
                }
            }
        }
        Ok(())
    }
}
