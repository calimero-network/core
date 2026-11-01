//! `wasi:http/types` and `wasi:http/outgoing-handler` (D12, S9), over `HostCore`.
//!
//! Every body is a bounded in-memory buffer (`MAX_HTTP_BODY_BYTES`): a write past it fails
//! the stream instead of blocking, and every pollable is ready at once, so no guest can
//! park a node thread past its epoch deadline (the reason wasmtime-wasi-http is not used).
//! Building fields, requests and bodies works in every run; `outgoing-handler.handle`
//! asks `HostCore::http_send`, which only an action passes (calls and views trap
//! `HostDenied`, S9) and which hands the request to the node's outbound gate. The response
//! is read whole before `handle` returns. The server side (incoming requests, outgoing
//! responses) is not part of the world's use and traps.

use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;

use calimero_app_host::{HostDenied, HttpRequest, MAX_HTTP_BODY_BYTES};
use wasmtime::component::Resource;
use wasmtime::Result;
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};
use wasmtime_wasi::p2::{DynInputStream, DynOutputStream, DynPollable, Pollable};

use crate::bindings::wasi::http::outgoing_handler;
use crate::bindings::wasi::http::types::{self as http, ErrorCode, HeaderError, Method, Scheme};
use crate::errors::denied;
use crate::StoreData;

/// Header entries one `fields` holds at most.
pub const MAX_HEADERS: usize = 100;

/// Header names and values.
type Entries = Vec<(String, Vec<u8>)>;

/// Shared header entries: a request's `headers()` handle and the request see one list.
#[derive(Clone, Default)]
pub struct Fields {
    entries: Arc<Mutex<Entries>>,
    immutable: bool,
}

