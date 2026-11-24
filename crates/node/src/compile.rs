//! Compiling a component a client hands the node: each component of a package
//! release at install (`packages::verify_release`) and a raw bundle at upload
//! (`POST /bundles`).
//!
//! Cranelift holds the thread it compiles on for seconds on a large component
//! (a componentize-js one is megabytes of StarlingMonkey), and wasmtime's
//! parallel compilation spreads it over every core. Left on an async worker it
//! would stall Raft heartbeats, the system group and HTTP beside it, so a
//! compile runs on the blocking pool, one at a time, the way [`crate::attest::Quoter`]
//! makes quotes.

use std::sync::Arc;
use std::time::Duration;

use calimero_app_host::Engine as _;
use calimero_app_host::Prepared;
use tokio::sync::Semaphore;

use crate::error::ApiError;
use crate::node::{Node, BAD_BUNDLE, MAX_BLOB_BYTES};

/// How many components a node compiles at once for installs and uploads. One:
/// wasmtime already compiles a component's functions on every core.
pub const MAX_CONCURRENT_COMPILES: usize = 1;

/// How many more compiles may wait for the slot; past them an install or
/// upload is refused `503 BUSY` at once.
pub const MAX_QUEUED_COMPILES: usize = 4;

/// How long a waiting compile waits for the slot before `503 BUSY`.
pub const COMPILE_WAIT: Duration = Duration::from_secs(10);

/// How long one compile may take before the install or upload is refused
/// `422 COMPILE_TIMEOUT`. Far above what a componentize-js component takes
/// (seconds), so only a component built to be slow to compile, or a machine
/// far past its load, meets it.
pub const COMPILE_TIMEOUT: Duration = Duration::from_secs(60);

/// The refusal when a component does not compile within [`COMPILE_TIMEOUT`].
pub const COMPILE_TIMED_OUT: &str = "COMPILE_TIMEOUT";

/// The largest component (one service's WASM or compute component, as the
/// `.mpk` holds it unpacked) a node installs: a blob, as every component must
/// reach other nodes as one (a replica move hands the bundle over, a leader
/// pushes a compute component to a job node). A `.mpk` is at most
/// [`crate::packages::MAX_MPK_BYTES`] compressed, but a component in it is
/// counted unpacked.
pub const MAX_COMPONENT_BYTES: usize = MAX_BLOB_BYTES;

/// Why a [`Compiler`] gave no answer.
#[derive(Debug, thiserror::Error)]
pub enum CompileError {
    /// Too many compiles already run or wait: ask again later.
    #[error("too many components compiling at once: retry in a second")]
    Busy,
    /// The compile ran past its time.
    #[error("the component did not compile within {0:?}")]
    TimedOut(Duration),
    /// The compile's task failed (it panicked).
    #[error("the compile's task: {0}")]
    Failed(String),
}

impl From<CompileError> for ApiError {
    fn from(e: CompileError) -> Self {
        match e {
            CompileError::Busy => Self::busy(&e.to_string()),
            CompileError::TimedOut(_) => Self::new(422, COMPILE_TIMED_OUT, &e.to_string()),
            CompileError::Failed(_) => Self::internal(&e.to_string()),
        }
    }
}

/// Runs a node's install and upload compiles off the async workers, one at a
/// time: [`MAX_QUEUED_COMPILES`] more wait for the slot, each at most
/// [`COMPILE_WAIT`], the rest are [`CompileError::Busy`] at once, and a compile
/// that takes more than [`COMPILE_TIMEOUT`] is [`CompileError::TimedOut`]. The
/// slot is released only when the compile returns, even when its caller has
/// gone or timed out: the thread still compiles, so the next one waits for it.
#[derive(Debug)]
pub struct Compiler {
    /// Compiles that run or wait.
    admitted: Arc<Semaphore>,
    /// Compiles running.
    running: Arc<Semaphore>,
    wait: Duration,
    timeout: Duration,
    /// The node `testing::slow_compiles` names (feature `testing`).
    #[cfg(feature = "testing")]
    public_url: Option<String>,
}

