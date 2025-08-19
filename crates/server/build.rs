use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::{env, fs};

use bytes::Bytes;
use cached_path::{Cache, Options};
use eyre::{bail, Context, OptionExt};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
struct Asset {
    name: String,
    url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
struct Release {
    assets: Vec<Asset>,
}

const USER_AGENT: &str = "calimero-server-build";
const FRESHNESS_LIFETIME: u64 = 60 * 60 * 24 * 7; // 1 week
const CALIMERO_WEBUI_REPO: &str = "calimero-network/admin-dashboard";
const CALIMERO_WEBUI_VERSION: &str = "latest";
const CALIMERO_WEBUI_SRC_URL: &str = "https://api.github.com/repos/{repo}/releases/{version}";

fn main() {
    if let Err(e) = try_main() {
        eprintln!("error: {e:?}");

        std::process::exit(1);
    }
}

fn try_main() -> eyre::Result<()> {
    let token = option_env!("CALIMERO_WEBUI_FETCH_TOKEN");

    let mut is_local_dir = false;

    let src = match option_env!("CALIMERO_WEBUI_SRC") {
        Some(src) => {
            match reqwest::Url::parse(src) {
                Ok(url) if !matches!(url.scheme(), "http" | "https") => {
                    bail!(
                        "CALIMERO_WEBUI_SRC must be an absolute path or a valid URL, got: {}",
                        src
                    );
                }
                Err(_) if !Path::new(src).is_absolute() => bail!(
                    "CALIMERO_WEBUI_SRC must be an absolute path or a valid URL, got: {}",
                    src
                ),
                Err(_) => is_local_dir = fs::metadata(src)?.is_dir(),
                _ => {}
            }

            Cow::from(src)
        }
        None => {
            let repo = option_env!("CALIMERO_WEBUI_REPO").unwrap_or(CALIMERO_WEBUI_REPO);
            let version = option_env!("CALIMERO_WEBUI_VERSION").unwrap_or(CALIMERO_WEBUI_VERSION);

            let release_url = replace(CALIMERO_WEBUI_SRC_URL.into(), |var| match var {
                "repo" => Some(repo),
                "version" => Some(version),
                _ => None,
            });

            let builder = reqwest::blocking::Client::builder()
                .user_agent(USER_AGENT)
                .build()?;

            let mut req = builder.get(&*release_url);

            if let Some(token) = token {
                req = req.bearer_auth(token);
            }

            let res = req.send()?;

            let Release { mut assets } = match Response::try_from(res)? {
                Response::Json(value) => serde_json::from_value(value)?,
                other => bail!("expected json response, got: {:?}", other),
            };

            let asset = match assets.pop() {
                None => bail!("no assets found in release"),
                Some(asset) if assets.is_empty() => asset,
                Some(asset) => {
                    let file = option_env!("CALIMERO_WEBUI_ASSET");
                    let file = file.ok_or_eyre(
                        "multiple assets found, but no `CALIMERO_WEBUI_ASSET` environment variable set"
                    )?;

                    let found = [asset]
                        .into_iter()
                        .chain(assets)
                        .find(|asset| asset.name == file);

                    found.ok_or_eyre(format!(
                        "no asset found with name `{}` in release (env: CALIMERO_WEBUI_ASSET)",
                        file
                    ))?
                }
            };

            asset.url.into()
        }
    };

    let webui_dir = if is_local_dir {
        Cow::from(Path::new(&*src))
    } else {
        let mut builder = reqwest_compat::blocking::Client::builder().user_agent(USER_AGENT);

        let base_headers = [(
            reqwest_compat::header::ACCEPT,
            reqwest_compat::header::HeaderValue::from_static("application/octet-stream"),
        )];

        let auth_header = token
            .map(|token| format!("Bearer {token}").try_into())
            .transpose()?
            .map(|token| (reqwest_compat::header::AUTHORIZATION, token));

        builder = builder.default_headers(base_headers.into_iter().chain(auth_header).collect());

        let cache = Cache::builder()
            .client_builder(builder)
            .freshness_lifetime(FRESHNESS_LIFETIME)
            .dir(target_dir()?.join("cache"))
            .build()?;

        let mut options = Options::default().subdir("webui").extract();

        let force = option_env!("CALIMERO_WEBUI_FETCH")
            .map_or(false, |c| matches!(c, "1" | "true" | "yes"));

        if force {
            options = options.force();
        }

        let workdir = cache.cached_path_with_options(&*src, &options)?;

        workdir.into()
    };

    println!("cargo:rerun-if-changed={}", webui_dir.display());
    println!(
        "cargo:rustc-env=CALIMERO_WEBUI_PATH={}",
        webui_dir.display()
    );

    Ok(())
}

// https://github.com/rust-lang/cargo/issues/9661#issuecomment-1722358176
fn target_dir() -> eyre::Result<PathBuf> {
    let mut out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let profile = env::var("PROFILE")?;

    while out_dir.pop() {
        if out_dir.ends_with(&profile) {
            return Ok(out_dir);
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

#[derive(Debug)]
enum Response {
    Bytes(Bytes),
    String(String),
    Json(serde_json::Value),
}

impl TryFrom<reqwest::blocking::Response> for Response {
    type Error = eyre::Report;

    #[track_caller]
    fn try_from(value: reqwest::blocking::Response) -> Result<Self, Self::Error> {
        let error = value.error_for_status_ref().err();

        let bytes = match value.bytes() {
            Ok(bytes) => bytes,
            Err(err) => {
                if let Some(error) = error {
                    return Err(err).wrap_err(error);
                }

                bail!(err)
            }
        };

        let res = match serde_json::from_slice(&*bytes) {
            Ok(res) => Response::Json(res),
            Err(_) => match std::str::from_utf8(&*bytes) {
                Ok(str) => Response::String(str.to_owned()),
                Err(_) => Response::Bytes(bytes),
            },
        };

        if let Some(error) = error {
            let res = match res {
                Response::Bytes(bytes) => {
                    format!(
                        "failed with raw bytes of length {}: {:?}",
                        bytes.len(),
                        bytes
                    )
                }
                Response::String(str) => format!("failed with response: {str}"),
                Response::Json(json) => format!("failed with json response: {json:#}"),
            };

            return Err(error).wrap_err(res);
        }

        Ok(res)
    }
}
