use std::fs;
use std::path::Path;

use calimero_storage::collections::CrdtType;
use calimero_wasm_abi::schema::Manifest;
use eyre::Result;

/// Load state schema from a JSON value
///
/// The JSON value should be in the format produced by `calimero-abi state`:
/// ```json
/// {
///   "state_root": "TypeName",
///   "types": { ... }
/// }
/// ```
///
/// This creates a schema containing only the state root type and its dependencies,
/// which is sufficient for deserializing state.
pub fn load_state_schema_from_json_value(schema_value: &serde_json::Value) -> Result<Manifest> {
    // Extract state_root and types
    let state_root = schema_value
        .get("state_root")
        .and_then(|v| v.as_str())
        .ok_or_else(|| eyre::eyre!("State schema missing 'state_root' field"))?
        .to_string();

    let types_value = schema_value
        .get("types")
        .ok_or_else(|| eyre::eyre!("State schema missing 'types' field"))?;

    // Parse types into BTreeMap<String, TypeDef>
    use calimero_wasm_abi::schema::TypeDef;
    use std::collections::BTreeMap;
    let types: BTreeMap<String, TypeDef> = serde_json::from_value(types_value.clone())
        .map_err(|e| eyre::eyre!("Failed to parse types from state schema: {}", e))?;

    // Create a schema with just the state types (Manifest is used as the container type)
    let schema = Manifest {
        schema_version: "wasm-abi/1".to_string(),
        types,
        methods: Vec::new(),
        events: Vec::new(),
        state_root: Some(state_root),
    };

    Ok(schema)
}

/// Load state schema from a JSON file
///
/// The JSON file should be in the format produced by `calimero-abi state`:
/// ```json
/// {
///   "state_root": "TypeName",
///   "types": { ... }
/// }
/// ```
///
/// This creates a schema containing only the state root type and its dependencies,
/// which is sufficient for deserializing state.
pub fn load_state_schema_from_json(schema_path: &Path) -> Result<Manifest> {
    let schema_json = fs::read_to_string(schema_path)
        .map_err(|e| eyre::eyre!("Failed to read state schema file: {}", e))?;

    let schema_value: serde_json::Value = serde_json::from_str(&schema_json)
        .map_err(|e| eyre::eyre!("Failed to parse state schema JSON: {}", e))?;

    load_state_schema_from_json_value(&schema_value)
}

