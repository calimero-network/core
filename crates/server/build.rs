use std::borrow::Cow;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::{env, fs};

use eyre::bail;
use serde::Deserialize;

#[derive(Deserialize)]
struct Asset {
    name: String,
    url: String,
}

#[derive(Deserialize)]
struct Release {
    assets: Vec<Asset>,
}

const USER_AGENT: &str = "calimero-server-build";
const CALIMERO_WEBUI_REPO: &str = "calimero-network/admin-dashboard";
const CALIMERO_WEBUI_VERSION: &str = "latest";
const CALIMERO_WEBUI_BUILD_ARTIFACT_NAME: &str = "admin-dashboard-build.zip";

fn main() {
    if let Err(e) = try_main() {
        eprintln!("error: {e:?}");
        std::process::exit(1);
    }
}

fn try_main() -> eyre::Result<()> {
    let token = option_env!("CALIMERO_WEBUI_FETCH_TOKEN");
    let target_dir = target_dir()?;
    let webui_dir = target_dir.join("webui");

    if let Some(src) = option_env!("CALIMERO_WEBUI_SRC") {
        let src_path = Path::new(src);
        if !src_path.is_dir() {
            bail!("CALIMERO_WEBUI_SRC must be a directory");
        }
        println!("cargo:rustc-env=CALIMERO_WEBUI_PATH={}", src_path.display());
        return Ok(());
    }

    let repo = CALIMERO_WEBUI_REPO;
    let version = CALIMERO_WEBUI_VERSION;
    let url = format!("https://api.github.com/repos/{}/releases/{}", repo, version);

    let client = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .build()?;

    let mut req = client.get(&*url);
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    let res = req.send()?;
    let release: Release = res.json()?;

    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == CALIMERO_WEBUI_BUILD_ARTIFACT_NAME)
        .ok_or_else(|| {
            eyre::eyre!(
                "Build artifact '{}' not found for repo '{repo}' and version '{version}'",
                CALIMERO_WEBUI_BUILD_ARTIFACT_NAME
            )
        })?;

    let mut req = client.get(&asset.url);
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    let res = req.header("Accept", "application/octet-stream").send()?;

    let archive_bytes = res.bytes()?;
    let mut archive = zip::ZipArchive::new(Cursor::new(archive_bytes))?;

    fs::create_dir_all(&webui_dir)?;
    archive.extract(&webui_dir)?;

    println!(
        "cargo:rustc-env=CALIMERO_WEBUI_PATH={}",
        webui_dir.display()
    );

    Ok(())
}

fn target_dir() -> eyre::Result<PathBuf> {
    let out_dir = env::var("OUT_DIR")?;
    let mut target_dir = Path::new(&out_dir);
    loop {
        if target_dir.ends_with("build") {
            break;
        }
        target_dir = target_dir
            .parent()
            .ok_or_else(|| eyre::eyre!("failed to find target dir"))?;
    }
    Ok(target_dir.to_path_buf())
}
