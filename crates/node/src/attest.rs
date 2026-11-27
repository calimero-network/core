//! Quote generation for this node; the document and its verification live in
//! `calimero-client`, shared with every client.

use std::sync::Arc;
use std::time::Duration;

pub use calimero_client::attest::*;
use tokio::sync::Semaphore;

use crate::error::ApiError;

/// How many quotes a [`Quoter`] makes at once. A TDX quote goes through the
/// kernel's TSM report interface, which makes one at a time and blocks the
/// calling thread for about a second, so more would only park threads there.
pub const MAX_CONCURRENT_QUOTES: usize = 1;

/// How many more requests may wait for a [`Quoter`]'s slot; past them a
/// request is refused `503 BUSY` at once.
pub const MAX_QUEUED_QUOTES: usize = 8;

/// How long a waiting request waits for the slot before `503 BUSY`.
pub const QUOTE_WAIT: Duration = Duration::from_secs(3);

/// How many requests of this cluster's own nodes ([`Lane::Peer`]) may make or
/// wait for a quote, apart from the [`MAX_QUEUED_QUOTES`] anyone may fill.
pub const MAX_QUEUED_PEER_QUOTES: usize = 8;

/// How long a [`Lane::Peer`] request waits for the slot before `503 BUSY`:
/// it waits for at most the quote running, one anonymous request and the
/// peers ahead of it, so longer than [`QUOTE_WAIT`].
pub const PEER_QUOTE_WAIT: Duration = Duration::from_secs(10);

/// The header a node sends with its `GET /attestation` of a peer of its
/// cluster: [`peer_attest_tag`] over the request's nonce. A request whose
/// header the answering node computes alike takes [`Lane::Peer`]; any other,
/// or none (a client, a node of an older release), takes [`Lane::Anyone`].
pub const PEER_ATTEST_HEADER: &str = "x-calimero-peer-attest";

/// [`PEER_ATTEST_HEADER`]'s value for `nonce`: a MAC under a key derived from
/// K_c, so only a node of `cluster` makes it, bound to the nonce so a value
/// seen once serves no other request.
#[must_use]
pub fn peer_attest_tag(cluster_key: &[u8; 32], cluster: &str, nonce: &[u8; 32]) -> String {
    let key = crate::backup::tag(cluster_key, format!("calimero.attest-peer.v1|{cluster}"));
    calimero_identity::b64url(&crate::backup::tag(&key, nonce))
}

/// Whether `headers` carry [`PEER_ATTEST_HEADER`] for `nonce` under `cluster_key`.
#[must_use]
pub fn is_peer_attest(
    headers: &axum::http::HeaderMap,
    cluster_key: &[u8; 32],
    cluster: &str,
    nonce: &[u8; 32],
) -> bool {
    headers
        .get(PEER_ATTEST_HEADER)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            crate::api::ct_eq(
                v.as_bytes(),
                peer_attest_tag(cluster_key, cluster, nonce).as_bytes(),
            )
        })
}

/// Attests a peer of this cluster as [`HttpAttestor`] does, sending
/// [`PEER_ATTEST_HEADER`] so the peer queues the quote in [`Lane::Peer`].
#[derive(Debug, Clone)]
pub struct PeerAttestor {
    pub attestor: HttpAttestor,
    pub cluster_key: [u8; 32],
    pub cluster: String,
}

impl calimero_sealed::client::Attest for PeerAttestor {
    fn attest_transport_key<'a>(
        &'a self,
        http: &'a reqwest::Client,
        api_url: &'a url::Url,
    ) -> futures_util::future::BoxFuture<'a, eyre::Result<[u8; 32]>> {
        Box::pin(self.attestor.attest_with(http, api_url, |req, nonce| {
            req.header(
                PEER_ATTEST_HEADER,
                peer_attest_tag(&self.cluster_key, &self.cluster, nonce),
            )
        }))
    }
}

/// Which requests a quote is queued with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    /// Anyone: a client opening a sealed channel, or a caller that sends no
    /// proof. [`MAX_QUEUED_QUOTES`] places, [`QUOTE_WAIT`], and at most one
    /// of them waits for the slot at a time.
    Anyone,
    /// A node of this cluster ([`PEER_ATTEST_HEADER`]), or this node itself:
    /// places of its own, and the slot before every anonymous request but the
    /// one already waiting, so requests nobody vouches for never keep the
    /// cluster's nodes from attesting each other.
    Peer,
}

