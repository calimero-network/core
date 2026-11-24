//! `meroctl registry`: read the app registry that `app publish` writes to.
//! Every read is the registry's `GET /api/bundles` (the listing its website
//! browses); search filters that listing here, as the website does, since the
//! registry has no search route. With an API key (`--api-key`,
//! `CALIMERO_REGISTRY_API_KEY`, or the one `registry login` saved for the
//! registry) the caller's private packages are listed too and `--mine` works.
//! No node.

use std::cmp::Ordering;
use std::time::Duration;

use clap::{Args, Subcommand};
use eyre::{bail, eyre};
use serde_json::{json, Value};

use crate::env::Home;
use crate::make::is_loopback;

/// The public app registry.
pub const DEFAULT_REGISTRY: &str = "https://apps.calimero.network";

/// The most of a registry answer read: the whole listing is a few hundred
/// KiB today; anything near this is not a registry.
const MAX_ANSWER_BYTES: usize = 32 << 20;

/// How long one registry request may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Args)]
pub struct RegistryArgs {
    /// The registry.
    #[arg(long, global = true, env = "CALIMERO_REGISTRY_URL", default_value = DEFAULT_REGISTRY)]
    pub registry: String,
    /// The registry API key (the registry's Organizations page, CLI Access);
    /// default: the one `meroctl registry login` saved for the registry.
    #[arg(
        long,
        global = true,
        env = "CALIMERO_REGISTRY_API_KEY",
        hide_env_values = true
    )]
    pub api_key: Option<String>,
    #[command(subcommand)]
    pub cmd: RegistryCmd,
}

#[derive(Debug, Subcommand)]
pub enum RegistryCmd {
    /// Save a registry API key for this registry (in `{home}/meroctl/registry.json`,
    /// 0600) after checking it; read from `--api-key`, stdin, or a prompt that
    /// does not echo it. Every registry command, `app publish` and `app
    /// release --publish` then use it.
    Login,
    /// Forget the saved API key for this registry.
    Logout,
    /// Every package and its latest version (your private ones too when
    /// signed in); `--mine` only the packages you own.
    #[command(alias = "ls")]
    List {
        #[arg(long)]
        mine: bool,
    },
    /// Packages whose id, name, tags, category, author or description hold
    /// every word of TERM (case-insensitive), the best matches first.
    Search {
        #[arg(required = true, num_args = 1..)]
        term: Vec<String>,
        /// Only this category.
        #[arg(long)]
        category: Option<String>,
        #[arg(long)]
        mine: bool,
    },
    /// Every published version of a package, newest first, yanked ones marked.
    Versions { package: String },
    /// One release's manifest as the registry lists it: `PACKAGE@VERSION`,
    /// or `PACKAGE` for its latest version that is not yanked.
    Get {
        #[arg(value_name = "PACKAGE[@VERSION]")]
        coords: String,
    },
}

/// A registry and the key requests to it carry.
pub struct Registry {
    pub url: String,
    key: Option<String>,
    http: reqwest::Client,
}

impl Registry {
    /// `url` checked (`https://`, or plain `http://` to loopback only: an API
    /// key is never sent in the clear), with `key`, else the saved one.
    pub fn new(url: &str, key: Option<String>, home: &Home) -> eyre::Result<Self> {
        let url = url.trim_end_matches('/').to_owned();
        if !url.starts_with("https://") && !is_loopback(&url) {
            bail!("--registry must be https:// (an API key is never sent in the clear)");
        }
        let key = key.or_else(|| home.registry_key(&url));
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        Ok(Self { url, key, http })
    }

    pub fn signed_in(&self) -> bool {
        self.key.is_some()
    }

    /// `GET /api/bundles` with `query`. A signed-in request asks for private
    /// packages too (an answer the registry never caches); `fresh` bypasses
    /// its edge cache, so a release just published is seen.
    async fn bundles(&self, query: &[(&str, &str)]) -> eyre::Result<Option<Vec<Value>>> {
        let mut req = self
            .http
            .get(format!("{}/api/bundles", self.url))
            .query(query)
            .query(&[("fresh", "1")]);
        if let Some(key) = &self.key {
            req = req.bearer_auth(key).query(&[("include_private", "1")]);
        }
        let mut res = req.send().await?;
        let status = res.status();
        let mut body = Vec::new();
        while let Some(chunk) = res.chunk().await? {
            if body.len() + chunk.len() > MAX_ANSWER_BYTES {
                bail!("the registry's answer is over {MAX_ANSWER_BYTES} bytes");
            }
            body.extend_from_slice(&chunk);
        }
        let answer: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        match status.as_u16() {
            200 => {}
            404 => return Ok(None),
            401 => bail!(
                "the registry refused the API key: sign in again with `meroctl registry login`"
            ),
            _ => {
                let code = answer["error"].as_str().unwrap_or("error");
                let message = answer["message"].as_str().unwrap_or_default();
                bail!("the registry answered {status} {code}: {message}");
            }
        }
        match answer {
            Value::Array(bundles) => Ok(Some(bundles)),
            other => Err(eyre!("the registry answered no listing: {other}")),
        }
    }

