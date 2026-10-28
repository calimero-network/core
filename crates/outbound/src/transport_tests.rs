//! The real HTTPS transport, against a local TLS server: what the gates in
//! `outbound_tests.rs` decide, this sends. Only the resolver and the trusted
//! root differ from production, so the server can be on loopback.

use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use calimero_primitives::outbound::{OutboundMethod, MAX_RESPONSE_BODY};
use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use zeroize::Zeroizing;

use super::*;

const HOST: &str = "echo.local.test";

/// Answers every name with one address, as a test's DNS.
struct Fixed(SocketAddr);

impl reqwest::dns::Resolve for Fixed {
    fn resolve(&self, _name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let addr = self.0;
        Box::pin(async move { Ok(Box::new(std::iter::once(addr)) as reqwest::dns::Addrs) })
    }
}

/// What the server answers a request with: the raw response bytes, or `None`
/// to hold the connection open without answering.
type Respond = dyn Fn(&str) -> Option<Vec<u8>> + Send + Sync;

/// A TLS server on loopback for `HOST`, signed by its own CA, recording each
/// request it reads.
struct Server {
    addr: SocketAddr,
    root: reqwest::Certificate,
    seen: Arc<Mutex<Vec<String>>>,
    _runtime: tokio::runtime::Runtime,
}

fn serve(respond: impl Fn(&str) -> Option<Vec<u8>> + Send + Sync + 'static) -> Server {
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

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let listener = runtime.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let respond: Arc<Respond> = Arc::new(respond);
    let recorded = Arc::clone(&seen);
    let _accepting = runtime.spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let (acceptor, respond, recorded) = (
                acceptor.clone(),
                Arc::clone(&respond),
                Arc::clone(&recorded),
            );
            let _connection = tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let Ok(request) = read_request(&mut tls).await else {
                    return;
                };
                recorded.lock().unwrap().push(request.clone());
                match respond(&request) {
                    Some(response) => {
                        let _ = tls.write_all(&response).await;
                        let _ = tls.shutdown().await;
                    }
                    None => tokio::time::sleep(Duration::from_secs(30)).await,
                }
            });
        }
    });

    Server {
        addr,
        root: reqwest::Certificate::from_der(ca.der()).unwrap(),
        seen,
        _runtime: runtime,
    }
}

/// Reads one HTTP/1.1 request: the head, and a body of `Content-Length`.
async fn read_request(stream: &mut (impl AsyncReadExt + Unpin)) -> io::Result<String> {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf);
        if let Some(end) = text.find("\r\n\r\n") {
            let length = text[..end]
                .lines()
                .find_map(|l| {
                    let (name, value) = l.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            if buf.len() >= end + 4 + length {
                break;
            }
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn transport(server: &Server, timeout: Duration) -> HttpsTransport {
    HttpsTransport {
        resolver: Arc::new(Fixed(server.addr)),
        roots: vec![server.root.clone()],
        timeout,
    }
}

fn request(server: &Server, method: OutboundMethod, path: &str, body: &str) -> Prepared {
    Prepared {
        method,
        url: format!("https://{HOST}:{}{path}", server.addr.port())
            .parse()
            .unwrap(),
        host: HOST.to_owned(),
        headers: vec![("X-Probe".to_owned(), Zeroizing::new("calimero".to_owned()))],
        body: Zeroizing::new(body.as_bytes().to_vec()),
        secrets: Vec::new(),
    }
}

fn response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body);
    out
}

#[test]
fn a_request_reaches_the_server_over_tls_and_its_answer_comes_back() {
    let server = serve(|_| Some(response("201 Created", "", b"{\"success\":true}")));
    let (status, body) = transport(&server, Duration::from_secs(5))
        .send(&request(
            &server,
            OutboundMethod::Post,
            "/v1/check?x=1",
            "a=b",
        ))
        .expect("sent");
    assert_eq!(status, 201);
    assert_eq!(body, b"{\"success\":true}");

    let seen = server.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0].starts_with("POST /v1/check?x=1 HTTP/1.1\r\n"),
        "{}",
        seen[0]
    );
    assert!(
        seen[0].to_ascii_lowercase().contains("x-probe: calimero"),
        "{}",
        seen[0]
    );
    assert!(seen[0].ends_with("\r\n\r\na=b"), "{}", seen[0]);
}

