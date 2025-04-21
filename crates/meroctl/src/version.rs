use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use reqwest::Client;
use semver::Version;
use serde::{Deserialize, Serialize};

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const CHECK_INTERVAL_HOURS: i64 = 24;

#[derive(Serialize, Deserialize, Debug)]
struct ToolVersionInfo {
    version: String,
    last_checked: DateTime<Utc>,
}

pub async fn check_for_update() {
    let mut path = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push(".calimero");
    path.push("meroctl");
    path.push("version-check.json");
    let cache_path = path;

    let cache: Option<ToolVersionInfo> = read_cache(&cache_path);

    if let Some(info) = &cache {
        let hours_since = (Utc::now() - info.last_checked).num_hours();
        if hours_since < CHECK_INTERVAL_HOURS {
            return;
        }
    }

    if let Some(latest) = fetch_latest_version().await {
        let current = match Version::parse(CURRENT_VERSION) {
            Ok(v) => v,
            Err(_) => return,
        };

        if latest > current {
            println!("\n🔔 New version of meroctl available: v{latest} (current: v{current})");
            println!("💡 To update: brew upgrade meroctl (or rerun your installer)\n");
        }

        let updated = ToolVersionInfo {
            version: CURRENT_VERSION.to_owned(),
            last_checked: Utc::now(),
        };

        if let Some(parent) = cache_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let _ = fs::write(&cache_path, serde_json::to_string_pretty(&updated).unwrap());
    }
}

fn read_cache(path: &Path) -> Option<ToolVersionInfo> {
    fs::read_to_string(path)
        .ok()
        .and_then(|data| serde_json::from_str(&data).ok())
}

async fn fetch_latest_version() -> Option<Version> {
    let url = "https://api.github.com/repos/calimero-network/core/releases/latest";

    let client = Client::new();
    let resp = client
        .get(url)
        .header("User-Agent", format!("meroctl-version-check"))
        .send()
        .await
        .ok()?;

    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
    }

    let release: Release = resp.json().await.ok()?;
    Version::parse(release.tag_name.trim_start_matches('v')).ok()
}