impl Default for Compiler {
    fn default() -> Self {
        Self::with_limits(COMPILE_WAIT, COMPILE_TIMEOUT)
    }
}

impl Compiler {
    fn with_limits(wait: Duration, timeout: Duration) -> Self {
        Self {
            admitted: Arc::new(Semaphore::new(
                MAX_CONCURRENT_COMPILES + MAX_QUEUED_COMPILES,
            )),
            running: Arc::new(Semaphore::new(MAX_CONCURRENT_COMPILES)),
            wait,
            timeout,
            #[cfg(feature = "testing")]
            public_url: None,
        }
    }

    /// This compiler, its compiles slowed by `testing::slow_compiles` for `public_url`.
    #[cfg(feature = "testing")]
    #[must_use]
    pub(crate) fn for_node(mut self, public_url: &str) -> Self {
        self.public_url = Some(public_url.to_owned());
        self
    }

    /// Run `f`, which compiles, on the blocking pool once the slot is free.
    pub async fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, CompileError> {
        let Ok(admitted) = Arc::clone(&self.admitted).try_acquire_owned() else {
            return Err(CompileError::Busy);
        };
        let Ok(Ok(running)) =
            tokio::time::timeout(self.wait, Arc::clone(&self.running).acquire_owned()).await
        else {
            return Err(CompileError::Busy);
        };
        #[cfg(feature = "testing")]
        let delay = self
            .public_url
            .as_deref()
            .and_then(crate::testing::compile_delay);
        let compiling = tokio::task::spawn_blocking(move || {
            let _held = (admitted, running);
            #[cfg(feature = "testing")]
            if let Some(delay) = delay {
                std::thread::sleep(delay);
            }
            f()
        });
        match tokio::time::timeout(self.timeout, compiling).await {
            Ok(Ok(out)) => Ok(out),
            Ok(Err(e)) => Err(CompileError::Failed(e.to_string())),
            Err(_) => Err(CompileError::TimedOut(self.timeout)),
        }
    }
}

/// Which of the node's engines a component is compiled on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// The replica engine every component app runs on.
    App,
    /// The compute engine a release's compute components are prepared on.
    Compute,
}

impl Node {
    /// Compile `component` (`hash` its SHA-256) on `engine` through the node's
    /// [`Compiler`], handing the bytes back with what the engine prepared:
    /// `413 TOO_LARGE` past [`MAX_COMPONENT_BYTES`], `400 BAD_BUNDLE` when the
    /// engine refuses it, `503 BUSY` or `422 COMPILE_TIMEOUT` as the compiler says.
    pub async fn compile(
        &self,
        engine: Engine,
        hash: &str,
        component: Vec<u8>,
    ) -> Result<(Prepared, Vec<u8>), ApiError> {
        check_size(&component)?;
        let wasm = match engine {
            Engine::App => self.app_engine_arc(),
            Engine::Compute => self.compute_engine_arc(),
        };
        let hash = hash.to_owned();
        let (prepared, component) = self
            .compiler
            .run(move || {
                let prepared = wasm.prepare(&hash, &component);
                (prepared, component)
            })
            .await?;
        let prepared = prepared.map_err(|e| ApiError::bad_request(BAD_BUNDLE, &e.to_string()))?;
        Ok((prepared, component))
    }
}

