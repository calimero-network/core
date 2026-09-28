//! `GET /tee/release`: the signed measurements of the mero-tee release this
//! node runs.
//!
//! A client seals its traffic to a TEE only after checking the TEE's quote
//! against the measurements of the release it claims to run: that release's
//! `published-mrtds.json`, keyless-signed by the `Release mero-tee` workflow.
//! A browser cannot fetch GitHub release assets (they carry no CORS headers),
//! so the node serves its own release's file and Sigstore bundle, byte for
//! byte. The node is only transport: the client verifies the bundle over the
//! file and matches its quote against it, so a node serving the wrong release
//! gets a quote that matches nothing. The node verifies the signature too,
//! before caching, so it does not hand out junk.
//!
//! Which release is `MERO_TEE_VERSION` ([`AdminState::tee_release_version`]):
//! the same release fleet-join names to admitters, so a browser checks this
//! node against exactly what the namespace admitted it on.
//!
//! The route needs no credential, so a request never reaches GitHub directly.
//! The release is fetched once and kept for the life of the process (a
//! published release never changes); concurrent requests wait for that one
//! fetch; and after a failure every request is answered from the failure,
//! without fetching, until a backoff that doubles with each consecutive
//! failure has passed.

use core::future::Future;
use core::time::Duration;
use std::sync::Arc;
use std::time::Instant;

use axum::http::header::RETRY_AFTER;
use axum::response::{IntoResponse, Response};
use axum::Extension;
use calimero_server_primitives::admin::TeeReleaseResponse;
use calimero_tee_release::{fetch_signed_node_release, SignedNodeRelease};
use eyre::Result as EyreResult;
use reqwest::StatusCode;
use tokio::runtime::Handle;
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

/// The wait after the first failed fetch; each consecutive failure doubles it.
const FIRST_BACKOFF: Duration = Duration::from_secs(30);
/// The longest wait between fetches, however many have failed.
const MAX_BACKOFF: Duration = Duration::from_secs(10 * 60);

/// This process's release, once fetched. Static like the collateral cache:
/// the release is a property of the process, not of any one request.
static CACHE: ReleaseCache = ReleaseCache::new();

pub async fn handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    respond(&CACHE, state.tee_release_version.as_deref(), from_github).await
}

/// Fetch from GitHub on a blocking-pool thread: Sigstore's verification
/// future holds its policy (`&dyn VerificationPolicy`) across an await, so it
/// is not `Send` and cannot run inside an axum handler's future directly. The
/// thread drives it on this runtime's handle, so the fetch uses the same I/O
/// and timers as everything else.
async fn from_github(version: String) -> EyreResult<SignedNodeRelease> {
    let runtime = Handle::current();
    tokio::task::spawn_blocking(move || runtime.block_on(fetch_signed_node_release(&version)))
        .await
        .map_err(|err| eyre::eyre!("the release fetch did not complete: {err}"))?
}

/// The response for a node that runs release `version` (`None`: it names
/// none), taking the release from `cache` or, when the cache has none and is
/// not backing off, from `fetch`.
async fn respond<F, Fut>(cache: &ReleaseCache, version: Option<&str>, fetch: F) -> Response
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = EyreResult<SignedNodeRelease>>,
{
    let Some(version) = version else {
        return ApiError {
            status_code: StatusCode::NOT_IMPLEMENTED,
            message: "This node names no mero-tee release (MERO_TEE_VERSION is unset): it is \
                      not a TEE node, or its image did not say which release it runs"
                .to_owned(),
        }
        .into_response();
    };

    match cache.get(version, fetch).await {
        Ok(release) => ApiResponse {
            payload: TeeReleaseResponse::new(
                release.version.clone(),
                release.published_mrtds.clone(),
                release.bundle.clone(),
            ),
        }
        .into_response(),
        Err(Unavailable::FetchFailed(reason)) => ApiError {
            status_code: StatusCode::BAD_GATEWAY,
            message: format!("Failed to fetch mero-tee release {version}: {reason}"),
        }
        .into_response(),
        Err(Unavailable::BackingOff { retry_in, reason }) => {
            // Rounded up, so a client that waits exactly this long is past it.
            let secs = retry_in.as_secs() + u64::from(retry_in.subsec_nanos() > 0);
            (
                [(RETRY_AFTER, secs.to_string())],
                ApiError {
                    status_code: StatusCode::SERVICE_UNAVAILABLE,
                    message: format!(
                        "Fetching mero-tee release {version} failed; retrying in {secs}s: {reason}"
                    ),
                },
            )
                .into_response()
        }
    }
}

/// Why the release could not be served.
#[derive(Debug)]
enum Unavailable {
    /// This request fetched, and the fetch failed.
    FetchFailed(String),
    /// A recent fetch failed, and the next is not due yet.
    BackingOff { retry_in: Duration, reason: String },
}

