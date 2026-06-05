#![expect(clippy::unwrap_in_result, reason = "Repr transmute")]
#![allow(clippy::multiple_inherent_impl, reason = "better readability")]

use calimero_governance_store::{
    MembershipRepository, MetaRepository, NamespaceRepository, SigningKeysRepository,
};
use std::collections::HashSet;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use actix::prelude::{ActorResponse, WrapFuture};
use actix::{Actor, AsyncContext};
use calimero_context_client::client::ContextClient;
use calimero_context_client::local_governance::SignedNamespaceOp;
use calimero_context_client::ContextGuard;
use calimero_context_config::types::ContextGroupId;
use calimero_dag::DagStore;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::{Application, ApplicationId};
use calimero_primitives::context::{Context, ContextId};
use calimero_store::Store;
use either::Either;
use prometheus_client::registry::Registry;
use tokio::sync::{Mutex, RwLock};

use calimero_governance_store::metrics::Metrics;
use calimero_wasm_abi::schema::MethodIntent;

pub mod auto_follow;
mod cache;
pub mod config;
pub mod error;
pub mod governance_dag;
pub mod handlers;
pub mod hlc_fence;
mod lifecycle;
pub mod self_purge;

pub(crate) use cache::{BoundedCache, Evictable};

// Backward-compat re-export shims for the modules moved to
// `calimero-governance-store` in #2307. External callers
// (`crates/server`, `crates/node`, `crates/meroctl`) continue to
// import via `calimero_context::group_store::*` /
// `calimero_context::governance_broadcast::*` without source changes;
// the curated symbol lists below are exactly the surface those callers
// reach for today (audited via `grep -rh 'calimero_context::group_store::'`
// on 2026-05-27). Anything not in this list is `pub(crate)` inside the
// new crate and not re-exported.
pub mod group_store {
    pub use calimero_governance_store::{
        apply_local_signed_group_op,
        apply_signed_namespace_op,
        enumerate_group_contexts,
        get_group_for_context,
        get_local_gov_nonce,
        get_op_head,
        is_currently_authorized_for_context,
        membership_status_at,
        now_millis,
        read_op_log_after,
        read_tee_admission_policy,
        register_context_in_group,
        sign_and_publish_namespace_op,
        sign_apply_and_publish,
        sign_apply_and_publish_namespace_op,
        // Absorb buffer (PR-6b straggler safety).
        AbsorbRecord,
        AbsorbRepository,
        AbsorbedEntity,
        // Typed errors (#2305). Bundled because external callers that
        // downcast on `eyre::Report` need access to the error types; only
        // adding the ones currently imported would surface the same
        // missing-import error every time a new caller wants to match.
        ApplyError,
        CapabilitiesError,
        // Repositories
        CapabilitiesRepository,
        ContextRegistrationError,
        DenyListRepository,
        GroupCreatedRejection,
        GroupDeletedRejection,
        GroupKeyring,
        KeyringError,
        MemberJoinedOpenRejection,
        MembershipError,
        MembershipPath,
        MembershipRepository,
        MembershipStatus,
        MetaError,
        MetaRepository,
        MetadataRepository,
        NamespaceDagService,
        NamespaceError,
        NamespaceGovernance,
        NamespaceRepository,
        SigningKeysError,
        SigningKeysRepository,
        UpgradesRepository,
    };
}

pub mod governance_broadcast {
    pub use calimero_governance_store::governance_broadcast::{
        ns_topic, sign_ack, verify_readiness_beacon, ObserveDelivery,
    };
}

use calimero_context_client::local_governance::AckRouter;

/// Runtime-tunable knobs for `ContextManager` behavior.
///
/// Distinct from the on-disk [`config::ContextConfig`] (which holds the
/// chain/client config). This struct centralises timing constants that
/// were previously hard-coded in handler modules so future operator
/// tooling can override them without source patches, plus behavioral
/// feature flags (e.g. [`Self::migration_v2`]) that gate in-progress
/// framework work. Timing defaults match the values that shipped with
/// #2237 Phase 12.
#[derive(Clone, Copy, Debug)]
pub struct ContextManagerConfig {
    /// How long `join_group` will wait for a `KeyDelivery` op to arrive
    /// via the gossip-fallback path after publishing `MemberJoined` and
    /// before failing the join. Reached only when the direct join
    /// response did not carry a key (served peer didn't hold it, or the
    /// direct stream request timed out). The wait fires after
    /// `MemberJoined` is on the wire, so the bound is "round-trip to any
    /// admin + their `publish_and_await_ack` budget", not the full
    /// gossipsub heartbeat reconciliation window.
    pub key_delivery_fallback_wait: Duration,

    /// Master switch for the hybrid zero-downtime migration framework.
    ///
    /// Defaults to `true`: a namespace-cascade migration no longer freezes
    /// writes group-wide; each context migrates lazily and stragglers are
    /// absorbed rather than dropped. Set to `false` to restore the legacy
    /// group-wide `InProgress` write-freeze (see [`handlers::execute`]'s
    /// `upgrade_blocks_write`).
    pub migration_v2: bool,
}

impl Default for ContextManagerConfig {
    fn default() -> Self {
        Self {
            key_delivery_fallback_wait: Duration::from_secs(5),
            migration_v2: true,
        }
    }
}

/// Upper bound on the number of contexts kept in the in-memory hot cache
/// (`ContextManager::contexts`). A safety valve against unbounded growth on
/// long-running nodes that access many distinct contexts — NOT a hard limit:
/// the cache may briefly exceed it when every entry is live (lock-gated
/// eviction; see [`ContextMeta`]'s [`Evictable`] impl). The datastore remains
/// authoritative, so an evicted context is simply re-fetched on next access.
const MAX_CACHED_CONTEXTS: usize = 1024;