impl Fields {
    fn list(&self) -> Entries {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn change(
        &self,
        f: impl FnOnce(&mut Entries) -> std::result::Result<(), HeaderError>,
    ) -> std::result::Result<(), HeaderError> {
        if self.immutable {
            return Err(HeaderError::Immutable);
        }
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut entries)?;
        if entries.len() > MAX_HEADERS {
            entries.truncate(MAX_HEADERS);
            return Err(HeaderError::Forbidden);
        }
        Ok(())
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// A request being built.
pub struct OutgoingRequest {
    method: Method,
    path: Option<String>,
    scheme: Option<Scheme>,
    authority: Option<String>,
    headers: Fields,
    body: MemoryOutputPipe,
    body_taken: bool,
}

/// A request's body: the buffer it writes into.
pub struct OutgoingBody {
    pipe: MemoryOutputPipe,
    stream_taken: bool,
    finished: Arc<AtomicBool>,
}

/// Timeouts a guest may set; the node's gate decides its own.
#[derive(Default)]
pub struct RequestOptions {
    connect: Option<u64>,
    first_byte: Option<u64>,
    between_bytes: Option<u64>,
}

/// A response, whole.
pub struct IncomingResponse {
    status: u16,
    headers: Vec<(String, Vec<u8>)>,
    body: Option<Vec<u8>>,
}

/// A response's body.
pub struct IncomingBody {
    body: Option<Vec<u8>>,
}

/// The answer to `handle`, ready at once; taken by the first `get`.
pub struct FutureIncomingResponse {
    answer: Option<std::result::Result<IncomingResponse, ErrorCode>>,
}

/// Trailers never come: ready at once with none.
pub struct FutureTrailers {
    taken: bool,
}

/// A pollable that is always ready.
struct Ready;

#[wasmtime_wasi::async_trait]
impl Pollable for Ready {
    async fn ready(&mut self) {}
}

fn ready(data: &mut StoreData) -> Result<Resource<DynPollable>> {
    let ready = data.table.push(Ready)?;
    wasmtime_wasi::p2::subscribe(&mut data.table, ready)
}

fn server_side() -> wasmtime::Error {
    denied(HostDenied(
        "the server side of wasi:http is not available".into(),
    ))
}

impl http::Host for StoreData {
    fn http_error_code(
        &mut self,
        _err: Resource<wasmtime_wasi::p2::bindings::sync::io::error::Error>,
    ) -> Result<Option<ErrorCode>> {
        Ok(None)
    }
}

impl http::HostFields for StoreData {
    fn new(&mut self) -> Result<Resource<Fields>> {
        Ok(self.table.push(Fields::default())?)
    }

    fn from_list(
        &mut self,
        entries: Vec<(String, Vec<u8>)>,
    ) -> Result<std::result::Result<Resource<Fields>, HeaderError>> {
        if entries.len() > MAX_HEADERS {
            return Ok(Err(HeaderError::Forbidden));
        }
        if entries.iter().any(|(n, _)| !valid_name(n)) {
            return Ok(Err(HeaderError::InvalidSyntax));
        }
        let fields = Fields {
            entries: Arc::new(Mutex::new(entries)),
            immutable: false,
        };
        Ok(Ok(self.table.push(fields)?))
    }

    fn get(&mut self, this: Resource<Fields>, name: String) -> Result<Vec<Vec<u8>>> {
        let f = self.table.get(&this)?;
        Ok(f.list()
            .into_iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(&name))
            .map(|(_, v)| v)
            .collect())
    }

    fn has(&mut self, this: Resource<Fields>, name: String) -> Result<bool> {
        let f = self.table.get(&this)?;
        Ok(f.list().iter().any(|(n, _)| n.eq_ignore_ascii_case(&name)))
    }

    fn set(
        &mut self,
        this: Resource<Fields>,
        name: String,
        values: Vec<Vec<u8>>,
    ) -> Result<std::result::Result<(), HeaderError>> {
        if !valid_name(&name) {
            return Ok(Err(HeaderError::InvalidSyntax));
        }
        let f = self.table.get(&this)?;
        Ok(f.change(|e| {
            e.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
            e.extend(values.into_iter().map(|v| (name.clone(), v)));
            Ok(())
        }))
    }

    fn delete(
        &mut self,
        this: Resource<Fields>,
        name: String,
    ) -> Result<std::result::Result<(), HeaderError>> {
        let f = self.table.get(&this)?;
        Ok(f.change(|e| {
            e.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
            Ok(())
        }))
    }

    fn append(
        &mut self,
        this: Resource<Fields>,
        name: String,
        value: Vec<u8>,
    ) -> Result<std::result::Result<(), HeaderError>> {
        if !valid_name(&name) {
            return Ok(Err(HeaderError::InvalidSyntax));
        }
        let f = self.table.get(&this)?;
        Ok(f.change(|e| {
            e.push((name, value));
            Ok(())
        }))
    }

    fn entries(&mut self, this: Resource<Fields>) -> Result<Vec<(String, Vec<u8>)>> {
        Ok(self.table.get(&this)?.list())
    }

    fn clone(&mut self, this: Resource<Fields>) -> Result<Resource<Fields>> {
        let copy = Fields {
            entries: Arc::new(Mutex::new(self.table.get(&this)?.list())),
            immutable: false,
        };
        Ok(self.table.push(copy)?)
    }

    fn drop(&mut self, rep: Resource<Fields>) -> Result<()> {
        let _gone = self.table.delete(rep)?;
        Ok(())
    }
}

impl http::HostOutgoingRequest for StoreData {
    fn new(&mut self, headers: Resource<Fields>) -> Result<Resource<OutgoingRequest>> {
        let headers = self.table.delete(headers)?;
        let request = OutgoingRequest {
            method: Method::Get,
            path: None,
            scheme: None,
            authority: None,
            headers,
            body: MemoryOutputPipe::new(MAX_HTTP_BODY_BYTES),
            body_taken: false,
        };
        Ok(self.table.push(request)?)
    }

    fn body(
        &mut self,
        this: Resource<OutgoingRequest>,
    ) -> Result<std::result::Result<Resource<OutgoingBody>, ()>> {
        let request = self.table.get_mut(&this)?;
        if request.body_taken {
            return Ok(Err(()));
        }
        request.body_taken = true;
        let body = OutgoingBody {
            pipe: request.body.clone(),
            stream_taken: false,
            finished: Arc::new(AtomicBool::new(false)),
        };
        Ok(Ok(self.table.push(body)?))
    }

    fn method(&mut self, this: Resource<OutgoingRequest>) -> Result<Method> {
        Ok(self.table.get(&this)?.method.clone())
    }

    fn set_method(
        &mut self,
        this: Resource<OutgoingRequest>,
        method: Method,
    ) -> Result<std::result::Result<(), ()>> {
        self.table.get_mut(&this)?.method = method;
        Ok(Ok(()))
    }

    fn path_with_query(&mut self, this: Resource<OutgoingRequest>) -> Result<Option<String>> {
        Ok(self.table.get(&this)?.path.clone())
    }