/// Why a [`Quoter`] made no quote.
#[derive(Debug, thiserror::Error)]
pub enum QuoteError {
    /// Too many requests already make or wait for a quote: ask again later.
    #[error("too many attestations at once: retry in a second")]
    Busy,
    /// The quote itself failed.
    #[error("{0:#}")]
    Failed(eyre::Report),
}

impl From<QuoteError> for ApiError {
    fn from(e: QuoteError) -> Self {
        match e {
            QuoteError::Busy => Self::busy(&e.to_string()),
            QuoteError::Failed(e) => Self::internal(&format!("{e:#}")),
        }
    }
}

/// Makes a process's quotes off the async workers, one at a time.
///
/// Anyone may ask for a quote (`/attestation` is in the clear and needs no
/// credential), and each blocks its thread in the kernel, so a quote made on
/// an async worker would stall Raft, the API and every timer beside it. A
/// quote runs on the blocking pool once it holds the one running slot;
/// [`MAX_QUEUED_QUOTES`] more requests wait for it, each at most
/// [`QUOTE_WAIT`], and the rest are [`QuoteError::Busy`] at once. Mock quotes
/// take the same path. Requests of the cluster's own nodes ([`Lane::Peer`])
/// have places of their own and pass the anonymous queue: an anonymous
/// request first takes `next`, so only one of them waits for the slot at a
/// time, and a peer, which waits for the slot alone, is behind at most it.
#[derive(Debug)]
pub struct Quoter {
    mode: Mode,
    /// Anonymous requests that make or wait for a quote.
    admitted: Arc<Semaphore>,
    /// Peer requests that make or wait for a quote.
    peers: Arc<Semaphore>,
    /// The one anonymous request that may wait for the slot.
    next: Arc<Semaphore>,
    /// Quotes being made.
    running: Arc<Semaphore>,
    /// The node `testing::slow_quotes` names (feature `testing`).
    #[cfg(feature = "testing")]
    public_url: Option<String>,
}

impl Quoter {
    #[must_use]
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            admitted: Arc::new(Semaphore::new(MAX_CONCURRENT_QUOTES + MAX_QUEUED_QUOTES)),
            peers: Arc::new(Semaphore::new(MAX_QUEUED_PEER_QUOTES)),
            next: Arc::new(Semaphore::new(1)),
            running: Arc::new(Semaphore::new(MAX_CONCURRENT_QUOTES)),
            #[cfg(feature = "testing")]
            public_url: None,
        }
    }

    /// This quoter, its quotes slowed by `testing::slow_quotes` for `public_url`.
    #[cfg(feature = "testing")]
    #[must_use]
    pub(crate) fn for_node(mut self, public_url: &str) -> Self {
        self.public_url = Some(public_url.to_owned());
        self
    }

    /// A quote over `report`, queued with anyone's ([`Lane::Anyone`]).
    pub async fn quote(&self, report: [u8; 64]) -> Result<Vec<u8>, QuoteError> {
        self.quote_in(Lane::Anyone, report).await
    }

    /// A quote over `report`, queued in `lane`.
    pub async fn quote_in(&self, lane: Lane, report: [u8; 64]) -> Result<Vec<u8>, QuoteError> {
        self.run_in(lane, move |mode| quote(mode, report)).await
    }

    /// Run `f`, which makes a quote, on the blocking pool once the slot is
    /// free; [`QuoteError::Busy`] when too many requests already wait.
    pub async fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce(Mode) -> eyre::Result<T> + Send + 'static,
    ) -> Result<T, QuoteError> {
        self.run_in(Lane::Anyone, f).await
    }

    /// [`Quoter::run`], queued in `lane`.
    pub async fn run_in<T: Send + 'static>(
        &self,
        lane: Lane,
        f: impl FnOnce(Mode) -> eyre::Result<T> + Send + 'static,
    ) -> Result<T, QuoteError> {
        let (places, wait) = match lane {
            Lane::Anyone => (&self.admitted, QUOTE_WAIT),
            Lane::Peer => (&self.peers, PEER_QUOTE_WAIT),
        };
        let Ok(admitted) = Arc::clone(places).try_acquire_owned() else {
            return Err(QuoteError::Busy);
        };
        let running = tokio::time::timeout(wait, async {
            // The slot's semaphore is fair: whoever waits first gets it, so an
            // anonymous request waits its turn for `next` first, and a peer
            // finds at most one of them ahead.
            let _next = match lane {
                Lane::Anyone => Some(Arc::clone(&self.next).acquire_owned().await?),
                Lane::Peer => None,
            };
            Arc::clone(&self.running).acquire_owned().await
        })
        .await;
        let Ok(Ok(running)) = running else {
            return Err(QuoteError::Busy);
        };
        let mode = self.mode;
        #[cfg(feature = "testing")]
        let delay = self
            .public_url
            .as_deref()
            .and_then(crate::testing::quote_delay);
        tokio::task::spawn_blocking(move || {
            // Held until the quote returns, even when the caller has gone:
            // the kernel still makes it, so the next one waits for it.
            let _held = (admitted, running);
            #[cfg(feature = "testing")]
            if let Some(delay) = delay {
                std::thread::sleep(delay);
            }
            f(mode)
        })
        .await
        .map_err(|e| QuoteError::Failed(eyre::eyre!("the quote's task: {e}")))?
        .map_err(QuoteError::Failed)
    }
}