/// Upper bound on cached application metadata (`ContextManager::applications`).
/// Mirrors the compiled-`modules` cap (`MAX_CACHED_MODULES`); application
/// metadata is a pure cache over the datastore, so eviction is harmless.
const MAX_CACHED_APPLICATIONS: usize = 256;

/// Upper bound on cached compiled WASM modules (`ContextManager::modules`),
/// keyed by `(application_id, service_name)`. Compiled native code is 2–10× the
/// WASM source size, so a tighter cap than `applications` is warranted on
/// multi-tenant nodes that rotate through many applications. A pure cache over
/// the datastore, so eviction is harmless (a recompile on next use).
const MAX_CACHED_MODULES: usize = 32;

/// Upper bound on resident per-namespace governance DAGs
/// (`ContextManager::namespace_dags`). A node that admits or observes ops for
/// many namespaces would otherwise retain one in-memory DAG per namespace for
/// the whole process lifetime. Lock-gated like `contexts` (an in-flight op
/// holds the DAG's `Arc<Mutex>`), so the map may sit briefly over-cap when
/// every entry is live. Evicted DAGs are rebuilt from the datastore on next
/// touch.
const MAX_CACHED_NAMESPACE_DAGS: usize = 1024;

/// Per-namespace in-memory governance-DAG history is bounded independently of
/// the [`MAX_CACHED_NAMESPACE_DAGS`] count: a single hot namespace that never
/// gets evicted would otherwise retain *every* applied op in its
/// `DagStore.deltas` for the process lifetime. Once a resident DAG exceeds
/// `NAMESPACE_DAG_PRUNE_THRESHOLD` applied deltas, it is pruned back to the most
/// recent `NAMESPACE_DAG_PRUNE_RETAIN`.
///
/// This is safe — and lossless for peers — because applied governance ops are
/// durably persisted as `NamespaceGovOp` rows and the backfill responder serves
/// peers *from RocksDB*, not from this in-memory DAG; the pruned ids are
/// discarded rather than deleted from disk. The in-memory DAG is already
/// transient (rebuilt from gossip/backfill after a restart), so pruning only
/// accelerates what a restart does anyway.
///
/// The threshold is set high so normal namespaces never prune, and the retain
/// window is many backfill batches deep (`MAX_BACKFILL_OPS` is 500), so a
/// re-delivered recent op still parents onto a retained delta rather than
/// re-pending on a pruned ancestor.
const NAMESPACE_DAG_PRUNE_THRESHOLD: usize = 8192;

/// Recent applied deltas retained when a namespace DAG is pruned (see
/// [`NAMESPACE_DAG_PRUNE_THRESHOLD`]). Heads are always retained on top of this.
const NAMESPACE_DAG_PRUNE_RETAIN: usize = 4096;

/// How often the periodic task logs context-cache effectiveness.
const CACHE_STATS_LOG_INTERVAL: Duration = Duration::from_secs(300);

/// Cumulative hit/miss counters for the in-memory context cache.
///
/// Source of truth for the periodic [`ContextManager::log_cache_stats`] line,
/// which is emitted even when Prometheus is disabled (headless/test nodes have
/// `metrics: None`). When a registry *is* present the same events are also
/// mirrored into the `context.cache.*` Prometheus series; the two are kept in
/// lockstep at the single increment site in `get_or_fetch_context`.
///
/// Counters are cumulative over the process lifetime — the reported hit rate is
/// therefore a lifetime average. Windowed rates are available from Prometheus
/// (`rate(...)`); a per-interval delta in the log line is a possible follow-up.
#[derive(Debug, Default)]
struct ContextCacheStats {
    hits: AtomicU64,
    misses: AtomicU64,
}

impl ContextCacheStats {
    /// Record a single cache access. `hit` is the `was_cached` outcome at the
    /// cache-aside entry point: `true` when the context was already resident,
    /// `false` when it had to be fetched from the datastore.
    fn record(&self, hit: bool) {
        let counter = if hit { &self.hits } else { &self.misses };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// `(hits, misses)` snapshot. Relaxed loads are fine: the counters are only
    /// ever read for human-facing logging, never to gate control flow.
    fn snapshot(&self) -> (u64, u64) {
        (
            self.hits.load(Ordering::Relaxed),
            self.misses.load(Ordering::Relaxed),
        )
    }
}

/// The per-context lock that serializes operations on a single context.
///
/// This wraps the `Arc<RwLock<ContextId>>` so that the entire lifecycle of the
/// lock — how it is acquired, the owned guard it hands out, and the
/// reference-count rule that decides when its owning cache entry may be evicted
/// — lives behind one type.
///
/// Two acquisition modes:
/// - [`lock`](Self::lock) — exclusive write guard; the default for any call
///   whose read/write intent is unknown. Every call serializes on this path.
/// - [`lock_read`](Self::lock_read) — shared read guard; only handed out for
///   methods explicitly declared read-only in the module ABI (`MethodIntent::ReadOnly`).
///   Multiple read-guard holders may run concurrently on the same context.
#[derive(Clone, Debug)]
struct ContextLock {
    lock: Arc<RwLock<ContextId>>,
}

impl ContextLock {
    /// Create a fresh per-context lock keyed by `id`.
    fn new(id: ContextId) -> Self {
        Self {
            lock: Arc::new(RwLock::new(id)),
        }
    }