    /// Every package with its latest version (the newest not yanked); `mine`
    /// only the caller's own, which needs a key.
    pub async fn list(&self, mine: bool) -> eyre::Result<Vec<Value>> {
        if mine && !self.signed_in() {
            bail!("--mine needs an API key: `meroctl registry login`, or --api-key");
        }
        let query: &[(&str, &str)] = if mine { &[("mine", "1")] } else { &[] };
        Ok(self.bundles(query).await?.unwrap_or_default())
    }

    /// Every version of `package`, newest first; empty when the registry has
    /// none the caller may see.
    pub async fn versions(&self, package: &str) -> eyre::Result<Vec<Value>> {
        let mut all = self
            .bundles(&[("package", package), ("all_versions", "true")])
            .await?
            .unwrap_or_default();
        all.sort_by(|a, b| newest_first(version_of(a), version_of(b)));
        Ok(all)
    }

    /// One release, or with no version the latest not yanked.
    pub async fn get(&self, package: &str, version: Option<&str>) -> eyre::Result<Option<Value>> {
        let mut query = vec![("package", package)];
        if let Some(v) = version {
            query.push(("version", v));
        }
        Ok(self
            .bundles(&query)
            .await?
            .and_then(|b| b.into_iter().next()))
    }

    /// The error for a package the caller cannot see, hinting at a sign-in
    /// when the request had no key (private packages are hidden then).
    pub fn not_found(&self, what: &str) -> eyre::Report {
        if self.signed_in() {
            eyre!("{} has no {what}", self.url)
        } else {
            eyre!(
                "{} has no public {what} (a private package needs `meroctl registry login`)",
                self.url
            )
        }
    }
}

fn version_of(b: &Value) -> &str {
    b["appVersion"].as_str().unwrap_or_default()
}

/// Semver descending; a version that does not parse sorts last.
fn newest_first(a: &str, b: &str) -> Ordering {
    match (semver::Version::parse(a), semver::Version::parse(b)) {
        (Ok(a), Ok(b)) => b.cmp(&a),
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        (Err(_), Err(_)) => b.cmp(a),
    }
}

/// What a listing shows of a package's release (not its icon, links or
/// signature: `registry get` has those).
fn summary(b: &Value) -> Value {
    let meta = &b["metadata"];
    json!({
        "package": b["package"],
        "version": b["appVersion"],
        "name": meta["name"],
        "description": meta["description"],
        "author": meta["author"],
        "category": meta["category"],
        "tags": meta["tags"],
        "downloads": b["downloads"],
        "verified": b["verified"],
        "visibility": b.get("visibility").cloned().unwrap_or(json!("public")),
        "yanked": b.get("yanked").cloned().unwrap_or(json!(false)),
        "publishedAt": b["publishedAt"],
    })
}

/// How well `b` matches every word of `words`: the sum of each word's best
/// field (0 the package id, 1 the name, 2 a tag or the category, 3 the author
/// or description); `None` when a word matches nowhere.
fn rank(b: &Value, words: &[String]) -> Option<usize> {
    let meta = &b["metadata"];
    let text = |v: &Value| v.as_str().unwrap_or_default().to_lowercase();
    let tags: Vec<String> = meta["tags"]
        .as_array()
        .map(|t| t.iter().map(text).collect())
        .unwrap_or_default();
    let fields = [
        (0, vec![text(&b["package"])]),
        (1, vec![text(&meta["name"])]),
        (2, [tags, vec![text(&meta["category"])]].concat()),
        (3, vec![text(&meta["author"]), text(&meta["description"])]),
    ];
    words.iter().try_fold(0, |score, word| {
        fields
            .iter()
            .find(|(_, values)| values.iter().any(|v| v.contains(word.as_str())))
            .map(|(rank, _)| score + rank)
    })
}

