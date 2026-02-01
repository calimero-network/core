use std::borrow::Cow;
use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use cached_path::Cache;
use cached_path::Options;
use eyre::bail;
use eyre::OptionExt;
use reqwest::blocking::Client as ReqwestClient;
use reqwest::redirect::Policy;
use reqwest::Url;
use reqwest_compat::blocking::Client as ReqwestCompatClient;
use reqwest_compat::header::AUTHORIZATION;

const USER_AGENT: &str = "calimero-auth-build";
const FRESHNESS_LIFETIME: u64 = 60 * 60 * 24 * 7; // 1 week
const CALIMERO_AUTH_FRONTEND_REPO: &str = "calimero-network/auth-frontend";
const CALIMERO_AUTH_FRONTEND_VERSION: &str = "latest";
const CALIMERO_AUTH_FRONTEND_DEFAULT_REF: &str = "master";
const CALIMERO_AUTH_FRONTEND_LATEST_RELEASE_URL: &str = "https://github.com/{repo}/releases/latest";

fn main() {
    if let Err(e) = try_main() {
        eprintln!("error: {e:?}");

        std::process::exit(1);
    }
}

fn try_main() -> eyre::Result<()> {
    let token = option_env!("CALIMERO_AUTH_FRONTEND_FETCH_TOKEN");

    let mut is_local_dir = false;

    let src = match option_env!("CALIMERO_AUTH_FRONTEND_SRC") {
        Some(src) => {
            match Url::parse(src) {
                Ok(url) if !matches!(url.scheme(), "http" | "https") => {
                    bail!(
                        "CALIMERO_AUTH_FRONTEND_SRC must be an absolute path or a valid URL, got: {}",
                        src
                    );
                }
                Err(_) if !Path::new(src).is_absolute() => bail!(
                    "CALIMERO_AUTH_FRONTEND_SRC must be an absolute path or a valid URL, got: {}",
                    src
                ),
                Err(_) => is_local_dir = fs::metadata(src)?.is_dir(),
                _ => {}
            }

            Cow::from(src)
        }
        None => {
            let repo =
                option_env!("CALIMERO_AUTH_FRONTEND_REPO").unwrap_or(CALIMERO_AUTH_FRONTEND_REPO);
            let version = option_env!("CALIMERO_AUTH_FRONTEND_VERSION")
                .unwrap_or(CALIMERO_AUTH_FRONTEND_VERSION);
            let asset = option_env!("CALIMERO_AUTH_FRONTEND_ASSET");
            let default_ref = option_env!("CALIMERO_AUTH_FRONTEND_REF")
                .unwrap_or(CALIMERO_AUTH_FRONTEND_DEFAULT_REF);

            let release_url = if let Some(asset) = asset {
                if version == "latest" {
                    format!("https://github.com/{repo}/releases/latest/download/{asset}")
                } else {
                    format!("https://github.com/{repo}/releases/download/{version}/{asset}")
                }
            } else if version == "latest" {
                if let Some(tag) = resolve_latest_release_tag(repo, token)? {
                    format!("https://github.com/{repo}/archive/refs/tags/{tag}.zip")
                } else {
                    format!("https://github.com/{repo}/archive/refs/heads/{default_ref}.zip")
                }
            } else {
                format!("https://github.com/{repo}/archive/refs/tags/{version}.zip")
            };

            release_url.into()
        }
    };

    let frontend_dir = if is_local_dir {
        Cow::from(Path::new(&*src))
    } else {
        let mut builder = ReqwestCompatClient::builder().user_agent(USER_AGENT);

        if let Some(token) = token {
            let headers = [(AUTHORIZATION, format!("Bearer {token}").try_into()?)].into_iter();

            builder = builder.default_headers(headers.collect());
        }

        let cache = Cache::builder()
            .client_builder(builder)
            .freshness_lifetime(FRESHNESS_LIFETIME)
            .dir(target_dir()?.join("cache"))
            .build()?;

        let mut options = Options::default().subdir("auth-frontend").extract();

        let force = option_env!("CALIMERO_AUTH_FRONTEND_FETCH")
            .map_or(false, |c| matches!(c, "1" | "true" | "yes"));

        if force {
            options = options.force();
        }

        let workdir = cache.cached_path_with_options(&src, &options)?;

        let repo = fs::read_dir(workdir)?
            .filter_map(Result::ok)
            .find(|entry| entry.path().is_dir())
            .ok_or_eyre("no extracted directory found")?;

        repo.path().join("build").into()
    };

    println!("cargo:rerun-if-changed={}", frontend_dir.display());
    println!(
        "cargo:rustc-env=CALIMERO_AUTH_FRONTEND_PATH={}",
        frontend_dir.display()
    );

    Ok(())
}

fn resolve_latest_release_tag(repo: &str, token: Option<&str>) -> eyre::Result<Option<String>> {
    let latest_release_url = CALIMERO_AUTH_FRONTEND_LATEST_RELEASE_URL.replace("{repo}", repo);
    let client = ReqwestClient::builder()
        .user_agent(USER_AGENT)
        .redirect(Policy::limited(5))
        .build()?;
    let mut request = client.get(latest_release_url);

    if let Some(token) = token {
        request = request.bearer_auth(token);
    }

    let response = request.send()?;
    let final_url = response.url();

    let tag = final_url
        .path_segments()
        .and_then(|segments| segments.last())
        .filter(|segment| !segment.is_empty() && *segment != "latest")
        .map(str::to_owned);

    Ok(tag)
}

// https://github.com/rust-lang/cargo/issues/9661#issuecomment-1722358176
fn target_dir() -> eyre::Result<PathBuf> {
    let mut out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let profile = env::var("PROFILE")?;
    let profile_names = ["profiling", "app-release", "release", "dev", &profile];

    while out_dir.pop() {
        if let Some(name) = out_dir.file_name().and_then(|n| n.to_str()) {
            if profile_names.iter().any(|&pn| pn == name) {
                return Ok(out_dir);
            }
        }
    }

    eyre::bail!("failed to resolve target dir");
}