    /// Acquire the lock in *exclusive* (write) mode.
    ///
    /// Default for any call whose read/write intent is not known to be read-only.
    /// Uses `try_write_owned()` (non-blocking fast path) then `write_owned()`.
    ///
    /// The `Right` branch returns a boxed future so both `lock()` and
    /// `lock_read()` have the same `Either` type and can be mixed in a single
    /// match arm without opaque-type inference conflicts.
    fn lock(
        &self,
    ) -> Either<ContextGuard, std::pin::Pin<Box<dyn Future<Output = ContextGuard> + Send>>> {
        let Ok(guard) = self.lock.clone().try_write_owned() else {
            let lock = self.lock.clone();
            return Either::Right(Box::pin(async move {
                ContextGuard::write(lock.write_owned().await)
            }));
        };

        Either::Left(ContextGuard::write(guard))
    }

    /// Acquire the lock in *shared* (read) mode.
    ///
    /// Only used for methods declared read-only in the module ABI. Multiple
    /// concurrent read-guard holders on the same context are safe as long as
    /// the method cannot write (enforced by the `ReadOnlyContextStorage` wrapper
    /// passed to the runtime in place of the normal mutable storage).
    fn lock_read(
        &self,
    ) -> Either<ContextGuard, std::pin::Pin<Box<dyn Future<Output = ContextGuard> + Send>>> {
        let Ok(guard) = self.lock.clone().try_read_owned() else {
            let lock = self.lock.clone();
            return Either::Right(Box::pin(async move {
                ContextGuard::read(lock.read_owned().await)
            }));
        };

        Either::Left(ContextGuard::read(guard))
    }

    /// Whether the owning cache entry may be evicted: true only while no
    /// operation is in flight against this context.
    ///
    /// The guard handed out by [`lock`](Self::lock) is an *owned* guard held
    /// outside the actor across the entire WASM execution — it can even be
    /// passed back in as `ContextAtomic::Held(..)`. An outstanding guard holds a
    /// clone of the `Arc`, so a context with an in-flight operation always has
    /// `Arc::strong_count >= 2`; an idle, evictable lock is exactly
    /// `strong_count == 1` (only the cache holds it).
    ///
    /// If we evicted a *live* entry, the next `get_or_fetch_context` would mint
    /// a brand-new `Arc<RwLock>` for that context, and two concurrent operations
    /// would then synchronize on *different* locks — breaking the invariant and
    /// corrupting state. Hence this lock-gated check.
    ///
    /// `strong_count` is racy in isolation, but the check is safe here because
    /// the only consumer — `BoundedCache::evict_if_full` — runs it and the
    /// subsequent `remove` synchronously (no `.await` between them) inside a
    /// single `ContextManager` actor turn. New guards are only ever minted by
    /// `lock()` on that same actor, so no acquisition can interleave between
    /// this returning `true` and the eviction: an entry seen idle stays idle
    /// until the eviction completes.
    fn is_idle(&self) -> bool {
        Arc::strong_count(&self.lock) == 1
    }
}

/// A metadata container for a single, in-memory context.
///
/// It holds the context's core properties and a per-context [`ContextLock`].
/// This lock is crucial for serializing operations on this specific context,
/// allowing the `ContextManager` to process requests for different contexts in parallel
/// while ensuring data consistency for any single context.
#[derive(Debug)]
struct ContextMeta {
    meta: Context,
    lock: ContextLock,
}

/// A context is evictable only while no operation is in flight against it; the
/// lock-gated rule lives on [`ContextLock::is_idle`].
impl Evictable for ContextMeta {
    fn is_idle(&self) -> bool {
        self.lock.is_idle()
    }
}

/// Application metadata is a pure clone of datastore state with no live handle,
/// so it is always safe to evict (the default).
impl Evictable for Application {}

/// A compiled module is an `Arc`-backed clone with no exclusive handle held by
/// the cache, so it is always safe to evict (the default).
impl Evictable for calimero_runtime::Module {}

/// A read-only method set is a plain `Arc<HashSet>` with no live handle; always
/// safe to evict (the default). The execute path re-derives it from the
/// embedded ABI manifest on the next compile cycle.
impl Evictable for Arc<HashSet<String>> {}

/// A per-namespace governance DAG is lock-gated exactly like [`ContextMeta`]:
/// an in-flight op holds the `Arc<Mutex<DagStore>>`, so the DAG is evictable
/// only at `strong_count == 1`. Evicting a live DAG would split it across two
/// `Arc<Mutex>` instances and let concurrent ops serialize on different locks.
impl Evictable for Arc<Mutex<DagStore<SignedNamespaceOp>>> {
    fn is_idle(&self) -> bool {
        Arc::strong_count(self) == 1
    }
}

/// The central actor responsible for managing the lifecycle of all contexts.
///
/// As an actor, it maintains its own state and processes incoming messages
/// sequentially from a mailbox.
#[derive(Debug)]
pub struct ContextManager {
    /// Handle to the persistent key-value store. Used for fetching context data on cache misses.
    datastore: Store,

    /// Client for interacting with the underlying Calimero node.
    node_client: NodeClient,
    /// The public-facing client API, also used internally to access convenience methods
    /// for interacting with the datastore.
    context_client: ContextClient,

