//! A stand-in for a node's attestation, and the attested-TLS tests.
//!
//! The quote here is only the 64 report data bytes, and [`ReportData`]
//! "verifies" it by comparing them: what is under test is which keys a client
//! asks to have bound, how it nests the bindings, and what it does with the keys
//! once they verify — not DCAP, which `calimero-tee-attestation` covers.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::TeeAttestRequest;
use calimero_tee_attestation::{attest_report_data_suffix, tls_spki_sha256};
use eyre::{bail, Result};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use hyper_util::service::TowerToHyperService;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::json;
use tokio::net::TcpListener;
use url::Url;

use super::tls::AttestedTls;
use super::{report_data_suffix, AttestedKeys, Attestor, Bind, QuoteVerifier};

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
    pub tls_spki_sha256: Option<[u8; 32]>,
    pub app_hash: Option<[u8; 32]>,
    pub calls: Arc<AtomicUsize>,
}

/// `POST /admin-api/tee/attest`, answering as the node does: report data is
/// the nonce and the bindings asked for, nested as the node nests them.
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
                let tls = request
                    .bind_tls_key
                    .then_some(attestation.tls_spki_sha256)
                    .flatten();
                let app = request.application_id.and(attestation.app_hash);
                let suffix = attest_report_data_suffix(app, tls.as_ref(), transport.as_ref())
                    .unwrap_or([0; 32]);
                let quote = [hex::decode(&request.nonce).unwrap(), suffix.to_vec()].concat();
                Json(json!({
                    "data": {
                        "quoteB64": base64::engine::general_purpose::STANDARD.encode(quote),
                        "transportPublicKey": transport.map(hex::encode),
                        "tlsSpkiSha256": tls.map(hex::encode),
                    }
                }))
            }
        }),
    )
}

async fn serve_plain(app: Router) -> Url {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    Url::parse(&format!("http://{addr}/")).unwrap()
}

#[tokio::test]
async fn keys_are_trusted_only_once_the_quote_verifies() {
    let attestation = Attestation {
        transport_public_key: Some([0x44; 32]),
        tls_spki_sha256: Some([0x55; 32]),
        ..Attestation::default()
    };
    let url = serve_plain(attest_route(attestation)).await;
    let http = reqwest::Client::new();
    let both = Bind {
        transport_key: true,
        tls_key: true,
    };

    let keys = Attestor::new(ReportData)
        .attest(&http, &url, both)
        .await
        .unwrap();
    assert_eq!(
        keys,
        AttestedKeys {
            transport_public_key: Some([0x44; 32]),
            tls_spki_sha256: Some([0x55; 32]),
        }
    );

    let refused = Attestor::new(Refuse).attest(&http, &url, both).await;
    assert!(
        refused.is_err(),
        "a quote the verifier refuses names no keys"
    );
}

#[tokio::test]
async fn a_node_that_does_not_name_a_key_asked_for_is_refused() {
    // A node predating the binding answers without the key and without
    // binding it; its quote verifies for what it did bind, which is not enough.
    let url = serve_plain(attest_route(Attestation::default())).await;
    let err = Attestor::new(ReportData)
        .attest(
            &reqwest::Client::new(),
            &url,
            Bind {
                tls_key: true,
                ..Bind::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("did not name its TLS key"),
        "{err}"
    );
}

#[tokio::test]
async fn the_application_hash_is_bound_under_the_keys() {
    let attestation = Attestation {
        transport_public_key: Some([0x44; 32]),
        app_hash: Some([0x22; 32]),
        ..Attestation::default()
    };
    let url = serve_plain(attest_route(attestation)).await;
    let http = reqwest::Client::new();
    let bind = Bind {
        transport_key: true,
        ..Bind::default()
    };
    let app = ApplicationId::from([9; 32]);

    let right = Attestor::new(ReportData).with_application(app, [0x22; 32]);
    assert!(right.attest(&http, &url, bind).await.is_ok());

    let wrong = Attestor::new(ReportData).with_application(app, [0x23; 32]);
    assert!(
        wrong.attest(&http, &url, bind).await.is_err(),
        "a node running other bytecode does not verify"
    );
}

#[test]
fn the_suffix_is_zeros_when_nothing_is_bound() {
    assert_eq!(report_data_suffix(None, &AttestedKeys::default()), [0; 32]);
}

/// A TLS server with a fresh self-signed key, serving `app`, and the SHA-256
/// of its key.
async fn serve_tls(app: Router) -> (SocketAddr, [u8; 32]) {
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let cert = CertificateDer::from(certified.cert.der().to_vec());
    let spki = tls_spki_sha256(&cert).unwrap();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(certified.key_pair.serialize_der()));
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![cert], key)
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            let app = app.clone();
            drop(tokio::spawn(async move {
                // A client that refuses the key ends the handshake here.
                let Ok(stream) = acceptor.accept(stream).await else {
                    return;
                };
                let _ended = auto::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(stream), TowerToHyperService::new(app))
                    .await;
            }));
        }
    }));
    (addr, spki)
}