/// Where quotes come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Development: a mock quote with the same report-data layout.
    Mock,
    /// A real TDX quote from the TDX module (configfs-tsm).
    Tdx,
}

/// Produce a quote over `report`.
pub fn quote(mode: Mode, report: [u8; 64]) -> eyre::Result<Vec<u8>> {
    match mode {
        Mode::Mock => {
            let mut q = MOCK_QUOTE_HEADER.to_vec();
            q.extend_from_slice(&report);
            q.resize(256, 0);
            Ok(q)
        }
        Mode::Tdx => calimero_tee_attestation::generate_attestation(report)
            .map(|r| r.quote_bytes)
            .map_err(|e| eyre::eyre!("TDX quote: {e:?}")),
    }
}

/// A release attestation over `nonce`: what a manager checks before mero-kms
/// releases a key to this TD. Needs no key and no store.
pub fn release_attestation(
    mode: Mode,
    nonce: &[u8; 32],
    claim: release::ReleaseClaim,
) -> eyre::Result<release::ReleaseAttestation> {
    claim.check()?;
    let report = release::release_report_data(nonce, &claim);
    let (quote, measurements) = match mode {
        Mode::Mock => (quote(Mode::Mock, report)?, None),
        Mode::Tdx => {
            let generated = calimero_tee_attestation::generate_attestation(report)
                .map_err(|e| eyre::eyre!("TDX quote: {e:?}"))?;
            if generated.is_mock {
                eyre::bail!("TDX quote: this build produced a mock quote; run with --dev for one");
            }
            let measurements = QuoteMeasurements::of(&generated.quote.body);
            (generated.quote_bytes, Some(measurements))
        }
    };
    Ok(release::ReleaseAttestation::new(
        &quote,
        nonce,
        claim,
        measurements,
        mode == Mode::Mock,
    ))
}

/// On TDX, refuse to run unless this TD's own registers match `tee_profile`
/// (`MERO_TEE_PROFILE`) in the signed release `tee_release` (`MERO_TEE_VERSION`):
/// the profile decides whether the node trusts debug peers, and both values
/// come from the host. Mock nodes have no registers and check nothing.
pub async fn check_own_profile(
    mode: Mode,
    tee_release: Option<&str>,
    tee_profile: Option<&str>,
) -> eyre::Result<()> {
    if mode == Mode::Mock {
        return Ok(());
    }
    let (Some(release), Some(profile)) = (tee_release, tee_profile) else {
        eyre::bail!(
            "a TDX node needs MERO_TEE_VERSION and MERO_TEE_PROFILE to check its own image"
        );
    };
    // The report data binds nothing: this quote never leaves the node, only
    // its registers are read.
    let generated = tokio::task::spawn_blocking(|| {
        calimero_tee_attestation::generate_attestation([0; 64])
            .map_err(|e| eyre::eyre!("TDX quote: {e:?}"))
    })
    .await??;
    if generated.is_mock {
        eyre::bail!("TDX quote: this build produced a mock quote; run with --dev for one");
    }
    let measured = QuoteMeasurements::of(&generated.quote.body);
    verify_own_profile(release, profile, &measured)
        .await
        .map_err(|e| eyre::eyre!("this node's own image: {e:#}"))
}