    /// An in-memory cache of active contexts (`ContextId` -> `ContextMeta`).
    /// This serves as a hot cache to avoid expensive disk I/O for frequently accessed contexts.
    ///
    /// Size-capped to `MAX_CACHED_CONTEXTS` so long-running nodes that touch
    /// many distinct contexts don't grow this map without bound. Eviction is
    /// gated on the per-context lock being idle — see [`ContextMeta`]'s
    /// [`Evictable`] impl for why that gate is load-bearing for correctness,
    /// not just hygiene.
    // todo! potentially make this a dashmap::DashMap
    contexts: BoundedCache<ContextId, ContextMeta>,
    /// An in-memory cache of application metadata (`ApplicationId` -> `Application`).
    /// Caching this prevents repeated fetching and parsing of application details.
    ///
    /// Size-capped to `MAX_CACHED_APPLICATIONS` via [`BoundedCache`] (a plain
    /// by-key-order eviction, since application metadata is a pure cache over
    /// the datastore).
    ///
    /// # Note
    /// Even when 2 applications point to the same bytecode,
    /// the application's metadata may include information
    /// that might be relevant in the compilation process,
    /// so we cannot blindly reuse compiled blobs across apps.
    applications: BoundedCache<ApplicationId, Application>,

    /// In-memory cache of compiled WASM modules, keyed by
    /// `(application_id, service_name)`. Populated on the first
    /// `get_module` call for a given key; reused on every subsequent
    /// execute. Cheap to clone (Arc-backed inside wasmer).
    ///
    /// Invalidated alongside `applications` on application updates or
    /// migrations, and replaced when `get_module` has to recompile.
    /// Without this, every execute request paid ~5% CPU to re-run
    /// `Engine::from_precompiled` (observed in #2238 follow-up
    /// profiling).
    ///
    /// Size-capped to `MAX_CACHED_MODULES` via [`BoundedCache`]. Compiled
    /// modules are 2–10× larger than the source WASM, so we cap to prevent
    /// unbounded growth on multi-tenant nodes that rotate through many
    /// applications. Eviction is by-key-order rather than true LRU — good
    /// enough as a safety valve; upgrade tracked alongside the `contexts` LRU
    /// TODO above.
    modules: BoundedCache<(ApplicationId, Option<String>), calimero_runtime::Module>,

    /// Per-application set of method names declared read-only via `#[app::view]`
    /// in the module ABI. Used by the execute handler to select a shared read
    /// lock instead of an exclusive write lock for qualifying calls.
    ///
    /// Keyed by `(ApplicationId, Option<String>)` — the same key as `modules` so
    /// both caches stay in sync. An absent entry means the app's manifest was not
    /// parsed yet (cold cache) or the app has no `#[app::view]` methods; in both
    /// cases the execute path defaults to the write lock (fail-safe). Populated
    /// alongside the module cache in `get_module`.
    ///
    /// Size-capped to `MAX_CACHED_MODULES` (one entry per compiled module).
    read_only_methods: BoundedCache<(ApplicationId, Option<String>), Arc<HashSet<String>>>,

    /// Cumulative hit/miss counters for the `contexts` hot cache, driving the
    /// periodic effectiveness log (see [`ContextCacheStats`] and
    /// [`Self::log_cache_stats`]). Independent of `metrics` so the log line
    /// works on nodes without a Prometheus registry.
    cache_stats: ContextCacheStats,

    /// Prometheus metrics for monitoring the health and performance of the manager,
    /// such as number of active contexts, message processing latency, etc.
    metrics: Option<Metrics>,

    /// Groups that currently have a running upgrade propagator. Prevents the
    /// manual retry handler from spawning a second propagator while an
    /// existing one is still active (e.g. sleeping in its backoff delay).
    active_propagators: HashSet<ContextGroupId>,

    /// Per-namespace governance DAG. Single DAG per namespace containing both
    /// root ops and encrypted group-scoped ops.
    ///
    /// Size-capped to `MAX_CACHED_NAMESPACE_DAGS` via [`BoundedCache`], with
    /// lock-gated eviction (an in-flight op holds the DAG's `Arc<Mutex>`); see
    /// the `Arc<Mutex<DagStore>>` [`Evictable`] impl. The datastore stays
    /// authoritative, so an evicted DAG is rebuilt on next touch.
    namespace_dags: BoundedCache<[u8; 32], Arc<Mutex<DagStore<SignedNamespaceOp>>>>,

    /// Routes incoming `SignedAck` messages from the wire receiver to the
    /// in-flight `publish_and_await_ack` caller waiting on a specific
    /// `op_hash`. Cloned from `context_client.ack_router()` so the
    /// receiver-side and publish-side share the same instance. See
    /// [`governance_broadcast`].
    pub(crate) ack_router: Arc<AckRouter>,

    /// Runtime-tunable timing knobs (see [`ContextManagerConfig`]).
    /// Populated with [`ContextManagerConfig::default`] by [`Self::new`].
    pub(crate) config: ContextManagerConfig,

    /// Operator-configured per-execution VM resource limits, baked into every
    /// engine the execute handler builds (precompiled + compile paths).
    /// Defaults to [`VMLimits::default`]; override via [`Self::with_vm_limits`]
    /// from the node config's `[runtime.limits]` section.
    pub(crate) vm_limits: calimero_runtime::logic::VMLimits,
}

/// Creates a new `ContextManager`.
///
/// # Arguments
///
/// * `datastore` - The persistent storage backend.
/// * `node_client` - Client for interacting with the underlying Calimero node.
/// * `context_client` - The context client facade.
/// * `prometheus_registry` - A mutable reference to a Prometheus registry for registering metrics.
impl ContextManager {
    pub fn new(
        datastore: Store,
        node_client: NodeClient,
        context_client: ContextClient,
        prometheus_registry: Option<&mut Registry>,
    ) -> Self {
        let ack_router = Arc::clone(context_client.ack_router());
        Self {
            datastore,
            node_client,
            context_client,

            contexts: BoundedCache::new(MAX_CACHED_CONTEXTS, "contexts"),
            applications: BoundedCache::new(MAX_CACHED_APPLICATIONS, "applications"),
            modules: BoundedCache::new(MAX_CACHED_MODULES, "modules"),
            read_only_methods: BoundedCache::new(MAX_CACHED_MODULES, "read_only_methods"),
            cache_stats: ContextCacheStats::default(),

            metrics: prometheus_registry.map(Metrics::new),
            active_propagators: HashSet::new(),
            namespace_dags: BoundedCache::new(MAX_CACHED_NAMESPACE_DAGS, "namespace_dags"),
            ack_router,
            config: ContextManagerConfig::default(),
            vm_limits: calimero_runtime::logic::VMLimits::default(),
        }
    }

