//! The WASI side of the world (D19, D43, D44): which interfaces come from wasmtime-wasi
//! and which the engine serves itself over `HostCore`.
//!
//! - wasmtime-wasi: `io/{error, poll, streams}`, `cli/*` (empty environment, empty stdin,
//!   no terminals, stdout and stderr captured into the app log, `exit` traps) and
//!   `filesystem/{types, preopens}` with no preopened directory, so no descriptor ever
//!   exists and nothing reaches the node's file system.
//! - this module: `clocks/wall-clock` and `clocks/monotonic-clock` (fixed per invocation;
//!   a sleep is ready at once and never blocks a node thread), `random/random` (the host
//!   core's RNG).
//! - `http/types` and `http/outgoing-handler`: every function a component imports from
//!   them is linked per component as a stub that traps `HostDenied`, and every resource
//!   as an opaque host type nobody can construct (Ruling 20). Actions are off in this
//!   milestone, so no store gets real HTTP; wasmtime-wasi-http's implementation is not
//!   linked at all, because its body streams can block a node thread past the epoch
//!   deadline. Real `http/types` will be linked only into action stores.
//!
//! `wasi:sockets` and `wasi:random/insecure*` are never linked, and `prepare` refuses any
//! component importing them.

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::task::Context;
use std::task::Poll;

use bytes::Bytes;
use calimero_app_host::{HostDenied, InvocationKind, MAX_LOG_BYTES};
use tokio::io::AsyncWrite;
use wasmtime::component::types::ComponentItem;
use wasmtime::component::Component;
use wasmtime::component::HasData;
use wasmtime::component::HasSelf;
use wasmtime::component::Linker;
use wasmtime::component::Resource;
use wasmtime::component::ResourceTable;
use wasmtime::component::ResourceType;
use wasmtime::Engine;
use wasmtime::Result;
use wasmtime_wasi::cli::IsTerminal;
use wasmtime_wasi::cli::StdoutStream;
use wasmtime_wasi::cli::WasiCli;
use wasmtime_wasi::cli::WasiCliView;
use wasmtime_wasi::filesystem::WasiFilesystem;
use wasmtime_wasi::filesystem::WasiFilesystemView;
use wasmtime_wasi::p2::bindings::clocks::monotonic_clock;
use wasmtime_wasi::p2::bindings::clocks::wall_clock;
use wasmtime_wasi::p2::bindings::random::random;
use wasmtime_wasi::p2::bindings::sync as wasi_sync;
use wasmtime_wasi::p2::bindings::{cli, filesystem};
use wasmtime_wasi::p2::DynPollable;
use wasmtime_wasi::p2::OutputStream;
use wasmtime_wasi::p2::Pollable;
use wasmtime_wasi::p2::StreamResult;

use crate::errors::denied;
use crate::StoreData;

const NANOS_PER_MILLI: u64 = 1_000_000;
const MILLIS_PER_SECOND: u64 = 1_000;
/// The clocks report this resolution; they never tick inside an invocation.
const CLOCK_RESOLUTION_NS: u64 = 1_000_000;
/// The most bytes one stdout or stderr write may hand over; later bytes are dropped once
/// the run's log is full, never refused (a full log must not fail the app).
const LOG_WRITE_PERMIT: usize = 64 * 1024;

/// Imports under this prefix are stubbed per component (Ruling 20).
const HTTP_PREFIX: &str = "wasi:http/";
const HTTP_OUTSIDE_ACTION: &str = "outgoing http is only allowed in actions";
const HTTP_DISABLED: &str = "outgoing http is not enabled";

