pub mod cli;

use borsh::BorshDeserialize;
use calimero_store::types::{
    ContextDagDelta as StoreContextDagDelta, ContextMeta as StoreContextMeta,
};
use eyre::{Result, WrapErr};
use rocksdb::{DBWithThreadMode, IteratorMode, SingleThreaded};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

use crate::types::Column;

/// Export DAG structure from the Generic column family
#[expect(
    clippy::too_many_lines,
    reason = "Existing DAG export logic is monolithic and will be refactored separately"
)]
pub fn export_dag(db: &DBWithThreadMode<SingleThreaded>) -> Result<Value> {
    // First, read ContextMeta to get dag_heads for each context
    let meta_cf_name = Column::Meta.as_str();
    let meta_cf = db
        .cf_handle(meta_cf_name)
        .ok_or_else(|| eyre::eyre!("Column family '{meta_cf_name}' not found"))?;

    let mut context_heads: HashMap<String, Vec<String>> = HashMap::new();
    let mut valid_contexts: HashSet<String> = HashSet::new();
    let meta_iter = db.iterator_cf(&meta_cf, IteratorMode::Start);

    for item in meta_iter {
        let (key, value) = item.wrap_err_with(|| {
            format!("Failed to read entry from column family '{meta_cf_name}'")
        })?;

        if key.len() == 32 {
            let context_id = hex::encode(&key);
            if !valid_contexts.insert(context_id.clone()) {
                continue;
            }
            if let Ok(meta) = StoreContextMeta::try_from_slice(&value) {
                let heads: Vec<String> = meta.dag_heads.iter().map(hex::encode).collect();
                let replaced = context_heads.insert(context_id, heads);
                debug_assert!(
                    replaced.is_none(),
                    "Context metadata appears multiple times"
                );
            }
        }
    }

    // Read deltas from both Delta and Generic column families
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut context_dags: HashMap<String, Vec<String>> = HashMap::new();
    let mut node_set = HashSet::new();
    let mut orphaned_deltas: Vec<(String, String)> = Vec::new(); // (context_id, delta_id)

    // Helper closure to process delta entries
    let mut process_deltas = |iter: rocksdb::DBIteratorWithThreadMode<
        '_,
        DBWithThreadMode<SingleThreaded>,
    >|
     -> Result<()> {
        for item in iter {
            let (key, value) = item.wrap_err("Failed to read delta entry")?;

            // Only process 64-byte keys (ContextId + DeltaId)
            if key.len() != 64 {
                continue;
            }

            // Try to parse as ContextDagDelta
            if let Ok(delta) = StoreContextDagDelta::try_from_slice(&value) {
                let context_id = hex::encode(&key[0..32]);

                // NOTE: Deltas for deleted contexts are INTENTIONALLY kept in the database!
                // They're part of the immutable distributed DAG and must remain for sync.
                // We track them separately for diagnostics but don't skip them.
                if !valid_contexts.contains(&context_id) {
                    let delta_id = hex::encode(delta.delta_id);
                    orphaned_deltas.push((context_id.clone(), delta_id));
                    // Don't continue - still process these deltas for visualization
                }

                let delta_id = hex::encode(delta.delta_id);
                let node_id = format!("{context_id}:{delta_id}");

                // Track which deltas belong to which context
                context_dags
                    .entry(context_id.clone())
                    .or_default()
                    .push(delta_id.clone());

                // Skip duplicate nodes
                if !node_set.insert(node_id.clone()) {
                    continue;
                }

                // Extract HLC information
                let timestamp = delta.hlc.inner();
                let raw_time = timestamp.get_time().as_u64();
                let physical_seconds = (raw_time >> 32_u32) as u32;
                let logical_counter = (raw_time & 0xF) as u32;

                // Convert parents to hex-encoded strings
                let parent_hashes: Vec<String> = delta.parents.iter().map(hex::encode).collect();

                // Check if this delta is a DAG head for its context
                let is_dag_head = context_heads
                    .get(&context_id)
                    .is_some_and(|heads| heads.contains(&delta_id));

                // Deserialize actions for human-readable display
                let actions_json = match delta.deserialize_actions() {
                    Ok(actions) => {
                        let actions_vec: Vec<Value> = actions
                            .iter()
                            .map(|action| {
                                use calimero_storage::action::Action;
                                match action {
                                    Action::Add {
                                        id,
                                        data,
                                        ancestors,
                                        metadata,
                                    } => json!({
                                        "type": "Add",
                                        "id": hex::encode(id.as_bytes()),
                                        "data_size": data.len(),
                                        "ancestors_count": ancestors.len(),
                                        "metadata": {
                                            "created_at": metadata.created_at(),
                                            "updated_at": metadata.updated_at(),
                                        }
                                    }),
                                    Action::Update {
                                        id,
                                        data,
                                        ancestors,
                                        metadata,
                                    } => json!({
                                        "type": "Update",
                                        "id": hex::encode(id.as_bytes()),
                                        "data_size": data.len(),
                                        "ancestors_count": ancestors.len(),
                                        "metadata": {
                                            "created_at": metadata.created_at(),
                                            "updated_at": metadata.updated_at(),
                                        }
                                    }),
                                    Action::DeleteRef { id, deleted_at } => json!({
                                        "type": "DeleteRef",
                                        "id": hex::encode(id.as_bytes()),
                                        "deleted_at": deleted_at,
                                    }),
                                    Action::Compare { id } => json!({
                                        "type": "Compare",
                                        "id": hex::encode(id.as_bytes()),
                                    }),
                                }
                            })
                            .collect();
                        Some(actions_vec)
                    }
                    Err(e) => {
                        eprintln!("Failed to deserialize actions for delta {delta_id}: {e}");
                        None
                    }
                };

                // Deserialize events if present
                let events_json = match delta.deserialize_events() {
                    Ok(Some(events)) => Some(events),
                    Ok(None) => None,
                    Err(e) => {
                        eprintln!("Failed to deserialize events for delta {delta_id}: {e}");
                        None
                    }
                };

                // Create node
                let mut node_json = json!({
                    "id": node_id,
                    "context_id": context_id,
                    "delta_id": delta_id,
                    "timestamp": raw_time,
                    "physical_time": physical_seconds,
                    "logical_counter": logical_counter,
                    "hlc": delta.hlc.to_string(),
                    "actions_size": delta.actions.len(),
                    "applied": delta.applied,
                    "parent_count": delta.parents.len(),
                    "parents": parent_hashes.clone(),
                    "is_dag_head": is_dag_head,
                    "has_missing_parents": false  // Will be updated later
                });

                // Add deserialized actions if available
                if let Some(actions) = actions_json {
                    node_json["actions"] = json!(actions);
                }

                // Add deserialized events if available
                if let Some(events) = events_json {
                    node_json["events"] = json!(events);
                }

                nodes.push(node_json);

                // Store parents for later edge creation
            }
        }
        Ok(())
    };

    // First, try the Delta column (new storage location)
    let delta_cf_name = Column::Delta.as_str();
    if let Some(delta_cf) = db.cf_handle(delta_cf_name) {
        let iter = db.iterator_cf(&delta_cf, IteratorMode::Start);
        process_deltas(iter)?;
    }

    // Also check Generic column (backwards compatibility)
    let generic_cf_name = Column::Generic.as_str();
    if let Some(generic_cf) = db.cf_handle(generic_cf_name) {
        let iter = db.iterator_cf(&generic_cf, IteratorMode::Start);
        process_deltas(iter)?;
    }

    // Add genesis nodes (one per context) for the all-zeros hash
    let genesis_hash = "0000000000000000000000000000000000000000000000000000000000000000";

    let mut sorted_context_ids: Vec<_> = context_dags.keys().cloned().collect();
    sorted_context_ids.sort_unstable();

    for context_id in sorted_context_ids {
        let genesis_node_id = format!("{context_id}:{genesis_hash}");

        // Add genesis node for this context
        nodes.push(json!({
            "id": genesis_node_id.clone(),
            "context_id": context_id,
            "delta_id": genesis_hash,
            "timestamp": 0_u64,
            "physical_time": 0_u64,
            "logical_counter": 0_u64,
            "hlc": "genesis",
            "actions_size": 0_u64,
            "applied": true,
            "parent_count": 0_u64,
            "parents": [],
            "is_dag_head": false,
            "is_genesis": true
        }));
    }

    // Build a set of all actual node IDs for validation
    let node_id_set: HashSet<String> = nodes
        .iter()
        .map(|n| n["id"].as_str().unwrap().to_owned())
        .collect();

    // Detect nodes with missing parents and mark them as detached
    for node in &mut nodes {
        let context_id = node["context_id"].as_str().unwrap();
        let parents = node["parents"].as_array().unwrap();

        if !parents.is_empty() {
            let mut has_missing = false;
            for parent in parents {
                let parent_id = parent.as_str().unwrap();
                let parent_node_id = format!("{context_id}:{parent_id}");

                if !node_id_set.contains(&parent_node_id) {
                    has_missing = true;
                    break;
                }
            }

            if has_missing {
                node["has_missing_parents"] = json!(true);
            }
        }
    }

    // Create edges only between nodes that actually exist
    for node in &nodes {
        let node_id = node["id"].as_str().unwrap();
        let context_id = node["context_id"].as_str().unwrap();
        let parents = node["parents"].as_array().unwrap();

        for parent in parents {
            let parent_id = parent.as_str().unwrap();
            let parent_node_id = format!("{context_id}:{parent_id}");

            // Only create edge if the parent node actually exists in our node set
            if node_id_set.contains(&parent_node_id) {
                edges.push(json!({
                    "source": parent_node_id,
                    "target": node_id,
                    "type": "parent"
                }));
            }
        }
    }

    // Find root nodes (nodes with no incoming edges from processed deltas)
    let mut targets: HashSet<String> = HashSet::new();
    for edge in &edges {
        if let Some(target) = edge.get("target").and_then(|v| v.as_str()) {
            let _ = targets.insert(target.to_owned());
        }
    }

    let mut sources: HashSet<String> = HashSet::new();
    for edge in &edges {
        if let Some(source) = edge.get("source").and_then(|v| v.as_str()) {
            let _ = sources.insert(source.to_owned());
        }
    }

    // Find root nodes (nodes with no incoming edges AND no missing parents)
    let root_nodes: Vec<String> = node_id_set
        .difference(&targets)
        .filter(|node_id| {
            nodes.iter().any(|n| {
                n["id"].as_str() == Some(node_id.as_str())
                    && !n["has_missing_parents"].as_bool().unwrap_or(false)
            })
        })
        .cloned()
        .collect();

    // Find detached nodes (nodes with missing parents)
    let detached_nodes: Vec<String> = nodes
        .iter()
        .filter(|n| n["has_missing_parents"].as_bool().unwrap_or(false))
        .map(|n| n["id"].as_str().unwrap().to_owned())
        .collect();

    // Find leaf nodes (nodes with no outgoing edges)
    let leaf_nodes: Vec<String> = targets.difference(&sources).cloned().collect();

    // Count contexts and add dag_heads info
    let context_count = context_dags.len();
    let contexts_summary: Vec<Value> = context_dags
        .iter()
        .map(|(ctx_id, deltas)| {
            let heads = context_heads.get(ctx_id).cloned().unwrap_or_default();
            json!({
                "context_id": ctx_id,
                "delta_count": deltas.len(),
                "dag_heads": heads,
                "dag_heads_count": heads.len()
            })
        })
        .collect();

    // Count how many nodes are dag_heads
    let dag_heads_count = nodes
        .iter()
        .filter(|n| n["is_dag_head"].as_bool().unwrap_or(false))
        .count();

    // Log deltas for deleted contexts (informational - these are intentionally kept!)
    if !orphaned_deltas.is_empty() {
        eprintln!(
            "INFO: Found {} deltas for deleted contexts",
            orphaned_deltas.len()
        );
        eprintln!("These deltas are intentionally kept in the database for distributed sync.");
        eprintln!("First 5 deltas for deleted contexts:");
        for (ctx_id, delta_id) in orphaned_deltas.iter().take(5) {
            let ctx_preview: String = ctx_id.chars().take(8).collect();
            let delta_preview: String = delta_id.chars().take(8).collect();
            eprintln!("  - Context: {ctx_preview}..., Delta: {delta_preview}...");
        }
    }

    Ok(json!({
        "type": "dag_export",
        "summary": {
            "total_nodes": nodes.len(),
            "total_edges": edges.len(),
            "context_count": context_count,
            "root_nodes": root_nodes.len(),
            "detached_nodes": detached_nodes.len(),
            "leaf_nodes": leaf_nodes.len(),
            "dag_heads_count": dag_heads_count,
            "deltas_for_deleted_contexts": orphaned_deltas.len()
        },
        "contexts": contexts_summary,
        "nodes": nodes,
        "edges": edges,
        "roots": root_nodes,
        "detached": detached_nodes,
        "leaves": leaf_nodes,
        "diagnostics": {
            "deltas_for_deleted_contexts": orphaned_deltas.iter().map(|(ctx, delta)| {
                json!({
                    "context_id": ctx,
                    "delta_id": delta,
                    "note": "Intentionally kept for distributed sync"
                })
            }).collect::<Vec<_>>()
        }
    }))
}
