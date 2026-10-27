//! The wasmtime engine for `calimero:app` components (spec sections 4-5).
//!
//! One `wasmtime::Engine` serves every app. `prepare` checks a component's imports
//! against the world and the D44 stubs, reads its ABI and compiles it through a checked
//! disk cache; `invoke` runs `handle` in a fresh store per call (D8) under fuel, an epoch
//! deadline, memory, table and stack limits, and maps every failure onto one `TrapKind`.

use std::panic::catch_unwind;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::PoisonError;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use calimero_app_host::{
    Abi, Budget, Engine, HostCore, Outcome, PrepareError, Prepared, Status, TrapKind, Usage,
    DEFAULT_MEMORY_PAGES,
};
use calimero_app_wit::{allowed_imports, stubbed_imports};
use eyre::Result as EyreResult;
use wasmtime::component::Component;
use wasmtime::component::Linker;
use wasmtime::component::ResourceTable;
use wasmtime::Config;
use wasmtime::InstanceAllocationStrategy;
use wasmtime::PoolingAllocationConfig;
use wasmtime::ResourceLimiter;
use wasmtime::Store;
use wasmtime::StoreLimits;
use wasmtime::StoreLimitsBuilder;
use wasmtime_wasi::WasiCtx;
use wasmtime_wasi::WasiCtxBuilder;
use wasmtime_wasi::WasiCtxView;
use wasmtime_wasi::WasiView;
use wasmtime_wasi_http::WasiHttpCtx;
use wasmtime_wasi_http::WasiHttpCtxView;
use wasmtime_wasi_http::WasiHttpView;

mod bindings;
mod cache;
mod errors;
mod wasi;

use bindings::AppPre;
use cache::Cache;
use errors::{classify, GrowRefused, HostPanic};
use wasi::{DenyHooks, LogPipe, SharedLog};

/// Wasm stack per call, as in the v1 runtime (spec section 5).
pub const MAX_WASM_STACK: usize = 200 * 1024;
/// Table elements per store.
pub const MAX_TABLE_ELEMENTS: usize = 100_000;
/// Core instances per store: a `wasm32-wasip2` component instantiates several core
/// modules (Ruling 18); memory stays bounded by the page limit.
pub const MAX_INSTANCES: usize = 20;
/// How often the ticker thread advances the engine's epoch.
pub const EPOCH_TICK: Duration = Duration::from_millis(10);

const WASM_PAGE_BYTES: usize = 64 * 1024;
/// Largest memory a pool slot holds: 1,024 pages (64 MiB).
const MAX_MEMORY_BYTES: usize = DEFAULT_MEMORY_PAGES as usize * WASM_PAGE_BYTES;
/// Component instances alive at once across the node, and the core instances, memories
/// and tables the pool reserves for them.
const POOL_COMPONENT_INSTANCES: u32 = 256;
const POOL_CORE_INSTANCES: u32 = POOL_COMPONENT_INSTANCES * MAX_INSTANCES as u32;
const POOL_MEMORIES: u32 = POOL_COMPONENT_INSTANCES * 2;
const POOL_TABLES: u32 = POOL_CORE_INSTANCES;

/// WASI interfaces are pinned to 0.2.12 (D43); an import of an older 0.2 patch links
/// through semver matching (Ruling 15).
const WASI_PREFIX: &str = "wasi:";
const WASI_MINOR: &str = "0.2.";

/// Runs `calimero:app` components on wasmtime.
pub struct WasmtimeEngine {
    engine: wasmtime::Engine,
    linker: Linker<StoreData>,
    cache: Cache,
    ticker_stop: Arc<AtomicBool>,
}

impl WasmtimeEngine {
    /// One engine for every app, caching compiled components under `cache_dir`, with a
    /// ticker thread advancing the epoch every `EPOCH_TICK`.
    pub fn new(cache_dir: PathBuf) -> EyreResult<Self> {
        let mut pool = PoolingAllocationConfig::default();
        let _ = pool
            .total_component_instances(POOL_COMPONENT_INSTANCES)
            .total_core_instances(POOL_CORE_INSTANCES)
            .total_memories(POOL_MEMORIES)
            .total_tables(POOL_TABLES)
            .max_memory_size(MAX_MEMORY_BYTES)
            .table_elements(MAX_TABLE_ELEMENTS);
        let mut config = Config::new();
        let _ = config
            .consume_fuel(true)
            .epoch_interruption(true)
            .max_wasm_stack(MAX_WASM_STACK)
            .wasm_threads(false)
            .wasm_component_model(true)
            .memory_reservation(MAX_MEMORY_BYTES as u64)
            .allocation_strategy(InstanceAllocationStrategy::Pooling(pool));
        let engine = wasmtime::Engine::new(&config).map_err(report)?;

        let mut linker = Linker::new(&engine);
        bindings::add_to_linker(&mut linker).map_err(report)?;
        wasi::add_to_linker(&mut linker).map_err(report)?;

        let cache = Cache::new(cache_dir, &engine)?;
        let ticker_stop = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&ticker_stop);
        let ticking = engine.clone();
        let _ = thread::Builder::new()
            .name("wasm-epoch".to_owned())
            .spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    thread::sleep(EPOCH_TICK);
                    ticking.increment_epoch();
                }
            })?;
        Ok(Self {
            engine,
            linker,
            cache,
            ticker_stop,
        })
    }
}