pub fn search(bundles: &[Value], term: &[String], category: Option<&str>) -> Vec<Value> {
    let words: Vec<String> = term
        .iter()
        .flat_map(|t| t.split_whitespace())
        .map(str::to_lowercase)
        .collect();
    let mut hits: Vec<(usize, &Value)> = bundles
        .iter()
        .filter(|b| {
            category.is_none_or(|c| {
                b["metadata"]["category"]
                    .as_str()
                    .is_some_and(|bc| bc.eq_ignore_ascii_case(c))
            })
        })
        .filter_map(|b| rank(b, &words).map(|r| (r, b)))
        .collect();
    hits.sort_by(|(ra, a), (rb, b)| {
        ra.cmp(rb).then_with(|| {
            a["package"]
                .as_str()
                .unwrap_or_default()
                .cmp(b["package"].as_str().unwrap_or_default())
        })
    });
    hits.into_iter().map(|(_, b)| summary(b)).collect()
}

pub async fn run(args: RegistryArgs, home: &Home) -> eyre::Result<Value> {
    let registry = Registry::new(&args.registry, args.api_key.clone(), home)?;
    Ok(match args.cmd {
        RegistryCmd::Login => {
            let key = match args.api_key {
                Some(key) => key,
                None => crate::deployment::read_secret("registry API key")?.to_string(),
            };
            let registry = Registry::new(&args.registry, Some(key.clone()), home)?;
            // `?mine=1` is answered only for a key the registry accepts.
            let mine = registry.list(true).await?;
            home.save_registry_key(&registry.url, Some(&key))?;
            json!({ "registry": registry.url, "signedIn": true, "packages": mine.len() })
        }
        RegistryCmd::Logout => {
            home.save_registry_key(&registry.url, None)?;
            json!({ "registry": registry.url, "signedIn": false })
        }
        RegistryCmd::List { mine } => {
            let list = registry.list(mine).await?;
            json!({ "registry": registry.url, "packages": list.iter().map(summary).collect::<Vec<_>>() })
        }
        RegistryCmd::Search {
            term,
            category,
            mine,
        } => {
            let list = registry.list(mine).await?;
            json!({ "registry": registry.url, "packages": search(&list, &term, category.as_deref()) })
        }
        RegistryCmd::Versions { package } => {
            let all = registry.versions(&package).await?;
            if all.is_empty() {
                return Err(registry.not_found(&format!("package {package}")));
            }
            let latest = all
                .iter()
                .find(|b| b["yanked"] != json!(true))
                .map(|b| b["appVersion"].clone())
                .unwrap_or(Value::Null);
            let versions: Vec<Value> = all
                .iter()
                .map(|b| {
                    json!({
                        "version": b["appVersion"],
                        "yanked": b.get("yanked").cloned().unwrap_or(json!(false)),
                        "publishedAt": b["publishedAt"],
                        "minRuntimeVersion": b["minRuntimeVersion"],
                    })
                })
                .collect();
            json!({ "registry": registry.url, "package": package, "latest": latest, "versions": versions })
        }
        RegistryCmd::Get { coords } => {
            let (package, version) = match coords.split_once('@') {
                Some((p, v)) => (p, Some(v)),
                None => (coords.as_str(), None),
            };
            registry.get(package, version).await?.ok_or_else(|| {
                registry.not_found(&match version {
                    Some(v) => format!("release {package}@{v}"),
                    None => format!("package {package}"),
                })
            })?
        }
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{newest_first, search};

    #[test]
    fn versions_sort_newest_first_by_semver() {
        let mut v = vec!["1.2.0", "1.10.0", "junk", "1.9.3", "2.0.0-rc.1"];
        v.sort_by(|a, b| newest_first(a, b));
        assert_eq!(v, ["2.0.0-rc.1", "1.10.0", "1.9.3", "1.2.0", "junk"]);
    }

    #[test]
    fn search_ranks_the_package_id_first_and_needs_every_word() {
        let b = |package: &str, name: &str, description: &str, category: &str| {
            json!({
                "package": package,
                "appVersion": "1.0.0",
                "metadata": { "name": name, "description": description, "category": category, "tags": ["crdt"] },
            })
        };
        let all = [
            b("com.acme.notes", "Notes", "Take notes", "productivity"),
            b("com.acme.chat", "Chat", "A chat that keeps notes", "social"),
            b("com.acme.kv", "Key Value", "A store", "tools"),
        ];
        let packages = |hits: Vec<serde_json::Value>| -> Vec<String> {
            hits.iter()
                .map(|h| h["package"].as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(
            packages(search(&all, &["NOTES".into()], None)),
            ["com.acme.notes", "com.acme.chat"]
        );
        assert_eq!(
            packages(search(&all, &["notes chat".into()], None)),
            ["com.acme.chat"]
        );
        assert_eq!(
            packages(search(&all, &["notes".into()], Some("Social"))),
            ["com.acme.chat"]
        );
        assert_eq!(packages(search(&all, &["crdt".into()], None)).len(), 3);
        assert!(search(&all, &["absent".into()], None).is_empty());
    }
}
