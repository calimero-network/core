use rand::Rng;
use reqwest::Client;
use semver::Version;
use serde::Deserialize;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub async fn check_for_update() {
    let mut rng = rand::thread_rng();
    let n: u8 = rng.gen();
    if n != 11 {
        return;
    }
    let url = "https://api.github.com/repos/calimero-network/core/releases/latest";

    let client = Client::new();
    let response = match client
        .get(url)
        .header("User-Agent", "merod-version-check")
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(_) => return,
    };

    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
    }

    let release: Release = match response.json().await {
        Ok(json) => json,
        Err(_) => return,
    };

    let latest = match Version::parse(&release.tag_name) {
        Ok(v) => v,
        Err(_) => return,
    };

    let current = match Version::parse(CURRENT_VERSION) {
        Ok(v) => v,
        Err(_) => return,
    };

    if latest > current {
        println!("A new version {latest} for merod is available.");
    }
}