impl Drop for WasmtimeEngine {
    fn drop(&mut self) {
        self.ticker_stop.store(true, Ordering::Relaxed);
    }
}

impl Engine for WasmtimeEngine {
    fn prepare(&self, bundle_hash: &str, component: &[u8]) -> Result<Prepared, PrepareError> {
        let imports = calimero_app_build::imports_of(component)
            .map_err(|e| PrepareError::Compile(e.to_string()))?;
        if let Some(forbidden) = imports.into_iter().find(|i| !import_allowed(i)) {
            return Err(PrepareError::ForbiddenImport(forbidden));
        }
        let abi = calimero_app_build::abi_of(component)
            .map_err(|e| PrepareError::BadAbi(e.to_string()))?
            .ok_or(PrepareError::NoAbi)?;
        let abi = Abi::from_json(&abi)?;

        let compiled: Component = self.cache.load_or_compile(&self.engine, component)?;
        let pre = self
            .linker
            .instantiate_pre(&compiled)
            .and_then(AppPre::new)
            .map_err(|e| PrepareError::Compile(e.to_string()))?;
        Ok(Prepared {
            hash: bundle_hash.to_owned(),
            abi,
            inner: Arc::new(pre),
        })
    }

    fn invoke(&self, app: &Prepared, host: HostCore, budget: Budget) -> Outcome {
        let Some(pre) = (*app.inner).downcast_ref::<AppPre<StoreData>>() else {
            let status = trap(TrapKind::Fault, "not prepared by this engine");
            return host.finish(status, Vec::new(), Usage::default());
        };
        let inv = host.invocation();
        let (kind, method, args) = (inv.kind.into(), inv.method.clone(), inv.args.clone());

        let log: SharedLog = Arc::default();
        let mut store = Store::new(&self.engine, StoreData::new(host, &budget, &log));
        store.limiter(|d| &mut d.limits);
        let ticks = budget
            .wall_clock
            .as_nanos()
            .div_ceil(EPOCH_TICK.as_nanos())
            .max(1);
        store.set_epoch_deadline(u64::try_from(ticks).unwrap_or(u64::MAX));
        let started = Instant::now();
        let ran = store.set_fuel(budget.fuel).and_then(|()| {
            // A host panic unwinds out of the wasm frames to here (S6).
            catch_unwind(AssertUnwindSafe(|| {
                let instance = pre.instantiate(&mut store)?;
                instance.call_handle(&mut store, kind, &method, &args)
            }))
            .unwrap_or_else(|_| Err(wasmtime::Error::new(HostPanic)))
        });
        let usage = Usage {
            fuel: budget.fuel.saturating_sub(store.get_fuel().unwrap_or(0)),
            cpu: started.elapsed(),
            writes: 0,
        };

        let mut data = store.into_data();
        let lines = log.lock().unwrap_or_else(PoisonError::into_inner).drain();
        for line in lines {
            data.host.log(line);
        }
        let (status, result) = match ran {
            Ok(Ok(result)) => (Status::Ok, result),
            Ok(Err(error)) => (
                Status::AppError {
                    code: error.code,
                    message: error.message,
                },
                Vec::new(),
            ),
            Err(error) => {
                let (kind, detail) = classify(&error);
                (Status::Trap { kind, detail }, Vec::new())
            }
        };
        data.host.finish(status, result, usage)
    }
}

/// `wasmtime::Error` is not a `std::error::Error`, so `?` can't lift it into eyre.
fn report(error: wasmtime::Error) -> eyre::Report {
    eyre::eyre!("{error:#}")
}

fn trap(kind: TrapKind, detail: &str) -> Status {
    Status::Trap {
        kind,
        detail: detail.to_owned(),
    }
}