#[test]
fn every_method_goes_out_as_itself() {
    let server = serve(|_| Some(response("204 No Content", "", b"")));
    let t = transport(&server, Duration::from_secs(5));
    for (method, name) in [
        (OutboundMethod::Get, "GET"),
        (OutboundMethod::Put, "PUT"),
        (OutboundMethod::Patch, "PATCH"),
        (OutboundMethod::Delete, "DELETE"),
    ] {
        let _ = t
            .send(&request(&server, method, "/v1/x", ""))
            .expect("sent");
        let last = server.seen.lock().unwrap().last().cloned().unwrap();
        assert!(last.starts_with(&format!("{name} /v1/x ")), "{last}");
    }
}

#[test]
fn a_redirect_is_handed_back_never_followed() {
    let server = serve(|_| {
        Some(response(
            "302 Found",
            "Location: https://169.254.169.254/latest/meta-data\r\n",
            b"",
        ))
    });
    let (status, _) = transport(&server, Duration::from_secs(5))
        .send(&request(&server, OutboundMethod::Get, "/v1/go", ""))
        .expect("sent");
    assert_eq!(status, 302);
    assert_eq!(server.seen.lock().unwrap().len(), 1, "nothing followed it");
}

#[test]
fn a_response_over_the_cap_is_refused_whether_or_not_it_says_its_length() {
    let big = vec![b'x'; MAX_RESPONSE_BODY + 1];
    let declared = big.clone();
    let server = serve(move |_| Some(response("200 OK", "", &declared)));
    let err = transport(&server, Duration::from_secs(5))
        .send(&request(&server, OutboundMethod::Get, "/v1/big", ""))
        .unwrap_err()
        .to_string();
    assert!(err.contains("longer than"), "{err}");

    // No Content-Length: the body ends when the connection closes, and the
    // transport stops reading at the cap.
    let server = serve(move |_| {
        let mut out = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
        out.extend_from_slice(&big);
        Some(out)
    });
    let err = transport(&server, Duration::from_secs(5))
        .send(&request(&server, OutboundMethod::Get, "/v1/big", ""))
        .unwrap_err()
        .to_string();
    assert!(err.contains("longer than"), "{err}");
}

#[test]
fn a_server_that_never_answers_runs_into_the_timeout() {
    let server = serve(|_| None);
    let started = std::time::Instant::now();
    let result = transport(&server, Duration::from_millis(300)).send(&request(
        &server,
        OutboundMethod::Get,
        "/v1/slow",
        "",
    ));
    assert!(result.is_err());
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_certificate_no_trusted_root_signed_is_refused() {
    let server = serve(|_| Some(response("200 OK", "", b"ok")));
    let other = serve(|_| Some(response("200 OK", "", b"ok")));
    // Trust the other server's CA only.
    let t = HttpsTransport {
        resolver: Arc::new(Fixed(server.addr)),
        roots: vec![other.root.clone()],
        timeout: Duration::from_secs(5),
    };
    assert!(t
        .send(&request(&server, OutboundMethod::Get, "/v1/x", ""))
        .is_err());
    assert!(
        server.seen.lock().unwrap().is_empty(),
        "no request was sent"
    );
}

#[test]
fn plain_http_is_refused_by_the_transport_too() {
    let server = serve(|_| Some(response("200 OK", "", b"ok")));
    let mut r = request(&server, OutboundMethod::Get, "/v1/x", "");
    r.url = format!("http://{HOST}:{}/v1/x", server.addr.port())
        .parse()
        .unwrap();
    assert!(transport(&server, Duration::from_secs(5)).send(&r).is_err());
    assert!(server.seen.lock().unwrap().is_empty());
}

#[test]
fn the_production_resolver_refuses_a_name_with_a_private_address() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let resolved = runtime.block_on(reqwest::dns::Resolve::resolve(
        &PublicOnly,
        "localhost".parse().unwrap(),
    ));
    let err = match resolved {
        Ok(_) => panic!("localhost resolved"),
        Err(e) => e.to_string(),
    };
    assert!(err.contains("not a public address"), "{err}");
}
