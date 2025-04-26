use calimero_primitives::application::ApplicationId;
use calimero_primitives::hash::Hash;
use calimero_server_primitives::admin::{
    InstallApplicationRequest, InstallApplicationResponse, InstallDevApplicationRequest,
};
use camino::Utf8PathBuf;
use clap::Parser;
use comfy_table::{Cell, Color, Table};
use eyre::{bail, Result as EyreResult};
use notify::event::ModifyKind;
use notify::{EventKind, RecursiveMode, Watcher};
use reqwest::Client;
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use url::Url;

use crate::cli::Environment;
use crate::common::{do_request, fetch_multiaddr, load_config, multiaddr_to_url, RequestType};
use crate::output::{ErrorLine, InfoLine, Report};

#[derive(Debug, Parser)]
#[command(about = "Install an application")]
pub struct InstallCommand {
    #[arg(long, short, conflicts_with = "url", help = "Path to the application")]
    pub path: Option<Utf8PathBuf>,

    #[clap(long, short, conflicts_with = "path", help = "Url of the application")]
    pub url: Option<String>,

    #[clap(short, long, help = "Metadata for the application")]
    pub metadata: Option<String>,

    #[clap(long, help = "Hash of the application")]
    pub hash: Option<Hash>,

    #[clap(long, short = 'w', requires = "path")]
    pub watch: bool,
}

impl Report for InstallApplicationResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Application Installed").fg(Color::Green)]);
        let _ = table.add_row(vec![format!(
            "Application ID: {}",
            self.data.application_id
        )]);
        println!("{table}");
    }
}

impl InstallCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let _ignored = self.install_app(environment).await?;
        if self.watch {
            self.watch_app(environment).await?;
        }
        Ok(())
    }

    pub async fn install_app(&self, environment: &Environment) -> EyreResult<ApplicationId> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;
        let mut is_dev_installation = false;
        let metadata = self
            .metadata
            .as_ref()
            .map(|s| s.as_bytes().to_vec())
            .unwrap_or_default();
        let request = if let Some(app_path) = self.path.as_ref() {
            is_dev_installation = true;
            serde_json::to_value(InstallDevApplicationRequest::new(
                app_path.canonicalize_utf8()?,
                metadata,
            ))?
        } else if let Some(app_url) = self.url.as_ref() {
            serde_json::to_value(InstallApplicationRequest::new(
                Url::parse(&app_url)?,
                self.hash,
                metadata,
            ))?
        } else {
            bail!("Either path or url must be provided");
        };

        let url = multiaddr_to_url(
            fetch_multiaddr(&config)?,
            if is_dev_installation {
                "admin-api/dev/install-dev-application"
            } else {
                "admin-api/dev/install-application"
            },
        )?;

        let response: InstallApplicationResponse = do_request(
            &Client::new(),
            url,
            Some(request),
            &config.identity,
            RequestType::Post,
        )
        .await?;
        environment.output.write(&response);
        Ok(response.data.application_id)
    }
    pub async fn watch_app(&self, environment: &Environment) -> EyreResult<()> {
        let Some(path) = self.path.as_ref() else {
            bail!("The path must be provided");
        };

        let (tx, mut rx) = mpsc::channel(1);
        let handle = Handle::current();
        let mut watcher = notify::recommended_watcher(move |evt| {
            handle.block_on(async {
                drop(tx.send(evt).await);
            });
        })?;

        watcher.watch(path.as_std_path(), RecursiveMode::NonRecursive)?;
        environment
            .output
            .write(&InfoLine(&format!("Watching for changes to {path}")));

        while let Some(event) = rx.recv().await {
            let event = match event {
                Ok(event) => event,
                Err(err) => {
                    environment.output.write(&ErrorLine(&format!("{err:?}")));
                    continue;
                }
            };

            match event.kind {
                EventKind::Modify(ModifyKind::Data(_)) => {}
                EventKind::Remove(_) => {
                    environment
                        .output
                        .write(&ErrorLine("File removed, ignoring.."));
                    continue;
                }
                EventKind::Any
                | EventKind::Access(_)
                | EventKind::Create(_)
                | EventKind::Modify(_)
                | EventKind::Other => continue,
            }

            let _application_id = InstallCommand {
                path: Some(path.clone()),
                url: None,
                metadata: self.metadata.clone(),
                hash: None,
                watch: false,
            }
            .install_app(environment)
            .await?;
        }
        Ok(())
    }
}