    /// Override the per-execution VM resource limits applied when running guest
    /// WASM. Builder-style so the production path (node startup) can thread
    /// operator config in while tests keep the [`VMLimits::default`] behavior.
    #[must_use]
    pub fn with_vm_limits(mut self, vm_limits: calimero_runtime::logic::VMLimits) -> Self {
        self.vm_limits = vm_limits;
        self
    }

    /// Override the [`ContextManagerConfig::migration_v2`] master switch.
    /// Builder-style so node startup can thread the operator's
    /// `[context] migration_v2` config through while tests (and any caller that
    /// doesn't set it) keep the default-off behavior. See [`Self::with_vm_limits`].
    #[must_use]
    pub fn with_migration_v2(mut self, migration_v2: bool) -> Self {
        self.config.migration_v2 = migration_v2;
        self
    }

    /// Get this node's identity for the namespace (root group) that contains `group_id`.
    /// Returns `None` if no identity has been stored for that namespace yet.
    pub fn node_namespace_identity(
        &self,
        group_id: &ContextGroupId,
    ) -> Option<(calimero_primitives::identity::PublicKey, [u8; 32])> {
        match NamespaceRepository::new(&self.datastore).resolve_identity(group_id) {
            Ok(Some((pk, sk, _sender))) => Some((pk, sk)),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(?group_id, error=?e, "failed to resolve namespace identity");
                None
            }
        }
    }

    /// Get or create this node's identity for the namespace containing `group_id`.
    /// Generates a new keypair if none exists. Returns (namespace_id, public_key, private_key, sender_key).
    pub fn get_or_create_namespace_identity(
        &self,
        group_id: &ContextGroupId,
    ) -> eyre::Result<(
        ContextGroupId,
        calimero_primitives::identity::PublicKey,
        [u8; 32],
        [u8; 32],
    )> {
        NamespaceRepository::new(&self.datastore).get_or_create_identity(group_id)
    }
}

/// Result of the governance preflight check, containing everything needed
/// to sign and publish a governance op.
pub struct GovernancePreflight {
    /// The resolved requester public key.
    pub requester: calimero_primitives::identity::PublicKey,
    /// The signing private key (as raw bytes).
    pub signing_key: [u8; 32],
    /// Cloned datastore for use in async blocks.
    pub datastore: Store,
    /// Cloned node client for use in async blocks.
    pub node_client: calimero_node_primitives::client::NodeClient,
}

impl GovernancePreflight {
    /// Convenience: build a `PrivateKey` from the stored signing key bytes.
    pub fn signer_sk(&self) -> calimero_primitives::identity::PrivateKey {
        calimero_primitives::identity::PrivateKey::from(self.signing_key)
    }
}

impl ContextManager {
    /// Common preflight for governance mutation handlers.
    ///
    /// Resolves the requester identity, loads group metadata, checks admin
    /// authorization, resolves or stores the signing key, and returns
    /// everything needed for `sign_apply_and_publish`.
    ///
    /// Returns `Err` if the group doesn't exist, the requester isn't authorized,
    /// or no signing key is available.
    pub fn governance_preflight(
        &self,
        group_id: &ContextGroupId,
        requester: Option<calimero_primitives::identity::PublicKey>,
        require_admin: bool,
    ) -> eyre::Result<GovernancePreflight> {
        let requester = match requester {
            Some(pk) => pk,
            None => self
                .node_namespace_identity(group_id)
                .map(|(pk, _)| pk)
                .ok_or_else(|| {
                    eyre::eyre!("requester not provided and node has no configured group identity")
                })?,
        };

        if MetaRepository::new(&self.datastore)
            .load(group_id)?
            .is_none()
        {
            eyre::bail!("group '{group_id:?}' not found");
        }
        if require_admin {
            MembershipRepository::new(&self.datastore).require_admin(group_id, &requester)?;
        }

        let signing_key = SigningKeysRepository::new(&self.datastore)
            .resolve(group_id, &requester)?
            .ok_or_else(|| {
                eyre::eyre!("local group governance requires a signing key for the requester")
            })?;

        Ok(GovernancePreflight {
            requester,
            signing_key,
            datastore: self.datastore.clone(),
            node_client: self.node_client.clone(),
        })
    }
}

impl ContextManager {
    /// Common pattern for governance mutation handlers that:
    /// 1. Run governance_preflight (identity + admin check + signing key)
    /// 2. Sign, apply, and publish a GroupOp
    ///
    /// Handles the boilerplate of cloning datastore/node_client, building the
    /// signer key, and wrapping in an ActorResponse::r#async.
    pub(crate) fn sign_and_publish_group_op(
        &mut self,
        group_id: &calimero_context_config::types::ContextGroupId,
        requester: Option<calimero_primitives::identity::PublicKey>,
        require_admin: bool,
        op: calimero_context_client::local_governance::GroupOp,
    ) -> ActorResponse<Self, eyre::Result<()>> {
        let preflight = match self.governance_preflight(group_id, requester, require_admin) {
            Ok(p) => p,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let datastore = preflight.datastore.clone();
        let node_client = preflight.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);
        let sk = preflight.signer_sk();
        let group_id = *group_id;
        let op_debug = format!("{op:?}");

        ActorResponse::r#async(
            async move {
                let _report = calimero_governance_store::sign_apply_and_publish(
                    &datastore,
                    &node_client,
                    &ack_router,
                    &group_id,
                    &sk,
                    op,
                )
                .await?;
                tracing::info!(?group_id, op = %op_debug, "governance op applied");
                Ok(())
            }
            .into_actor(self),
        )
    }
}

