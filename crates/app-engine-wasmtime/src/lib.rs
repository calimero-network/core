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
    Abi, Budget, Engine, HostCore, Outcome, PrepareError, Prepared, Status, TimerCtx, TrapKind,
    Usage, DEFAULT_MEMORY_PAGES,
};
use calimero_app_wit::{allowed_imports, stubbed_imports};
use eyre::Result as EyreResult;
use wasmparser::Parser;
use wasmparser::Payload;
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
use wasmtime::WasmFeatures;
use wasmtime_wasi::WasiCtx;
use wasmtime_wasi::WasiCtxBuilder;
use wasmtime_wasi::WasiCtxView;
use wasmtime_wasi::WasiView;

mod bindings;
mod cache;
mod errors;
mod wasi;

use bindings::AppPre;
use cache::Cache;
use errors::{classify, GrowRefused, HostPanic, Phase};
use wasi::{LogPipe, SharedLog};

/// Wasm stack per call, as in the v1 runtime (spec section 5).
pub const MAX_WASM_STACK: usize = 200 * 1024;
/// Table elements per store.
pub const MAX_TABLE_ELEMENTS: usize = 100_000;
/// Core instances per store: a `wasm32-wasip2` component instantiates several core
/// modules (Ruling 18); memory stays bounded by the page limit.
pub const MAX_INSTANCES: usize = 20;
/// Linear memories per store, measured (Ruling 21): a Rust `wasm32-wasip2` guest and a
/// componentize-js 0.23 guest each define exactly one. Checked statically at `prepare`
/// and again by the store's limiter.
pub const MAX_MEMORIES: usize = 1;
/// Tables per store, measured the same way: both toolchains define two (the module's
/// indirect-call table and the component shim's).
pub const MAX_TABLES: usize = 2;
/// Host resources (streams, pollables, cursors' backing entries) one store may hold.
pub const MAX_RESOURCES: usize = 4_096;
/// How often the ticker thread advances the engine's epoch.
pub const EPOCH_TICK: Duration = Duration::from_millis(10);

const WASM_PAGE_BYTES: usize = 64 * 1024;
/// Largest memory a pool slot holds: 1,024 pages (64 MiB).
const MAX_MEMORY_BYTES: usize = DEFAULT_MEMORY_PAGES as usize * WASM_PAGE_BYTES;
/// Component instances alive at once across the node, and the core instances, memories
/// and tables the pool reserves for them.
const POOL_COMPONENT_INSTANCES: u32 = 256;
const POOL_CORE_INSTANCES_PER_INSTANCE: u32 = MAX_INSTANCES as u32;
const POOL_MEMORIES_PER_INSTANCE: u32 = MAX_MEMORIES as u32;
const POOL_TABLES_PER_INSTANCE: u32 = MAX_TABLES as u32;

/// Proposals refused at compile time (Ruling 21). Several are compiled out with the crate
/// features anyway; naming them keeps the set explicit if a feature is turned back on.
const DISABLED_FEATURES: WasmFeatures = WasmFeatures::THREADS
    .union(WasmFeatures::SHARED_EVERYTHING_THREADS)
    .union(WasmFeatures::MULTI_MEMORY)
    .union(WasmFeatures::MEMORY64)
    .union(WasmFeatures::GC)
    .union(WasmFeatures::FUNCTION_REFERENCES)
    .union(WasmFeatures::EXCEPTIONS)
    .union(WasmFeatures::LEGACY_EXCEPTIONS)
    .union(WasmFeatures::STACK_SWITCHING)
    .union(WasmFeatures::TAIL_CALL)
    .union(WasmFeatures::WIDE_ARITHMETIC)
    .union(WasmFeatures::CUSTOM_PAGE_SIZES)
    .union(WasmFeatures::RELAXED_SIMD)
    .union(WasmFeatures::CM_ASYNC)
    .union(WasmFeatures::CM_MORE_ASYNC_BUILTINS)
    .union(WasmFeatures::CM_ASYNC_STACKFUL)
    .union(WasmFeatures::CM_THREADING)
    .union(WasmFeatures::CM_ERROR_CONTEXT)
    .union(WasmFeatures::CM_GC);

