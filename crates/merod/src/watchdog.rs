//! Reasons to stop that no signal announces: the process that spawned this node
//! going away, or the data directory it opened being replaced underneath it.

use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::time::Duration;

/// A stat is microseconds, so this is short enough to bound how long a node whose
/// directory is gone keeps writing, which is the damage this prevents.
pub const DATA_DIR_CHECK_INTERVAL: Duration = Duration::from_secs(5);

/// Identifies a directory rather than its name. `rm -rf` removes the name, so the
/// path can resolve to a different directory while this one is still held open.
fn identity(meta: &std::fs::Metadata) -> (u64, u64) {
    (meta.dev(), meta.ino())
}

/// Resolves when the directory this node opened is no longer the one its path
/// points at - deleted, replaced, or on a volume that went away.
pub async fn data_dir_replaced(path: PathBuf, interval: Duration) -> String {
    // Held open for as long as the watch runs: that keeps the inode allocated, so a
    // directory created later at this path cannot reuse the number and slip past.
    let held_dir = match std::fs::File::open(&path) {
        Ok(dir) => dir,
        Err(err) => {
            tracing::warn!(path = %path.display(), %err, "cannot watch the data directory");
            return std::future::pending().await;
        }
    };
    let Ok(held) = held_dir.metadata().as_ref().map(identity) else {
        tracing::warn!(path = %path.display(), "cannot identify the data directory");
        return std::future::pending().await;
    };

    loop {
        tokio::time::sleep(interval).await;
        match std::fs::metadata(&path).as_ref().map(identity) {
            Ok(now) if now == held => continue,
            Ok(_) => {
                return format!(
                    "the data directory at {} is not the one this node opened; it was replaced",
                    path.display()
                )
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return format!("the data directory at {} is gone", path.display())
            }
            // A transient stat failure is not evidence the directory was replaced.
            Err(err) => {
                tracing::warn!(path = %path.display(), %err, "could not check the data directory")
            }
        }
    }
}

/// Resolves when the write end of `fd` is closed, which the kernel does when the
/// process holding it exits - including a `SIGKILL`, which runs no code of its own.
/// Parks when there is no fd, so a caller can watch it either way.
pub async fn parent_closed(fd: Option<RawFd>) -> String {
    use tokio::io::AsyncReadExt;

    let Some(fd) = fd else {
        return std::future::pending().await;
    };
    // SAFETY: the caller names an fd it inherited and does not otherwise use;
    // owning it here closes it once, when this future is done with it.
    let inherited = unsafe { std::fs::File::from_raw_fd(fd) };
    // Async rather than a blocking read on a worker thread: a thread parked in
    // `read` cannot be cancelled, and runtime shutdown waits for it, so a node
    // whose parent outlives it would hang on the way out.
    let mut pipe = match tokio::net::unix::pipe::Receiver::from_file(inherited) {
        Ok(pipe) => pipe,
        Err(err) => {
            tracing::warn!(%fd, %err, "cannot watch the process that started this node");
            return std::future::pending().await;
        }
    };

    let mut scratch = [0_u8; 64];
    loop {
        match pipe.read(&mut scratch).await {
            Ok(0) => return "the process that started this node exited".to_owned(),
            // A parent that writes down the pipe is not saying anything yet, so
            // anything it sends is discarded rather than being read as an exit.
            Ok(_) => continue,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => {
                return format!("the pipe to the process that started this node failed: {err}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::IntoRawFd;

    use super::*;

    /// The whole point: no code runs in the parent at exit, so only the kernel
    /// closing the pipe can report it.
    #[tokio::test]
    async fn dropping_the_write_end_is_the_parent_going_away() {
        let (reader, writer) = std::io::pipe().expect("pipe");
        let watch = tokio::spawn(parent_closed(Some(reader.into_raw_fd())));

        drop(writer);

        let reason = watch.await.expect("watch task");
        assert_eq!(reason, "the process that started this node exited");
    }

    /// A parent that is alive but quiet must not read as one that has gone.
    #[tokio::test]
    async fn a_silent_parent_is_not_a_dead_parent() {
        let (reader, _writer) = std::io::pipe().expect("pipe");
        let mut watch = tokio::spawn(parent_closed(Some(reader.into_raw_fd())));

        let waited = tokio::time::timeout(std::time::Duration::from_millis(200), &mut watch).await;

        assert!(
            waited.is_err(),
            "the node must keep running while its parent holds the pipe"
        );
        watch.abort();
    }

    /// Written bytes are not an exit: only EOF is.
    #[tokio::test]
    async fn a_parent_that_writes_is_still_alive() {
        use std::io::Write;

        let (reader, mut writer) = std::io::pipe().expect("pipe");
        let watch = tokio::spawn(parent_closed(Some(reader.into_raw_fd())));

        writer.write_all(b"still here").expect("write");
        writer.flush().expect("flush");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!watch.is_finished(), "a write must not be read as an exit");

        drop(writer);
        assert_eq!(
            watch.await.expect("watch task"),
            "the process that started this node exited"
        );
    }

    /// The incident: the directory is deleted and re-created at the same path, so
    /// the node holds one directory while its path names another.
    #[tokio::test]
    async fn a_replaced_directory_is_noticed() {
        let dir = scratch("replaced");
        let watch = tokio::spawn(data_dir_replaced(dir.clone(), Duration::from_millis(50)));
        tokio::time::sleep(Duration::from_millis(150)).await;

        std::fs::remove_dir_all(&dir).expect("remove");
        std::fs::create_dir_all(&dir).expect("re-create");

        let reason = watch.await.expect("watch task");
        assert!(reason.contains("was replaced"), "got: {reason}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_deleted_directory_is_noticed() {
        let dir = scratch("deleted");
        let watch = tokio::spawn(data_dir_replaced(dir.clone(), Duration::from_millis(50)));
        tokio::time::sleep(Duration::from_millis(150)).await;

        std::fs::remove_dir_all(&dir).expect("remove");

        let reason = watch.await.expect("watch task");
        assert!(reason.contains("is gone"), "got: {reason}");
    }

    /// A node whose directory is untouched must never be stopped by this.
    #[tokio::test]
    async fn an_untouched_directory_is_left_alone() {
        let dir = scratch("untouched");
        let mut watch = tokio::spawn(data_dir_replaced(dir.clone(), Duration::from_millis(20)));

        let waited = tokio::time::timeout(Duration::from_millis(300), &mut watch).await;

        assert!(
            waited.is_err(),
            "a directory that is still there is not a reason to stop"
        );
        watch.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("merod-watchdog-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }
}
