use calimero_primitives::blobs::{BlobId, BlobInfo};
use calimero_primitives::hash::Hash;
use camino::Utf8PathBuf;
use clap::Parser;
use comfy_table::{Cell, Color, Table};
use eyre::{bail, Result as EyreResult};
use serde::{Deserialize, Serialize};
use tokio::fs::File;
use tokio::io::{stdin, AsyncReadExt};
use url::Url;

use crate::cli::Environment;
use crate::output::Report;

#[derive(Debug, Parser, Clone)]
#[command(about = "Upload a blob")]
pub struct UploadCommand {
    #[arg(long, short, conflicts_with_all = ["url", "stdin"], help = "Path to the file to upload")]
    pub path: Option<Utf8PathBuf>,

    #[arg(long, short, conflicts_with_all = ["path", "stdin"], help = "URL to download and upload as blob")]
    pub url: Option<String>,

    #[arg(long, conflicts_with_all = ["path", "url"], help = "Read blob data from stdin")]
    pub stdin: bool,

    #[arg(long, help = "Expected hash of the blob for verification")]
    pub hash: Option<Hash>,

    #[arg(long, help = "Expected size of the blob")]
    pub size: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BlobUploadResponse {
    pub data: BlobInfo,
}

impl Report for BlobUploadResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Blob Uploaded").fg(Color::Green)]);
        let _ = table.add_row(vec![format!("Blob ID: {}", self.data.blob_id)]);
        let _ = table.add_row(vec![format!("Size: {} bytes", self.data.size)]);

        // Show size in human-readable format for larger files
        if self.data.size > 1024 {
            let size_mb = self.data.size as f64 / (1024.0 * 1024.0);
            if size_mb >= 1.0 {
                let _ = table.add_row(vec![format!("Size: {:.2} MB", size_mb)]);
            } else {
                let size_kb = self.data.size as f64 / 1024.0;
                let _ = table.add_row(vec![format!("Size: {:.2} KB", size_kb)]);
            }
        }

        println!("{table}");
    }
}

impl UploadCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        // Validate that exactly one input method is specified
        let input_count = [self.path.is_some(), self.url.is_some(), self.stdin]
            .iter()
            .filter(|&&x| x)
            .count();

        if input_count == 0 {
            bail!("Must specify one of: --path, --url, or --stdin");
        }

        if input_count > 1 {
            bail!("Cannot specify multiple input methods");
        }

        let response = if let Some(ref path) = self.path {
            self.upload_from_file(environment, path).await?
        } else if let Some(ref url) = self.url {
            self.upload_from_url(environment, url).await?
        } else if self.stdin {
            self.upload_from_stdin(environment).await?
        } else {
            unreachable!("One input method must be specified");
        };

        environment.output.write(&response);
        Ok(())
    }

    /// Upload a blob from a local file
    async fn upload_from_file(
        &self,
        environment: &Environment,
        path: &Utf8PathBuf,
    ) -> EyreResult<BlobUploadResponse> {
        let connection = environment.connection()?;

        // Read the file
        let mut file = File::open(&path).await?;
        let metadata = file.metadata().await?;
        let file_size = metadata.len();

        // Validate expected size if provided
        if let Some(expected_size) = self.size {
            if file_size != expected_size {
                bail!(
                    "File size mismatch: expected {} bytes, got {} bytes",
                    expected_size,
                    file_size
                );
            }
        }

        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).await?;

        self.upload_binary_data(connection, buffer, Some(file_size))
            .await
    }

    /// Upload a blob from a URL
    async fn upload_from_url(
        &self,
        environment: &Environment,
        url_str: &String,
    ) -> EyreResult<BlobUploadResponse> {
        let connection = environment.connection()?;

        // Parse and validate URL
        let url: Url = url_str.parse()?;

        // Download the content
        let response = reqwest::Client::new().get(url).send().await?;

        if !response.status().is_success() {
            bail!("Failed to download from URL: HTTP {}", response.status());
        }

        let content_length = response.content_length();
        let data = response.bytes().await?;

        // Validate expected size if provided
        if let Some(expected_size) = self.size {
            if data.len() as u64 != expected_size {
                bail!(
                    "Downloaded size mismatch: expected {} bytes, got {} bytes",
                    expected_size,
                    data.len()
                );
            }
        }

        self.upload_binary_data(connection, data.to_vec(), content_length)
            .await
    }

    /// Upload a blob from stdin
    async fn upload_from_stdin(&self, environment: &Environment) -> EyreResult<BlobUploadResponse> {
        let connection = environment.connection()?;

        // Read from stdin
        let mut stdin = stdin();
        let mut buffer = Vec::new();
        stdin.read_to_end(&mut buffer).await?;

        // Validate expected size if provided
        if let Some(expected_size) = self.size {
            if buffer.len() as u64 != expected_size {
                bail!(
                    "Stdin size mismatch: expected {} bytes, got {} bytes",
                    expected_size,
                    buffer.len()
                );
            }
        }

        self.upload_binary_data(connection, buffer, None).await
    }

    /// Upload binary data to the blob endpoint
    async fn upload_binary_data(
        &self,
        connection: &crate::connection::ConnectionInfo,
        data: Vec<u8>,
        _content_length: Option<u64>,
    ) -> EyreResult<BlobUploadResponse> {
        // Build the URL with query parameters
        let mut url = connection.api_url.clone();
        url.set_path("admin-api/blobs");

        // Add query parameters if hash is provided
        if let Some(hash) = &self.hash {
            let query_string = format!("hash={}", hash);
            url.set_query(Some(&query_string));
        }

        // Build the request
        let mut builder = connection
            .client
            .put(url) // Server expects PUT for blob upload
            .header("Content-Type", "application/octet-stream")
            .body(data);

        // Add authentication headers if present
        if let Some(ref tokens) = *connection.jwt_tokens.lock().unwrap() {
            builder = builder.header("Authorization", format!("Bearer {}", tokens.access_token));
        }

        // Send the request
        let response = builder.send().await?;
        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await?;
            bail!(crate::cli::ApiError {
                status_code: status.as_u16(),
                message: error_text,
            });
        }

        // Parse the response
        let response_text = response.text().await?;
        let upload_response: BlobUploadResponse = serde_json::from_str(&response_text)?;

        Ok(upload_response)
    }
}
