use std::sync::LazyLock;

use calimero_primitives::version::Version as CalimeroVersion;
use eyre::Result as EyreResult;
use reqwest::Client;
use semver::Version as SemverVersion;
use serde::Deserialize;

pub static CURRENT_BUILD_INFO: LazyLock<CalimeroVersion> = LazyLock::new(|| {
    CalimeroVersion::from_build_env(
        env!("MEROD_VERSION"),
        env!("MEROD_BUILD"),
        env!("MEROD_COMMIT"),
        env!("MEROD_RUSTC_VERSION"),
    )
});

/// Current parsed semver release, if the build-time version is semver-compatible.
///
/// We keep this optional to avoid panicking in local/dev builds where the version string
/// may not be valid semver (e.g., custom tags).
pub static CURRENT_VERSION: LazyLock<Option<SemverVersion>> =
    LazyLock::new(|| match SemverVersion::parse(&CURRENT_BUILD_INFO.version) {
        Ok(version) => Some(version),
        Err(err) => {
            eprintln!(
                "Skipping update checks: invalid current version `{}` ({})",
                CURRENT_BUILD_INFO.version, err
            );
            None
        }
    });

#[derive(Deserialize)]
struct Release {
    tag_name: SemverVersion,
}

pub fn check_for_update() {
    if rand::random::<u8>() % 10 != 0 {
        return;
    }

    let _ignored = tokio::spawn(async move {
        if let Err(err) = _check_for_update().await {
            eprintln!("Version check failed: {}", err);
        }
    });
}

async fn _check_for_update() -> EyreResult<()> {
    let Some(current_version) = CURRENT_VERSION.as_ref() else {
        return Ok(());
    };

    let url = "https://api.github.com/repos/calimero-network/core/releases/latest";
    let client = Client::new();

    let response = client
        .get(url)
        .header("User-Agent", "merod-version-check")
        .send()
        .await?;

    let release: Release = response.json().await?;

    if release.tag_name > *current_version {
        eprintln!(
            "\n🔔 New version of merod available: v{} (current: v{})",
            release.tag_name, *current_version
        );
    }

    Ok(())
}