impl ContextManager {
    fn get_or_create_namespace_dag(
        &mut self,
        namespace_id: &[u8; 32],
    ) -> Arc<Mutex<DagStore<SignedNamespaceOp>>> {
        self.namespace_dags
            .get_or_insert_with(*namespace_id, || {
                Arc::new(Mutex::new(DagStore::new([0u8; 32])))
            })
            .clone()
    }
}

/// Implements the `Actor` trait for `ContextManager`, allowing it to run within the Actix framework.
///
/// By implementing `Actor`, `ContextManager` gains a "Context" (an execution environment) and a mailbox.
/// Messages sent to the manager are queued in its mailbox and processed one at a time in the order
/// they are received, which is the core of the actor model's safety guarantee for its internal state.
impl Actor for ContextManager {
    type Context = actix::Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.recover_in_progress_upgrades(ctx);
        self.start_namespace_heartbeat(ctx);
        // Periodically report in-memory context-cache effectiveness.
        // Runs on the actor thread, so it reads cache state without locking.
        // The returned SpawnHandle is intentionally dropped: this interval
        // runs for the actor's entire lifetime and is never cancelled (same as
        // `start_namespace_heartbeat`).
        ctx.run_interval(CACHE_STATS_LOG_INTERVAL, |act, _ctx| {
            act.log_cache_stats();
        });
        // Auto-follow handler (see the auto-follow architecture doc) — reacts to governance
        // op-apply events and emits JoinContext on behalf of members
        // with `auto_follow.contexts = true`.
        auto_follow::spawn(self.datastore.clone(), self.context_client.clone());
        // Self-purge handler (see docs/adr/0002-fleet-tee-leave-protocol.md) — reacts
        // to `OpEvent::TeeMemberRemoved` (paired follow-up emitted ONLY when the
        // removed member's prior role was `ReadOnlyTee`) for our own identity and
        // drops the local rows (signing keys, gov ops, namespace identity,
        // membership-side metadata) that the apply layer leaves behind after TEE
        // eviction. The listener intentionally does NOT react to plain
        // `OpEvent::MemberRemoved` — non-TEE removals (admin kicks, voluntary
        // leave, leave-and-rejoin via inheritance) keep soft-leave semantics so
        // existing rejoin codepaths can re-establish state. Mirrors
        // auto_follow's listener pattern. Idempotent across restarts.
        self_purge::spawn(self.datastore.clone(), self.node_client.clone());
    }
}

// Lifecycle methods (recover_in_progress_upgrades, start_namespace_heartbeat)
// are in lifecycle.rs

impl ContextMeta {
    /// Acquire this context's lock in exclusive (write) mode; see [`ContextLock::lock`].
    fn lock(
        &self,
    ) -> Either<ContextGuard, std::pin::Pin<Box<dyn Future<Output = ContextGuard> + Send>>> {
        self.lock.lock()
    }

    /// Acquire this context's lock in shared (read) mode; see [`ContextLock::lock_read`].
    fn lock_read(
        &self,
    ) -> Either<ContextGuard, std::pin::Pin<Box<dyn Future<Output = ContextGuard> + Send>>> {
        self.lock.lock_read()
    }
}

impl ContextManager {
    /// Record one context-cache access against both the always-on in-memory
    /// counters and (when a registry is configured) the Prometheus series.
    /// `hit` is the `was_cached` outcome at the cache-aside entry point.
    fn record_cache_access(&self, hit: bool) {
        self.cache_stats.record(hit);

        if let Some(metrics) = &self.metrics {
            let counter = if hit {
                &metrics.context_cache_hits
            } else {
                &metrics.context_cache_misses
            };
            counter.inc();
        }
    }

