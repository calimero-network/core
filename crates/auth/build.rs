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
const CALIMERO_AUTH_FRONTEND_LATEST_ASSET_URL: &str =
    "https://github.com/{repo}/releases/latest/download/{asset}";
const CALIMERO_AUTH_FRONTEND_VERSIONED_ASSET_URL: &str =
    "https://github.com/{repo}/releases/download/{version}/{asset}";
const CALIMERO_AUTH_FRONTEND_LATEST_RELEASE_URL: &str = "https://github.com/{repo}/releases/latest";
const CALIMERO_AUTH_FRONTEND_REF_ARCHIVE_URL: &str =
    "https://github.com/{repo}/archive/refs/heads/{ref}.zip";
const CALIMERO_AUTH_FRONTEND_TAG_ARCHIVE_URL: &str =
    "https://github.com/{repo}/archive/refs/tags/{version}.zip";

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

            let mut resolved_version = None;
            let mut resolved_asset = None;
            let mut resolved_ref = None;
            let release_url_template = if let Some(asset) = asset {
                resolved_asset = Some(asset);
                if version == "latest" {
                    CALIMERO_AUTH_FRONTEND_LATEST_ASSET_URL
                } else {
                    CALIMERO_AUTH_FRONTEND_VERSIONED_ASSET_URL
                }
            } else if version == "latest" {
                if let Some(tag) = resolve_latest_release_tag(repo)? {
                    resolved_version = Some(tag);
                    CALIMERO_AUTH_FRONTEND_TAG_ARCHIVE_URL
                } else {
                    resolved_ref = Some(default_ref);
                    CALIMERO_AUTH_FRONTEND_REF_ARCHIVE_URL
                }
            } else {
                CALIMERO_AUTH_FRONTEND_TAG_ARCHIVE_URL
            };

            let version_value = resolved_version.as_deref().unwrap_or(version);

            let release_url = replace(release_url_template.into(), |var| match var {
                "repo" => Some(repo),
                "version" => Some(version_value),
                "asset" => resolved_asset,
                "ref" => resolved_ref,
                _ => None,
            });

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

fn resolve_latest_release_tag(repo: &str) -> eyre::Result<Option<String>> {
    let latest_release_url =
        replace(
            CALIMERO_AUTH_FRONTEND_LATEST_RELEASE_URL.into(),
            |var| match var {
                "repo" => Some(repo),
                _ => None,
            },
        );
    let client = ReqwestClient::builder()
        .user_agent(USER_AGENT)
        .redirect(Policy::limited(5))
        .build()?;
    let response = client.get(&*latest_release_url).send()?;
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

#[expect(single_use_lifetimes, reason = "necessary to return itself when empty")]
fn replace<'a: 'b, 'b>(str: Cow<'a, str>, replace: impl Fn(&str) -> Option<&str>) -> Cow<'b, str> {
    let mut idx = 0;
    let mut buf = str.as_ref();
    let mut out = String::new();

    while let Some(start) = buf[idx..].find('{') {
        let start = start + 1;

        let Some(end) = buf[idx + start..].find(['{', '}']) else {
            break;
        };

        if buf.as_bytes()[idx + start + end] == b'{' {
            idx += start + end;
            continue;
        }

        let var = &buf[idx + start..idx + start + end];

        if let Some(sub) = replace(var) {
            out.push_str(&buf[..idx + start - 1]);
            out.push_str(sub);
            buf = &buf[idx + start + end + 1..];
            idx = 0;
        } else {
            idx += start + end;
        }
    }

    if out.is_empty() {
        return str;
    }

    out.push_str(buf);

    out.into()
}