/// Links every WASI interface the world imports, plus the D44 stubs.
pub(crate) fn add_to_linker(linker: &mut Linker<StoreData>) -> Result<()> {
    let l = linker;
    wasi_sync::io::error::add_to_linker::<StoreData, HasTable>(l, |d| &mut d.table)?;
    wasi_sync::io::poll::add_to_linker::<StoreData, HasTable>(l, |d| &mut d.table)?;
    wasi_sync::io::streams::add_to_linker::<StoreData, HasTable>(l, |d| &mut d.table)?;

    wall_clock::add_to_linker::<StoreData, HasSelf<StoreData>>(l, |d| d)?;
    monotonic_clock::add_to_linker::<StoreData, HasSelf<StoreData>>(l, |d| d)?;
    random::add_to_linker::<StoreData, HasSelf<StoreData>>(l, |d| d)?;

    cli::environment::add_to_linker::<StoreData, WasiCli>(l, StoreData::cli)?;
    cli::stdout::add_to_linker::<StoreData, WasiCli>(l, StoreData::cli)?;
    cli::stderr::add_to_linker::<StoreData, WasiCli>(l, StoreData::cli)?;
    cli::stdin::add_to_linker::<StoreData, WasiCli>(l, StoreData::cli)?;
    cli::exit::add_to_linker::<StoreData, WasiCli>(l, StoreData::cli)?;
    cli::terminal_input::add_to_linker::<StoreData, WasiCli>(l, StoreData::cli)?;
    cli::terminal_output::add_to_linker::<StoreData, WasiCli>(l, StoreData::cli)?;
    cli::terminal_stdin::add_to_linker::<StoreData, WasiCli>(l, StoreData::cli)?;
    cli::terminal_stdout::add_to_linker::<StoreData, WasiCli>(l, StoreData::cli)?;
    cli::terminal_stderr::add_to_linker::<StoreData, WasiCli>(l, StoreData::cli)?;
    wasi_sync::filesystem::types::add_to_linker::<StoreData, WasiFilesystem>(
        l,
        StoreData::filesystem,
    )?;
    filesystem::preopens::add_to_linker::<StoreData, WasiFilesystem>(l, StoreData::filesystem)?;

    Ok(())
}

struct HasTable;

impl HasData for HasTable {
    type Data<'a> = &'a mut ResourceTable;
}

// --- clocks and randomness (D19) -------------------------------------------------------

impl wall_clock::Host for StoreData {
    fn now(&mut self) -> Result<wall_clock::Datetime> {
        let ms = self.host.wall_ms();
        Ok(wall_clock::Datetime {
            seconds: ms / MILLIS_PER_SECOND,
            // Below 1,000,000,000, so it fits a u32.
            nanoseconds: u32::try_from((ms % MILLIS_PER_SECOND) * NANOS_PER_MILLI)
                .unwrap_or(u32::MAX),
        })
    }

    fn resolution(&mut self) -> Result<wall_clock::Datetime> {
        Ok(wall_clock::Datetime {
            seconds: 0,
            nanoseconds: u32::try_from(CLOCK_RESOLUTION_NS).unwrap_or(u32::MAX),
        })
    }
}

/// A pollable that is ready at once: the monotonic clock never moves inside an
/// invocation, so a guest sleep ends immediately instead of holding a node thread.
struct Ready;

#[async_trait::async_trait]
impl Pollable for Ready {
    async fn ready(&mut self) {}
}

impl StoreData {
    fn ready_pollable(&mut self) -> Result<Resource<DynPollable>> {
        let ready = self.table.push(Ready)?;
        wasmtime_wasi::p2::subscribe(&mut self.table, ready)
    }
}

impl monotonic_clock::Host for StoreData {
    fn now(&mut self) -> Result<u64> {
        Ok(self.host.monotonic_ns())
    }

    fn resolution(&mut self) -> Result<u64> {
        Ok(CLOCK_RESOLUTION_NS)
    }

    fn subscribe_instant(&mut self, _when: u64) -> Result<Resource<DynPollable>> {
        self.ready_pollable()
    }

    fn subscribe_duration(&mut self, _duration: u64) -> Result<Resource<DynPollable>> {
        self.ready_pollable()
    }
}

impl random::Host for StoreData {
    fn get_random_bytes(&mut self, len: u64) -> Result<Vec<u8>> {
        // A request larger than the instance's memory could never be returned; refuse it
        // before allocating.
        let len = usize::try_from(len)
            .ok()
            .filter(|len| *len <= self.max_random_bytes)
            .ok_or_else(|| denied(HostDenied("random request larger than memory".into())))?;
        let mut buf = vec![0; len];
        self.host.random_fill(&mut buf);
        Ok(buf)
    }

    fn get_random_u64(&mut self) -> Result<u64> {
        let mut buf = [0; 8];
        self.host.random_fill(&mut buf);
        Ok(u64::from_le_bytes(buf))
    }
}

// --- outgoing HTTP (D12, S9) -----------------------------------------------------------

/// Only actions may send, and actions are not enabled in this milestone; either way the
/// call traps HostDenied and no request leaves the node.
fn outgoing_refusal(data: &StoreData) -> &'static str {
    if data.host.invocation().kind == InvocationKind::Action {
        HTTP_DISABLED
    } else {
        HTTP_OUTSIDE_ACTION
    }
}

/// The opaque host type behind every stubbed `wasi:http` resource; never constructed.
struct HttpStub;

