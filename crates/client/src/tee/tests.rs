//! A stand-in for a node's attestation, and the attestor's tests.
//!
//! The quote here is only the 64 report data bytes, and [`ReportData`]
//! "verifies" it by comparing them: what is under test is what a client asks to
//! have bound, how it recomputes the binding, and that it trusts a key only
//! once the verifier accepts the quote — not DCAP, which
//! `calimero-tee-attestation` covers.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use axum::routing::post;
use axum::{Json, Router};
use base64::Engine as _;
use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::TeeAttestRequest;
use calimero_tee_attestation::attest_transport_binding;
use eyre::{bail, Result};
use serde_json::json;
use tokio::net::TcpListener;
use url::Url;

use super::{report_data_suffix, Attestor, QuoteVerifier};

/// Accepts a "quote" that is exactly `nonce || report_data_suffix`.
pub(super) struct ReportData;

#[async_trait]
impl QuoteVerifier for ReportData {
    async fn verify(
        &self,
        quote: &[u8],
        nonce: &[u8; 32],
        report_data_suffix: &[u8; 32],
    ) -> Result<()> {
        if quote != [&nonce[..], report_data_suffix].concat() {
            bail!("report data does not match");
        }
        Ok(())
    }
}

/// Refuses every quote, as a verifier does for a TD running the wrong image.
pub(super) struct Refuse;

#[async_trait]
impl QuoteVerifier for Refuse {
    async fn verify(&self, _: &[u8], _: &[u8; 32], _: &[u8; 32]) -> Result<()> {
        bail!("not an image this policy accepts")
    }
}

/// What a stand-in node binds when asked, and how often it was asked.
#[derive(Clone, Default)]
pub(super) struct Attestation {
    pub transport_public_key: Option<[u8; 32]>,
    pub app_hash: Option<[u8; 32]>,
    pub calls: Arc<AtomicUsize>,
}

/// `POST /admin-api/tee/attest`, answering as the node does: report data is
/// the nonce, then the transport binding over the app hash (else zeros), or
/// the app hash alone when the key was not asked for.
pub(super) fn attest_route(attestation: Attestation) -> Router {
    Router::new().route(
        "/admin-api/tee/attest",
        post(move |Json(request): Json<TeeAttestRequest>| {
            let attestation = attestation.clone();
            async move {
                let _previous = attestation.calls.fetch_add(1, Ordering::SeqCst);
                let transport = request
                    .bind_transport_key
                    .then_some(attestation.transport_public_key)
                    .flatten();
                let inner = request
                    .application_id
                    .and(attestation.app_hash)
                    .unwrap_or([0; 32]);
                let suffix = transport.map_or(inner, |key| attest_transport_binding(&inner, &key));
                let quote = [hex::decode(&request.nonce).unwrap(), suffix.to_vec()].concat();
                Json(json!({
                    "data": {
                        "quoteB64": base64::engine::general_purpose::STANDARD.encode(quote),
                        "transportPublicKey": transport.map(hex::encode),
                    }
                }))
            }
        }),
    )
}

async fn serve(app: Router) -> Url {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    Url::parse(&format!("http://{addr}/")).unwrap()
}

#[tokio::test]
async fn the_key_is_trusted_only_once_the_quote_verifies() {
    let attestation = Attestation {
        transport_public_key: Some([0x44; 32]),
        ..Attestation::default()
    };
    let url = serve(attest_route(attestation)).await;
    let http = reqwest::Client::new();

    let key = Attestor::new(ReportData)
        .attest_transport_key(&http, &url)
        .await
        .unwrap();
    assert_eq!(key, [0x44; 32]);

    let refused = Attestor::new(Refuse)
        .attest_transport_key(&http, &url)
        .await;
    assert!(
        refused.is_err(),
        "a quote the verifier refuses names no key"
    );
}

#[tokio::test]
async fn a_node_that_does_not_name_its_key_is_refused() {
    // A node predating sealed transport answers without the key and without
    // binding it; its quote verifies for what it did bind, which is not enough.
    let url = serve(attest_route(Attestation::default())).await;
    let err = Attestor::new(ReportData)
        .attest_transport_key(&reqwest::Client::new(), &url)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("did not name its transport key"),
        "{err}"
    );
}

#[tokio::test]
async fn the_application_hash_is_bound_under_the_key() {
    let attestation = Attestation {
        transport_public_key: Some([0x44; 32]),
        app_hash: Some([0x22; 32]),
        ..Attestation::default()
    };
    let url = serve(attest_route(attestation)).await;
    let http = reqwest::Client::new();
    let app = ApplicationId::from([9; 32]);

    let right = Attestor::new(ReportData).with_application(app, [0x22; 32]);
    assert!(right.attest_transport_key(&http, &url).await.is_ok());

    let wrong = Attestor::new(ReportData).with_application(app, [0x23; 32]);
    assert!(
        wrong.attest_transport_key(&http, &url).await.is_err(),
        "a node running other bytecode does not verify"
    );
}

#[test]
fn the_suffix_matches_the_published_transport_vector() {
    // `calimero-tee-attestation`'s vector, which mero-js repeats: the client
    // recomputes exactly what the node binds.
    assert_eq!(
        hex::encode(report_data_suffix(Some(&[0x11; 32]), &[0x22; 32])),
        "30274595433e8afc5d4035e30a2b599d93caf0d470867527f13dc6af92fa12a8"
    );
}