/// Interfaces are pinned to WASI 0.2.12 (D43) and `calimero:app` 0.5.1; an import of an older
/// patch of the same minor links through semver matching (Ruling 15), so an app built before
/// a patch keeps running.
const PATCHED: [(&str, &str); 2] = [("wasi:", "0.2."), ("calimero:app/", "0.5.")];

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
    /// `cache_key` authenticates the cached artifacts (S8): derive it from the node's
    /// storage key, so only this node's own compilations are ever loaded.
    pub fn new(cache_dir: PathBuf, cache_key: &[u8; 32]) -> EyreResult<Self> {
        Self::build(cache_dir, cache_key, POOL_COMPONENT_INSTANCES)
    }

    /// An engine whose pool holds only `instances` component instances, so tests can
    /// exhaust it.
    #[cfg(feature = "testing")]
    pub fn with_pool_size(
        cache_dir: PathBuf,
        cache_key: &[u8; 32],
        instances: u32,
    ) -> EyreResult<Self> {
        Self::build(cache_dir, cache_key, instances)
    }

    fn build(cache_dir: PathBuf, cache_key: &[u8; 32], instances: u32) -> EyreResult<Self> {
        let mut pool = PoolingAllocationConfig::default();
        let _ = pool
            .total_component_instances(instances)
            .total_core_instances(instances.saturating_mul(POOL_CORE_INSTANCES_PER_INSTANCE))
            .total_memories(instances.saturating_mul(POOL_MEMORIES_PER_INSTANCE))
            .total_tables(instances.saturating_mul(POOL_TABLES_PER_INSTANCE))
            .max_memory_size(MAX_MEMORY_BYTES)
            .table_elements(MAX_TABLE_ELEMENTS);
        let mut config = Config::new();
        let _ = config
            .consume_fuel(true)
            .epoch_interruption(true)
            .max_wasm_stack(MAX_WASM_STACK)
            // The feature set is pinned (Ruling 21): what Rust wasip2 and componentize-js
            // output needs (MVP, bulk memory, reference types, multi-value, SIMD, sign
            // extension, saturating conversions, the component model), nothing else.
            .wasm_features(DISABLED_FEATURES, false)
            .wasm_component_model(true)
            .memory_reservation(MAX_MEMORY_BYTES as u64)
            .allocation_strategy(InstanceAllocationStrategy::Pooling(pool));
        let engine = wasmtime::Engine::new(&config).map_err(report)?;

        let mut linker = Linker::new(&engine);
        bindings::add_to_linker(&mut linker).map_err(report)?;
        wasi::add_to_linker(&mut linker).map_err(report)?;

        let cache = Cache::new(cache_dir, &engine, cache_key)?;
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
        let memories = defined_memories(component)?;
        if memories > MAX_MEMORIES {
            return Err(PrepareError::Compile(format!(
                "component defines {memories} memories; at most {MAX_MEMORIES}"
            )));
        }

        let compiled: Component = self.cache.load_or_compile(&self.engine, component)?;
        let mut linker = self.linker.clone();
        let pre = wasi::stub_http(&mut linker, &self.engine, &compiled)
            .and_then(|()| linker.instantiate_pre(&compiled))
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
        let args = inv
            .timer
            .as_ref()
            .map_or_else(|| inv.args.clone(), timer_args);
        let (kind, method) = (inv.kind.into(), inv.method.clone());

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
        let mut phase = Phase::Instantiate;
        let ran = store.set_fuel(budget.fuel).and_then(|()| {
            // A host panic unwinds out of the wasm frames to here (S6). Asserting unwind
            // safety is sound because a panic always ends the run as `Trap(Fault)`, and
            // `HostCore::finish` drops the diff and the events of any status but `Ok`, so
            // state a panic left half-updated never reaches the log; only the app log,
            // which is kept, is read afterwards.
            catch_unwind(AssertUnwindSafe(|| {
                let instance = pre.instantiate(&mut store)?;
                phase = Phase::Call;
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
                let (kind, detail) = classify(&error, phase);
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

/// An import is allowed if the world or the D44 stubs list it, or if it is a WASI or
/// `calimero:app` interface they list at a patch of the same minor no newer than the listed
/// one (D43, Ruling 15).
fn import_allowed(import: &str) -> bool {
    let listed = || allowed_imports().iter().chain(stubbed_imports());
    if listed().any(|l| *l == import) {
        return true;
    }
    let Some((interface, version)) = import.split_once('@') else {
        return false;
    };
    let Some(patch) = patch_of(interface, version) else {
        return false;
    };
    listed().any(|l| {
        l.split_once('@').is_some_and(|(li, lv)| {
            li == interface && patch_of(li, lv).is_some_and(|max| patch <= max)
        })
    })
}

/// Linear memories the component's core modules define, counted before compiling.
fn defined_memories(component: &[u8]) -> Result<usize, PrepareError> {
    let mut count = 0usize;
    for payload in Parser::new(0).parse_all(component) {
        let payload = payload.map_err(|e| PrepareError::Compile(e.to_string()))?;
        if let Payload::MemorySection(section) = payload {
            count = count.saturating_add(section.count() as usize);
        }
    }
    Ok(count)
}

/// The patch number of a `wasi:*@0.2.<patch>` or `calimero:app/*@0.5.<patch>` version.
fn patch_of(interface: &str, version: &str) -> Option<u32> {
    let (_, minor) = PATCHED
        .iter()
        .find(|(prefix, _)| interface.starts_with(prefix))?;
    version.strip_prefix(minor)?.parse().ok()
}

/// A store's data: the host core plus what wasmtime-wasi needs. One per invocation.
pub(crate) struct StoreData {
    pub(crate) host: HostCore,
    table: ResourceTable,
    wasi: WasiCtx,
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
        let mut table = ResourceTable::new();
        table.set_max_capacity(MAX_RESOURCES);
        Self {
            host,
            table,
            wasi,
            limits: Limits(
                StoreLimitsBuilder::new()
                    .memory_size(memory_bytes)
                    .table_elements(MAX_TABLE_ELEMENTS)
                    .instances(MAX_INSTANCES)
                    .memories(MAX_MEMORIES)
                    .tables(MAX_TABLES)
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

/// The `args` of a timer tick, as the guest reads them: the slot and the previous applied
/// slot, in milliseconds, as decimal strings (D20). The host core holds the tick
/// (`Invocation::timer`), so a timer invocation's own `args` are ignored.
fn timer_args(tick: &TimerCtx) -> Vec<u8> {
    let previous = tick
        .previous_ms
        .map_or_else(|| "null".to_owned(), |p| format!("\"{p}\""));
    format!(
        "{{\"scheduled\":\"{}\",\"previous\":{previous}}}",
        tick.scheduled_ms
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::import_allowed;

    #[test]
    fn imports_match_the_lists_with_older_patches_only() {
        assert!(import_allowed("calimero:app/storage@0.5.1"));
        assert!(import_allowed("calimero:app/storage@0.5.0"));
        assert!(!import_allowed("calimero:app/storage@0.5.2"));
        assert!(!import_allowed("calimero:app/storage@0.6.0"));
        assert!(!import_allowed("calimero:app/nothing@0.5.0"));
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