    /// Emit a single context-cache effectiveness line and refresh the cache-size
    /// gauges. Driven every [`CACHE_STATS_LOG_INTERVAL`] from [`Actor::started`].
    ///
    /// Hits/misses are cumulative over the process lifetime, so `hit_rate` is a
    /// lifetime average. When no accesses have happened yet the line is dropped
    /// to `debug` to keep idle nodes quiet; an active cache logs at `info`.
    fn log_cache_stats(&self) {
        let (hits, misses) = self.cache_stats.snapshot();
        let total = hits + misses;
        let context_cache_size = self.contexts.len();
        let application_cache_size = self.applications.len();

        if let Some(metrics) = &self.metrics {
            // i64 gauges; cache sizes are bounded by MAX_CACHED_* (≤ 1024) so the
            // cast is always lossless.
            metrics.context_cache_size.set(context_cache_size as i64);
            metrics
                .application_cache_size
                .set(application_cache_size as i64);
        }

        // Avoid division by zero and don't spam idle nodes with "0.0%" lines.
        if total == 0 {
            tracing::debug!(
                context_cache_size,
                application_cache_size,
                "Context cache statistics (no accesses yet)"
            );
            return;
        }

        #[expect(
            clippy::cast_precision_loss,
            reason = "hit rate is a display-only percentage; f64 has ample precision for cache counters"
        )]
        let hit_rate = hits as f64 / total as f64;

        tracing::info!(
            hits,
            misses,
            hit_rate = %format!("{:.1}%", hit_rate * 100.0),
            context_cache_size,
            application_cache_size,
            "Context cache statistics"
        );
    }

    /// Retrieves context metadata, fetching from the datastore if not present in the cache.
    ///
    /// This function implements the "cache-aside" pattern. It first checks the in-memory
    /// `contexts` BTreeMap. On a cache miss, it falls back to querying the persistent
    /// `datastore` via the `context_client`, populates the cache with the result,
    /// and then returns the data.
    ///
    /// # Arguments
    ///
    /// * `context_id` - The unique identifier of the context to retrieve.
    ///
    /// # Returns
    ///
    /// A `Result` containing an `Option` with a reference to the `ContextMeta`.
    /// Returns `Ok(Some(&ContextMeta))` if the context is found in the cache or datastore.
    /// Returns `Ok(None)` if the context does not exist in the datastore.
    /// Returns `Err` if a datastore error occurs.
    fn get_or_fetch_context(
        &mut self,
        context_id: &ContextId,
    ) -> eyre::Result<Option<&ContextMeta>> {
        // Cache miss: fetch first, so a lookup for a non-existent context never
        // triggers (and wastes) an eviction. Only once we know the context
        // exists do we make room and insert. Kept as a self-contained block so
        // its borrows end before the single returning borrow taken below (the
        // borrow checker can't return a reference through a split hit/miss
        // match otherwise).
        let was_cached = self.contexts.contains_key(context_id);
        if was_cached {
            self.record_cache_access(true);
        } else {
            let Some(context) = self.context_client.get_context(context_id)? else {
                // The context exists in neither the cache nor the datastore, so
                // this is a probe for a non-existent context rather than a cache
                // miss — leave the effectiveness counters untouched.
                return Ok(None);
            };
            self.record_cache_access(false);

            let _ = self.contexts.insert_new(
                *context_id,
                ContextMeta {
                    meta: context,
                    lock: ContextLock::new(*context_id),
                },
            );
        }

        let Some(cached) = self.contexts.get_mut(context_id) else {
            // Unreachable: the entry was either already cached or just inserted
            // above, and this actor processes messages serially.
            debug_assert!(false, "context entry vanished between insert and lookup");
            return Ok(None);
        };

        // For an already-cached entry, reload from the DB to pick up
        // out-of-band changes. A freshly fetched-and-inserted entry is already
        // current, so skip the redundant read.
        if was_cached {
            // CRITICAL FIX: Always reload dag_heads from database to get latest state
            // The dag_heads can be updated by delta_store when receiving network deltas,
            // but the cached Context object won't reflect these changes.
            // This was causing all deltas to use genesis as parent instead of actual dag_heads.
            let handle = self.datastore.handle();
            let key = calimero_store::key::ContextMeta::new(*context_id);

            if let Some(meta) = handle.get(&key)? {
                // Update dag_heads if they changed in DB
                if cached.meta.dag_heads != meta.dag_heads {
                    tracing::debug!(
                        %context_id,
                        old_heads_count = cached.meta.dag_heads.len(),
                        new_heads_count = meta.dag_heads.len(),
                        "Refreshing dag_heads from database (cache was stale)"
                    );
                    cached.meta.dag_heads = meta.dag_heads;
                }

                // Also update root_hash in case it changed
                cached.meta.root_hash = meta.root_hash.into();

                // Refresh application_id too. A LazyOnAccess upgrade (or
                // a cascade target-application change) rewrites the
                // context's bound application in the DB out-of-band of
                // this in-memory cache. Callers (notably the execute
                // path's post-lazy-upgrade re-fetch) resolve the WASM
                // module from `application_id`; if the cache still holds
                // the pre-upgrade id, the method runs the OLD module
                // against the freshly-migrated (new-shaped) state — a
                // borsh "Not all bytes read" panic. Keeping app_id in
                // lockstep with the DB closes that window.
                let db_application_id = meta.application.application_id();
                if cached.meta.application_id != db_application_id {
                    tracing::debug!(
                        %context_id,
                        old_application_id = %cached.meta.application_id,
                        new_application_id = %db_application_id,
                        "Refreshing application_id from database (cache was stale)"
                    );
                    cached.meta.application_id = db_application_id;
                }
            }
        }

        Ok(Some(cached))
    }
}

#[cfg(test)]
mod cache_eviction_tests {
    use std::str::FromStr as _;

    use super::*;
    use calimero_primitives::hash::Hash;

    /// Distinct `ContextId` per index (two bytes cover well beyond the cap).
    fn cid(i: usize) -> ContextId {
        let mut bytes = [0u8; 32];
        bytes[0] = (i & 0xff) as u8;
        bytes[1] = ((i >> 8) & 0xff) as u8;
        ContextId::from(bytes)
    }

    /// An entry whose lock is held by nobody → `strong_count == 1` (evictable).
    fn idle_meta(id: ContextId) -> ContextMeta {
        ContextMeta {
            meta: Context::new(id, ApplicationId::from([0u8; 32]), Hash::default()),
            lock: ContextLock::new(id),
        }
    }

    /// An entry with an outstanding owned guard → `strong_count == 2` (live,
    /// MUST NOT be evicted). The returned guard must be kept alive by the test.
    fn live_meta(id: ContextId) -> (ContextMeta, ContextGuard) {
        let lock = ContextLock::new(id);
        let guard = match lock.lock() {
            Either::Left(guard) => guard,
            Either::Right(_) => unreachable!("fresh lock is free"),
        };
        let meta = ContextMeta {
            meta: Context::new(id, ApplicationId::from([0u8; 32]), Hash::default()),
            lock,
        };
        (meta, guard)
    }

