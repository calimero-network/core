//! Uploading sealed files: one request for a file of up to one part, else a session of parts.

use std::io::SeekFrom;
use std::time::Duration;

use eyre::WrapErr;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt};

use super::{retry_unavailable, route, Client, Error, Result};
use crate::types::UploadCompleteResponse;

/// The largest file sent as one request, and the part size a session is asked for.
pub const FILE_PART_BYTES: u64 = 16 * 1024 * 1024;
/// Times a part is sent again after a failed send before the node is asked where the session is.
const PART_RESENDS: u32 = 1;
/// Failed sends in a row, with no part accepted between them, before the upload gives up.
const MAX_FAILURES: u32 = 8;
/// `PART_OUT_OF_ORDER` answers followed in one session before the upload gives up.
const MAX_JUMPS: u32 = 16;
/// Times `complete` is sent again while the node is unavailable.
const COMPLETE_RETRIES: u32 = 8;
/// The wait before the first resend, doubling up to `BACKOFF_MAX`.
const BACKOFF_FIRST: Duration = Duration::from_millis(200);
const BACKOFF_MAX: Duration = Duration::from_secs(5);

fn backoff(failures: u32) -> Duration {
    (BACKOFF_FIRST * 2u32.saturating_pow(failures.saturating_sub(1))).min(BACKOFF_MAX)
}

/// Whether the same request may succeed if sent again: the node answered `503`,
/// or the connection failed or dropped before an answer came.
fn is_transient(e: &Error) -> bool {
    e.is_unavailable()
        || matches!(e, Error::Transport(r)
            if r.chain().any(|c| c.downcast_ref::<reqwest::Error>().is_some()))
}

/// The `next` an answer names, which cannot be past the last part.
fn next_part(next: u64, parts: u64) -> Result<u64> {
    if next > parts {
        return Err(eyre::eyre!("the node answered a bad next: {next} of {parts}").into());
    }
    Ok(next)
}

/// The `next` a refusal's `details` name (`409 PART_OUT_OF_ORDER`).
fn next_in(details: &Value, parts: u64) -> Option<u64> {
    details["next"].as_u64().filter(|n| *n <= parts)
}

/// Part `k` of a file of `size` bytes, read from `file`.
async fn read_part<R>(file: &mut R, k: u64, part_bytes: u64, size: u64) -> Result<bytes::Bytes>
where
    R: AsyncRead + AsyncSeek + Unpin,
{
    let start = k * part_bytes;
    let len = part_bytes.min(size - start);
    let mut buf = vec![0; usize::try_from(len).map_err(eyre::Report::from)?];
    let _pos = file
        .seek(SeekFrom::Start(start))
        .await
        .map_err(eyre::Report::from)?;
    let _read = file
        .read_exact(&mut buf)
        .await
        .wrap_err_with(|| format!("the file ended inside part {k}"))?;
    Ok(buf.into())
}

impl Client {
    /// Upload a sealed file of `size` bytes for component app `app`; answers `{ blobId, id, size, sha256, index }`.
    /// Up to [`FILE_PART_BYTES`] it is one request, else a session of parts, each resent after a `503`.
    ///
    /// # Errors
    /// The node's refusal (`413`, `429` over quota, ...), or a transport failure that outlasted the retries.
    pub async fn upload_file<R>(&self, app: &str, mut file: R, size: u64) -> Result<Value>
    where
        R: AsyncRead + AsyncSeek + Unpin + Send,
    {
        if size <= FILE_PART_BYTES {
            let body = read_part(&mut file, 0, FILE_PART_BYTES, size).await?;
            return self.upload_once(app, body.to_vec()).await;
        }
        let done = match self.upload_session(app, &mut file, size).await {
            Err(e) if e.code() == Some("UPLOAD_UNKNOWN") => {
                self.upload_session(app, &mut file, size).await
            }
            done => done,
        }?;
        route::value(&done)
    }

    /// The single request: the node mints a new key for each, so one that may have reached it is
    /// never sent again; only a failed connect is.
    async fn upload_once(&self, app: &str, body: Vec<u8>) -> Result<Value> {
        let mut failures = 0;
        loop {
            let sent = self.apps().files_upload(app, body.clone()).await;
            match sent {
                Err(Error::Transport(e))
                    if failures < MAX_FAILURES
                        && e.chain().any(|c| {
                            c.downcast_ref::<reqwest::Error>()
                                .is_some_and(reqwest::Error::is_connect)
                        }) =>
                {
                    failures += 1;
                    tokio::time::sleep(backoff(failures)).await;
                }
                done => return route::value(&done?),
            }
        }
    }