/// The release, fetched at most once at a time and at most once per backoff.
struct ReleaseCache {
    /// Held across a fetch, so concurrent requests wait for one fetch rather
    /// than each starting one.
    state: Mutex<CacheState>,
}

struct CacheState {
    release: Option<Arc<SignedNodeRelease>>,
    failure: Option<Failure>,
}

/// The last run of consecutive failed fetches.
struct Failure {
    version: String,
    at: Instant,
    count: u32,
    reason: String,
}

impl ReleaseCache {
    const fn new() -> Self {
        Self {
            state: Mutex::const_new(CacheState {
                release: None,
                failure: None,
            }),
        }
    }

    async fn get<F, Fut>(
        &self,
        version: &str,
        fetch: F,
    ) -> Result<Arc<SignedNodeRelease>, Unavailable>
    where
        F: FnOnce(String) -> Fut,
        Fut: Future<Output = EyreResult<SignedNodeRelease>>,
    {
        let mut state = self.state.lock().await;
        if let Some(release) = state.release.as_ref().filter(|r| r.version == version) {
            return Ok(Arc::clone(release));
        }

        let failures = match state.failure.as_ref().filter(|f| f.version == version) {
            Some(failure) => {
                let wait = backoff(failure.count);
                let waited = failure.at.elapsed();
                if waited < wait {
                    return Err(Unavailable::BackingOff {
                        retry_in: wait.saturating_sub(waited),
                        reason: failure.reason.clone(),
                    });
                }
                failure.count
            }
            None => 0,
        };

        match fetch(version.to_owned()).await {
            Ok(release) => {
                info!(release = %release.version, "Fetched this node's signed mero-tee release");
                let release = Arc::new(release);
                state.release = Some(Arc::clone(&release));
                state.failure = None;
                Ok(release)
            }
            Err(err) => {
                let reason = err.to_string();
                let count = failures.saturating_add(1);
                warn!(
                    release = version,
                    failures = count,
                    error = %reason,
                    "Failed to fetch this node's signed mero-tee release"
                );
                state.failure = Some(Failure {
                    version: version.to_owned(),
                    at: Instant::now(),
                    count,
                    reason: reason.clone(),
                });
                Err(Unavailable::FetchFailed(reason))
            }
        }
    }
}