    fn set_path_with_query(
        &mut self,
        this: Resource<OutgoingRequest>,
        path: Option<String>,
    ) -> Result<std::result::Result<(), ()>> {
        self.table.get_mut(&this)?.path = path;
        Ok(Ok(()))
    }

    fn scheme(&mut self, this: Resource<OutgoingRequest>) -> Result<Option<Scheme>> {
        Ok(self.table.get(&this)?.scheme.clone())
    }

    fn set_scheme(
        &mut self,
        this: Resource<OutgoingRequest>,
        scheme: Option<Scheme>,
    ) -> Result<std::result::Result<(), ()>> {
        self.table.get_mut(&this)?.scheme = scheme;
        Ok(Ok(()))
    }

    fn authority(&mut self, this: Resource<OutgoingRequest>) -> Result<Option<String>> {
        Ok(self.table.get(&this)?.authority.clone())
    }

    fn set_authority(
        &mut self,
        this: Resource<OutgoingRequest>,
        authority: Option<String>,
    ) -> Result<std::result::Result<(), ()>> {
        self.table.get_mut(&this)?.authority = authority;
        Ok(Ok(()))
    }

    fn headers(&mut self, this: Resource<OutgoingRequest>) -> Result<Resource<Fields>> {
        let headers = self.table.get(&this)?.headers.clone();
        Ok(self.table.push(headers)?)
    }

    fn drop(&mut self, rep: Resource<OutgoingRequest>) -> Result<()> {
        let _gone = self.table.delete(rep)?;
        Ok(())
    }
}

impl http::HostOutgoingBody for StoreData {
    fn write(
        &mut self,
        this: Resource<OutgoingBody>,
    ) -> Result<std::result::Result<Resource<DynOutputStream>, ()>> {
        let body = self.table.get_mut(&this)?;
        if body.stream_taken {
            return Ok(Err(()));
        }
        body.stream_taken = true;
        let stream: DynOutputStream = Box::new(body.pipe.clone());
        Ok(Ok(self.table.push(stream)?))
    }

    fn finish(
        &mut self,
        this: Resource<OutgoingBody>,
        _trailers: Option<Resource<Fields>>,
    ) -> Result<std::result::Result<(), ErrorCode>> {
        let body = self.table.delete(this)?;
        body.finished.store(true, Ordering::Relaxed);
        Ok(Ok(()))
    }

    fn drop(&mut self, rep: Resource<OutgoingBody>) -> Result<()> {
        let _gone = self.table.delete(rep)?;
        Ok(())
    }
}

impl http::HostRequestOptions for StoreData {
    fn new(&mut self) -> Result<Resource<RequestOptions>> {
        Ok(self.table.push(RequestOptions::default())?)
    }

    fn connect_timeout(&mut self, this: Resource<RequestOptions>) -> Result<Option<u64>> {
        Ok(self.table.get(&this)?.connect)
    }

    fn set_connect_timeout(
        &mut self,
        this: Resource<RequestOptions>,
        duration: Option<u64>,
    ) -> Result<std::result::Result<(), ()>> {
        self.table.get_mut(&this)?.connect = duration;
        Ok(Ok(()))
    }

    fn first_byte_timeout(&mut self, this: Resource<RequestOptions>) -> Result<Option<u64>> {
        Ok(self.table.get(&this)?.first_byte)
    }

    fn set_first_byte_timeout(
        &mut self,
        this: Resource<RequestOptions>,
        duration: Option<u64>,
    ) -> Result<std::result::Result<(), ()>> {
        self.table.get_mut(&this)?.first_byte = duration;
        Ok(Ok(()))
    }

    fn between_bytes_timeout(&mut self, this: Resource<RequestOptions>) -> Result<Option<u64>> {
        Ok(self.table.get(&this)?.between_bytes)
    }

    fn set_between_bytes_timeout(
        &mut self,
        this: Resource<RequestOptions>,
        duration: Option<u64>,
    ) -> Result<std::result::Result<(), ()>> {
        self.table.get_mut(&this)?.between_bytes = duration;
        Ok(Ok(()))
    }

    fn drop(&mut self, rep: Resource<RequestOptions>) -> Result<()> {
        let _gone = self.table.delete(rep)?;
        Ok(())
    }
}

impl http::HostFutureIncomingResponse for StoreData {
    fn subscribe(
        &mut self,
        _this: Resource<FutureIncomingResponse>,
    ) -> Result<Resource<DynPollable>> {
        ready(self)
    }