    async fn upload_session<R>(
        &self,
        app: &str,
        file: &mut R,
        size: u64,
    ) -> Result<UploadCompleteResponse>
    where
        R: AsyncRead + AsyncSeek + Unpin,
    {
        let begin = retry_unavailable(|| self.apps().upload_begin(app, size)).await?;
        let id = begin.upload_id.as_str();
        let part_bytes = begin.part_bytes;
        if id.is_empty() || part_bytes == 0 || begin.parts != size.div_ceil(part_bytes) {
            return Err(eyre::eyre!(
                "the node began an upload it cannot take: {} parts of {part_bytes} bytes",
                begin.parts
            )
            .into());
        }
        let done = self.send_parts(app, id, file, size, part_bytes).await;
        if done
            .as_ref()
            .is_err_and(|e| e.code() != Some("UPLOAD_UNKNOWN"))
        {
            let _aborted = self.apps().upload_abort(app, id).await;
        }
        done
    }

    async fn send_parts<R>(
        &self,
        app: &str,
        upload: &str,
        file: &mut R,
        size: u64,
        part_bytes: u64,
    ) -> Result<UploadCompleteResponse>
    where
        R: AsyncRead + AsyncSeek + Unpin,
    {
        let parts = size.div_ceil(part_bytes);
        let (mut next, mut failures, mut jumps) = (0, 0, 0);
        while next < parts {
            let body = read_part(file, next, part_bytes, size).await?;
            let sent = self
                .apps()
                .upload_part(app, upload, &next.to_string(), body.to_vec())
                .await;
            let failed = match sent {
                Ok(v) => {
                    next = next_part(v.next, parts)?;
                    failures = 0;
                    continue;
                }
                Err(e) if e.code() == Some("PART_OUT_OF_ORDER") => {
                    jumps += 1;
                    let named = e.details().and_then(|d| next_in(d, parts));
                    next = named.filter(|_| jumps <= MAX_JUMPS).ok_or(e)?;
                    continue;
                }
                Err(e) if is_transient(&e) => e,
                Err(e) => return Err(e),
            };
            failures += 1;
            if failures > MAX_FAILURES {
                return Err(failed);
            }
            tokio::time::sleep(backoff(failures)).await;
            if failures > PART_RESENDS {
                match self.apps().upload_status(app, upload).await {
                    Ok(v) => {
                        let node_next = next_part(v.next, parts)?;
                        if node_next > next {
                            failures = 0;
                        }
                        next = node_next;
                    }
                    Err(e) if is_transient(&e) => {}
                    Err(e) => return Err(e),
                }
            }
        }
        self.complete(app, upload).await
    }

    /// `complete`, sent again while the node is unavailable: a retried one resumes.
    async fn complete(&self, app: &str, upload: &str) -> Result<UploadCompleteResponse> {
        let mut failures = 0;
        loop {
            match self.apps().upload_complete(app, upload).await {
                Err(e) if is_transient(&e) && failures < COMPLETE_RETRIES => {
                    failures += 1;
                    tokio::time::sleep(backoff(failures)).await;
                }
                done => return done,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, SeekFrom};
    use std::net::SocketAddr;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};

    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::{Method, StatusCode, Uri};
    use axum::response::{IntoResponse, Response};
    use axum::routing::any;
    use axum::{Json, Router};
    use calimero_sealed::{intercept, SealedOptions, SealedTransport as Server};
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncRead, AsyncSeek, ReadBuf};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::Notify;

    use super::FILE_PART_BYTES;
    use crate::{Client, Error, Policy};

    /// What a scripted fault does to the request it matches, once.
    enum Reply {
        /// Answer this status and body instead of handling the request.
        Say(u16, Value),
        /// Forget the live session, then handle the request as usual.
        Forget,
        /// Cut every connection to the node while the request is open;
        /// `stored` says whether the node handled the request first.
        Cut { stored: bool },
    }

    struct Fault {
        method: &'static str,
        /// The request path after `apps/app/files/`; empty is the single-request route.
        tail: &'static str,
        reply: Reply,
    }

    enum Outcome {
        Say(u16, Value),
        Cut,
    }

