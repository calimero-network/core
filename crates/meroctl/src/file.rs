//! `meroctl file`: a context's sealed files.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use eyre::WrapErr;
use serde_json::{json, Value};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

use crate::env::Env;

#[derive(Debug, Subcommand)]
pub enum FileCmd {
    /// Upload a sealed file (one request up to 16 MiB, else resumable parts); prints its id, size and SHA-256.
    Upload {
        file: PathBuf,
        #[arg(
            long = "instance",
            short = 'i',
            alias = "context",
            short_alias = 'c',
            value_name = "INSTANCE"
        )]
        #[arg(value_name = "INSTANCE")]
        context: String,
    },
    /// Download a sealed file's plaintext to `--out`: the node sends ciphertext and the file's key
    /// wrapped to a key made here, and `--out` appears only after every byte is checked.
    Download {
        /// The app instance (alias or id).
        app: String,
        id: String,
        #[arg(long, short)]
        out: PathBuf,
    },
}

/// A download in progress: written to `<target>.part`, moved onto the target by [`Part::commit`],
/// and removed when dropped first, so a failed download leaves the target as it was.
struct Part {
    file: File,
    part: Option<PathBuf>,
}

impl Part {
    async fn create(target: &Path) -> eyre::Result<Self> {
        let mut name = OsString::from(target.as_os_str());
        name.push(".part");
        let part = PathBuf::from(name);
        // Drop a stale `.part` (or a link planted there) so O_EXCL creates a fresh private file.
        if std::fs::symlink_metadata(&part).is_ok() {
            std::fs::remove_file(&part).wrap_err_with(|| part.display().to_string())?;
        }
        let file = calimero_utils_fs::create_owner_only(&part)
            .map(File::from_std)
            .wrap_err_with(|| part.display().to_string())?;
        Ok(Self {
            file,
            part: Some(part),
        })
    }

    async fn commit(mut self, target: &Path) -> eyre::Result<()> {
        self.file.flush().await?;
        let part = self.part.take().expect("committed once");
        tokio::fs::rename(&part, target)
            .await
            .wrap_err_with(|| target.display().to_string())
    }
}

impl Drop for Part {
    fn drop(&mut self) {
        if let Some(part) = &self.part {
            let _gone = std::fs::remove_file(part);
        }
    }
}

/// Exports file `id` of `app` and writes its plaintext to `out`; prints `{ id, size, path }`.
/// `out` appears only once every check passed.
pub async fn download(env: &Env, app: &str, id: &str, out: &Path) -> eyre::Result<Value> {
    let mut part = Part::create(out).await?;
    let done = env.client.export_file_to(app, id, &mut part.file).await?;
    part.commit(out).await?;
    Ok(json!({ "id": id, "size": done.size, "path": out.display().to_string() }))
}

pub async fn run(cmd: FileCmd, env: &Env) -> eyre::Result<Value> {
    Ok(match cmd {
        FileCmd::Upload { file, context } => {
            let at = || file.display().to_string();
            let reader = tokio::fs::File::open(&file).await.wrap_err_with(at)?;
            let size = reader.metadata().await.wrap_err_with(at)?.len();
            env.client
                .upload_file(&env.app(&context).await?, reader, size)
                .await?
        }
        FileCmd::Download { app, id, out } => {
            download(env, &env.app(&app).await?, &id, &out).await?
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn a_part_dropped_before_commit_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.bin");
        let mut part = Part::create(&target).await.unwrap();
        part.file.write_all(b"half").await.unwrap();
        assert_eq!(names(dir.path()), ["out.bin.part"]);
        drop(part);
        assert!(names(dir.path()).is_empty());
    }

    #[tokio::test]
    async fn a_dropped_part_leaves_an_existing_target_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.bin");
        std::fs::write(&target, b"old").unwrap();
        let mut part = Part::create(&target).await.unwrap();
        part.file.write_all(b"new but failed").await.unwrap();
        drop(part);
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert_eq!(names(dir.path()), ["out.bin"]);
    }

    #[tokio::test]
    async fn a_committed_part_replaces_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.bin");
        std::fs::write(&target, b"old").unwrap();
        let mut part = Part::create(&target).await.unwrap();
        part.file.write_all(b"new").await.unwrap();
        part.commit(&target).await.unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert_eq!(names(dir.path()), ["out.bin"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_committed_download_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.bin");
        let mut part = Part::create(&target).await.unwrap();
        part.file.write_all(b"secret").await.unwrap();
        part.commit(&target).await.unwrap();
        let mode = std::fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_link_planted_at_the_part_path_is_not_written_through() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.bin");
        let victim = dir.path().join("victim");
        std::fs::write(&victim, b"keep").unwrap();
        std::os::unix::fs::symlink(&victim, dir.path().join("out.bin.part")).unwrap();
        let mut part = Part::create(&target).await.unwrap();
        part.file.write_all(b"secret").await.unwrap();
        part.commit(&target).await.unwrap();
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep");
        assert_eq!(std::fs::read(&target).unwrap(), b"secret");
    }
}