/// Defines every function and resource `component` imports from `wasi:http/*` on
/// `linker`: functions trap `HostDenied`, resources are `HttpStub`, which no function
/// ever creates. Done per component so any 0.2 patch it imports is covered.
pub(crate) fn stub_http(
    linker: &mut Linker<StoreData>,
    engine: &Engine,
    component: &Component,
) -> Result<()> {
    let component_type = component.component_type();
    let imports: Vec<_> = component_type.imports(engine).collect();
    // Resources `http/types` re-exports from `wasi:io` (streams, pollable) are io's own,
    // already linked; only resources the http interfaces define get the stub type.
    let foreign: Vec<_> = imports
        .iter()
        .filter(|(name, _)| !name.starts_with(HTTP_PREFIX))
        .filter_map(|(_, item)| match &item.ty {
            ComponentItem::ComponentInstance(instance) => Some(instance.exports(engine)),
            _ => None,
        })
        .flatten()
        .filter_map(|(_, item)| match item.ty {
            ComponentItem::Resource(resource) => Some(resource),
            _ => None,
        })
        .collect();
    for (name, item) in &imports {
        if !name.starts_with(HTTP_PREFIX) {
            continue;
        }
        let ComponentItem::ComponentInstance(instance) = &item.ty else {
            continue;
        };
        let mut defined = linker.instance(name)?;
        for (export, item) in instance.exports(engine) {
            match item.ty {
                ComponentItem::ComponentFunc(_) => {
                    defined.func_new(export, |store, _ty, _params, _results| {
                        Err(denied(HostDenied(outgoing_refusal(store.data()).into())))
                    })?;
                }
                ComponentItem::Resource(resource) if !foreign.contains(&resource) => {
                    defined.resource(export, ResourceType::host::<HttpStub>(), |_, _| Ok(()))?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

// --- stdout and stderr into the owner-only app log -------------------------------------

/// Bytes written to stdout and stderr, split into lines. Bounded: once the run has
/// written more than `MAX_LOG_BYTES`, later bytes are dropped (the host core then caps
/// the log and adds its marker).
#[derive(Default)]
pub(crate) struct LogBuffer {
    lines: Vec<String>,
    partial: [Vec<u8>; 2],
    taken: usize,
}

impl LogBuffer {
    fn write(&mut self, stream: usize, bytes: &[u8]) {
        let room = (MAX_LOG_BYTES + 1).saturating_sub(self.taken);
        let bytes = &bytes[..bytes.len().min(room)];
        self.taken += bytes.len();
        let mut rest = bytes;
        while let Some(end) = rest.iter().position(|b| *b == b'\n') {
            let partial = &mut self.partial[stream];
            partial.extend_from_slice(&rest[..end]);
            self.lines
                .push(String::from_utf8_lossy(&std::mem::take(partial)).into_owned());
            rest = &rest[end + 1..];
        }
        self.partial[stream].extend_from_slice(rest);
    }

    /// Every line written, unterminated ones last.
    pub(crate) fn drain(&mut self) -> Vec<String> {
        let mut lines = std::mem::take(&mut self.lines);
        for partial in &mut self.partial {
            if !partial.is_empty() {
                lines.push(String::from_utf8_lossy(&std::mem::take(partial)).into_owned());
            }
        }
        lines
    }
}

pub(crate) type SharedLog = Arc<Mutex<LogBuffer>>;

/// stdout (`stream` 0) or stderr (1) of one invocation.
#[derive(Clone)]
pub(crate) struct LogPipe {
    log: SharedLog,
    stream: usize,
}

impl LogPipe {
    pub(crate) fn stdout(log: &SharedLog) -> Self {
        Self {
            log: Arc::clone(log),
            stream: 0,
        }
    }

    pub(crate) fn stderr(log: &SharedLog) -> Self {
        Self {
            log: Arc::clone(log),
            stream: 1,
        }
    }

    fn append(&self, bytes: &[u8]) {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .write(self.stream, bytes);
    }
}

impl IsTerminal for LogPipe {
    fn is_terminal(&self) -> bool {
        false
    }
}

impl StdoutStream for LogPipe {
    fn p2_stream(&self) -> Box<dyn OutputStream> {
        Box::new(self.clone())
    }

    fn async_stream(&self) -> Box<dyn AsyncWrite + Send + Sync> {
        Box::new(self.clone())
    }
}

#[async_trait::async_trait]
impl OutputStream for LogPipe {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        self.append(&bytes);
        Ok(())
    }

    fn flush(&mut self) -> StreamResult<()> {
        Ok(())
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        Ok(LOG_WRITE_PERMIT)
    }
}

#[async_trait::async_trait]
impl Pollable for LogPipe {
    async fn ready(&mut self) {}
}

impl AsyncWrite for LogPipe {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.append(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