fn https(addr: SocketAddr) -> Url {
    Url::parse(&format!("https://{addr}/")).unwrap()
}

fn ping() -> Router {
    Router::new().route("/ping", get(|| async { "pong" }))
}

/// A node serving its own key, attesting to it.
async fn node() -> (SocketAddr, [u8; 32]) {
    // The route needs the key before the server exists, so the server is
    // started first with a placeholder and the key read back from it.
    let spki = Arc::new(std::sync::OnceLock::new());
    let app = {
        let spki = Arc::clone(&spki);
        ping().route(
            "/admin-api/tee/attest",
            post(move |Json(request): Json<TeeAttestRequest>| {
                let spki = Arc::clone(&spki);
                async move {
                    let spki: [u8; 32] = *spki.get().unwrap();
                    let tls = request.bind_tls_key.then_some(spki);
                    let suffix =
                        attest_report_data_suffix(None, tls.as_ref(), None).unwrap_or([0; 32]);
                    let quote = [hex::decode(&request.nonce).unwrap(), suffix.to_vec()].concat();
                    Json(json!({
                        "data": {
                            "quoteB64": base64::engine::general_purpose::STANDARD.encode(quote),
                            "tlsSpkiSha256": tls.map(hex::encode),
                        }
                    }))
                }
            }),
        )
    };
    let (addr, key) = serve_tls(app).await;
    spki.set(key).unwrap();
    (addr, key)
}

async fn get_ping(tls: &AttestedTls, addr: SocketAddr) -> reqwest::Result<String> {
    tls.client()
        .get(https(addr).join("ping").unwrap())
        .send()
        .await?
        .text()
        .await
}

#[tokio::test]
async fn attested_tls_pins_the_key_the_quote_commits_to() {
    let (addr, spki) = node().await;
    let tls = AttestedTls::connect(&https(addr), &Attestor::new(ReportData))
        .await
        .unwrap();
    assert_eq!(tls.spki_sha256(), &spki);
    // A self-signed certificate, reached by IP: no CA or name would accept it,
    // and the pin does.
    assert_eq!(get_ping(&tls, addr).await.unwrap(), "pong");
}

#[tokio::test]
async fn a_server_without_the_pinned_key_is_refused() {
    let (node_addr, _) = node().await;
    let tls = AttestedTls::connect(&https(node_addr), &Attestor::new(ReportData))
        .await
        .unwrap();

    // Whoever else answers — a proxy terminating TLS with its own key, or
    // another server entirely — does not hold the pinned key.
    let (other, _) = serve_tls(ping()).await;
    assert!(get_ping(&tls, other).await.is_err());
}

#[tokio::test]
async fn an_impostor_relaying_a_genuine_attestation_cannot_use_it() {
    // The impostor answers the attestation with the genuine node's quote and
    // key, which verify, but it cannot complete a handshake with that key.
    let (_, genuine_spki) = node().await;
    let impostor_answer = Attestation {
        tls_spki_sha256: Some(genuine_spki),
        ..Attestation::default()
    };
    let (impostor, _) = serve_tls(ping().merge(attest_route(impostor_answer))).await;

    let tls = AttestedTls::connect(&https(impostor), &Attestor::new(ReportData))
        .await
        .unwrap();
    assert_eq!(tls.spki_sha256(), &genuine_spki);
    assert!(
        get_ping(&tls, impostor).await.is_err(),
        "the pinned key is the genuine node's, which the impostor does not hold"
    );
}

#[tokio::test]
async fn attested_tls_refuses_what_it_cannot_pin() {
    let (addr, _) = node().await;
    assert!(
        AttestedTls::connect(&https(addr), &Attestor::new(Refuse))
            .await
            .is_err(),
        "an attestation the verifier refuses pins nothing"
    );
    let plain = Url::parse(&format!("http://{addr}/")).unwrap();
    assert!(AttestedTls::connect(&plain, &Attestor::new(ReportData))
        .await
        .is_err());
}
