//! `meroctl app uses`: pin the apps a crate `uses` to a release and vendor that release's
//! ABI (`update`), or check the declaration, `calimero.lock` and the vendored ABIs agree
//! (`check`). The files are `calimero_app_build::uses`'s, which the app's `build.rs` generates
//! its typed clients from and `app pack` packs into the signed manifest (`uses_of`).

use std::path::{Path, PathBuf};

use calimero_app_build::uses::{self, Declared, Locked};
use clap::{Args, Subcommand};
use eyre::{bail, eyre, WrapErr};
use serde_json::{json, Value};

use super::{crate_meta, target_dir};

#[derive(Debug, Subcommand)]
pub enum UsesCmd {
    /// Pin reference NAME to a release of its package and vendor its ABI
    /// (`calimero.lock`, `calimero/abi/NAME.json`), after checking it is the declared
    /// package, inside the declared range, and has the methods the reference lists.
    Update(UpdateArgs),
    /// Check that `[package.metadata.calimero.uses]`, `calimero.lock` and the vendored ABIs
    /// agree, and that each pinned release is inside its range.
    Check {
        /// The app's crate.
        #[arg(default_value = ".")]
        dir: PathBuf,
    },
}

#[derive(Debug, Args)]
pub struct UpdateArgs {
    /// The reference, as `[package.metadata.calimero.uses]` names it.
    pub name: String,
    /// The release to pin: the target app's crate directory (built, its package and version
    /// read from its `Cargo.toml`), its signed `.mpk`, its component or its ABI JSON.
    #[arg(long)]
    pub from: PathBuf,
    /// The release's version: needed for a component or ABI JSON, else checked against the
    /// one `--from` names.
    #[arg(long)]
    pub version: Option<String>,
    /// The service of a multi-service `.mpk`.
    #[arg(long)]
    pub service: Option<String>,
    /// The app's crate.
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,
}

pub fn run(cmd: &UsesCmd) -> eyre::Result<Value> {
    match cmd {
        UsesCmd::Update(a) => update(a),
        UsesCmd::Check { dir } => {
            let refs = check(dir)?;
            Ok(json!({ "uses": refs.iter().map(locked_json).collect::<Vec<_>>() }))
        }
    }
}

/// The ABI `from` holds, with the package and version it names, if it names them.
struct Release {
    abi: Value,
    package: Option<String>,
    version: Option<String>,
}

fn release(from: &Path, service: Option<&str>) -> eyre::Result<Release> {
    if from.join("Cargo.toml").is_file() {
        let meta = crate_meta(from)?;
        let built = calimero_app_build::build_component(from, &target_dir(from))
            .wrap_err_with(|| format!("building {}", from.display()))?;
        return Ok(Release {
            abi: built
                .abi
                .ok_or_else(|| eyre!("{} embeds no ABI", from.display()))?,
            package: meta.package,
            version: meta.version,
        });
    }
    let loaded = calimero_app_abi::load::load_one(from, service)?;
    Ok(Release {
        abi: loaded.json,
        package: loaded.package,
        version: loaded.app_version,
    })
}

fn update(a: &UpdateArgs) -> eyre::Result<Value> {
    let declared = uses::declared(&a.dir)?;
    let d = declared
        .get(&a.name)
        .ok_or_else(|| eyre!("{} is not in [package.metadata.calimero.uses]", a.name))?;
    let release = release(&a.from, a.service.as_deref())?;
    if let Some(package) = &release.package {
        if *package != d.package {
            bail!(
                "{} is {package}, but uses.{} names {}",
                a.from.display(),
                a.name,
                d.package
            );
        }
    }
    let version = match (&a.version, &release.version) {
        (Some(given), Some(named)) if given != named => {
            bail!("{} is {named}, not --version {given}", a.from.display())
        }
        (Some(v), _) | (None, Some(v)) => v.clone(),
        (None, None) => bail!("{} names no version: pass --version", a.from.display()),
    };
    in_range(&a.name, d, &version)?;
    let locked = uses::vendor(&a.dir, &a.name, &version, &release.abi)?;
    Ok(locked_json(&locked))
}

/// [`uses::check`], and each pinned version inside its declared range.
fn check(dir: &Path) -> eyre::Result<Vec<Locked>> {
    uses::check(dir)?
        .into_iter()
        .map(|r| {
            in_range(&r.name, &r.declared, &r.locked.version)?;
            Ok(r.locked)
        })
        .collect()
}

fn in_range(name: &str, d: &Declared, version: &str) -> eyre::Result<()> {
    let v = semver::Version::parse(version)
        .wrap_err_with(|| format!("uses.{name}: version {version}"))?;
    let range = semver::VersionReq::parse(&d.range)
        .wrap_err_with(|| format!("uses.{name}: range {}", d.range))?;
    if !range.matches(&v) {
        bail!(
            "uses.{name}: {version} is outside the declared range {}",
            d.range
        );
    }
    Ok(())
}

fn locked_json(l: &Locked) -> Value {
    json!({
        "name": l.name,
        "package": l.package,
        "version": l.version,
        "range": l.range,
        "mode": l.mode,
        "abi": l.abi,
    })
}
