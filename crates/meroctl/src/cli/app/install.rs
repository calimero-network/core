use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::{InstallApplicationRequest, InstallDevApplicationRequest};
use camino::Utf8PathBuf;
use clap::Parser;
use eyre::{bail, Result};
use notify::event::ModifyKind;
use notify::{EventKind, RecursiveMode, Watcher};
use tokio::runtime::Handle;
use tokio::sync::mpsc;

use crate::cli::validation::validate_file_exists;
use crate::cli::Environment;
use crate::output::{ErrorLine, InfoLine};

#[derive(Debug, Parser)]
#[command(about = "Install an application")]
pub struct InstallCommand {
    #[arg(
        value_name = "PACKAGE@VERSION",
        conflicts_with = "path",
        help = "Coordinates of a published application, e.g. com.example.myapp@1.0.0"
    )]
    pub coords: Option<String>,

    #[arg(
        long,
        short,
        conflicts_with = "coords",
        help = "Path to the application's signed .mpk bundle"
    )]
    pub path: Option<Utf8PathBuf>,

    #[clap(long, short = 'w', requires = "path")]
    pub watch: bool,
}

/// Split `package@version`. Both halves are required: a lone package is not a
/// location, and the node addresses one published version.
fn split_coords(coords: &str) -> Result<(String, String)> {
    match coords.split_once('@') {
        Some((package, version)) if !package.is_empty() && !version.is_empty() => {
            Ok((package.to_owned(), version.to_owned()))
        }
        _ => bail!("expected PACKAGE@VERSION, got '{coords}'"),
    }
}

impl InstallCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let _ignored = self.install_app(environment).await?;
        if self.watch {
            self.watch_app(environment).await?;
        }
        Ok(())
    }

    pub async fn install_app(&self, environment: &mut Environment) -> Result<ApplicationId> {
        let client = environment.client()?;

        let response = if let Some(app_path) = self.path.as_ref() {
            // Validate file exists before attempting to install
            validate_file_exists(app_path.as_std_path())?;

            let request = InstallDevApplicationRequest::new(app_path.canonicalize_utf8()?);
            client.install_dev_application(request).await?
        } else if let Some(coords) = self.coords.as_ref() {
            let (package, version) = split_coords(coords)?;
            client
                .install_application(InstallApplicationRequest::new(package, version))
                .await?
        } else {
            bail!("Either a PACKAGE@VERSION or --path must be provided");
        };

        environment.output.write(&response);
        Ok(response.data.application_id)
    }

    pub async fn watch_app(&self, environment: &mut Environment) -> Result<()> {
        let Some(path) = self.path.as_ref() else {
            bail!("The path must be provided");
        };

        // Validate file exists before watching
        validate_file_exists(path.as_std_path())?;

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
                coords: None,
                path: Some(path.clone()),
                watch: false,
            }
            .install_app(environment)
            .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::split_coords;

    #[test]
    fn splits_a_coordinate_pair() {
        assert_eq!(
            split_coords("com.example.myapp@1.0.0").expect("valid"),
            ("com.example.myapp".to_owned(), "1.0.0".to_owned())
        );
    }

    #[test]
    fn refuses_a_half_pair() {
        for bad in ["com.example.myapp", "@1.0.0", "com.example.myapp@", ""] {
            assert!(split_coords(bad).is_err(), "{bad} must be refused");
        }
    }
}
