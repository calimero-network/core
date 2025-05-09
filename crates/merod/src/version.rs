use eyre::Result as EyreResult;
use reqwest::Client;
use semver::Version;
use serde::Deserialize;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub async fn check_for_update(client: &Client) -> EyreResult<()> {
    let url = "https://api.github.com/repos/calimero-network/core/releases/latest";

    let response = client
        .get(url)
        .header("User-Agent", "merod-version-check")
        .send()
        .await?;
    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
    }

    let release: Release = response.json().await?;
    let latest = Version::parse(&release.tag_name)?;
    let current = Version::parse(CURRENT_VERSION)?;
    if latest > current {
        println!("\n🔔 New version of merod available: v{latest} (current: v{current})");
    }
    Ok(())
}