    // These tests drive the real `ContextMeta` / `Application` types through
    // `BoundedCache` so they pin the *wiring* — the cap constants and the
    // `Evictable` impls (lock-gated for contexts, always-idle for apps). The
    // generic cap/eviction mechanics themselves are covered in `cache::tests`.

    #[test]
    fn no_eviction_below_cap() {
        let mut contexts = BoundedCache::new(MAX_CACHED_CONTEXTS, "contexts");
        for i in 0..(MAX_CACHED_CONTEXTS - 1) {
            let _ = contexts.insert_new(cid(i), idle_meta(cid(i)));
        }
        assert_eq!(contexts.len(), MAX_CACHED_CONTEXTS - 1);
    }

    #[test]
    fn evicts_one_idle_entry_at_cap() {
        let mut contexts = BoundedCache::new(MAX_CACHED_CONTEXTS, "contexts");
        for i in 0..MAX_CACHED_CONTEXTS {
            let _ = contexts.insert_new(cid(i), idle_meta(cid(i)));
        }
        assert_eq!(contexts.len(), MAX_CACHED_CONTEXTS);

        // A new key at cap evicts exactly one idle entry, holding at cap.
        let new_id = cid(MAX_CACHED_CONTEXTS);
        let _ = contexts.insert_new(new_id, idle_meta(new_id));
        assert_eq!(contexts.len(), MAX_CACHED_CONTEXTS);
    }

    #[test]
    fn never_evicts_a_live_entry() {
        let mut contexts = BoundedCache::new(MAX_CACHED_CONTEXTS, "contexts");
        let mut guards = Vec::new();

        // Every entry live except a single idle one (the lowest key, so it's
        // also the first candidate eviction would consider by key order).
        let idle_id = cid(0);
        let _ = contexts.insert_new(idle_id, idle_meta(idle_id));
        for i in 1..MAX_CACHED_CONTEXTS {
            let (meta, guard) = live_meta(cid(i));
            let _ = contexts.insert_new(cid(i), meta);
            guards.push(guard);
        }

        // At cap; the next new insert must evict the single idle entry, never
        // a live one.
        let new_id = cid(MAX_CACHED_CONTEXTS);
        let _ = contexts.insert_new(new_id, idle_meta(new_id));

        assert_eq!(contexts.len(), MAX_CACHED_CONTEXTS);
        assert!(
            !contexts.contains_key(&idle_id),
            "idle entry should be gone"
        );
        for i in 1..MAX_CACHED_CONTEXTS {
            assert!(contexts.contains_key(&cid(i)), "live entry {i} was evicted");
        }
        drop(guards);
    }

    #[test]
    fn no_eviction_when_all_entries_live() {
        let mut contexts = BoundedCache::new(MAX_CACHED_CONTEXTS, "contexts");
        let mut guards = Vec::new();
        for i in 0..MAX_CACHED_CONTEXTS {
            let (meta, guard) = live_meta(cid(i));
            let _ = contexts.insert_new(cid(i), meta);
            guards.push(guard);
        }

        // Cache is at cap but nothing is evictable → it stays at cap rather
        // than corrupting a live context's lock identity.
        contexts.evict_if_full();
        assert_eq!(contexts.len(), MAX_CACHED_CONTEXTS);
        drop(guards);
    }

    fn app(i: usize) -> Application {
        use calimero_primitives::application::{ApplicationBlob, ApplicationSource};
        use calimero_primitives::blobs::BlobId;
        // Two key bytes so indices beyond 255 stay distinct (the cap is 256).
        let mut bytes = [0u8; 32];
        bytes[0] = (i & 0xff) as u8;
        bytes[1] = ((i >> 8) & 0xff) as u8;
        let id = ApplicationId::from(bytes);
        Application::new(
            id,
            ApplicationBlob {
                bytecode: BlobId::from([0u8; 32]),
                compiled: BlobId::from([0u8; 32]),
            },
            0,
            ApplicationSource::from_str("http://example.test").expect("valid url"),
            Vec::new(),
        )
    }

    #[test]
    fn application_cap_evicts_at_cap_only() {
        let mut apps = BoundedCache::new(MAX_CACHED_APPLICATIONS, "applications");
        for i in 0..MAX_CACHED_APPLICATIONS {
            let a = app(i);
            let _ = apps.insert_new(a.id, a);
        }
        assert_eq!(apps.len(), MAX_CACHED_APPLICATIONS);

        // Application metadata is always idle, so a new key evicts exactly one
        // (the lowest), holding at cap.
        let a = app(MAX_CACHED_APPLICATIONS);
        let _ = apps.insert_new(a.id, a);
        assert_eq!(apps.len(), MAX_CACHED_APPLICATIONS);
    }

    #[test]
    fn cache_stats_record_and_snapshot() {
        let stats = ContextCacheStats::default();
        assert_eq!(stats.snapshot(), (0, 0));

        stats.record(true);
        stats.record(true);
        stats.record(false);
        assert_eq!(stats.snapshot(), (2, 1));
    }

    /// Mirrors the percentage math in `log_cache_stats` so the formatting
    /// contract (one decimal, `%` suffix) is pinned without constructing a full
    /// `ContextManager`.
    #[test]
    fn cache_stats_hit_rate_formatting() {
        let stats = ContextCacheStats::default();
        for _ in 0..3 {
            stats.record(true);
        }
        stats.record(false); // 3 hits / 4 total = 75.0%

        let (hits, misses) = stats.snapshot();
        let total = hits + misses;
        let hit_rate = hits as f64 / total as f64;
        assert_eq!(format!("{:.1}%", hit_rate * 100.0), "75.0%");
    }
}