/// An import is allowed if the world or the D44 stubs list it, or if it is a WASI
/// interface they list at a 0.2 patch no newer than the listed one (D43, Ruling 15).
fn import_allowed(import: &str) -> bool {
    let listed = || allowed_imports().iter().chain(stubbed_imports());
    if listed().any(|l| *l == import) {
        return true;
    }
    let Some((interface, version)) = import.split_once('@') else {
        return false;
    };
    let Some(patch) = wasi_patch(interface, version) else {
        return false;
    };
    listed().any(|l| {
        l.split_once('@').is_some_and(|(li, lv)| {
            li == interface && wasi_patch(li, lv).is_some_and(|max| patch <= max)
        })
    })
}

/// The patch number of a `wasi:*@0.2.<patch>` version.
fn wasi_patch(interface: &str, version: &str) -> Option<u32> {
    if !interface.starts_with(WASI_PREFIX) {
        return None;
    }
    version.strip_prefix(WASI_MINOR)?.parse().ok()
}

/// A store's data: the host core plus what wasmtime-wasi needs. One per invocation.
pub(crate) struct StoreData {
    pub(crate) host: HostCore,
    table: ResourceTable,
    wasi: WasiCtx,
    http: WasiHttpCtx,
    hooks: DenyHooks,
    limits: Limits,
    max_random_bytes: usize,
}

impl StoreData {
    fn new(host: HostCore, budget: &Budget, log: &SharedLog) -> Self {
        let memory_bytes = (budget.memory_pages as usize)
            .saturating_mul(WASM_PAGE_BYTES)
            .min(MAX_MEMORY_BYTES);
        let wasi = WasiCtxBuilder::new()
            .stdout(LogPipe::stdout(log))
            .stderr(LogPipe::stderr(log))
            .build();
        Self {
            host,
            table: ResourceTable::new(),
            wasi,
            http: WasiHttpCtx::new(),
            hooks: DenyHooks,
            limits: Limits(
                StoreLimitsBuilder::new()
                    .memory_size(memory_bytes)
                    .table_elements(MAX_TABLE_ELEMENTS)
                    .instances(MAX_INSTANCES)
                    .build(),
            ),
            max_random_bytes: memory_bytes,
        }
    }
}

impl WasiView for StoreData {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl WasiHttpView for StoreData {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            hooks: &mut self.hooks,
            table: &mut self.table,
            ctx: &mut self.http,
        }
    }
}

/// `StoreLimits`, except that a refused growth traps `GrowRefused` instead of handing the
/// guest a failed `memory.grow`, so the run ends `Trap(OutOfMemory)`.
struct Limits(StoreLimits);

impl ResourceLimiter for Limits {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if self.0.memory_growing(current, desired, maximum)? {
            return Ok(true);
        }
        Err(wasmtime::Error::new(GrowRefused))
    }

    fn memory_grow_failed(&mut self, _error: wasmtime::Error) -> wasmtime::Result<()> {
        Err(wasmtime::Error::new(GrowRefused))
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if self.0.table_growing(current, desired, maximum)? {
            return Ok(true);
        }
        Err(wasmtime::Error::new(GrowRefused))
    }

    fn table_grow_failed(&mut self, _error: wasmtime::Error) -> wasmtime::Result<()> {
        Err(wasmtime::Error::new(GrowRefused))
    }

    fn instances(&self) -> usize {
        self.0.instances()
    }

    fn tables(&self) -> usize {
        self.0.tables()
    }

    fn memories(&self) -> usize {
        self.0.memories()
    }
}

#[cfg(test)]
mod tests {
    use super::import_allowed;

    #[test]
    fn imports_match_the_lists_with_older_wasi_patches_only() {
        assert!(import_allowed("calimero:app/storage@0.5.0"));
        assert!(import_allowed("wasi:cli/stdin@0.2.12"));
        assert!(import_allowed("wasi:cli/stdin@0.2.0"));
        assert!(import_allowed("wasi:http/types@0.2.10"));
        assert!(!import_allowed("wasi:cli/stdin@0.2.13"));
        assert!(!import_allowed("wasi:cli/stdin@0.3.0"));
        assert!(!import_allowed("wasi:sockets/tcp@0.2.12"));
        assert!(!import_allowed("wasi:sockets/tcp@0.2.9"));
        assert!(!import_allowed("wasi:random/insecure@0.2.12"));
        assert!(!import_allowed("calimero:app/storage@0.4.0"));
        assert!(!import_allowed("wasi:cli/stdin"));
    }
}
