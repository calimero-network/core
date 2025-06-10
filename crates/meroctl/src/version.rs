use std::sync::LazyLock;

use calimero_version::CalimeroVersion;
use eyre::Result as EyreResult;
use reqwest::Client;
use semver::Version;
use serde::Deserialize;

pub static CURRENT_VERSION: LazyLock<Version> = LazyLock::new(|| {
    Version::parse(&CalimeroVersion::current().release).expect("Invalid cargo version")
});

#[derive(Deserialize)]
struct Release {
    tag_name: Version,
}

pub async fn check_for_update() {
    if rand::random::<u8>() % 10 != 0 {
        return;
    }

    if let Err(err) = _check_for_update().await {
        eprintln!("Version check failed: {}", err);
    }
}

async fn _check_for_update() -> EyreResult<()> {
    let url = "https://api.github.com/repos/calimero-network/core/releases/latest";
    let client = Client::new();

    let response = client
        .get(url)
        .header("User-Agent", "meroctl-version-check")
        .send()
        .await?;

    let release: Release = response.json().await?;
    if release.tag_name > *CURRENT_VERSION {
        eprintln!(
            "\n🔔 New version of meroctl available: v{} (current: v{})",
            release.tag_name, *CURRENT_VERSION
        );
    }
    Ok(())
}
