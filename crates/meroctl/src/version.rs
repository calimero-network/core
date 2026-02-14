use std::sync::LazyLock;

use eyre::Result;
use reqwest::Client;
use semver::Version;
use serde::Deserialize;

pub static CURRENT_VERSION: LazyLock<Option<Version>> =
    LazyLock::new(|| match Version::parse(env!("MEROCTL_VERSION")) {
        Ok(version) => Some(version),
        Err(err) => {
            eprintln!(
                "Skipping update checks: invalid current version `{}` ({})",
                env!("MEROCTL_VERSION"),
                err
            );
            None
        }
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

async fn _check_for_update() -> Result<()> {
    let Some(current_version) = CURRENT_VERSION.as_ref() else {
        return Ok(());
    };

    let url = "https://api.github.com/repos/calimero-network/core/releases/latest";
    let client = Client::new();

    let response = client
        .get(url)
        .header("User-Agent", "meroctl-version-check")
        .send()
        .await?;

    let release: Release = response.json().await?;
    if release.tag_name > *current_version {
        eprintln!(
            "\n🔔 New version of meroctl available: v{} (current: v{})",
            release.tag_name, *current_version
        );
    }
    Ok(())
}