    /// The node's side of the files API, as far as the client sees it.
    #[derive(Default)]
    struct Node {
        log: Vec<String>,
        faults: Vec<Fault>,
        session: Option<String>,
        begun: u32,
        parts: u64,
        next: u64,
        hashes: Vec<Option<[u8; 32]>>,
        /// Where a new session starts: a node already ahead of the client.
        begin_next: u64,
    }

    impl Node {
        fn step(&mut self, method: &str, path: &str, body: &[u8]) -> Outcome {
            let shown = if path.is_empty() { "files" } else { path };
            self.log.push(format!("{method} {shown}"));
            let mut cut = false;
            let hit = self.faults.iter().position(|f| {
                f.method == method
                    && if f.tail.is_empty() {
                        path.is_empty()
                    } else {
                        path.ends_with(f.tail)
                    }
            });
            if let Some(i) = hit {
                match self.faults.remove(i).reply {
                    Reply::Say(status, v) => return Outcome::Say(status, v),
                    Reply::Forget => self.session = None,
                    Reply::Cut { stored } => {
                        if !stored {
                            return Outcome::Cut;
                        }
                        cut = true;
                    }
                }
            }
            let out = self.route(method, path, body);
            if cut {
                Outcome::Cut
            } else {
                out
            }
        }

        fn route(&mut self, method: &str, path: &str, body: &[u8]) -> Outcome {
            let segs: Vec<&str> = path.split('/').collect();
            match (method, segs.as_slice()) {
                ("POST", [""]) => Outcome::Say(
                    200,
                    json!({
                        "blobId": "b1",
                        "id": "f1",
                        "size": body.len(),
                        "sha256": hex::encode(Sha256::digest(body)),
                        "index": 1,
                    }),
                ),
                ("POST", ["uploads"]) => {
                    let size = serde_json::from_slice::<Value>(body).unwrap()["size"]
                        .as_u64()
                        .unwrap();
                    self.begun += 1;
                    self.session = Some(format!("u{}", self.begun));
                    self.parts = size.div_ceil(FILE_PART_BYTES);
                    self.next = self.begin_next;
                    self.hashes = vec![None; self.parts as usize];
                    Outcome::Say(
                        200,
                        json!({
                            "uploadId": self.session,
                            "partBytes": FILE_PART_BYTES,
                            "parts": self.parts,
                            "expiresAt": 0,
                        }),
                    )
                }
                (_, ["uploads", id, ..]) if self.session.as_deref() != Some(*id) => {
                    Outcome::Say(404, json!({ "error": "UPLOAD_UNKNOWN", "message": "gone" }))
                }
                ("PUT", ["uploads", _, "parts", k]) => self.put_part(k.parse().unwrap(), body),
                ("GET", ["uploads", _]) => Outcome::Say(
                    200,
                    json!({
                        "uploadId": self.session,
                        "partBytes": FILE_PART_BYTES,
                        "parts": self.parts,
                        "expiresAt": 0,
                        "next": self.next,
                    }),
                ),
                ("POST", ["uploads", _, "complete"]) => {
                    assert_eq!(self.next, self.parts, "complete before every part");
                    let mut all = Sha256::new();
                    for h in self.hashes.iter().flatten() {
                        all.update(h);
                    }
                    Outcome::Say(
                        200,
                        json!({ "blobId": "b1", "id": "f1", "size": 0, "sha256": hex::encode(all.finalize()), "index": 7 }),
                    )
                }
                ("DELETE", ["uploads", _]) => {
                    self.session = None;
                    Outcome::Say(200, json!({}))
                }
                _ => panic!("unexpected request {method} {path}"),
            }
        }

        fn put_part(&mut self, k: u64, body: &[u8]) -> Outcome {
            let hash: [u8; 32] = Sha256::digest(body).into();
            let ahead = || {
                Outcome::Say(
                    409,
                    json!({
                        "error": "PART_OUT_OF_ORDER",
                        "message": "ahead",
                        "details": { "next": self.next },
                    }),
                )
            };
            if k > self.next {
                return ahead();
            }
            if k < self.next {
                return match self.hashes[k as usize] {
                    Some(old) if old == hash => Outcome::Say(200, json!({ "next": k + 1 })),
                    Some(_) => {
                        Outcome::Say(409, json!({ "error": "PART_CONFLICT", "message": "" }))
                    }
                    None => ahead(),
                };
            }
            self.hashes[k as usize] = Some(hash);
            self.next += 1;
            Outcome::Say(200, json!({ "next": self.next }))
        }
    }

