//! What a run gets from the node besides its state: the app's blobs, search
//! over the app's own indexes, and the facts the node reads from a bundle's
//! ABI (timers, handlers, xcall entry points).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use calimero_primitives::search::{
    ExtractRequest, ExtractResponse, ScanRequest, ScanResponse, SearchIndexSchema, SearchRequest,
    EXTRACT_EXPORT, SCAN_EXPORT, SCHEMA_EXPORT,
};
use calimero_runtime::logic::{BlobHost, SearchHost, SearchOutput};
use calimero_runtime::store::Storage;
use calimero_search::index::{ContextIndex, SchemaOptions};
use calimero_wasm_abi::schema::XCallCallers;
use tracing::{debug, warn};

use crate::app::{blob_key, AppMachine, Executor, RunEnv, Status};
use crate::node::Node;
use crate::types::{AppId, Caller};

/// How long a run waits for a peer to hand over a blob this node misses.
const BLOB_FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// The cluster's blob store, as one app's runs see it.
#[derive(Debug)]
pub struct AppBlobs {
    pub node: Weak<Node>,
}

impl BlobHost for AppBlobs {
    fn get(&self, blob: [u8; 32], state: &dyn Storage) -> Result<Option<Vec<u8>>, String> {
        if state.host_get(&blob_key(&blob)).is_none() {
            return Ok(None);
        }
        let node = self.node.upgrade().ok_or("node is shutting down")?;
        let id = hex::encode(blob);
        if let Some(bytes) = node.engine.get_blob(&id).map_err(|e| e.to_string())? {
            return Ok(Some(bytes));
        }
        // Held for the app, but this node missed it (it was down when the blob
        // was stored): a peer has it. Runs happen on the runtime's worker
        // threads, so the wait is handed off the scheduler.
        let fetched = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                tokio::time::timeout(BLOB_FETCH_TIMEOUT, node.fetch_blob(&id)).await
            })
        });
        match fetched {
            Ok(Ok(bytes)) => Ok(Some(bytes)),
            Ok(Err(e)) => Err(e.message),
            Err(_) => Err(format!("no peer handed over blob {id} in time")),
        }
    }
}

/// The ABI facts the node acts on, per bundle.
#[derive(Clone, Debug, Default)]
pub struct BundleFacts {
    /// `#[app::tee(every = "..")]` methods and their period in seconds.
    pub timers: BTreeMap<String, u64>,
    /// `#[app::handler]` methods.
    pub handlers: Vec<String>,
    /// `#[app::xcall]` entry points and who may call them.
    pub xcalls: BTreeMap<String, XCallCallers>,
    /// `#[app::view]` methods: a call of one runs as a view (v1 ran a declared
    /// view read-only whichever way it was invoked).
    pub views: Vec<String>,
}

impl BundleFacts {
    /// Read them from the ABI a bundle embeds; none when it embeds none.
    #[must_use]
    pub fn of(wasm: &[u8]) -> Self {
        let Some(manifest) = calimero_wasm_abi::embed::read_embedded_state_schema(wasm) else {
            return Self::default();
        };
        let mut facts = Self::default();
        for m in manifest.methods {
            if let Some(every) = m.tee_every_secs.filter(|s| *s > 0) {
                let _prev = facts.timers.insert(m.name.clone(), every);
            }
            if m.handler {
                facts.handlers.push(m.name.clone());
            }
            if m.intent == calimero_wasm_abi::schema::MethodIntent::ReadOnly {
                facts.views.push(m.name.clone());
            }
            if m.xcall_callable {
                let _prev = facts.xcalls.insert(m.name, m.xcall_callers);
            }
        }
        facts
    }
}

/// One app's search indexes, built from its search exports.
pub struct AppIndexes {
    bundle: String,
    /// The applied index the indexes reflect.
    built_at: u64,
    indexes: HashMap<String, ContextIndex>,
}