/// `413 TOO_LARGE` when `bytes` is over [`MAX_COMPONENT_BYTES`].
pub fn check_size(bytes: &[u8]) -> Result<(), ApiError> {
    if bytes.len() > MAX_COMPONENT_BYTES {
        return Err(ApiError::new(
            413,
            "TOO_LARGE",
            &format!(
                "a component is limited to {} MiB unpacked; this one is {} bytes",
                MAX_COMPONENT_BYTES >> 20,
                bytes.len()
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use axum::response::IntoResponse;

    use super::*;

    fn sleeper(d: Duration) -> impl FnOnce() + Send + 'static {
        move || std::thread::sleep(d)
    }

    /// A compile blocks a thread of the blocking pool, not an async worker: on
    /// a runtime with one worker, another task keeps its pace beside it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn a_compile_does_not_hold_an_async_worker() {
        let ticker = tokio::spawn(async {
            let mut worst = Duration::ZERO;
            let mut last = Instant::now();
            for _ in 0..40 {
                tokio::time::sleep(Duration::from_millis(10)).await;
                worst = worst.max(last.elapsed());
                last = Instant::now();
            }
            worst
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        let c = Compiler::default();
        // On the worker, as a request handler is.
        tokio::spawn(async move { c.run(sleeper(Duration::from_millis(300))).await })
            .await
            .unwrap()
            .unwrap();
        let worst = ticker.await.unwrap();
        assert!(
            worst < Duration::from_millis(150),
            "a task waited {worst:?}"
        );
    }

    /// Compiles run one at a time; with the slot and the queue taken the next
    /// is refused `503 BUSY` with `Retry-After: 1` at once.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn compiles_past_the_queue_are_busy_at_once() {
        let c = Arc::new(Compiler::default());
        let (in_flight, most) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let admitted: Vec<_> = (0..MAX_CONCURRENT_COMPILES + MAX_QUEUED_COMPILES)
            .map(|_| {
                let (c, in_flight, most) =
                    (Arc::clone(&c), Arc::clone(&in_flight), Arc::clone(&most));
                tokio::spawn(async move {
                    c.run(move || {
                        let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                        let _ = most.fetch_max(now, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(100));
                        let _ = in_flight.fetch_sub(1, Ordering::SeqCst);
                    })
                    .await
                })
            })
            .collect();
        while c.admitted.available_permits() > 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let started = Instant::now();
        let refused = c.run(|| ()).await;
        assert!(matches!(refused, Err(CompileError::Busy)), "{refused:?}");
        assert!(started.elapsed() < Duration::from_millis(50));
        let answer = ApiError::from(CompileError::Busy).into_response();
        assert_eq!(answer.status().as_u16(), 503);
        assert_eq!(
            answer
                .headers()
                .get(axum::http::header::RETRY_AFTER)
                .unwrap(),
            "1"
        );
        for a in admitted {
            a.await.unwrap().unwrap();
        }
        assert_eq!(most.load(Ordering::SeqCst), 1, "two compiles ran at once");
    }

    /// A compile past its time is `422 COMPILE_TIMEOUT`, and keeps the slot
    /// until its thread is done: the next compile waits for it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_compile_past_its_time_is_refused_and_holds_the_slot_until_it_returns() {
        let c = Compiler::with_limits(Duration::from_secs(5), Duration::from_millis(100));
        let started = Instant::now();
        let slow = c.run(sleeper(Duration::from_millis(400))).await;
        assert!(matches!(slow, Err(CompileError::TimedOut(_))), "{slow:?}");
        assert!(started.elapsed() < Duration::from_millis(300));
        let e = ApiError::from(slow.unwrap_err());
        assert_eq!((e.status, e.code.as_str()), (422, COMPILE_TIMED_OUT));

        c.run(|| ()).await.unwrap();
        assert!(
            started.elapsed() >= Duration::from_millis(400),
            "the next compile ran beside the one that timed out: {:?}",
            started.elapsed()
        );
    }

    /// A compile that waits past [`COMPILE_WAIT`] for the slot is busy, and
    /// gives its place in the queue back.
    #[tokio::test]
    async fn a_compile_that_waits_too_long_is_busy() {
        let c = Compiler::with_limits(Duration::from_millis(50), COMPILE_TIMEOUT);
        let _slot = Arc::clone(&c.running).try_acquire_owned().unwrap();
        assert!(matches!(c.run(|| ()).await, Err(CompileError::Busy)));
        assert_eq!(
            c.admitted.available_permits(),
            MAX_CONCURRENT_COMPILES + MAX_QUEUED_COMPILES
        );
    }

    #[test]
    fn a_component_past_a_blob_is_too_large() {
        assert!(check_size(&vec![0; MAX_COMPONENT_BYTES]).is_ok());
        let e = check_size(&vec![0; MAX_COMPONENT_BYTES + 1]).unwrap_err();
        assert_eq!((e.status, e.code.as_str()), (413, "TOO_LARGE"));
    }
}