    struct Mock {
        node: Mutex<Node>,
        cut: Notify,
    }

    async fn handle(
        State(mock): State<Arc<Mock>>,
        method: Method,
        uri: Uri,
        body: Bytes,
    ) -> Response {
        let path = uri
            .path()
            .strip_prefix("/apps/app/files")
            .unwrap_or_else(|| panic!("unexpected path {}", uri.path()))
            .trim_start_matches('/')
            .to_owned();
        let out = mock
            .node
            .lock()
            .unwrap()
            .step(method.as_str(), &path, &body);
        match out {
            Outcome::Say(status, v) => {
                (StatusCode::from_u16(status).unwrap(), Json(v)).into_response()
            }
            Outcome::Cut => {
                mock.cut.notify_waiters();
                std::future::pending().await
            }
        }
    }

    /// Forwards to `upstream` until the mock asks for the connections to be cut.
    async fn proxy(listener: TcpListener, upstream: SocketAddr, mock: Arc<Mock>) {
        loop {
            let (mut a, _) = listener.accept().await.unwrap();
            let mock = Arc::clone(&mock);
            drop(tokio::spawn(async move {
                let mut b = TcpStream::connect(upstream).await.unwrap();
                tokio::select! {
                    _ = tokio::io::copy_bidirectional(&mut a, &mut b) => {}
                    () = mock.cut.notified() => {}
                }
            }));
        }
    }

    /// A client over the sealed channel to a mock node, through a proxy that can cut it.
    async fn rig() -> (Client, Arc<Mock>) {
        let mock = Arc::new(Mock {
            node: Mutex::default(),
            cut: Notify::new(),
        });
        let server = Arc::new(Server::generate(&SealedOptions::default()));
        let router = Router::new()
            .route("/{*rest}", any(handle))
            .layer(axum::extract::DefaultBodyLimit::disable())
            .with_state(Arc::clone(&mock));
        let app = tower::Layer::layer(
            &axum::middleware::from_fn_with_state(Arc::clone(&server), intercept),
            router,
        );
        let node = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let node_addr = node.local_addr().unwrap();
        drop(tokio::spawn(async move {
            axum::serve(
                node,
                axum::ServiceExt::<axum::extract::Request>::into_make_service(app),
            )
            .await
            .unwrap();
        }));
        let front = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base: url::Url = format!("http://{}/", front.local_addr().unwrap())
            .parse()
            .unwrap();
        drop(tokio::spawn(proxy(front, node_addr, Arc::clone(&mock))));
        let http = reqwest::Client::new();
        let sealed = Arc::new(calimero_sealed::client::SealedTransport::with_key(
            base.clone(),
            http.clone(),
            server.public_key(),
        ));
        let client = Client {
            transport: crate::Transport::from_parts(base, http, sealed),
            policy: Policy::default(),
        };
        (client, mock)
    }

    impl Mock {
        fn fault(&self, method: &'static str, tail: &'static str, reply: Reply) {
            self.node.lock().unwrap().faults.push(Fault {
                method,
                tail,
                reply,
            });
        }

        fn log(&self) -> Vec<String> {
            self.node.lock().unwrap().log.clone()
        }
    }

    fn data(len: u64) -> Vec<u8> {
        (0..len).map(|i| (i * 31 % 251) as u8).collect()
    }

    /// What `complete` answers for `data` when every part arrived intact.
    fn sha_of_parts(data: &[u8]) -> String {
        let mut all = Sha256::new();
        for part in data.chunks(FILE_PART_BYTES as usize) {
            all.update(Sha256::digest(part));
        }
        hex::encode(all.finalize())
    }

    /// Three parts: two whole and a last of one byte.
    const THREE_PARTS: u64 = 2 * FILE_PART_BYTES + 1;

    fn put(k: u64) -> String {
        format!("PUT uploads/u1/parts/{k}")
    }

    const BEGIN: &str = "POST uploads";
    const COMPLETE: &str = "POST uploads/u1/complete";

    /// A reader that remembers the most it was asked for in one read.
    struct Counting {
        inner: Cursor<Vec<u8>>,
        widest: Arc<AtomicU64>,
    }

