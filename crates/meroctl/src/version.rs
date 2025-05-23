use std::sync::LazyLock;

use eyre::Result as EyreResult;
use reqwest::Client;
use semver::Version;
use serde::Deserialize;

pub static CURRENT_VERSION: LazyLock<Version> =
    LazyLock::new(|| Version::parse(env!("CARGO_PKG_VERSION")).expect("Invalid cargo version"));

#[derive(Deserialize)]
struct Release {
    tag_name: Version,
}

pub async fn check_for_update() -> EyreResult<()> {
    let url = "https://api.github.com/repos/calimero-network/core/releases/latest";
    let client = Client::new();

    let response = client
        .get(url)
        .header("User-Agent", "meroctl-version-check")
        .send()
        .await?;

    let release: Release = response.json().await?;
    if release.tag_name > *CURRENT_VERSION {
        println!(
            "\n🔔 New version of meroctl available: v{} (current: v{})",
            release.tag_name, *CURRENT_VERSION
        );
    }
    Ok(())
}