    #[allow(clippy::type_complexity, reason = "the WIT signature")]
    fn get(
        &mut self,
        this: Resource<FutureIncomingResponse>,
    ) -> Result<
        Option<std::result::Result<std::result::Result<Resource<IncomingResponse>, ErrorCode>, ()>>,
    > {
        let Some(answer) = self.table.get_mut(&this)?.answer.take() else {
            // Taken already: the WIT's `err` of the outer result.
            return Ok(Some(Err(())));
        };
        Ok(Some(Ok(match answer {
            Ok(response) => Ok(self.table.push(response)?),
            Err(code) => Err(code),
        })))
    }

    fn drop(&mut self, rep: Resource<FutureIncomingResponse>) -> Result<()> {
        let _gone = self.table.delete(rep)?;
        Ok(())
    }
}

impl http::HostIncomingResponse for StoreData {
    fn status(&mut self, this: Resource<IncomingResponse>) -> Result<u16> {
        Ok(self.table.get(&this)?.status)
    }

    fn headers(&mut self, this: Resource<IncomingResponse>) -> Result<Resource<Fields>> {
        let fields = Fields {
            entries: Arc::new(Mutex::new(self.table.get(&this)?.headers.clone())),
            immutable: true,
        };
        Ok(self.table.push(fields)?)
    }

    fn consume(
        &mut self,
        this: Resource<IncomingResponse>,
    ) -> Result<std::result::Result<Resource<IncomingBody>, ()>> {
        let Some(body) = self.table.get_mut(&this)?.body.take() else {
            return Ok(Err(()));
        };
        Ok(Ok(self.table.push(IncomingBody { body: Some(body) })?))
    }

    fn drop(&mut self, rep: Resource<IncomingResponse>) -> Result<()> {
        let _gone = self.table.delete(rep)?;
        Ok(())
    }
}

impl http::HostIncomingBody for StoreData {
    fn stream(
        &mut self,
        this: Resource<IncomingBody>,
    ) -> Result<std::result::Result<Resource<DynInputStream>, ()>> {
        let Some(body) = self.table.get_mut(&this)?.body.take() else {
            return Ok(Err(()));
        };
        let stream: DynInputStream = Box::new(MemoryInputPipe::new(body));
        Ok(Ok(self.table.push(stream)?))
    }

    fn finish(&mut self, this: Resource<IncomingBody>) -> Result<Resource<FutureTrailers>> {
        let _body = self.table.delete(this)?;
        Ok(self.table.push(FutureTrailers { taken: false })?)
    }

    fn drop(&mut self, rep: Resource<IncomingBody>) -> Result<()> {
        let _gone = self.table.delete(rep)?;
        Ok(())
    }
}

impl http::HostFutureTrailers for StoreData {
    fn subscribe(&mut self, _this: Resource<FutureTrailers>) -> Result<Resource<DynPollable>> {
        ready(self)
    }

    #[allow(clippy::type_complexity, reason = "the WIT signature")]
    fn get(
        &mut self,
        this: Resource<FutureTrailers>,
    ) -> Result<
        Option<std::result::Result<std::result::Result<Option<Resource<Fields>>, ErrorCode>, ()>>,
    > {
        let t = self.table.get_mut(&this)?;
        if t.taken {
            return Ok(Some(Err(())));
        }
        t.taken = true;
        Ok(Some(Ok(Ok(None))))
    }