impl std::fmt::Debug for AppIndexes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppIndexes")
            .field("bundle", &self.bundle)
            .field("built_at", &self.built_at)
            .field("indexes", &self.indexes.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Full-text search for every app on this node: v1's per-context tantivy
/// indexes, kept in memory and brought up to date from the app's log before a
/// view runs. Derived data, like v1's: never replicated, rebuilt from state
/// after a restart.
#[derive(Default)]
pub struct Search {
    apps: Mutex<HashMap<AppId, Arc<Mutex<AppIndexes>>>>,
}

/// A view's search, bound to its app's indexes.
#[derive(Debug)]
pub struct AppSearch(Arc<Mutex<AppIndexes>>);

impl SearchHost for AppSearch {
    fn search(&self, _context: [u8; 32], request: &[u8]) -> Result<SearchOutput, String> {
        let request: SearchRequest = borsh::from_slice(request).map_err(|e| e.to_string())?;
        let indexes = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let index = indexes
            .indexes
            .get(&request.index)
            .ok_or_else(|| format!("the app declares no search index {:?}", request.index))?;
        let response = index.search(&request).map_err(|e| e.to_string())?;
        Ok(SearchOutput {
            matched: response.total,
            hits: response.hits.len() as u64,
            response: borsh::to_vec(&response).map_err(|e| e.to_string())?,
        })
    }
}

/// Run one of the app's search exports, read-only, and decode its borsh answer.
fn export<T: borsh::BorshDeserialize>(
    module: &calimero_runtime::Module,
    m: &AppMachine,
    method: &str,
    input: &[u8],
) -> Result<Option<T>, String> {
    let caller = Caller::App {
        app: m.app_id.clone(),
    };
    let (out, ret) = Executor::run_bytes(
        module,
        &m.app_id,
        &caller,
        method,
        input,
        &m.state.kv,
        &RunEnv::default(),
    );
    if out.status != Status::Ok {
        let why = out.error.to_string();
        // An app without search has no such export.
        if why.contains("MethodNotFound") {
            return Ok(None);
        }
        return Err(format!("{method}: {why}"));
    }
    let bytes = ret.ok_or_else(|| format!("{method} returned nothing"))?;
    borsh::from_slice(&bytes)
        .map(Some)
        .map_err(|e| format!("{method}: {e}"))
}

impl Search {
    /// The app's search, brought up to `applied`, for a view of `bundle`.
    /// `None` when the app declares no index.
    pub fn for_view(
        &self,
        module: &calimero_runtime::Module,
        bundle: &str,
        m: &AppMachine,
        applied: u64,
    ) -> Option<Arc<dyn SearchHost>> {
        let entry = {
            let mut apps = self.apps.lock().unwrap_or_else(PoisonError::into_inner);
            Arc::clone(apps.entry(m.app_id.clone()).or_insert_with(|| {
                Arc::new(Mutex::new(AppIndexes {
                    bundle: String::new(),
                    built_at: 0,
                    indexes: HashMap::new(),
                }))
            }))
        };
        {
            let mut indexes = entry.lock().unwrap_or_else(PoisonError::into_inner);
            if let Err(e) = Self::refresh(&mut indexes, module, bundle, m, applied) {
                warn!(app = %m.app_id, error = %e, "search index not brought up to date");
                return None;
            }
            if indexes.indexes.is_empty() {
                return None;
            }
        }
        Some(Arc::new(AppSearch(entry)))
    }

    fn refresh(
        ix: &mut AppIndexes,
        module: &calimero_runtime::Module,
        bundle: &str,
        m: &AppMachine,
        applied: u64,
    ) -> Result<(), String> {
        if ix.bundle == bundle && ix.built_at == applied {
            return Ok(());
        }
        if ix.bundle == bundle {
            // An app without search stays without until its bundle changes.
            if ix.indexes.is_empty() {
                ix.built_at = applied;
                return Ok(());
            }
            if let Some(touched) = m.touched_after(ix.built_at) {
                let ids: Vec<[u8; 32]> = touched
                    .iter()
                    .filter_map(|h| hex::decode(h).ok())
                    .filter_map(|b| <[u8; 32]>::try_from(b.as_slice()).ok())
                    .collect();
                if !ids.is_empty() {
                    for (name, index) in &ix.indexes {
                        let request = borsh::to_vec(&ExtractRequest {
                            index: name.clone(),
                            ids: ids.clone(),
                        })
                        .map_err(|e| e.to_string())?;
                        let docs: ExtractResponse = export(module, m, EXTRACT_EXPORT, &request)?
                            .ok_or("the app lost its extract export")?;
                        let _n = index
                            .apply(ids.iter().zip(docs.iter().map(Option::as_ref)))
                            .map_err(|e| e.to_string())?;
                        index.commit(applied, [0; 32]).map_err(|e| e.to_string())?;
                    }
                }
                ix.built_at = applied;
                return Ok(());
            }
        }
        // A new bundle, or entries this replica cannot account for: rebuild.
        debug!(app = %m.app_id, applied, "building search indexes");
        let schemas: Option<Vec<SearchIndexSchema>> = export(module, m, SCHEMA_EXPORT, &[])?;
        let mut indexes = HashMap::new();
        for def in schemas.unwrap_or_default() {
            let dir = Box::new(tantivy::directory::RamDirectory::create());
            let (index, _state) = ContextIndex::open(dir, &def, SchemaOptions::default())
                .map_err(|e| e.to_string())?;
            let index = index.ok_or("a new index opened stale")?;
            let mut from = Vec::new();
            loop {
                let request = borsh::to_vec(&ScanRequest {
                    index: def.name.clone(),
                    from: from.clone(),
                    limit: 256,
                })
                .map_err(|e| e.to_string())?;
                let page: ScanResponse = export(module, m, SCAN_EXPORT, &request)?
                    .ok_or("the app lost its scan export")?;
                let _n = index
                    .apply(page.docs.iter().map(|d| (&d.id, Some(d))))
                    .map_err(|e| e.to_string())?;
                match page.next {
                    Some(next) if next > from => from = next,
                    _ => break,
                }
            }
            index.commit(applied, [0; 32]).map_err(|e| e.to_string())?;
            let _prev = indexes.insert(def.name.clone(), index);
        }
        *ix = AppIndexes {
            bundle: bundle.to_owned(),
            built_at: applied,
            indexes,
        };
        Ok(())
    }

    /// Forget an app's indexes (it moved off this node).
    pub fn drop_app(&self, app_id: &str) {
        let _gone = self
            .apps
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(app_id);
    }
}

/// The migration an upgrade from `current` to `target` runs, planned from the
/// two bundles' embedded ABIs as v1 plans it (`calimero-context`'s
/// `plan_upgrade`): `None` when the state shape is unchanged, the target's
/// `#[app::migrate]` edge from the current state version for one step up.
/// Bundles that embed no ABI (built without `cargo mero`) upgrade code-only.
///
/// # Errors
/// A downgrade, a jump of more than one version, a missing edge, or one side
/// with an ABI and the other without.
pub fn plan_upgrade(current: &[u8], target: &[u8]) -> Result<Option<String>, String> {
    use calimero_wasm_abi::embed::read_embedded_state_schema as abi;
    let (cur, tgt) = match (abi(current), abi(target)) {
        (None, None) => return Ok(None),
        (Some(c), Some(t)) => (c, t),
        (None, Some(_)) => return Err("the current bundle embeds no ABI to plan from".into()),
        (Some(_), None) => return Err("the target bundle embeds no ABI to plan from".into()),
    };
    // v1's identity-downgrade gate (#2587): a field that loses identity-gated
    // semantics (an `AuthoredMap` that becomes an `UnorderedMap`) would strip
    // every entry's authorship or writer set for everyone.
    if let Some(d) = calimero_wasm_abi::downgrade::identity_downgrades(&cur, &tgt)
        .into_iter()
        .next()
    {
        return Err(format!(
            "identity downgrade forbidden: field '{}' {} -> {} strips authorship (rewrite it \
             under the owner instead)",
            d.field, d.from, d.to
        ));
    }
    let (from, to) = (
        cur.state_version_or_default(),
        tgt.state_version_or_default(),
    );
    if from == to {
        return Ok(None);
    }
    if to < from {
        return Err(format!(
            "a downgrade of the state from version {from} to {to}"
        ));
    }
    if to != from + 1 {
        return Err(format!(
            "state version {from} to {to}: upgrade one version at a time"
        ));
    }
    tgt.edge_from(from)
        .map(|e| Some(e.method))
        .ok_or_else(|| format!("the target declares no migration from state version {from}"))
}

#[cfg(test)]
mod tests {
    use calimero_wasm_abi::embed::write_embedded_state_schema;
    use calimero_wasm_abi::schema::{Manifest, MigrationEdgeAbi};

    use super::plan_upgrade;

    /// An empty module carrying a manifest at `version`, with migrations from
    /// each of `edges` (named `migrate_from_<v>`).
    fn bundle(version: u32, edges: &[u32]) -> Vec<u8> {
        let mut m = Manifest::new();
        m.state_version = Some(version);
        m.migrations = edges
            .iter()
            .map(|v| MigrationEdgeAbi {
                method: format!("migrate_from_{v}"),
                from_version: *v,
            })
            .collect();
        write_embedded_state_schema(b"\0asm\x01\0\0\0", &m).unwrap()
    }

    #[test]
    fn an_upgrade_runs_the_targets_migration_edge_as_v1_plans_it() {
        assert_eq!(plan_upgrade(&bundle(1, &[]), &bundle(1, &[])), Ok(None));
        assert_eq!(
            plan_upgrade(&bundle(1, &[]), &bundle(2, &[1])),
            Ok(Some("migrate_from_1".to_owned()))
        );
        assert!(plan_upgrade(&bundle(2, &[]), &bundle(1, &[])).is_err());
        assert!(plan_upgrade(&bundle(1, &[]), &bundle(3, &[1, 2])).is_err());
        assert!(plan_upgrade(&bundle(1, &[]), &bundle(2, &[])).is_err());
        // Built without `cargo mero` on both sides: code only.
        let plain = b"\0asm\x01\0\0\0".to_vec();
        assert_eq!(plan_upgrade(&plain, &plain), Ok(None));
        assert!(plan_upgrade(&plain, &bundle(2, &[1])).is_err());
    }
}
