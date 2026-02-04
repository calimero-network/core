use calimero_primitives::application::ApplicationId;
use calimero_primitives::hash::Hash;
use calimero_server_primitives::admin::{InstallApplicationRequest, InstallDevApplicationRequest};
use camino::Utf8PathBuf;
use clap::Parser;
use eyre::{bail, Result};
use notify::event::ModifyKind;
use notify::{EventKind, RecursiveMode, Watcher};
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use url::Url;

use crate::cli::validation::{valid_url, validate_file_exists};
use crate::cli::Environment;
use crate::output::{ErrorLine, InfoLine};

#[derive(Debug, Parser)]
#[command(about = "Install an application")]
pub struct InstallCommand {
    #[arg(long, short, conflicts_with = "url", help = "Path to the application")]
    pub path: Option<Utf8PathBuf>,

    #[clap(long, short, conflicts_with = "path", help = "Url of the application", value_parser = valid_url)]
    pub url: Option<String>,

    #[clap(short, long, help = "Metadata for the application")]
    pub metadata: Option<String>,

    #[clap(long, help = "Hash of the application")]
    pub hash: Option<Hash>,

    #[clap(long, short = 'w', requires = "path")]
    pub watch: bool,

    #[clap(
        long,
        help = "Package name (e.g., com.example.myapp)",
        default_value = "unknown"
    )]
    pub package: String,

    #[clap(long, help = "Version (e.g., 1.0.0)", default_value = "0.0.0")]
    pub version: String,
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
        let metadata = self
            .metadata
            .as_ref()
            .map(|s| s.as_bytes().to_vec())
            .unwrap_or_default();

        let client = environment.client()?;

        let response = if let Some(app_path) = self.path.as_ref() {
            // Validate file exists before attempting to install
            validate_file_exists(app_path.as_std_path())?;

            let request = InstallDevApplicationRequest::new(
                app_path.canonicalize_utf8()?,
                metadata,
                Some(self.package.clone()),
                Some(self.version.clone()),
            );
            client.install_dev_application(request).await?
        } else if let Some(app_url) = self.url.as_ref() {
            let request = InstallApplicationRequest::new(
                Url::parse(&app_url)?,
                self.hash,
                metadata,
                Some(self.package.clone()),
                Some(self.version.clone()),
            );
            client.install_application(request).await?
        } else {
            bail!("Either path or url must be provided");
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
                path: Some(path.clone()),
                url: None,
                metadata: self.metadata.clone(),
                hash: None,
                watch: false,
                package: self.package.clone(),
                version: self.version.clone(),
            }
            .install_app(environment)
            .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_install_command_parsing_with_path() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "fake wasm content").unwrap();
        let path = temp_file.path().to_str().unwrap();

        let cmd = InstallCommand::try_parse_from(["install", "--path", path]).unwrap();

        assert!(cmd.path.is_some());
        assert!(cmd.url.is_none());
        assert!(!cmd.watch);
    }

    #[test]
    fn test_install_command_parsing_short_path_flag() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "fake wasm content").unwrap();
        let path = temp_file.path().to_str().unwrap();

        let cmd = InstallCommand::try_parse_from(["install", "-p", path]).unwrap();

        assert!(cmd.path.is_some());
    }

    #[test]
    fn test_install_command_parsing_with_url() {
        let cmd =
            InstallCommand::try_parse_from(["install", "--url", "https://example.com/app.wasm"])
                .unwrap();

        assert!(cmd.path.is_none());
        assert!(cmd.url.is_some());
        assert_eq!(cmd.url.unwrap(), "https://example.com/app.wasm");
    }

    #[test]
    fn test_install_command_parsing_with_metadata() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "fake wasm content").unwrap();
        let path = temp_file.path().to_str().unwrap();

        let cmd = InstallCommand::try_parse_from([
            "install",
            "--path",
            path,
            "--metadata",
            "some metadata",
        ])
        .unwrap();

        assert_eq!(cmd.metadata, Some("some metadata".to_string()));
    }

    #[test]
    fn test_install_command_parsing_with_package_and_version() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "fake wasm content").unwrap();
        let path = temp_file.path().to_str().unwrap();

        let cmd = InstallCommand::try_parse_from([
            "install",
            "--path",
            path,
            "--package",
            "com.example.app",
            "--version",
            "1.2.3",
        ])
        .unwrap();

        assert_eq!(cmd.package, "com.example.app");
        assert_eq!(cmd.version, "1.2.3");
    }

    #[test]
    fn test_install_command_parsing_with_watch() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "fake wasm content").unwrap();
        let path = temp_file.path().to_str().unwrap();

        let cmd = InstallCommand::try_parse_from(["install", "--path", path, "--watch"]).unwrap();

        assert!(cmd.watch);
    }

    #[test]
    fn test_install_command_default_package_and_version() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "fake wasm content").unwrap();
        let path = temp_file.path().to_str().unwrap();

        let cmd = InstallCommand::try_parse_from(["install", "--path", path]).unwrap();

        assert_eq!(cmd.package, "unknown");
        assert_eq!(cmd.version, "0.0.0");
    }

    #[test]
    fn test_install_command_path_and_url_conflict() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "fake wasm content").unwrap();
        let path = temp_file.path().to_str().unwrap();

        let result = InstallCommand::try_parse_from([
            "install",
            "--path",
            path,
            "--url",
            "https://example.com/app.wasm",
        ]);
        assert!(
            result.is_err(),
            "Command should fail when both --path and --url are provided"
        );
    }

    #[test]
    fn test_install_command_invalid_url_fails() {
        let result = InstallCommand::try_parse_from(["install", "--url", "not-a-valid-url"]);
        assert!(result.is_err(), "Command should fail with invalid URL");
    }

    #[test]
    fn test_install_command_watch_requires_path() {
        // --watch requires --path to be present
        let result = InstallCommand::try_parse_from(["install", "--watch"]);
        assert!(
            result.is_err(),
            "Command should fail when --watch is used without --path"
        );
    }
}
