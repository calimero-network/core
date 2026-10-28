//! Quote generation for this node; the document and its verification live in
//! `calimero-client`, shared with every client.

pub use calimero_client::attest::*;

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

/// How many quotes the release server makes at once; more requests wait. A
/// TDX quote goes through one configfs-tsm entry, so more would only queue
/// there.
pub const MAX_CONCURRENT_RELEASE_QUOTES: usize = 1;

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
    use std::sync::Arc;

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
        mode: Mode,
        claim: release::ReleaseClaim,
        permits: tokio::sync::Semaphore,
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
        let Ok(_permit) = s.permits.acquire().await else {
            return refuse(
                StatusCode::SERVICE_UNAVAILABLE,
                "STOPPING",
                "the server is stopping".into(),
            );
        };
        let (mode, claim) = (s.mode, s.claim.clone());
        match tokio::task::spawn_blocking(move || release_attestation(mode, &nonce, claim)).await {
            Ok(Ok(doc)) => Json(doc).into_response(),
            Ok(Err(e)) => refuse(StatusCode::INTERNAL_SERVER_ERROR, "QUOTE", format!("{e:#}")),
            Err(e) => refuse(StatusCode::INTERNAL_SERVER_ERROR, "QUOTE", e.to_string()),
        }
    }

    claim.check()?;
    let state = Arc::new(Served {
        mode,
        claim,
        permits: tokio::sync::Semaphore::new(MAX_CONCURRENT_RELEASE_QUOTES),
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
