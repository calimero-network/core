use calimero_primitives::application::ApplicationId;
use calimero_primitives::hash::Hash;
use calimero_server_primitives::admin::{
    InstallApplicationRequest, InstallApplicationResponse, InstallDevApplicationRequest,
};
use camino::Utf8PathBuf;
use clap::Parser;
use comfy_table::{Cell, Color, Table};
use eyre::{bail, Result};
use notify::event::ModifyKind;
use notify::{EventKind, RecursiveMode, Watcher};
use tokio::io::{stdin, AsyncReadExt};
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use url::Url;

use crate::cli::Environment;
use crate::output::{ErrorLine, InfoLine, Report};

#[derive(Debug, Parser)]
#[command(about = "Install an application")]
pub struct InstallCommand {
    #[arg(long, short, conflicts_with_all = ["url", "stdin"], help = "Path to the application")]
    pub path: Option<Utf8PathBuf>,

    #[clap(long, short, conflicts_with_all = ["path", "stdin"], help = "Url of the application")]
    pub url: Option<String>,

    #[clap(short, long, help = "Metadata for the application")]
    pub metadata: Option<String>,

    #[clap(long, help = "Hash of the application")]
    pub hash: Option<Hash>,

    #[clap(long, short = 'w', requires = "path")]
    pub watch: bool,

    #[clap(long, help = "Expected size of the application")]
    pub size: Option<u64>,

    #[clap(long, conflicts_with_all = ["path", "url"], help = "Read application from stdin")]
    pub stdin: bool,
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
    pub async fn run(self, environment: &Environment) -> Result<()> {
        let _ignored = self.install_app(environment).await?;
        if self.watch {
            self.watch_app(environment).await?;
        }
        Ok(())
    }

    /// Install an application by reading data from standard input.
    ///
    /// Example usage:
    /// `cat app.wasm | meroctl app install --stdin`
    ///
    /// The implementation reads all available data from stdin, encodes its metadata as base64 and sends the binary data
    /// as an HTTP POST request to the server's stream endpoint.
    async fn install_from_stdin(&self, environment: &Environment) -> EyreResult<ApplicationId> {
        use base64::engine::general_purpose::STANDARD;
        use base64::Engine;
        use chrono::Utc;

        let connection = environment.connection()?;

        let metadata = self
            .metadata
            .as_ref()
            .map(|s| s.as_bytes().to_vec())
            .unwrap_or_default();

        // Read from stdin
        let mut stdin = stdin();
        let mut buffer = Vec::new();
        stdin.read_to_end(&mut buffer).await?;

        // Build the full URL
        let mut url = connection.api_url.clone();
        url.set_path("admin-api/dev/install-application-stream");

        // Add query parameters
        let metadata_b64 = STANDARD.encode(&metadata);
        let mut query_pairs = vec![("metadata", metadata_b64)];

        if let Some(size) = self.size {
            query_pairs.push(("expectedSize", size.to_string()));
        }

        if let Some(hash) = &self.hash {
            query_pairs.push(("expectedHash", hash.to_string()));
        }

        // Set query string
        let query_string = query_pairs
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        url.set_query(Some(&query_string));

        // DEBUG: Print the URL being called
        eprintln!("DEBUG: Final URL: {}", url);
        eprintln!("DEBUG: Data size: {} bytes", buffer.len());
        eprintln!("DEBUG: Metadata base64: '{}'", STANDARD.encode(&metadata));

        // Build the request using the connection's client directly
        let mut builder = connection
            .client
            .post(url.clone()) // Clone the URL for debugging
            .header("Content-Type", "application/octet-stream")
            .body(buffer);

        // Add authentication headers if present
        if let Some(keypair) = &connection.auth_key {
            let timestamp = Utc::now().timestamp().to_string();
            let signature = keypair.sign(timestamp.as_bytes())?;

            builder = builder
                .header("X-Signature", bs58::encode(signature).into_string())
                .header("X-Timestamp", timestamp);
        }

        // Send the request
        eprintln!("DEBUG: Sending request...");
        let response = builder.send().await?;
        eprintln!("DEBUG: Response status: {}", response.status());

        let status = response.status();
        let text = response.text().await?;

        if !status.is_success() {
            eprintln!("DEBUG: Response status: {}", status);
            eprintln!("DEBUG: Error response: {}", text);

            bail!(crate::cli::ApiError {
                status_code: status.as_u16(),
                message: text,
            });
        }

        // Manually parse the JSON from the text
        let install_response: InstallApplicationResponse = serde_json::from_str(&text)?;
        eprintln!(
            "DEBUG: Success! Application ID: {}",
            install_response.data.application_id
        );
        Ok(install_response.data.application_id)
    }

    pub async fn install_app(&self, environment: &Environment) -> EyreResult<ApplicationId> {
        let connection = environment.connection()?;

        let metadata = self
            .metadata
            .as_ref()
            .map(|s| s.as_bytes().to_vec())
            .unwrap_or_default();

        let response = if let Some(app_path) = self.path.as_ref() {
            let request =
                InstallDevApplicationRequest::new(app_path.canonicalize_utf8()?, metadata);
            connection
                .post::<_, InstallApplicationResponse>("admin-api/install-dev-application", request)
                .await?
        } else if let Some(app_url) = self.url.as_ref() {
            let request =
                InstallApplicationRequest::new(Url::parse(&app_url)?, self.hash, metadata);
            connection
                .post::<_, InstallApplicationResponse>("admin-api/install-application", request)
                .await?
        } else if self.stdin {
            return self.install_from_stdin(environment).await;
        } else {
            bail!("Either path, url, or stdin must be provided");
        };

        environment.output.write(&response);
        Ok(response.data.application_id)
    }

    pub async fn watch_app(&self, environment: &Environment) -> Result<()> {
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
                stdin: false,
                size: None,
            }
            .install_app(environment)
            .await?;
        }
        Ok(())
    }
}