/// Infer state schema from database by reading field names and CRDT types from metadata
///
/// This function scans the State column for EntityIndex entries and builds a schema
/// based on field_name and crdt_type found in metadata. This enables schema-free
/// database inspection when field names are stored in metadata.
///
/// # Arguments
/// * `db` - The database to scan
/// * `context_id` - Optional context ID to filter by. If None, scans all contexts (may find fields from multiple contexts)
pub fn infer_schema_from_database(
    db: &rocksdb::DBWithThreadMode<rocksdb::SingleThreaded>,
    context_id: Option<&[u8]>,
) -> Result<Manifest> {
    use calimero_wasm_abi::schema::{
        CollectionType, CrdtCollectionType, Field, ScalarType, TypeDef, TypeRef,
    };
    use std::collections::BTreeMap;

    let state_cf = db
        .cf_handle("State")
        .ok_or_else(|| eyre::eyre!("State column family not found"))?;

    let mut fields = Vec::new();
    let mut seen_field_names = std::collections::HashSet::new();

    // Root ID depends on context:
    // - If context_id is provided, root ID is that context_id (Id::root() returns context_id())
    // - If no context_id, we can't determine root fields reliably, so use all zeros as fallback
    let root_id_bytes: [u8; 32] = match context_id {
        Some(ctx_id) => ctx_id.try_into().map_err(|_| {
            eyre::eyre!(
                "context_id must be exactly 32 bytes, got {} bytes",
                ctx_id.len()
            )
        })?,
        None => {
            eprintln!(
                "[WARNING] No context_id provided for schema inference. \
                Using [0; 32] as fallback root ID. This may produce incorrect or incomplete \
                schema if the database contains multiple contexts. Consider providing a \
                specific context_id for accurate schema inference."
            );
            [0u8; 32]
        }
    };

    // Scan State column for EntityIndex entries
    let iter = db.iterator_cf(&state_cf, rocksdb::IteratorMode::Start);
    for item in iter {
        let (key, value) = item?;

        // Filter by context_id if provided (key format: context_id (32 bytes) + state_key (32 bytes))
        if let Some(expected_context_id) = context_id {
            if key.len() < 32 || &key[..32] != expected_context_id {
                continue;
            }
        }

        // Try to deserialize as EntityIndex
        if let Ok(index) = borsh::from_slice::<crate::export::EntityIndex>(&value) {
            // Check if this is a root-level field (parent_id is None or equals root/context_id)
            let is_root_field = index.parent_id.is_none()
                || index
                    .parent_id
                    .as_ref()
                    .map(|id| id.as_bytes() == &root_id_bytes)
                    .unwrap_or(false);

            if is_root_field {
                // Check if we have field_name in metadata
                if let Some(ref field_name) = index.metadata.field_name {
                    if !seen_field_names.contains(field_name) {
                        seen_field_names.insert(field_name.clone());

                        // Infer type from crdt_type
                        let type_ref = if let Some(crdt_type) = index.metadata.crdt_type {
                            match crdt_type {
                                CrdtType::LwwRegister => TypeRef::Collection {
                                    collection: CollectionType::Record { fields: Vec::new() },
                                    crdt_type: Some(CrdtCollectionType::LwwRegister),
                                    inner_type: Some(Box::new(TypeRef::string())),
                                },
                                CrdtType::GCounter | CrdtType::PnCounter => TypeRef::Collection {
                                    // Counters are stored as Map<ExecutorId, u64> internally
                                    collection: CollectionType::Map {
                                        key: Box::new(TypeRef::string()),
                                        value: Box::new(TypeRef::Scalar(ScalarType::U64)),
                                    },
                                    crdt_type: Some(CrdtCollectionType::Counter),
                                    inner_type: None,
                                },
                                CrdtType::Rga => TypeRef::Collection {
                                    collection: CollectionType::Record { fields: Vec::new() },
                                    crdt_type: Some(CrdtCollectionType::ReplicatedGrowableArray),
                                    inner_type: None,
                                },
                                CrdtType::UnorderedMap => {
                                    // Default to Map<String, String> - can be refined later
                                    TypeRef::Collection {
                                        collection: CollectionType::Map {
                                            key: Box::new(TypeRef::string()),
                                            value: Box::new(TypeRef::string()),
                                        },
                                        crdt_type: Some(CrdtCollectionType::UnorderedMap),
                                        inner_type: None,
                                    }
                                }
                                CrdtType::UnorderedSet => TypeRef::Collection {
                                    collection: CollectionType::List {
                                        items: Box::new(TypeRef::string()),
                                    },
                                    crdt_type: Some(CrdtCollectionType::UnorderedSet),
                                    inner_type: None,
                                },
                                CrdtType::Vector => TypeRef::Collection {
                                    collection: CollectionType::List {
                                        items: Box::new(TypeRef::string()),
                                    },
                                    crdt_type: Some(CrdtCollectionType::Vector),
                                    inner_type: None,
                                },
                                CrdtType::UserStorage => TypeRef::Collection {
                                    collection: CollectionType::Map {
                                        key: Box::new(TypeRef::string()),
                                        value: Box::new(TypeRef::string()),
                                    },
                                    crdt_type: Some(CrdtCollectionType::UnorderedMap),
                                    inner_type: None,
                                },
                                CrdtType::FrozenStorage => TypeRef::Collection {
                                    collection: CollectionType::Map {
                                        key: Box::new(TypeRef::string()),
                                        value: Box::new(TypeRef::string()),
                                    },
                                    crdt_type: Some(CrdtCollectionType::UnorderedMap),
                                    inner_type: None,
                                },
                                CrdtType::Custom(_) => {
                                    // Custom type - can't infer without schema
                                    TypeRef::Collection {
                                        collection: CollectionType::Record { fields: Vec::new() },
                                        crdt_type: None,
                                        inner_type: None,
                                    }
                                }
                                // Handle future CRDT types
                                _ => TypeRef::Collection {
                                    collection: CollectionType::Record { fields: Vec::new() },
                                    crdt_type: None,
                                    inner_type: None,
                                },
                            }
                        } else {
                            // No CRDT type - default to LWW register
                            TypeRef::Collection {
                                collection: CollectionType::Record { fields: Vec::new() },
                                crdt_type: Some(CrdtCollectionType::LwwRegister),
                                inner_type: Some(Box::new(TypeRef::string())),
                            }
                        };

                        fields.push(Field {
                            name: field_name.clone(),
                            type_: type_ref,
                            nullable: None,
                        });
                    }
                }
            }
        }
    }

    // Create a record type with all inferred fields
    let state_root_type = "InferredStateRoot".to_string();
    let mut types = BTreeMap::new();
    types.insert(
        state_root_type.clone(),
        TypeDef::Record {
            fields: fields.clone(),
        },
    );

    Ok(Manifest {
        schema_version: "wasm-abi/1".to_string(),
        types,
        methods: Vec::new(),
        events: Vec::new(),
        state_root: Some(state_root_type),
    })
}