/// How long to wait after `failures` consecutive failed fetches (from 1):
/// [`FIRST_BACKOFF`], doubling, capped at [`MAX_BACKOFF`].
fn backoff(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(31);
    FIRST_BACKOFF
        .saturating_mul(1_u32 << doublings)
        .min(MAX_BACKOFF)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;

    /// Deliberately not canonical JSON: odd spacing, key order, a trailing
    /// newline. A server that parsed and re-serialized it would change it.
    const MRTDS: &str = "{ \"tag\":\"mero-tee-v2.3.87\",\n  \"profiles\" : {}\t}\n";
    const BUNDLE: &str = "{\"mediaType\":\"application/vnd.dev.sigstore.bundle.v0.3+json\" }\r\n";

    fn release(version: &str) -> SignedNodeRelease {
        SignedNodeRelease {
            version: version.to_owned(),
            published_mrtds: MRTDS.to_owned(),
            bundle: BUNDLE.to_owned(),
        }
    }

    /// A GitHub stand-in that counts its fetches and fails them all when `fail`.
    #[derive(Clone)]
    struct Stub {
        fetches: Arc<AtomicUsize>,
        fail: bool,
    }

    impl Stub {
        fn new(fail: bool) -> Self {
            Self {
                fetches: Arc::new(AtomicUsize::new(0)),
                fail,
            }
        }

        fn fetches(&self) -> usize {
            self.fetches.load(Ordering::SeqCst)
        }

        fn fetch(&self) -> impl FnOnce(String) -> BoxedFetch {
            let stub = self.clone();
            move |version| {
                Box::pin(async move {
                    let _ = stub.fetches.fetch_add(1, Ordering::SeqCst);
                    // Yield, so concurrent requests really do overlap a fetch.
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    if stub.fail {
                        eyre::bail!(
                            "Failed to fetch published-mrtds.json: 500 Internal Server Error"
                        );
                    }
                    Ok(release(&version))
                })
            }
        }
    }

    type BoxedFetch =
        core::pin::Pin<Box<dyn Future<Output = EyreResult<SignedNodeRelease>> + Send>>;

    /// The route as a node serves it, over `cache` and `stub`.
    fn app(cache: Arc<ReleaseCache>, version: Option<&'static str>, stub: Stub) -> Router {
        Router::new().route(
            "/tee/release",
            get(move || {
                let cache = Arc::clone(&cache);
                let stub = stub.clone();
                async move { respond(&cache, version, stub.fetch()).await }
            }),
        )
    }

    async fn call(app: &Router) -> (StatusCode, Option<String>, Value) {
        let response = app
            .clone()
            .oneshot(
                Request::get("/tee/release")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers");
        let status = response.status();
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .map(|v| v.to_str().expect("ascii").to_owned());
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        (
            status,
            retry_after,
            serde_json::from_slice(&body).expect("body is JSON"),
        )
    }

    #[tokio::test]
    async fn the_release_files_are_served_verbatim() {
        let stub = Stub::new(false);
        let app = app(Arc::new(ReleaseCache::new()), Some("2.3.87"), stub.clone());
        let (status, _, body) = call(&app).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["data"]["version"], "2.3.87");
        assert_eq!(
            body["data"]["publishedMrtds"].as_str(),
            Some(MRTDS),
            "the signed bytes, not a re-serialization"
        );
        assert_eq!(body["data"]["bundle"].as_str(), Some(BUNDLE));
    }

    #[tokio::test]
    async fn a_second_call_is_served_from_the_cache() {
        let stub = Stub::new(false);
        let app = app(Arc::new(ReleaseCache::new()), Some("2.3.87"), stub.clone());
        assert_eq!(call(&app).await.0, StatusCode::OK);
        assert_eq!(call(&app).await.0, StatusCode::OK);
        assert_eq!(
            stub.fetches(),
            1,
            "the release never changes; fetch it once"
        );
    }

    #[tokio::test]
    async fn concurrent_calls_share_one_fetch() {
        let stub = Stub::new(false);
        let app = app(Arc::new(ReleaseCache::new()), Some("2.3.87"), stub.clone());
        let calls: Vec<_> = (0..8)
            .map(|_| {
                let app = app.clone();
                tokio::spawn(async move { call(&app).await.0 })
            })
            .collect();
        for call in calls {
            assert_eq!(call.await.expect("call completes"), StatusCode::OK);
        }
        assert_eq!(stub.fetches(), 1);
    }

    #[tokio::test]
    async fn a_failed_fetch_is_a_502_and_then_backs_off_with_503() {
        let stub = Stub::new(true);
        let cache = Arc::new(ReleaseCache::new());
        let app = app(Arc::clone(&cache), Some("2.3.87"), stub.clone());

        let (status, _, body) = call(&app).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        let error = body["error"].as_str().expect("an error message");
        assert!(error.contains("2.3.87") && error.contains("500"), "{error}");

        let (status, retry_after, body) = call(&app).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(retry_after.as_deref(), Some("30"), "{body}");
        assert!(body["error"].as_str().expect("an error").contains("500"));
        assert_eq!(stub.fetches(), 1, "no fetch while backing off");

        // Once the backoff has passed the next call fetches again, and a
        // second failure doubles the wait.
        cache
            .state
            .lock()
            .await
            .failure
            .as_mut()
            .expect("a failure")
            .at = Instant::now()
            .checked_sub(FIRST_BACKOFF)
            .expect("the clock is past the first backoff");
        assert_eq!(call(&app).await.0, StatusCode::BAD_GATEWAY);
        assert_eq!(stub.fetches(), 2);
        let (status, retry_after, _) = call(&app).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(retry_after.as_deref(), Some("60"));
        assert_eq!(stub.fetches(), 2);
    }

    #[tokio::test]
    async fn a_node_that_names_no_release_says_so_without_fetching() {
        let stub = Stub::new(false);
        let app = app(Arc::new(ReleaseCache::new()), None, stub.clone());
        let (status, _, body) = call(&app).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert!(
            body["error"]
                .as_str()
                .expect("an error")
                .contains("MERO_TEE_VERSION"),
            "{body}"
        );
        assert_eq!(stub.fetches(), 0);
    }

    #[test]
    fn the_backoff_doubles_up_to_its_cap() {
        assert_eq!(backoff(1), FIRST_BACKOFF);
        assert_eq!(backoff(2), FIRST_BACKOFF * 2);
        assert_eq!(backoff(5), FIRST_BACKOFF * 16);
        assert_eq!(backoff(6), MAX_BACKOFF);
        assert_eq!(backoff(u32::MAX), MAX_BACKOFF);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_github_source_runs_off_the_handler_future_and_reports_its_error() {
        // An invalid version fails before any request, so this exercises the
        // hop to the blocking pool and back without touching the network.
        let err = from_github("latest".to_owned())
            .await
            .expect_err("not a release version");
        assert!(err.to_string().contains("latest"), "{err}");
    }

    #[tokio::test]
    async fn the_public_tee_router_serves_the_route() {
        // Without the admin state the handler cannot run, but a mounted route
        // answers 500 for the missing extension, where an unmounted one is 404.
        let response = super::super::service()
            .oneshot(
                Request::get("/release")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers");
        assert_ne!(response.status(), StatusCode::NOT_FOUND);
        assert_ne!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }
}