/// Serve `GET /attestation/release?nonce=<32 bytes hex>` (and nothing else) on
/// `listen` until `shutdown` resolves: `merod attest --serve`, for a TD that
/// holds no key yet, so a manager can attest it over its own fresh nonce.
/// Unauthenticated, like `/attestation`: a quote is a statement, not a secret.
pub async fn serve_release(
    listen: std::net::SocketAddr,
    mode: Mode,
    claim: release::ReleaseClaim,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> eyre::Result<()> {
    use axum::extract::rejection::QueryRejection;
    use axum::extract::{Query, State};
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;
    use axum::{Json, Router};

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct NonceQuery {
        nonce: String,
    }

    struct Served {
        claim: release::ReleaseClaim,
        quoter: Quoter,
    }

    async fn answer(
        State(s): State<Arc<Served>>,
        q: Result<Query<NonceQuery>, QueryRejection>,
    ) -> Response {
        let refuse = |status: StatusCode, code: &str, message: String| {
            (
                status,
                Json(serde_json::json!({ "error": code, "message": message })),
            )
                .into_response()
        };
        let Some(nonce) = q
            .ok()
            .and_then(|Query(q)| hex::decode(q.nonce).ok())
            .and_then(|v| <[u8; 32]>::try_from(v).ok())
        else {
            return refuse(
                StatusCode::BAD_REQUEST,
                "BAD_NONCE",
                "nonce must be 32 bytes hex".into(),
            );
        };
        let claim = s.claim.clone();
        match s
            .quoter
            .run(move |mode| release_attestation(mode, &nonce, claim))
            .await
        {
            Ok(doc) => Json(doc).into_response(),
            Err(e @ QuoteError::Busy) => ApiError::from(e).into_response(),
            Err(QuoteError::Failed(e)) => {
                refuse(StatusCode::INTERNAL_SERVER_ERROR, "QUOTE", format!("{e:#}"))
            }
        }
    }

    claim.check()?;
    let state = Arc::new(Served {
        claim,
        quoter: Quoter::new(mode),
    });
    let app = Router::new()
        .route("/attestation/release", get(answer))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(listen).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use axum::response::IntoResponse;

    use super::*;

    /// A quote that blocks its thread for `d`, as a TDX quote does, and
    /// records in `most` how many ran at once.
    fn blocking(
        in_flight: &Arc<AtomicUsize>,
        most: &Arc<AtomicUsize>,
        d: Duration,
    ) -> impl FnOnce(Mode) -> eyre::Result<()> + Send + 'static {
        let (in_flight, most) = (Arc::clone(in_flight), Arc::clone(most));
        move |_| {
            let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            let _prev = most.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(d);
            let _prev = in_flight.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// A caller that goes away (a client that disconnects) does not free the
    /// slot while its quote still runs: the next quote waits for it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn only_one_quote_runs_even_when_its_caller_goes_away() {
        let q = Quoter::new(Mode::Mock);
        let (in_flight, most) = (Arc::default(), Arc::default());
        let gone = tokio::time::timeout(
            Duration::from_millis(50),
            q.run(blocking(&in_flight, &most, Duration::from_millis(300))),
        )
        .await;
        assert!(
            gone.is_err(),
            "the first quote returned before its caller left"
        );
        let started = Instant::now();
        q.run(blocking(&in_flight, &most, Duration::from_millis(10)))
            .await
            .unwrap();
        assert_eq!(most.load(Ordering::SeqCst), 1, "two quotes ran at once");
        assert!(
            started.elapsed() >= Duration::from_millis(200),
            "the second quote did not wait for the first: {:?}",
            started.elapsed()
        );
    }

    /// With one quote running and the queue full, the next request is refused
    /// `503 BUSY` with `Retry-After: 1` at once; those that waited are served.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn requests_past_the_queue_are_busy_at_once() {
        let q = Arc::new(Quoter::new(Mode::Mock));
        let (in_flight, most) = (Arc::default(), Arc::default());
        let admitted: Vec<_> = (0..MAX_CONCURRENT_QUOTES + MAX_QUEUED_QUOTES)
            .map(|_| {
                let q = Arc::clone(&q);
                let f = blocking(&in_flight, &most, Duration::from_millis(100));
                tokio::spawn(async move { q.run(f).await })
            })
            .collect();
        while q.admitted.available_permits() > 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        let started = Instant::now();
        let refused = q.quote([0; 64]).await;
        let took = started.elapsed();
        assert!(matches!(refused, Err(QuoteError::Busy)), "{refused:?}");
        assert!(
            took < Duration::from_millis(50),
            "refused only after {took:?}"
        );
        let answer = ApiError::from(QuoteError::Busy).into_response();
        assert_eq!(answer.status().as_u16(), 503);
        assert_eq!(
            answer
                .headers()
                .get(axum::http::header::RETRY_AFTER)
                .unwrap(),
            "1"
        );

        // Nine quotes of 100 ms each finish within QUOTE_WAIT.
        for a in admitted {
            a.await.unwrap().unwrap();
        }
        assert_eq!(most.load(Ordering::SeqCst), 1, "two quotes ran at once");
        assert_eq!(q.quote([0; 64]).await.unwrap().len(), 256);
    }

    /// Anonymous requests that fill their queue, each sent again as soon as it
    /// is answered, neither refuse a peer's request nor keep it waiting for
    /// more than the quote running and the one anonymous request waiting.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_peer_passes_a_full_anonymous_queue() {
        const QUOTE: Duration = Duration::from_millis(200);
        let q = Arc::new(Quoter::new(Mode::Mock));
        let (in_flight, most) = (Arc::<AtomicUsize>::default(), Arc::default());
        let until = Instant::now() + Duration::from_secs(3);
        let flood: Vec<_> = (0..2 * (MAX_CONCURRENT_QUOTES + MAX_QUEUED_QUOTES))
            .map(|_| {
                let (q, in_flight, most) = (Arc::clone(&q), Arc::clone(&in_flight), Arc::clone(&most));
                tokio::spawn(async move {
                    while Instant::now() < until {
                        if q.run(blocking(&in_flight, &most, QUOTE)).await.is_err() {
                            tokio::time::sleep(Duration::from_millis(5)).await;
                        }
                    }
                })
            })
            .collect();
        while q.admitted.available_permits() > 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert!(
            matches!(q.quote([0; 64]).await, Err(QuoteError::Busy)),
            "the anonymous queue was not full"
        );
        for _ in 0..3 {
            let started = Instant::now();
            q.run_in(Lane::Peer, blocking(&in_flight, &most, QUOTE))
                .await
                .expect("a peer's quote");
            let took = started.elapsed();
            assert!(took < 4 * QUOTE, "a peer waited {took:?}");
        }
        for f in flood {
            f.await.unwrap();
        }
        assert_eq!(most.load(Ordering::SeqCst), 1, "two quotes ran at once");
    }

    /// A [`PEER_ATTEST_HEADER`] takes [`Lane::Peer`] only for the nonce, the
    /// cluster and the key it was made for.
    #[test]
    fn the_peer_header_binds_its_nonce_cluster_and_key() {
        let headers = |v: &str| {
            let mut h = axum::http::HeaderMap::new();
            let _prev = h.insert(PEER_ATTEST_HEADER, v.parse().unwrap());
            h
        };
        let tag = peer_attest_tag(&[7; 32], "c", &[1; 32]);
        assert!(is_peer_attest(&headers(&tag), &[7; 32], "c", &[1; 32]));
        assert!(!is_peer_attest(&headers(&tag), &[7; 32], "c", &[2; 32]));
        assert!(!is_peer_attest(&headers(&tag), &[7; 32], "d", &[1; 32]));
        assert!(!is_peer_attest(&headers(&tag), &[8; 32], "c", &[1; 32]));
        assert!(!is_peer_attest(
            &axum::http::HeaderMap::new(),
            &[7; 32],
            "c",
            &[1; 32]
        ));
    }

    /// A request that waits [`QUOTE_WAIT`] without getting the slot is busy.
    #[tokio::test]
    async fn a_request_that_waits_too_long_is_busy() {
        let q = Quoter::new(Mode::Mock);
        let _slot = Arc::clone(&q.running).try_acquire_owned().unwrap();
        let started = Instant::now();
        assert!(matches!(q.quote([0; 64]).await, Err(QuoteError::Busy)));
        assert!(started.elapsed() >= QUOTE_WAIT);
        assert_eq!(
            q.admitted.available_permits(),
            MAX_CONCURRENT_QUOTES + MAX_QUEUED_QUOTES,
            "the request that gave up still holds its place"
        );
    }

    /// A quote blocks a thread of the blocking pool, not an async worker: on a
    /// runtime with one worker, another task keeps its pace beside it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn a_quote_does_not_hold_an_async_worker() {
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
        let q = Quoter::new(Mode::Mock);
        let (in_flight, most) = (Arc::default(), Arc::default());
        // On the worker, as a request handler is (the test's own body runs on
        // the thread that blocks on it, not on a worker).
        tokio::spawn(async move {
            q.run(blocking(&in_flight, &most, Duration::from_millis(300)))
                .await
        })
        .await
        .unwrap()
        .unwrap();
        let worst = ticker.await.unwrap();
        assert!(
            worst < Duration::from_millis(150),
            "a task waited {worst:?}"
        );
    }
}