    fn drop(&mut self, rep: Resource<FutureTrailers>) -> Result<()> {
        let _gone = self.table.delete(rep)?;
        Ok(())
    }
}

impl http::HostIncomingRequest for StoreData {
    fn method(&mut self, _this: Resource<http::IncomingRequest>) -> Result<Method> {
        Err(server_side())
    }
    fn path_with_query(
        &mut self,
        _this: Resource<http::IncomingRequest>,
    ) -> Result<Option<String>> {
        Err(server_side())
    }
    fn scheme(&mut self, _this: Resource<http::IncomingRequest>) -> Result<Option<Scheme>> {
        Err(server_side())
    }
    fn authority(&mut self, _this: Resource<http::IncomingRequest>) -> Result<Option<String>> {
        Err(server_side())
    }
    fn headers(&mut self, _this: Resource<http::IncomingRequest>) -> Result<Resource<Fields>> {
        Err(server_side())
    }
    fn consume(
        &mut self,
        _this: Resource<http::IncomingRequest>,
    ) -> Result<std::result::Result<Resource<IncomingBody>, ()>> {
        Err(server_side())
    }
    fn drop(&mut self, _rep: Resource<http::IncomingRequest>) -> Result<()> {
        Ok(())
    }
}

impl http::HostOutgoingResponse for StoreData {
    fn new(&mut self, _headers: Resource<Fields>) -> Result<Resource<http::OutgoingResponse>> {
        Err(server_side())
    }
    fn status_code(&mut self, _this: Resource<http::OutgoingResponse>) -> Result<u16> {
        Err(server_side())
    }
    fn set_status_code(
        &mut self,
        _this: Resource<http::OutgoingResponse>,
        _code: u16,
    ) -> Result<std::result::Result<(), ()>> {
        Err(server_side())
    }
    fn headers(&mut self, _this: Resource<http::OutgoingResponse>) -> Result<Resource<Fields>> {
        Err(server_side())
    }
    fn body(
        &mut self,
        _this: Resource<http::OutgoingResponse>,
    ) -> Result<std::result::Result<Resource<OutgoingBody>, ()>> {
        Err(server_side())
    }
    fn drop(&mut self, _rep: Resource<http::OutgoingResponse>) -> Result<()> {
        Ok(())
    }
}

impl http::HostResponseOutparam for StoreData {
    fn send_informational(
        &mut self,
        _this: Resource<http::ResponseOutparam>,
        _status: u16,
        _headers: Resource<Fields>,
    ) -> Result<std::result::Result<(), ErrorCode>> {
        Err(server_side())
    }
    fn set(
        &mut self,
        _param: Resource<http::ResponseOutparam>,
        _response: std::result::Result<Resource<http::OutgoingResponse>, ErrorCode>,
    ) -> Result<()> {
        Err(server_side())
    }
    fn drop(&mut self, _rep: Resource<http::ResponseOutparam>) -> Result<()> {
        Ok(())
    }
}

fn method_name(method: &Method) -> String {
    match method {
        Method::Get => "GET".into(),
        Method::Head => "HEAD".into(),
        Method::Post => "POST".into(),
        Method::Put => "PUT".into(),
        Method::Delete => "DELETE".into(),
        Method::Connect => "CONNECT".into(),
        Method::Options => "OPTIONS".into(),
        Method::Trace => "TRACE".into(),
        Method::Patch => "PATCH".into(),
        Method::Other(other) => other.clone(),
    }
}

impl outgoing_handler::Host for StoreData {
    fn handle(
        &mut self,
        request: Resource<OutgoingRequest>,
        _options: Option<Resource<RequestOptions>>,
    ) -> Result<std::result::Result<Resource<FutureIncomingResponse>, ErrorCode>> {
        let request = self.table.delete(request)?;
        let scheme = match &request.scheme {
            None | Some(Scheme::Https) => "https".to_owned(),
            Some(Scheme::Http) => "http".to_owned(),
            Some(Scheme::Other(s)) => s.clone(),
        };
        let Some(authority) = request.authority.clone() else {
            return Ok(Err(ErrorCode::HttpRequestUriInvalid));
        };
        let url = format!(
            "{scheme}://{authority}{}",
            request.path.as_deref().unwrap_or("/")
        );
        let headers = request
            .headers
            .list()
            .into_iter()
            .map(|(n, v)| (n, String::from_utf8_lossy(&v).into_owned()))
            .collect();
        let outgoing = HttpRequest {
            method: method_name(&request.method),
            url,
            headers,
            body: request.body.contents().to_vec(),
        };
        // Anything but an action traps here (S9).
        let answer = match self.host.http_send(&outgoing).map_err(denied)? {
            Ok(response) => {
                if response.body.len() > MAX_HTTP_BODY_BYTES {
                    Err(ErrorCode::HttpResponseBodySize(Some(
                        u64::try_from(MAX_HTTP_BODY_BYTES).unwrap_or(u64::MAX),
                    )))
                } else {
                    Ok(IncomingResponse {
                        status: response.status,
                        headers: response
                            .headers
                            .into_iter()
                            .map(|(n, v)| (n, v.into_bytes()))
                            .collect(),
                        body: Some(response.body),
                    })
                }
            }
            Err(why) => Err(ErrorCode::InternalError(Some(why))),
        };
        let future = FutureIncomingResponse {
            answer: Some(answer),
        };
        Ok(Ok(self.table.push(future)?))
    }
}
