//! `--tls-extra-roots`: a peer whose certificate chains to a CA outside the web
//! roots (the Let's Encrypt staging CA on a test cluster) is refused by the
//! peer client as built before, and reached once that CA is an extra root.

use std::net::SocketAddr;
use std::sync::Arc;

use calimero_node::peer_tls::{load_extra_roots, peer_client};
use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

const HOST: &str = "peer.local.test";

/// An HTTPS server on loopback for `HOST` answering `200 ok` to anything, with
/// a leaf signed by its own CA; returns its address and the CA in PEM.
async fn serve() -> (SocketAddr, String) {
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca_key = KeyPair::generate().unwrap();
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let leaf_key = KeyPair::generate().unwrap();
    let leaf = CertificateParams::new(vec![HOST.to_owned()])
        .unwrap()
        .signed_by(&leaf_key, &ca, &ca_key)
        .unwrap();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![CertificateDer::from(leaf.der().to_vec())],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der())),
    )
    .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            drop(tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut buf = [0_u8; 4096];
                let _read = tls.read(&mut buf).await;
                let _written = tls
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
                    )
                    .await;
                let _closed = tls.shutdown().await;
            }));
        }
    }));
    (addr, ca.pem())
}

fn write_pem(dir: &tempfile::TempDir, name: &str, pem: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, pem).unwrap();
    path
}

#[tokio::test]
async fn a_peer_under_an_extra_root_is_refused_without_it_and_reached_with_it() {
    let (addr, ca_pem) = serve().await;
    let url = format!("https://{HOST}:{}/attestation", addr.port());
    let dir = tempfile::tempdir().unwrap();

    let web_roots_only = peer_client(&[]).resolve(HOST, addr).build().unwrap();
    let refused = web_roots_only.get(&url).send().await;
    assert!(
        refused.is_err(),
        "a CA outside the web roots must not be trusted by default"
    );

    let roots = load_extra_roots(&write_pem(&dir, "roots.pem", &ca_pem)).unwrap();
    assert_eq!(roots.len(), 1);
    let with_root = peer_client(&roots).resolve(HOST, addr).build().unwrap();
    let answer = with_root.get(&url).send().await.unwrap();
    assert_eq!(answer.status(), 200);
    assert_eq!(answer.text().await.unwrap(), "ok");
}

#[test]
fn an_extra_roots_file_without_a_certificate_stops_the_node() {
    let dir = tempfile::tempdir().unwrap();
    let empty = write_pem(&dir, "empty.pem", "");
    let err = load_extra_roots(&empty).unwrap_err().to_string();
    assert!(err.contains("holds no certificate"), "{err}");

    let not_pem = write_pem(&dir, "notes.txt", "these are not certificates\n");
    let err = load_extra_roots(&not_pem).unwrap_err().to_string();
    assert!(err.contains("holds no certificate"), "{err}");

    let missing = dir.path().join("missing.pem");
    let err = load_extra_roots(&missing).unwrap_err().to_string();
    assert!(err.contains("cannot read"), "{err}");

    let broken = write_pem(
        &dir,
        "broken.pem",
        "-----BEGIN CERTIFICATE-----\nnot base64 at all!\n-----END CERTIFICATE-----\n",
    );
    assert!(load_extra_roots(&broken).is_err());
}