    impl AsyncRead for Counting {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            let _prev = self
                .widest
                .fetch_max(buf.remaining() as u64, Ordering::Relaxed);
            Pin::new(&mut self.inner).poll_read(cx, buf)
        }
    }

    impl AsyncSeek for Counting {
        fn start_seek(mut self: Pin<&mut Self>, position: SeekFrom) -> std::io::Result<()> {
            Pin::new(&mut self.inner).start_seek(position)
        }

        fn poll_complete(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<u64>> {
            Pin::new(&mut self.inner).poll_complete(cx)
        }
    }

    #[tokio::test]
    async fn a_small_file_is_one_request() {
        let (client, mock) = rig().await;
        let bytes = data(1000);
        let done = client
            .upload_file("app", Cursor::new(bytes.clone()), 1000)
            .await
            .unwrap();
        assert_eq!(mock.log(), ["POST files"]);
        assert_eq!(done["sha256"], hex::encode(Sha256::digest(&bytes)));
        assert_eq!(done["size"], 1000);
    }

    #[tokio::test]
    async fn a_file_of_one_part_is_one_request() {
        let (client, mock) = rig().await;
        let bytes = data(FILE_PART_BYTES);
        let _done = client
            .upload_file("app", Cursor::new(bytes), FILE_PART_BYTES)
            .await
            .unwrap();
        assert_eq!(mock.log(), ["POST files"]);
    }

    #[tokio::test]
    async fn a_large_file_goes_up_in_three_parts_read_one_at_a_time() {
        let (client, mock) = rig().await;
        let bytes = data(THREE_PARTS);
        let widest = Arc::new(AtomicU64::new(0));
        let reader = Counting {
            inner: Cursor::new(bytes.clone()),
            widest: Arc::clone(&widest),
        };
        let done = client
            .upload_file("app", reader, THREE_PARTS)
            .await
            .unwrap();
        assert_eq!(
            mock.log(),
            [BEGIN, &put(0), &put(1), &put(2), COMPLETE].map(String::from)
        );
        assert_eq!(done["sha256"], sha_of_parts(&bytes));
        assert_eq!(done["index"], 7);
        assert!(widest.load(Ordering::Relaxed) <= FILE_PART_BYTES);
    }

    #[tokio::test]
    async fn a_connection_dropped_mid_part_resends_the_part() {
        let (client, mock) = rig().await;
        mock.fault("PUT", "parts/1", Reply::Cut { stored: false });
        let bytes = data(THREE_PARTS);
        let done = client
            .upload_file("app", Cursor::new(bytes.clone()), THREE_PARTS)
            .await
            .unwrap();
        assert_eq!(
            mock.log(),
            [BEGIN, &put(0), &put(1), &put(1), &put(2), COMPLETE].map(String::from)
        );
        assert_eq!(done["sha256"], sha_of_parts(&bytes));
    }

    #[tokio::test]
    async fn a_part_the_node_kept_but_never_acked_resumes_from_next() {
        let (client, mock) = rig().await;
        mock.fault("PUT", "parts/1", Reply::Cut { stored: true });
        mock.fault("PUT", "parts/1", Reply::Cut { stored: false });
        let bytes = data(THREE_PARTS);
        let done = client
            .upload_file("app", Cursor::new(bytes.clone()), THREE_PARTS)
            .await
            .unwrap();
        assert_eq!(
            mock.log(),
            [
                BEGIN,
                &put(0),
                &put(1),
                &put(1),
                "GET uploads/u1",
                &put(2),
                COMPLETE
            ]
            .map(String::from)
        );
        assert_eq!(done["sha256"], sha_of_parts(&bytes));
    }

    #[tokio::test]
    async fn a_503_on_a_part_resends_it() {
        let (client, mock) = rig().await;
        let busy = json!({ "error": "FILES_UNAVAILABLE", "message": "" });
        mock.fault("PUT", "parts/1", Reply::Say(503, busy));
        let bytes = data(THREE_PARTS);
        let done = client
            .upload_file("app", Cursor::new(bytes.clone()), THREE_PARTS)
            .await
            .unwrap();
        assert_eq!(
            mock.log(),
            [BEGIN, &put(0), &put(1), &put(1), &put(2), COMPLETE].map(String::from)
        );
        assert_eq!(done["sha256"], sha_of_parts(&bytes));
    }

    #[tokio::test]
    async fn part_out_of_order_jumps_to_the_named_next() {
        let (client, mock) = rig().await;
        mock.node.lock().unwrap().begin_next = 2;
        let _done = client
            .upload_file("app", Cursor::new(data(THREE_PARTS)), THREE_PARTS)
            .await
            .unwrap();
        assert_eq!(
            mock.log(),
            [BEGIN, &put(0), &put(2), COMPLETE].map(String::from)
        );
    }

    #[tokio::test]
    async fn an_unknown_session_restarts_once_from_the_beginning() {
        let (client, mock) = rig().await;
        mock.fault("PUT", "parts/1", Reply::Forget);
        let bytes = data(THREE_PARTS);
        let done = client
            .upload_file("app", Cursor::new(bytes.clone()), THREE_PARTS)
            .await
            .unwrap();
        let put2 = |k: u64| format!("PUT uploads/u2/parts/{k}");
        assert_eq!(
            mock.log(),
            [
                BEGIN.to_owned(),
                put(0),
                put(1),
                BEGIN.to_owned(),
                put2(0),
                put2(1),
                put2(2),
                "POST uploads/u2/complete".to_owned()
            ]
        );
        assert_eq!(done["sha256"], sha_of_parts(&bytes));
    }

    #[tokio::test]
    async fn a_second_unknown_session_is_an_error() {
        let (client, mock) = rig().await;
        mock.fault("PUT", "parts/0", Reply::Forget);
        mock.fault("PUT", "parts/0", Reply::Forget);
        let err = client
            .upload_file("app", Cursor::new(data(THREE_PARTS)), THREE_PARTS)
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some("UPLOAD_UNKNOWN"));
        assert_eq!(mock.log().iter().filter(|l| *l == BEGIN).count(), 2);
    }

    #[tokio::test]
    async fn complete_is_sent_again_on_503() {
        let (client, mock) = rig().await;
        let busy = json!({ "error": "FILES_UNAVAILABLE", "message": "" });
        mock.fault("POST", "complete", Reply::Say(503, busy.clone()));
        mock.fault("POST", "complete", Reply::Say(503, busy));
        let bytes = data(THREE_PARTS);
        let done = client
            .upload_file("app", Cursor::new(bytes.clone()), THREE_PARTS)
            .await
            .unwrap();
        assert_eq!(
            mock.log(),
            [
                BEGIN,
                &put(0),
                &put(1),
                &put(2),
                COMPLETE,
                COMPLETE,
                COMPLETE
            ]
            .map(String::from)
        );
        assert_eq!(done["sha256"], sha_of_parts(&bytes));
    }

    #[tokio::test]
    async fn a_413_stops_the_upload() {
        let (client, mock) = rig().await;
        let body = json!({ "error": "TOO_LARGE", "message": "over the limit" });
        mock.fault("POST", "uploads", Reply::Say(413, body));
        let err = client
            .upload_file("app", Cursor::new(data(THREE_PARTS)), THREE_PARTS)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Api { status: 413, .. }), "{err:?}");
        assert_eq!(err.code(), Some("TOO_LARGE"));
        assert_eq!(mock.log(), [BEGIN]);
    }

    #[tokio::test]
    async fn a_429_stops_the_upload_and_aborts_the_session() {
        let (client, mock) = rig().await;
        let body = json!({ "error": "QUOTA_EXCEEDED", "message": "over quota" });
        mock.fault("PUT", "parts/1", Reply::Say(429, body));
        let err = client
            .upload_file("app", Cursor::new(data(THREE_PARTS)), THREE_PARTS)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Api { status: 429, .. }), "{err:?}");
        assert_eq!(
            mock.log(),
            [BEGIN, &put(0), &put(1), "DELETE uploads/u1"].map(String::from)
        );
    }

    #[tokio::test]
    async fn a_429_on_a_small_file_is_not_sent_again() {
        let (client, mock) = rig().await;
        let body = json!({ "error": "QUOTA_EXCEEDED", "message": "over quota" });
        mock.fault("POST", "", Reply::Say(429, body));
        let err = client
            .upload_file("app", Cursor::new(data(10)), 10)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Api { status: 429, .. }), "{err:?}");
        assert_eq!(mock.log(), ["POST files"]);
    }

    #[tokio::test]
    async fn a_503_on_a_small_file_is_not_sent_again() {
        let (client, mock) = rig().await;
        let busy = json!({ "error": "FILES_UNAVAILABLE", "message": "" });
        mock.fault("POST", "", Reply::Say(503, busy));
        let err = client
            .upload_file("app", Cursor::new(data(10)), 10)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Api { status: 503, .. }), "{err:?}");
        assert_eq!(mock.log(), ["POST files"]);
    }
}
