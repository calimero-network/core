pub mod cli;

use borsh::BorshDeserialize;
use calimero_store::types::ContextDagDelta as StoreContextDagDelta;
use calimero_wasm_abi::schema::{
    CollectionType, CrdtCollectionType, Field, Manifest, ScalarType, TypeDef, TypeRef,
};
use core::convert::TryFrom;
use core::ops::Deref;
use eyre::{Result, WrapErr};
use rocksdb::{DBWithThreadMode, IteratorMode, SingleThreaded};
use serde_json::{json, Value};
use std::io::{Cursor, Read};

use crate::deserializer;
use crate::types::{parse_key, parse_value, Column};

/// Export data from RocksDB to JSON
pub fn export_data(
    db: &DBWithThreadMode<SingleThreaded>,
    columns: &[Column],
    manifest: &Manifest,
) -> Result<Value> {
    let mut data = serde_json::Map::new();

    for column in columns {
        let cf_name = column.as_str();
        let cf = db
            .cf_handle(cf_name)
            .ok_or_else(|| eyre::eyre!("Column family '{cf_name}' not found"))?;

        let mut entries = Vec::new();
        let iter = db.iterator_cf(&cf, IteratorMode::Start);

        for item in iter {
            let (key, value) = item
                .wrap_err_with(|| format!("Failed to read entry from column family '{cf_name}'"))?;

            let key_json = parse_key(*column, &key)
                .wrap_err_with(|| format!("Failed to parse key in column '{cf_name}'"))?;

            let value_json = parse_value_with_abi(*column, &value, manifest, Some((db, &key)))
                .wrap_err_with(|| format!("Failed to parse value in column '{cf_name}'"))?;

            entries.push(json!({
                "key": key_json,
                "value": value_json
            }));
        }

        drop(data.insert(
            cf_name.to_owned(),
            json!({
                "count": entries.len(),
                "entries": entries
            }),
        ));
    }

    Ok(json!({
        "database": "Calimero RocksDB Export",
        "exported_columns": columns.iter().map(Column::as_str).collect::<Vec<_>>(),
        "data": data
    }))
}

fn delta_hlc_snapshot(delta: &StoreContextDagDelta) -> (u64, Value) {
    let timestamp = delta.hlc.inner();
    let raw_time = timestamp.get_time().as_u64();
    let id_hex = format!("{:032x}", u128::from(*timestamp.get_id()));
    let physical_seconds = (raw_time >> 32_i32) as u32;
    let logical_counter = (raw_time & 0xF) as u32;

    let hlc_json = json!({
        "raw": delta.hlc.to_string(),
        "time_ntp64": raw_time,
        "physical_time_secs": physical_seconds,
        "logical_counter": logical_counter,
        "id_hex": id_hex,
    });

    (raw_time, hlc_json)
}

#[derive(Clone)]
struct MapField {
    name: String,
    key_type: TypeRef,
    value_type: TypeRef,
}

/// Try to decode a collection entry by looking up the actual entry data from an EntityIndex
/// Supports Map entries (Entry<(K, V)>) and List entries (Entry<T>)
fn try_decode_collection_entry_from_index(
    index: &EntityIndex,
    db: &DBWithThreadMode<SingleThreaded>,
    current_key: &[u8],
    manifest: &Manifest,
) -> Option<Value> {
    eprintln!(
        "[try_decode_collection_entry_from_index] Starting lookup for index id: {}",
        hex::encode(index.id.as_bytes())
    );

    // Extract context_id from current key (first 32 bytes)
    if current_key.len() < 32 {
        eprintln!(
            "[try_decode_collection_entry_from_index] Current key too short: {} bytes",
            current_key.len()
        );
        return None;
    }
    let context_id = &current_key[..32];
    eprintln!(
        "[try_decode_collection_entry_from_index] Context ID: {}",
        hex::encode(context_id)
    );

    // Construct the key for the entry: context_id (32 bytes) + entry_id (32 bytes)
    let mut entry_key = Vec::with_capacity(64);
    entry_key.extend_from_slice(context_id);
    entry_key.extend_from_slice(index.id.as_bytes());
    eprintln!(
        "[try_decode_collection_entry_from_index] Constructed entry key: {}",
        hex::encode(&entry_key)
    );

    // Look up the entry data in State column
    let state_cf = match db.cf_handle("State") {
        Some(cf) => cf,
        None => {
            eprintln!("[try_decode_collection_entry_from_index] State column family not found");
            return None;
        }
    };

    let entry_bytes = match db.get_cf(&state_cf, &entry_key) {
        Ok(Some(bytes)) => {
            eprintln!(
                "[try_decode_collection_entry_from_index] Found entry data: {} bytes",
                bytes.len()
            );
            bytes
        }
        Ok(None) => {
            eprintln!("[try_decode_collection_entry_from_index] Entry not found in State column");
            return None;
        }
        Err(e) => {
            eprintln!(
                "[try_decode_collection_entry_from_index] Error looking up entry: {}",
                e
            );
            return None;
        }
    };

    // Get state root fields to find matching collection types
    let root_name = manifest.state_root.as_ref()?;
    eprintln!(
        "[try_decode_collection_entry_from_index] State root: {}",
        root_name
    );

    let TypeDef::Record {
        fields: record_fields,
    } = manifest.types.get(root_name)?
    else {
        eprintln!(
            "[try_decode_collection_entry_from_index] State root type not found or not a record"
        );
        return None;
    };

    eprintln!(
        "[try_decode_collection_entry_from_index] Found {} fields in state root",
        record_fields.len()
    );

    // If we have a parent_id, try to find the collection field that matches it
    // Otherwise, try all collection fields
    let fields_to_try: Vec<&Field> = if let Some(parent_id) = &index.parent_id {
        eprintln!(
            "[try_decode_collection_entry_from_index] Has parent_id: {}",
            hex::encode(parent_id.as_bytes())
        );
        // Try to find the collection field that has this parent_id
        // We can do this by looking up the parent's state_key and checking which field it belongs to
        let mut parent_key = Vec::with_capacity(64);
        parent_key.extend_from_slice(context_id);
        parent_key.extend_from_slice(parent_id.as_bytes());

        // Try to find which field the parent belongs to by checking if it's a collection
        let fields: Vec<&Field> = record_fields
            .iter()
            .filter(|field| matches!(&field.type_, TypeRef::Collection { .. }))
            .collect();
        eprintln!(
            "[try_decode_collection_entry_from_index] Found {} collection fields to try",
            fields.len()
        );
        fields
    } else {
        eprintln!(
            "[try_decode_collection_entry_from_index] No parent_id, trying all collection fields"
        );
        // No parent_id, try all collection fields
        let fields: Vec<&Field> = record_fields
            .iter()
            .filter(|field| matches!(&field.type_, TypeRef::Collection { .. }))
            .collect();
        eprintln!(
            "[try_decode_collection_entry_from_index] Found {} collection fields to try",
            fields.len()
        );
        fields
    };

    // Try to decode as Map entry (Entry<(K, V)>)
    for field in fields_to_try.iter() {
        if let TypeRef::Collection {
            collection: CollectionType::Map { key, value },
            ..
        } = &field.type_
        {
            eprintln!(
                "[try_decode_collection_entry_from_index] Trying Map entry (field: {})",
                field.name
            );
            let map_field = MapField {
                name: field.name.clone(),
                key_type: (**key).clone(),
                value_type: (**value).clone(),
            };

            match decode_map_entry(&entry_bytes, &map_field, manifest) {
                Ok(decoded) => {
                    eprintln!("[try_decode_collection_entry_from_index] Successfully decoded as Map entry");
                    return add_index_metadata(decoded, index);
                }
                Err(e) => {
                    eprintln!("[try_decode_collection_entry_from_index] Failed to decode as Map entry: {}", e);
                }
            }
        }
    }

    // Try to decode as List entry (Entry<T>) - for Vector, UnorderedSet, etc.
    for field in fields_to_try.iter() {
        if let TypeRef::Collection {
            collection: CollectionType::List { items },
            ..
        } = &field.type_
        {
            eprintln!(
                "[try_decode_collection_entry_from_index] Trying List entry (field: {})",
                field.name
            );
            // For Vector, UnorderedSet, and other list-based collections
            match decode_list_entry(&entry_bytes, field, items, manifest) {
                Ok(decoded) => {
                    eprintln!("[try_decode_collection_entry_from_index] Successfully decoded as List entry");
                    return add_index_metadata(decoded, index);
                }
                Err(e) => {
                    eprintln!("[try_decode_collection_entry_from_index] Failed to decode as List entry: {}", e);
                }
            }
        }
    }

    // Try to decode as Record entry (Entry<T>) - for Counter, ReplicatedGrowableArray, etc.
    for field in fields_to_try.iter() {
        if let TypeRef::Collection {
            collection: CollectionType::Record { .. },
            crdt_type,
            inner_type,
        } = &field.type_
        {
            eprintln!("[try_decode_collection_entry_from_index] Trying Record entry (field: {}, crdt_type: {:?})", field.name, crdt_type);
            // For Counter, ReplicatedGrowableArray, and other record-based CRDTs
            match decode_record_entry(&entry_bytes, field, crdt_type, inner_type, manifest) {
                Ok(decoded) => {
                    eprintln!("[try_decode_collection_entry_from_index] Successfully decoded as Record entry");
                    return add_index_metadata(decoded, index);
                }
                Err(e) => {
                    eprintln!("[try_decode_collection_entry_from_index] Failed to decode as Record entry: {}", e);
                }
            }
        }
    }

    eprintln!("[try_decode_collection_entry_from_index] All decode attempts failed");
    None
}

/// Decode a list entry (Entry<T>) where T is the item type
fn decode_list_entry(
    bytes: &[u8],
    field: &Field,
    item_type: &TypeRef,
    manifest: &Manifest,
) -> Result<Value> {
    let mut cursor = Cursor::new(bytes);

    // Deserialize the item value
    let item_value =
        deserializer::deserialize_type_ref_from_cursor(&mut cursor, item_type, manifest)
            .wrap_err_with(|| format!("Failed to deserialize list item (type: {:?})", item_type))?;
    let item_end = usize::try_from(cursor.position()).unwrap_or(bytes.len());
    let item_raw = bytes[..item_end].to_vec();

    // Deserialize Element (which contains id)
    // Element serializes as just id: Id (32 bytes)
    let element_id = if cursor.position() + 32 <= bytes.len() as u64 {
        let mut id_bytes = [0_u8; 32];
        if cursor.read_exact(&mut id_bytes).is_ok() {
            Some(hex::encode(id_bytes))
        } else {
            None
        }
    } else {
        None
    };

    Ok(json!({
        "type": "Entry",
        "field": field.name.clone(),
        "element": {
            "id": element_id
        },
        "item": {
            "parsed": item_value,
            "raw": hex::encode(&item_raw),
            "type": type_ref_label(item_type)
        }
    }))
}

/// Decode a record entry (Entry<T>) where T is a CRDT type like Counter or ReplicatedGrowableArray
fn decode_record_entry(
    bytes: &[u8],
    field: &Field,
    crdt_type: &Option<CrdtCollectionType>,
    inner_type: &Option<Box<TypeRef>>,
    manifest: &Manifest,
) -> Result<Value> {
    use std::io::Cursor;
    use std::io::Read;

    let mut cursor = Cursor::new(bytes);

    // Deserialize the CRDT value based on its type
    // Use the proper CRDT-aware deserializer by constructing the TypeRef
    let crdt_value = if let Some(crdt) = crdt_type {
        // Construct TypeRef that will trigger CRDT-aware deserialization
        let type_ref = TypeRef::Collection {
            collection: CollectionType::Record { fields: vec![] },
            crdt_type: Some(crdt.clone()),
            inner_type: inner_type.clone(),
        };

        // Use the deserializer which will properly handle CRDT types (Counter, RGA, etc.)
        match deserializer::deserialize_type_ref_from_cursor(&mut cursor, &type_ref, manifest) {
            Ok(parsed) => parsed,
            Err(e) => {
                // If deserialization fails, return error info
                json!({
                    "raw": hex::encode(bytes),
                    "crdt_type": format!("{:?}", crdt),
                    "error": format!("Failed to deserialize: {}", e)
                })
            }
        }
    } else {
        // No CRDT type specified, try to deserialize using inner_type
        if let Some(inner) = inner_type {
            let value =
                deserializer::deserialize_type_ref_from_cursor(&mut cursor, inner, manifest)
                    .wrap_err("Failed to deserialize record entry")?;
            json!(value)
        } else {
            // No type info, return raw bytes
            let remaining_bytes = bytes.len() - 32; // Reserve 32 bytes for Element ID
            let mut record_bytes = vec![0_u8; remaining_bytes.min(bytes.len())];
            cursor
                .read_exact(&mut record_bytes)
                .wrap_err("Failed to read record bytes")?;
            json!({
                "raw": hex::encode(&record_bytes),
                "note": "Record entry without CRDT type or inner type"
            })
        }
    };

    // Deserialize Element ID (last 32 bytes)
    let element_id = if cursor.position() + 32 <= bytes.len() as u64 {
        let mut id_bytes = [0_u8; 32];
        if cursor.read_exact(&mut id_bytes).is_ok() {
            Some(hex::encode(id_bytes))
        } else {
            None
        }
    } else {
        None
    };

    Ok(json!({
        "type": "Entry",
        "field": field.name.clone(),
        "element": {
            "id": element_id
        },
        "value": crdt_value
    }))
}

/// Add EntityIndex metadata to a decoded entry
fn add_index_metadata(mut entry_json: Value, index: &EntityIndex) -> Option<Value> {
    if let Some(obj) = entry_json.as_object_mut() {
        obj.insert(
            "index".to_string(),
            json!({
                "id": hex::encode(index.id.as_bytes()),
                "parent_id": index.parent_id.as_ref().map(|id| hex::encode(id.as_bytes())),
                "full_hash": hex::encode(index.full_hash),
                "own_hash": hex::encode(index.own_hash),
                "created_at": index.metadata.created_at,
                "updated_at": *index.metadata.updated_at,
                "deleted_at": index.deleted_at
            }),
        );
    }
    Some(entry_json)
}

fn type_ref_label(type_ref: &TypeRef) -> String {
    match type_ref {
        TypeRef::Reference { ref_ } => ref_.clone(),
        TypeRef::Scalar(s) => format!("scalar::{s:?}"),
        TypeRef::Collection {
            collection,
            crdt_type,
            inner_type,
        } => {
            let crdt_str = if let Some(crdt) = crdt_type {
                format!(" (CRDT: {crdt:?})")
            } else {
                String::new()
            };
            let inner_str = if let Some(inner) = inner_type {
                format!(" (inner: {inner:?})")
            } else {
                String::new()
            };
            format!("collection::{collection:?}{crdt_str}{inner_str}")
        }
    }
}

fn decode_map_entry(bytes: &[u8], field: &MapField, manifest: &Manifest) -> Result<Value> {
    // Entry<T> where T = (K, V) serializes as:
    // - item: (K, V) - the tuple itself
    // - storage: Element - metadata (ID, timestamps, etc.)
    // So we need to deserialize: (K, V, Element)

    eprintln!(
        "[decode_map_entry] Starting decode for field: {}, bytes length: {}",
        field.name,
        bytes.len()
    );
    eprintln!(
        "[decode_map_entry] First 128 bytes: {}",
        hex::encode(&bytes[..bytes.len().min(128)])
    );

    // Quick format check: For String keys, verify it looks like a Borsh-serialized string
    // Borsh strings start with u32 length. If the first 4 bytes don't look like a reasonable length,
    // or if the first 32 bytes look like a raw ID, this is probably not an Entry<(K, V)>.
    if let TypeRef::Scalar(ScalarType::String) = field.key_type {
        if bytes.len() < 4 {
            return Err(eyre::eyre!(
                "Entry too short to contain a Borsh-serialized string key"
            ));
        }
        let length_bytes = [bytes[0], bytes[1], bytes[2], bytes[3]];
        let key_length = u32::from_le_bytes(length_bytes) as usize;

        // Sanity check: String length should be reasonable (< 1MB) and the entry should be long enough
        if key_length > 1_000_000 || bytes.len() < 4 + key_length {
            eprintln!("[decode_map_entry] First 4 bytes don't look like a valid string length: {} (u32: {})", 
                hex::encode(&length_bytes), key_length);
            return Err(eyre::eyre!(
                "Entry doesn't appear to be Entry<(String, V)> format (invalid string length: {})",
                key_length
            ));
        }

        // Additional check: If the first 32 bytes look like a raw ID (all non-zero, no obvious string pattern),
        // this is probably not an Entry<(K, V)>
        if bytes.len() >= 32 {
            let first_32 = &bytes[..32];
            // Check if it looks like a raw ID (32 bytes, mostly non-zero, not starting with a small u32)
            if first_32.iter().all(|&b| b != 0)
                && u32::from_le_bytes([first_32[0], first_32[1], first_32[2], first_32[3]]) > 1000
            {
                eprintln!("[decode_map_entry] First 32 bytes look like a raw ID, not a Borsh-serialized string");
                return Err(eyre::eyre!(
                    "Entry doesn't appear to be Entry<(String, V)> format (looks like raw ID)"
                ));
            }
        }
    }

    let mut cursor = Cursor::new(bytes);

    // Deserialize the tuple (K, V)
    // For Borsh, a tuple (K, V) serializes as: K (serialized) + V (serialized)
    // For a String key, Borsh serializes as: u32 length + bytes
    eprintln!(
        "[decode_map_entry] Attempting to deserialize key (type: {:?})",
        field.key_type
    );
    let key_value =
        deserializer::deserialize_type_ref_from_cursor(&mut cursor, &field.key_type, manifest)
            .wrap_err_with(|| {
                format!(
                    "Failed to deserialize key (type: {:?}, first 32 bytes: {})",
                    field.key_type,
                    hex::encode(&bytes[..bytes.len().min(32)])
                )
            })?;
    eprintln!(
        "[decode_map_entry] Successfully deserialized key: {:?}",
        key_value
    );
    let key_end = usize::try_from(cursor.position()).unwrap_or(bytes.len());
    let key_raw = bytes[..key_end].to_vec();

    let value_value =
        deserializer::deserialize_type_ref_from_cursor(&mut cursor, &field.value_type, manifest)
            .wrap_err_with(|| {
                format!(
                    "Failed to deserialize value (type: {:?}, remaining bytes: {})",
                    field.value_type,
                    bytes.len() - key_end
                )
            })?;
    let value_end = usize::try_from(cursor.position()).unwrap_or(bytes.len());
    let value_raw = bytes[key_end..value_end].to_vec();

    // Now deserialize Element (which contains id, timestamps, etc.)
    // Element serializes as: (id: Option<Id>, parent_id: Option<Id>, children: Option<Vec<ChildInfo>>, full_hash: [u8; 32], own_hash: [u8; 32], metadata: Metadata, deleted_at: Option<u64>)
    // For simplicity, we'll just try to read the ID (first 32 bytes if Some, or 1 byte if None)
    let element_id = if let Ok(id) = borsh::from_slice::<Id>(&bytes[value_end..value_end + 32]) {
        Some(hex::encode(id.as_bytes()))
    } else {
        None
    };

    Ok(json!({
        "type": "Entry",
        "field": field.name.clone(),
        "element": {
            "id": element_id
        },
        "key": {
            "parsed": key_value,
            "raw": hex::encode(&key_raw),
            "type": type_ref_label(&field.key_type)
        },
        "value": {
            "parsed": value_value,
            "raw": hex::encode(&value_raw),
            "type": type_ref_label(&field.value_type)
        }
    }))
}

fn decode_state_entry(
    bytes: &[u8],
    manifest: &Manifest,
    db_and_key: Option<(&DBWithThreadMode<SingleThreaded>, &[u8])>,
) -> Option<Value> {
    eprintln!(
        "[decode_state_entry] Attempting to decode {} bytes",
        bytes.len()
    );

    // Try to decode as EntityIndex first (these are smaller, metadata-only)
    if let Ok(index) = borsh::from_slice::<EntityIndex>(bytes) {
        eprintln!(
            "[decode_state_entry] Successfully decoded as EntityIndex, id: {}",
            hex::encode(index.id.as_bytes())
        );

        // Check if this EntityIndex is for a collection entry and try to look up the actual entry data
        if let Some((db, current_key)) = db_and_key {
            eprintln!("[decode_state_entry] Attempting to lookup entry data from EntityIndex");
            if let Some(entry_data) =
                try_decode_collection_entry_from_index(&index, db, current_key, manifest)
            {
                eprintln!("[decode_state_entry] Successfully decoded entry data from EntityIndex");
                return Some(entry_data);
            } else {
                eprintln!("[decode_state_entry] Failed to decode entry data from EntityIndex, returning EntityIndex metadata");
            }
        }

        return Some(json!({
            "type": "EntityIndex",
            "id": hex::encode(index.id.as_bytes()),
            "parent_id": index.parent_id.map(|id| hex::encode(id.as_bytes())),
            "children_count": index.children.as_ref().map_or(0, Vec::len),
            "full_hash": hex::encode(index.full_hash),
            "own_hash": hex::encode(index.own_hash),
            "created_at": index.metadata.created_at,
            "updated_at": *index.metadata.updated_at,
            "deleted_at": index.deleted_at
        }));
    } else {
        eprintln!("[decode_state_entry] Not an EntityIndex (deserialization failed)");
    }

    // Check if it's just a raw ID (32 bytes)
    if bytes.len() == 32 {
        if let Ok(id) = borsh::from_slice::<Id>(bytes) {
            eprintln!(
                "[decode_state_entry] Decoded as RawId: {}",
                hex::encode(id.as_bytes())
            );
            return Some(json!({
                "type": "RawId",
                "id": hex::encode(id.as_bytes()),
                "note": "Direct ID storage (possibly root collection reference or internal metadata)"
            }));
        }
    }

    // Get all fields from the state root
    let root_name = manifest.state_root.as_ref()?;
    eprintln!("[decode_state_entry] State root type: {}", root_name);
    let Some(TypeDef::Record {
        fields: record_fields,
    }) = manifest.types.get(root_name)
    else {
        eprintln!(
            "[decode_state_entry] State root type '{}' not found in manifest types",
            root_name
        );
        return None;
    };

    eprintln!(
        "[decode_state_entry] Found {} fields in state root",
        record_fields.len()
    );

    // Try to decode as map entry (Entry<(K, V)>)
    // This handles cases where the bytes directly contain Entry data (not EntityIndex)
    // Try ALL map fields, not just the first one that matches
    let mut map_fields_tried = 0;
    for field in record_fields.iter() {
        if let TypeRef::Collection {
            collection: CollectionType::Map { key, value },
            ..
        } = &field.type_
        {
            map_fields_tried += 1;
            eprintln!("[decode_state_entry] Trying to decode as Map entry (field: {}, key_type: {:?}, value_type: {:?})", 
                field.name, key, value);

            let map_field = MapField {
                name: field.name.clone(),
                key_type: (**key).clone(),
                value_type: (**value).clone(),
            };

            // Try to decode - if it succeeds, return immediately
            match decode_map_entry(bytes, &map_field, manifest) {
                Ok(decoded) => {
                    eprintln!(
                        "[decode_state_entry] Successfully decoded as Map entry (field: {})",
                        field.name
                    );
                    return Some(decoded);
                }
                Err(e) => {
                    eprintln!(
                        "[decode_state_entry] Failed to decode as Map entry (field: {}): {}",
                        field.name, e
                    );
                }
            }
            // If it fails, continue trying other fields
        }
    }

    if map_fields_tried > 0 {
        eprintln!(
            "[decode_state_entry] Tried {} map fields, all failed",
            map_fields_tried
        );
    }

    // Try to decode as List entries (Vector, UnorderedSet) - Entry<T>
    let mut list_fields_tried = 0;
    for field in record_fields.iter() {
        if let TypeRef::Collection {
            collection: CollectionType::List { items },
            ..
        } = &field.type_
        {
            list_fields_tried += 1;
            eprintln!(
                "[decode_state_entry] Trying to decode as List entry (field: {}, item_type: {:?})",
                field.name, items
            );

            match decode_list_entry(bytes, field, items, manifest) {
                Ok(decoded) => {
                    eprintln!(
                        "[decode_state_entry] Successfully decoded as List entry (field: {})",
                        field.name
                    );
                    return Some(decoded);
                }
                Err(e) => {
                    eprintln!(
                        "[decode_state_entry] Failed to decode as List entry (field: {}): {}",
                        field.name, e
                    );
                }
            }
        }
    }

    if list_fields_tried > 0 {
        eprintln!(
            "[decode_state_entry] Tried {} list fields, all failed",
            list_fields_tried
        );
    }

    // Try to decode as Record entries (Counter, ReplicatedGrowableArray) - Entry<T>
    let mut record_fields_tried = 0;
    for field in record_fields.iter() {
        if let TypeRef::Collection {
            collection: CollectionType::Record { .. },
            crdt_type,
            inner_type,
        } = &field.type_
        {
            record_fields_tried += 1;
            eprintln!("[decode_state_entry] Trying to decode as Record entry (field: {}, crdt_type: {:?}, inner_type: {:?})", 
                field.name, crdt_type, inner_type);

            // For RGA, individual entries are (CharKey, RgaChar) tuples, not full RGA structures
            if crdt_type.as_ref() == Some(&CrdtCollectionType::ReplicatedGrowableArray) {
                use std::io::Cursor;
                let mut cursor = Cursor::new(bytes);

                // Try to decode as (CharKey, RgaChar) tuple
                if let (Ok(time), Ok(id), Ok(seq)) = (
                    u64::deserialize_reader(&mut cursor),
                    u128::deserialize_reader(&mut cursor),
                    u32::deserialize_reader(&mut cursor),
                ) {
                    let char_id = deserializer::CharIdData { time, id, seq };

                    // Deserialize RgaChar
                    if let (Ok(content), Ok(left_time), Ok(left_id), Ok(left_seq)) = (
                        u32::deserialize_reader(&mut cursor),
                        u64::deserialize_reader(&mut cursor),
                        u128::deserialize_reader(&mut cursor),
                        u32::deserialize_reader(&mut cursor),
                    ) {
                        let left = deserializer::CharIdData {
                            time: left_time,
                            id: left_id,
                            seq: left_seq,
                        };
                        let left_str = format!("{}#{}", left.time, left.seq);
                        let _rga_char = deserializer::RgaCharData { content, left };
                        let char_value = char::from_u32(content).unwrap_or('\u{FFFD}');

                        eprintln!("[decode_state_entry] Successfully decoded as RGA character entry (field: {})", field.name);
                        return Some(json!({
                            "type": "Entry",
                            "field": field.name,
                            "crdt_type": "ReplicatedGrowableArray",
                            "value": {
                                "type": "RgaChar",
                                "char_id": format!("{}#{}", char_id.time, char_id.seq),
                                "char": char_value,
                                "content": content,
                                "left": left_str,
                            }
                        }));
                    }
                }

                // If deserialization fails, try the regular decode_record_entry as fallback
                eprintln!("[decode_state_entry] Failed to decode as RGA character tuple, trying full RGA deserialization");
            }

            match decode_record_entry(bytes, field, crdt_type, inner_type, manifest) {
                Ok(decoded) => {
                    eprintln!(
                        "[decode_state_entry] Successfully decoded as Record entry (field: {})",
                        field.name
                    );
                    return Some(decoded);
                }
                Err(e) => {
                    eprintln!(
                        "[decode_state_entry] Failed to decode as Record entry (field: {}): {}",
                        field.name, e
                    );
                }
            }
        }
    }

    if record_fields_tried > 0 {
        eprintln!(
            "[decode_state_entry] Tried {} record fields, all failed",
            record_fields_tried
        );
    }

    // Try to decode as scalar entry (Entry<T> where T is a scalar/enum/reference)
    for field in record_fields.iter() {
        // Skip collection fields (already tried above)
        if matches!(&field.type_, TypeRef::Collection { .. }) {
            continue;
        }

        if let Ok(decoded) = decode_scalar_entry(bytes, field, manifest) {
            return Some(decoded);
        }
    }

    // Final fallback: try to deserialize directly as each field's type
    // This handles cases where the value is a raw CRDT collection (not wrapped in Entry)
    for field in record_fields {
        // Try to deserialize directly as the field's type (without Entry wrapper)
        let mut cursor = Cursor::new(bytes);
        match deserializer::deserialize_type_ref_from_cursor(&mut cursor, &field.type_, manifest) {
            Ok(parsed) => {
                // Check if we consumed all bytes (indicates successful deserialization)
                if cursor.position() == bytes.len() as u64 {
                    return Some(json!({
                        "type": "RawCrdtValue",
                        "field": field.name.clone(),
                        "parsed": parsed,
                        "raw": hex::encode(bytes),
                        "size": bytes.len(),
                        "note": "Deserialized as raw CRDT value (not wrapped in Entry)"
                    }));
                }
                // Partial deserialization - might be an Entry, try next field
            }
            Err(_) => {
                // Deserialization failed for this field type, try next
            }
        }
    }

    // Last resort: if it's exactly 92 bytes, it's likely an EntityIndex that failed to deserialize
    // Try to extract what we can manually
    if bytes.len() == 92 {
        // EntityIndex structure: id (32) + parent_id (1 + 32?) + children (variable) + hashes (64) + metadata + deleted_at
        // For now, just show it as a potential EntityIndex
        return Some(json!({
            "type": "PotentialEntityIndex",
            "size": bytes.len(),
            "raw": hex::encode(bytes),
            "note": "92-byte entry that might be EntityIndex but failed to deserialize. First 32 bytes (ID): ".to_owned() + &hex::encode(&bytes[..32.min(bytes.len())])
        }));
    }

    // If we have db access, try one more time with a more lenient EntityIndex deserialization
    // Some EntityIndex structures might have slightly different formats
    if db_and_key.is_some() && (bytes.len() >= 32 && bytes.len() <= 200) {
        // Try to extract at least the ID from the first 32 bytes
        if bytes.len() >= 32 {
            let potential_id = &bytes[..32];
            if let Ok(id) = borsh::from_slice::<Id>(potential_id) {
                return Some(json!({
                    "type": "PartialEntityIndex",
                    "id": hex::encode(id.as_bytes()),
                    "size": bytes.len(),
                    "raw": hex::encode(bytes),
                    "note": "Extracted ID from entry, but full EntityIndex deserialization failed"
                }));
            }
        }
    }

    None
}

fn decode_scalar_entry(bytes: &[u8], field: &Field, manifest: &Manifest) -> Result<Value> {
    let mut cursor = Cursor::new(bytes);

    // Deserialize the value (not a tuple, just the value itself)
    let value_parsed =
        deserializer::deserialize_type_ref_from_cursor(&mut cursor, &field.type_, manifest)?;
    let value_end = usize::try_from(cursor.position()).unwrap_or(bytes.len());
    let value_raw = bytes[..value_end].to_vec();

    // Read the Element ID
    let mut element_id = [0_u8; 32];
    cursor
        .read_exact(&mut element_id)
        .wrap_err("Failed to read entry element id")?;

    if cursor.position() != bytes.len() as u64 {
        eyre::bail!("Entry bytes not fully consumed");
    }

    Ok(json!({
        "type": "ScalarEntry",
        "field": field.name.clone(),
        "element": {
            "id": hex::encode(element_id)
        },
        "value": {
            "parsed": value_parsed,
            "raw": String::from_utf8_lossy(&value_raw),
            "type": type_ref_label(&field.type_)
        }
    }))
}

// EntityIndex structure for decoding
#[derive(borsh::BorshDeserialize)]
struct EntityIndex {
    id: Id,
    parent_id: Option<Id>,
    children: Option<Vec<ChildInfo>>,
    full_hash: [u8; 32],
    own_hash: [u8; 32],
    metadata: Metadata,
    deleted_at: Option<u64>,
}

#[derive(borsh::BorshDeserialize)]
struct Id {
    bytes: [u8; 32],
}

impl Id {
    const fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }
}

#[derive(borsh::BorshDeserialize)]
#[expect(
    dead_code,
    reason = "Fields required for Borsh deserialization structure"
)]
struct ChildInfo {
    id: Id,
    merkle_hash: [u8; 32],
    metadata: Metadata,
}

#[derive(borsh::BorshDeserialize)]
struct Metadata {
    created_at: u64,
    updated_at: UpdatedAt,
}

#[derive(borsh::BorshDeserialize)]
struct UpdatedAt(u64);

impl Deref for UpdatedAt {
    type Target = u64;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Parse a value with the state schema present
///
/// `db_and_key` is optional and used for looking up entry data when decoding EntityIndex structures.
pub fn parse_value_with_abi(
    column: Column,
    value: &[u8],
    manifest: &Manifest,
    db_and_key: Option<(&DBWithThreadMode<SingleThreaded>, &[u8])>,
) -> Result<Value> {
    match column {
        Column::State => {
            if let Some(decoded) = decode_state_entry(value, manifest, db_and_key) {
                return Ok(decoded);
            }

            Ok(json!({
                "raw": String::from_utf8_lossy(value),
                "size": value.len(),
                "note": "Unable to decode with ABI"
            }))
        }
        Column::Generic => {
            if let Ok(delta) = StoreContextDagDelta::try_from_slice(value) {
                if let Some(root) = manifest.state_root.as_ref() {
                    if let Ok(parsed) =
                        deserializer::deserialize_with_abi(&delta.actions, manifest, root)
                    {
                        let (timestamp_raw, hlc_json) = delta_hlc_snapshot(&delta);
                        return Ok(json!({
                            "type": "context_dag_delta",
                            "delta_id": hex::encode(delta.delta_id),
                            "parents": delta.parents.iter().map(hex::encode).collect::<Vec<_>>(),
                            "actions": {
                                "parsed": parsed,
                                "raw": String::from_utf8_lossy(&delta.actions)
                            },
                            "timestamp": timestamp_raw,
                            "hlc": hlc_json,
                            "applied": delta.applied,
                            "expected_root_hash": hex::encode(delta.expected_root_hash)
                        }));
                    }
                }

                let (timestamp_raw, hlc_json) = delta_hlc_snapshot(&delta);
                return Ok(json!({
                    "type": "context_dag_delta",
                    "delta_id": hex::encode(delta.delta_id),
                    "parents": delta.parents.iter().map(hex::encode).collect::<Vec<_>>(),
                    "actions": {
                        "raw": String::from_utf8_lossy(&delta.actions),
                        "note": "Unable to decode actions with ABI"
                    },
                    "timestamp": timestamp_raw,
                    "hlc": hlc_json,
                    "applied": delta.applied,
                    "expected_root_hash": hex::encode(delta.expected_root_hash)
                }));
            }

            // Fall back to parse_value which properly handles Generic column entries
            parse_value(column, value)
        }
        _ => {
            // For other columns, use default parsing
            parse_value(column, value)
        }
    }
}

/// List all available contexts without building their trees
/// This is a lightweight operation that only reads the Meta column
#[cfg(feature = "gui")]
pub fn list_contexts(db: &DBWithThreadMode<SingleThreaded>) -> Result<Vec<Value>> {
    let meta_cf = db
        .cf_handle("Meta")
        .ok_or_else(|| eyre::eyre!("Meta column family not found"))?;

    let mut contexts = Vec::new();
    let iter = db.iterator_cf(&meta_cf, IteratorMode::Start);

    for item in iter {
        let (key, value) = item.wrap_err("Failed to read Meta entry")?;

        let key_json = parse_key(Column::Meta, &key)?;
        let context_id = key_json
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| eyre::eyre!("Failed to extract context_id from Meta key"))?;

        let value_json = parse_value(Column::Meta, &value)?;
        let root_hash_str = value_json
            .get("root_hash")
            .and_then(|v| v.as_str())
            .ok_or_else(|| eyre::eyre!("Failed to extract root_hash from Meta value"))?;

        contexts.push(json!({
            "context_id": context_id,
            "root_hash": root_hash_str
        }));
    }

    Ok(contexts)
}

/// Extract state tree for a specific context
/// This builds the tree on-demand for only the requested context
#[cfg(feature = "gui")]
pub fn extract_context_tree(
    db: &DBWithThreadMode<SingleThreaded>,
    context_id_hex: &str,
    manifest: &Manifest,
) -> Result<Value> {
    let state_cf = db
        .cf_handle("State")
        .ok_or_else(|| eyre::eyre!("State column family not found"))?;

    let context_id_bytes =
        hex::decode(context_id_hex).wrap_err("Failed to decode context_id from hex")?;

    if context_id_bytes.len() != 32 {
        return Err(eyre::eyre!(
            "Invalid context_id length: expected 32 bytes, got {}",
            context_id_bytes.len()
        ));
    }

    // Get the root_hash from ContextMeta
    let root_hash = get_root_hash_from_meta(db, &context_id_bytes)?;

    let tree =
        find_and_build_tree_for_context(db, state_cf, &context_id_bytes, root_hash, manifest)?;

    Ok(json!({
        "context_id": context_id_hex,
        "root_hash": hex::encode(root_hash),
        "tree": tree
    }))
}

/// Get the root_hash from ContextMeta for a given context
#[cfg(feature = "gui")]
fn get_root_hash_from_meta(
    db: &DBWithThreadMode<SingleThreaded>,
    context_id: &[u8],
) -> Result<[u8; 32]> {
    use crate::types::{parse_key, parse_value, Column};

    let meta_cf = db
        .cf_handle("Meta")
        .ok_or_else(|| eyre::eyre!("Meta column family not found"))?;

    // ContextMeta key is just the context_id (32 bytes)
    let key = context_id;

    let value = db
        .get_cf(meta_cf, key)
        .wrap_err("Failed to query Meta column")?
        .ok_or_else(|| {
            eyre::eyre!(
                "ContextMeta not found for context {}",
                hex::encode(context_id)
            )
        })?;

    let value_json =
        parse_value(Column::Meta, &value).wrap_err("Failed to parse ContextMeta value")?;

    let root_hash_str = value_json
        .get("root_hash")
        .and_then(|v| v.as_str())
        .ok_or_else(|| eyre::eyre!("Failed to extract root_hash from ContextMeta"))?;

    let root_hash_bytes =
        hex::decode(root_hash_str).wrap_err("Failed to decode root_hash from hex")?;

    if root_hash_bytes.len() != 32 {
        return Err(eyre::eyre!(
            "Invalid root_hash length: expected 32 bytes, got {}",
            root_hash_bytes.len()
        ));
    }

    let mut root_hash = [0_u8; 32];
    root_hash.copy_from_slice(&root_hash_bytes);
    Ok(root_hash)
}

/// Find the root node for a context and build the tree using schema-driven traversal
/// Uses root_hash from ContextMeta to directly find the root state node
/// Implements BFS traversal: start at root, follow schema structure, use parent_id relationships
#[cfg(feature = "gui")]
fn find_and_build_tree_for_context(
    db: &DBWithThreadMode<SingleThreaded>,
    state_cf: &rocksdb::ColumnFamily,
    context_id: &[u8],
    root_hash: [u8; 32],
    manifest: &Manifest,
) -> Result<Value> {
    use std::collections::HashMap;

    eprintln!(
        "[find_and_build_tree_for_context] START: Looking for root node with root_hash={}",
        hex::encode(root_hash)
    );
    eprintln!(
        "[find_and_build_tree_for_context] Context ID: {}",
        hex::encode(context_id)
    );

    // Scan State column family to find the root EntityIndex by matching full_hash to root_hash
    // The root_hash from ContextMeta is the full_hash of the root EntityIndex
    // EntityIndex entries are stored in the State column, but we need to scan to find them
    // since the key format might not match our expectations

    let mut root_state_key: Option<String> = None;
    let mut root_index: Option<EntityIndex> = None;
    let iter = db.iterator_cf(state_cf, IteratorMode::Start);
    let mut scanned_count = 0;
    let mut context_entries = 0;
    let mut entity_index_entries = 0;

    for item in iter {
        let (key, value) = item.wrap_err("Failed to read State entry")?;
        scanned_count += 1;

        // Check if this key belongs to our context (first 32 bytes match context_id)
        if key.len() == 64 && &key[0..32] == context_id {
            context_entries += 1;

            // Try to decode as EntityIndex
            // EntityIndex starts with an Option<Id> for parent_id, so first byte should be 0 or 1
            // If it's not, this is probably state data, not an EntityIndex
            if value.len() > 0 && (value[0] == 0 || value[0] == 1) {
                // Try to decode as EntityIndex - use a more lenient approach
                // EntityIndex structure: Option<Id> (parent_id), Option<Vec<ChildInfo>> (children), [u8;32] (full_hash), [u8;32] (own_hash), Metadata, Option<u64> (deleted_at)
                // The first field is parent_id: Option<Id>
                // For root, parent_id should be None (0)
                // But we need to check if the structure matches EntityIndex
                match borsh::from_slice::<EntityIndex>(&value) {
                    Ok(index) => {
                        entity_index_entries += 1;

                        // Check if this node's full_hash matches the root_hash from ContextMeta
                        if index.full_hash == root_hash {
                            if root_state_key.is_some() {
                                return Err(eyre::eyre!(
                                    "Multiple nodes with root_hash found for context {}. This indicates data corruption.",
                                    hex::encode(context_id)
                                ));
                            }
                            let state_key = hex::encode(&key[32..64]);
                            eprintln!("[find_and_build_tree_for_context] Found root EntityIndex: state_key={}, id={}, full_hash={}, scanned {} entries (context_entries={}, entity_index_entries={})", 
                                state_key, hex::encode(index.id.as_bytes()), hex::encode(root_hash), scanned_count, context_entries, entity_index_entries);
                            root_state_key = Some(state_key);
                            root_index = Some(index);
                            break; // Found root, stop scanning
                        }
                    }
                    Err(e) => {
                        // Not an EntityIndex, skip silently
                        // Only log if we haven't found many EntityIndex entries yet
                        if entity_index_entries < 5 && scanned_count % 20 == 0 {
                            eprintln!("[find_and_build_tree_for_context] Failed to decode as EntityIndex (entry {}): {}", scanned_count, e);
                        }
                    }
                }
            }
        }
    }

    eprintln!("[find_and_build_tree_for_context] Scan complete: total_scanned={}, context_entries={}, entity_index_entries={}, root_found={}", 
        scanned_count, context_entries, entity_index_entries, root_index.is_some());

    let (root_index, root_state_key) = root_index
        .zip(root_state_key)
        .ok_or_else(|| {
            eprintln!("[find_and_build_tree_for_context] ERROR: Root EntityIndex not found! Scanned {} entries, context_entries={}, entity_index_entries={}", 
                scanned_count, context_entries, entity_index_entries);

            eyre::eyre!(
                "Root EntityIndex not found for context {}. root_hash={}. Scanned {} entries, found {} context entries, {} EntityIndex entries.",
                hex::encode(context_id),
                hex::encode(root_hash),
                scanned_count,
                context_entries,
                entity_index_entries
            )
        })?;

    // We found the root node, decode it using BFS traversal following the schema
    let root_idx = root_index;
    eprintln!("[find_and_build_tree_for_context] About to call decode_state_root_bfs with root_state_key={}", root_state_key);

    // Build element_id -> state_key mapping lazily as we traverse
    // We'll build it on-demand when we need to look up children
    let mut element_to_state: HashMap<String, String> = HashMap::new();

    // Store root mapping
    let root_element_id = hex::encode(root_idx.id.as_bytes());
    element_to_state.insert(root_element_id.clone(), root_state_key.clone());

    // Decode the root state using BFS traversal
    let result = decode_state_root_bfs(
        db,
        state_cf,
        context_id,
        &root_idx,
        &root_state_key,
        manifest,
        &mut element_to_state,
    );
    eprintln!(
        "[find_and_build_tree_for_context] decode_state_root_bfs returned: {:?}",
        result
            .as_ref()
            .map(|_| "Ok")
            .map_err(|e| format!("Err: {}", e))
    );
    result
}

/// Decode the state root using BFS traversal following the schema structure
#[cfg(feature = "gui")]
fn decode_state_root_bfs(
    db: &DBWithThreadMode<SingleThreaded>,
    state_cf: &rocksdb::ColumnFamily,
    context_id: &[u8],
    root_index: &EntityIndex,
    root_state_key: &str,
    manifest: &Manifest,
    element_to_state: &mut std::collections::HashMap<String, String>,
) -> Result<Value> {
    eprintln!(
        "[decode_state_root_bfs] START: Decoding state root, root_state_key={}",
        root_state_key
    );

    let root_name = manifest
        .state_root
        .as_ref()
        .ok_or_else(|| eyre::eyre!("No state_root defined in manifest"))?;

    eprintln!(
        "[decode_state_root_bfs] State root type name: {}",
        root_name
    );

    let TypeDef::Record { fields } = manifest
        .types
        .get(root_name)
        .ok_or_else(|| eyre::eyre!("State root type '{}' not found in manifest", root_name))?
    else {
        return Err(eyre::eyre!(
            "State root type '{}' is not a Record",
            root_name
        ));
    };

    eprintln!(
        "[decode_state_root_bfs] Found {} fields in state root type",
        fields.len()
    );

    let root_element_id = hex::encode(root_index.id.as_bytes());
    let mut state_fields = serde_json::Map::new();

    // Build element_id -> state_key mapping for root's children
    let root_children = root_index
        .children
        .as_ref()
        .map(|c| c.iter().collect::<Vec<_>>())
        .unwrap_or_default();
    eprintln!(
        "[decode_state_root_bfs] Root has {} children",
        root_children.len()
    );

    for child_info in &root_children {
        let child_element_id = hex::encode(child_info.id.as_bytes());
        // Find state key for this child by scanning State column family
        // The state key is the last 32 bytes of the RocksDB key
        let mut found = false;
        for item in db.iterator_cf(state_cf, rocksdb::IteratorMode::Start) {
            if let Ok((key_bytes, value_bytes)) = item {
                if key_bytes.len() == 64 && &key_bytes[0..32] == context_id {
                    if let Ok(child_index) = borsh::from_slice::<EntityIndex>(&value_bytes) {
                        if child_index.id.as_bytes() == child_info.id.as_bytes() {
                            let state_key = hex::encode(&key_bytes[32..64]);
                            element_to_state.insert(child_element_id.clone(), state_key);
                            eprintln!(
                                "[decode_state_root_bfs] Mapped child {} to state key",
                                child_element_id
                            );
                            found = true;
                            break;
                        }
                    }
                }
            }
        }
        if !found {
            eprintln!(
                "[decode_state_root_bfs] Warning: Could not find state key for child {}",
                child_element_id
            );
        }
    }

    eprintln!(
        "[decode_state_root_bfs] Processing {} fields from state root schema",
        fields.len()
    );

    // For each field in the state root schema, find and decode its children using BFS
    // Match children to fields by iterating through root's children
    let mut used_children = std::collections::HashSet::new();
    for field in fields {
        eprintln!("[decode_state_root_bfs] Decoding field: {}", field.name);

        // For collection fields, try to find a matching child from root's children list
        let field_value = if matches!(&field.type_, TypeRef::Collection { .. }) {
            // Find an unused child that is a collection root
            let mut matched_child = None;
            for child_info in &root_children {
                let child_element_id = hex::encode(child_info.id.as_bytes());
                if used_children.contains(&child_element_id) {
                    continue;
                }

                // Check if this child is a collection root by loading its EntityIndex
                if let Some(state_key) = element_to_state.get(&child_element_id) {
                    let child_key_bytes = hex::decode(state_key).wrap_err_with(|| {
                        format!("Failed to decode child_state_key: {}", state_key)
                    })?;
                    let mut child_key = Vec::with_capacity(64);
                    child_key.extend_from_slice(context_id);
                    child_key.extend_from_slice(&child_key_bytes);

                    if let Ok(Some(child_value)) = db.get_cf(state_cf, &child_key) {
                        if let Ok(child_index) = borsh::from_slice::<EntityIndex>(&child_value) {
                            // This is a collection root - it matches this collection field
                            matched_child = Some((state_key.clone(), child_index));
                            used_children.insert(child_element_id);
                            break;
                        }
                    }
                }
            }

            if let Some((collection_root_key, collection_root_index)) = matched_child {
                // Decode this collection field using the found collection root
                decode_collection_field_with_root(
                    db,
                    state_cf,
                    context_id,
                    field,
                    &field.type_,
                    &root_element_id,
                    &collection_root_key,
                    &collection_root_index,
                    manifest,
                    element_to_state,
                )?
            } else {
                // No matching child found - return empty collection
                json!({
                    "field": field.name,
                    "type": "collection",
                    "children": [],
                    "count": 0,
                    "note": "Collection root not found"
                })
            }
        } else {
            // Non-collection field - decode directly (these are stored in the root itself)
            json!({
                "field": field.name,
                "type": "scalar_or_record",
                "value": null,
                "children": [],
                "note": "Non-collection fields are stored in the state root itself"
            })
        };

        eprintln!(
            "[decode_state_root_bfs] Field {} decoded successfully",
            field.name
        );
        state_fields.insert(field.name.clone(), field_value);
    }

    let state_fields_count = state_fields.len();
    eprintln!("[decode_state_root_bfs] Decoded {} fields, converting to children array (schema has {} fields)", state_fields_count, fields.len());

    // If we didn't decode any fields but the schema has fields, something went wrong
    // Create placeholder children for all schema fields
    if state_fields.is_empty() && !fields.is_empty() {
        eprintln!("[decode_state_root_bfs] ERROR: No fields were decoded but schema has {} fields! Creating placeholder children.", fields.len());
        for field in fields {
            state_fields.insert(
                field.name.clone(),
                json!({
                    "field": field.name,
                    "type": "unknown",
                    "children": [],
                    "note": "Field decoding failed or field not found"
                }),
            );
        }
    }

    // Convert fields to children array for D3 hierarchy
    let mut children = Vec::new();
    for (field_name, field_value) in state_fields {
        // Each field becomes a child node
        // If the field_value has children (collections), extract them to be direct children
        let (field_data_without_children, field_children) =
            if let Some(field_obj) = field_value.as_object() {
                // Check if this field has a "children" array (from collections)
                if let Some(children_array) = field_obj.get("children").and_then(|v| v.as_array()) {
                    // Extract children and create a new field object without nested children
                    let mut field_data = field_obj.clone();
                    field_data.remove("children");
                    (json!(field_data), Some(children_array.clone()))
                } else {
                    (field_value.clone(), None)
                }
            } else {
                (field_value.clone(), None)
            };

        let mut field_obj = json!({
            "id": format!("{}_{}", root_element_id, field_name),
            "type": "Field",
            "field": field_name,
            "data": field_data_without_children,
            "parent_id": root_element_id,
        });

        // If we extracted children, add them as direct children of the field node
        if let Some(field_children_array) = field_children {
            if let Some(field_obj_map) = field_obj.as_object_mut() {
                field_obj_map.insert("children".to_string(), json!(field_children_array));
            }
        }

        children.push(field_obj);
    }

    eprintln!(
        "[decode_state_root_bfs] Created {} children for root node",
        children.len()
    );

    // Debug: Log the structure of the first child if it exists
    if !children.is_empty() {
        eprintln!(
            "[decode_state_root_bfs] First child structure: {:?}",
            serde_json::to_string(&children[0])
        );
    } else {
        eprintln!(
            "[decode_state_root_bfs] WARNING: No children created! state_fields_count = {}",
            state_fields_count
        );
        eprintln!(
            "[decode_state_root_bfs] Root children count: {}",
            root_children.len()
        );
        eprintln!(
            "[decode_state_root_bfs] Schema fields count: {}",
            fields.len()
        );
    }

    Ok(json!({
        "id": root_element_id,
        "type": "StateRoot",
        "name": "Root",
        "children": children,
        "metadata": {
            "parent_id": root_index.parent_id.as_ref().map(|id| hex::encode(id.as_bytes())),
            "full_hash": hex::encode(root_index.full_hash),
            "own_hash": hex::encode(root_index.own_hash),
            "created_at": root_index.metadata.created_at,
            "updated_at": *root_index.metadata.updated_at,
            "deleted_at": root_index.deleted_at,
        }
    }))
}

/// Decode a field using BFS: find children by parent_id, decode according to schema
#[cfg(feature = "gui")]
fn decode_field_bfs(
    db: &DBWithThreadMode<SingleThreaded>,
    state_cf: &rocksdb::ColumnFamily,
    context_id: &[u8],
    field: &Field,
    field_type: &TypeRef,
    parent_element_id: &str,
    manifest: &Manifest,
    element_to_state: &mut std::collections::HashMap<String, String>,
) -> Result<Value> {
    match field_type {
        TypeRef::Collection {
            collection,
            crdt_type,
            inner_type,
        } => {
            // Find collection root child by parent_id == parent_element_id
            let collection_root = find_child_by_parent_id(
                db,
                state_cf,
                context_id,
                parent_element_id,
                element_to_state,
            )?;

            if let Some((root_state_key, root_index)) = collection_root {
                // For RGA, we need special handling: collect all entries and reconstruct text directly
                if crdt_type.as_ref() == Some(&CrdtCollectionType::ReplicatedGrowableArray) {
                    // Collect all RGA entries (CharKey, RgaChar) pairs with their element IDs
                    let rga_entries = collect_rga_entries(
                        db,
                        state_cf,
                        context_id,
                        &root_index,
                        element_to_state,
                    )?;

                    // rga_entries is already the reconstructed JSON value
                    let rga_value = rga_entries;

                    // For RGA, we don't have individual entry children (the text is reconstructed)
                    // But we still need to return a structure that can be displayed
                    Ok(json!({
                        "field": field.name,
                        "type": format!("{:?}", collection),
                        "crdt_type": "ReplicatedGrowableArray",
                        "collection_root": root_state_key,
                        "value": rga_value,
                        "children": [], // RGA doesn't have individual entry children in the tree
                    }))
                } else {
                    // For other collection types, decode entries individually
                    let entries = decode_collection_entries_bfs(
                        db,
                        state_cf,
                        context_id,
                        &root_index,
                        &root_state_key,
                        field,
                        collection,
                        crdt_type,
                        inner_type,
                        manifest,
                        element_to_state,
                    )?;

                    // Convert entries to children for D3 hierarchy
                    let entries_count = entries.len();
                    let mut entry_children = Vec::new();
                    for entry in &entries {
                        if let Some(entry_obj) = entry.as_object() {
                            let entry_data = entry_obj.get("entry").cloned().unwrap_or(json!(null));
                            let state_key = entry_obj
                                .get("state_key")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");

                            entry_children.push(json!({
                                "id": state_key,
                                "type": "Entry",
                                "field": field.name,
                                "data": entry_data,
                                "parent_id": format!("{}_{}", parent_element_id, field.name),
                            }));
                        }
                    }

                    Ok(json!({
                        "field": field.name,
                        "type": format!("{:?}", collection),
                        "crdt_type": crdt_type.as_ref().map(|c| format!("{:?}", c)),
                        "collection_root": root_state_key,
                        "count": entries_count,
                        "children": entry_children,
                        "entries": entries, // Keep original entries for detailed view
                    }))
                }
            } else {
                // No collection root found
                Ok(json!({
                    "field": field.name,
                    "type": format!("{:?}", collection),
                    "crdt_type": crdt_type.as_ref().map(|c| format!("{:?}", c)),
                    "entries": [],
                    "count": 0,
                    "note": "Collection root not found"
                }))
            }
        }
        _ => {
            // For non-collection fields (scalars, records, etc.), they're stored directly in the state root
            Ok(json!({
                "field": field.name,
                "type": "scalar_or_record",
                "note": "Non-collection fields are stored in the state root itself, not as separate entries"
            }))
        }
    }
}

/// Find a child node by parent_id (BFS: follow parent_id relationships)
#[cfg(feature = "gui")]
fn find_child_by_parent_id(
    db: &DBWithThreadMode<SingleThreaded>,
    state_cf: &rocksdb::ColumnFamily,
    context_id: &[u8],
    parent_element_id: &str,
    element_to_state: &mut std::collections::HashMap<String, String>,
) -> Result<Option<(String, EntityIndex)>> {
    use rocksdb::IteratorMode;

    // If we haven't built the mapping yet, scan for this parent's children
    let parent_id_bytes =
        hex::decode(parent_element_id).wrap_err("Failed to decode parent_element_id")?;
    if parent_id_bytes.len() != 32 {
        return Err(eyre::eyre!(
            "Invalid parent_id length: expected 32 bytes, got {}",
            parent_id_bytes.len()
        ));
    }
    let mut parent_id_array = [0u8; 32];
    parent_id_array.copy_from_slice(&parent_id_bytes);
    let parent_id = Id {
        bytes: parent_id_array,
    };

    let iter = db.iterator_cf(state_cf, IteratorMode::Start);

    for item in iter {
        let (key, value) = item.wrap_err("Failed to read State entry")?;

        // Check if this key belongs to our context
        if key.len() == 64 && &key[0..32] == context_id {
            if let Ok(index) = borsh::from_slice::<EntityIndex>(&value) {
                // Check if this node's parent_id matches
                if let Some(ref node_parent_id) = index.parent_id {
                    if node_parent_id.bytes == parent_id.bytes {
                        let element_id = hex::encode(index.id.as_bytes());
                        let state_key = hex::encode(&key[32..64]);
                        element_to_state.insert(element_id.clone(), state_key.clone());
                        return Ok(Some((state_key, index)));
                    }
                }
            }
        }
    }

    Ok(None)
}

/// Collect all RGA entries and reconstruct the text
#[cfg(feature = "gui")]
fn collect_rga_entries(
    db: &DBWithThreadMode<SingleThreaded>,
    state_cf: &rocksdb::ColumnFamily,
    context_id: &[u8],
    collection_root_index: &EntityIndex,
    element_to_state: &mut std::collections::HashMap<String, String>,
) -> Result<Value> {
    use std::io::Cursor;

    let collection_root_element_id = hex::encode(collection_root_index.id.as_bytes());
    let mut chars: Vec<(deserializer::CharIdData, deserializer::RgaCharData, String)> = Vec::new();

    // Find all children of collection root
    if let Some(children) = &collection_root_index.children {
        for child_info in children {
            let entry_element_id = hex::encode(child_info.id.as_bytes());

            // Get or find state key for this entry
            let entry_state_key = if let Some(key) = element_to_state.get(&entry_element_id) {
                key.clone()
            } else {
                // Find by parent_id
                if let Some((key, _)) = find_child_by_parent_id(
                    db,
                    state_cf,
                    context_id,
                    &collection_root_element_id,
                    element_to_state,
                )? {
                    key
                } else {
                    continue; // Entry not found
                }
            };

            // Get entry value (this is the (CharKey, RgaChar) tuple for RGA)
            let entry_key_bytes =
                hex::decode(&entry_state_key).wrap_err("Failed to decode entry_state_key")?;
            let mut entry_key = Vec::with_capacity(64);
            entry_key.extend_from_slice(context_id);
            entry_key.extend_from_slice(&entry_key_bytes);

            if let Some(entry_value) = db
                .get_cf(state_cf, &entry_key)
                .wrap_err("Failed to query entry")?
            {
                // Deserialize (CharKey, RgaChar) tuple
                let mut cursor = Cursor::new(&entry_value);

                // Deserialize CharKey (which is just CharId)
                let time = u64::deserialize_reader(&mut cursor)
                    .wrap_err("Failed to deserialize RGA CharId timestamp")?;
                let id = u128::deserialize_reader(&mut cursor)
                    .wrap_err("Failed to deserialize RGA CharId id")?;
                let seq = u32::deserialize_reader(&mut cursor)
                    .wrap_err("Failed to deserialize RGA CharId seq")?;

                let char_id = deserializer::CharIdData { time, id, seq };

                // Deserialize RgaChar
                let content = u32::deserialize_reader(&mut cursor)
                    .wrap_err("Failed to deserialize RGA character content")?;

                let left_time = u64::deserialize_reader(&mut cursor)
                    .wrap_err("Failed to deserialize RGA left timestamp")?;
                let left_id = u128::deserialize_reader(&mut cursor)
                    .wrap_err("Failed to deserialize RGA left id")?;
                let left_seq = u32::deserialize_reader(&mut cursor)
                    .wrap_err("Failed to deserialize RGA left seq")?;

                let left = deserializer::CharIdData {
                    time: left_time,
                    id: left_id,
                    seq: left_seq,
                };
                let rga_char = deserializer::RgaCharData { content, left };

                chars.push((char_id, rga_char, entry_element_id));
            }
        }
    }

    // Reconstruct text using the deserializer's logic
    let text = deserializer::reconstruct_rga_text(&chars);

    // Build entries map for detailed view
    let mut entries = serde_json::Map::new();
    for (char_id, rga_char, element_id) in &chars {
        let char_value = char::from_u32(rga_char.content).unwrap_or('\u{FFFD}');
        let char_id_str = format!("{}#{}", char_id.time, char_id.seq);
        drop(entries.insert(
            char_id_str,
            json!({
                "char": char_value,
                "content": rga_char.content,
                "left": format!("{}#{}", rga_char.left.time, rga_char.left.seq),
                "element_id": element_id
            }),
        ));
    }

    Ok(json!({
        "text": text,
        "length": text.chars().count(),
        "entries": entries,
        "crdt_type": "ReplicatedGrowableArray"
    }))
}

/// Decode a collection field when we already have the collection root
#[cfg(feature = "gui")]
fn decode_collection_field_with_root(
    db: &DBWithThreadMode<SingleThreaded>,
    state_cf: &rocksdb::ColumnFamily,
    context_id: &[u8],
    field: &Field,
    field_type: &TypeRef,
    parent_element_id: &str,
    collection_root_key: &str,
    collection_root_index: &EntityIndex,
    manifest: &Manifest,
    element_to_state: &mut std::collections::HashMap<String, String>,
) -> Result<Value> {
    let TypeRef::Collection {
        collection,
        crdt_type,
        inner_type,
    } = field_type
    else {
        return Err(eyre::eyre!(
            "Expected Collection type for field {}",
            field.name
        ));
    };

    // For RGA, we need special handling: collect all entries and reconstruct text directly
    if crdt_type.as_ref() == Some(&CrdtCollectionType::ReplicatedGrowableArray) {
        // Collect all RGA entries (CharKey, RgaChar) pairs with their element IDs
        let rga_entries = collect_rga_entries(
            db,
            state_cf,
            context_id,
            collection_root_index,
            element_to_state,
        )?;

        // rga_entries is already the reconstructed JSON value
        let rga_value = rga_entries;

        // For RGA, we don't have individual entry children (the text is reconstructed)
        // But we still need to return a structure that can be displayed
        Ok(json!({
            "field": field.name,
            "type": format!("{:?}", collection),
            "crdt_type": "ReplicatedGrowableArray",
            "collection_root": collection_root_key,
            "value": rga_value,
            "children": [], // RGA doesn't have individual entry children in the tree
        }))
    } else {
        // For other collection types, decode entries individually
        let entries = decode_collection_entries_bfs(
            db,
            state_cf,
            context_id,
            collection_root_index,
            collection_root_key,
            field,
            collection,
            crdt_type,
            inner_type,
            manifest,
            element_to_state,
        )?;

        // Convert entries to children for D3 hierarchy
        let entries_count = entries.len();
        let mut entry_children = Vec::new();
        for entry in &entries {
            if let Some(entry_obj) = entry.as_object() {
                let entry_data = entry_obj.get("entry").cloned().unwrap_or(json!(null));
                let state_key = entry_obj
                    .get("state_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");

                entry_children.push(json!({
                    "id": state_key,
                    "type": "Entry",
                    "field": field.name,
                    "data": entry_data,
                    "parent_id": format!("{}_{}", parent_element_id, field.name),
                }));
            }
        }

        Ok(json!({
            "field": field.name,
            "type": format!("{:?}", collection),
            "crdt_type": crdt_type.as_ref().map(|c| format!("{:?}", c)),
            "collection_root": collection_root_key,
            "count": entries_count,
            "children": entry_children,
            "entries": entries, // Keep original entries for detailed view
        }))
    }
}

#[cfg(feature = "gui")]
fn decode_collection_entries_bfs(
    db: &DBWithThreadMode<SingleThreaded>,
    state_cf: &rocksdb::ColumnFamily,
    context_id: &[u8],
    collection_root_index: &EntityIndex,
    collection_root_state_key: &str,
    field: &Field,
    collection: &CollectionType,
    crdt_type: &Option<CrdtCollectionType>,
    inner_type: &Option<Box<TypeRef>>,
    manifest: &Manifest,
    element_to_state: &mut std::collections::HashMap<String, String>,
) -> Result<Vec<Value>> {
    // RGA entries should never be decoded individually - they're handled in decode_field_bfs
    // If we get here for RGA, it means something went wrong, but we should still handle it gracefully
    if crdt_type.as_ref() == Some(&CrdtCollectionType::ReplicatedGrowableArray) {
        // For RGA, return a single entry representing the entire collection
        // The actual RGA reconstruction should have been done in decode_field_bfs
        return Ok(vec![json!({
            "state_key": collection_root_state_key,
            "entry": {
                "type": "ReplicatedGrowableArray",
                "note": "RGA entries are reconstructed as a complete collection, not individual entries"
            }
        })]);
    }

    let collection_root_element_id = hex::encode(collection_root_index.id.as_bytes());
    let mut entries = Vec::new();

    // Find all children of collection root (entries in the collection)
    // Use the children list from EntityIndex if available, otherwise scan by parent_id
    if let Some(children) = &collection_root_index.children {
        for child_info in children {
            let entry_element_id = hex::encode(child_info.id.as_bytes());

            // Get or find state key for this entry
            let entry_state_key = if let Some(key) = element_to_state.get(&entry_element_id) {
                key.clone()
            } else {
                // Find by parent_id
                if let Some((key, _)) = find_child_by_parent_id(
                    db,
                    state_cf,
                    context_id,
                    &collection_root_element_id,
                    element_to_state,
                )? {
                    key
                } else {
                    continue; // Entry not found
                }
            };

            // Get entry value
            let entry_key_bytes =
                hex::decode(&entry_state_key).wrap_err("Failed to decode entry_state_key")?;
            let mut entry_key = Vec::with_capacity(64);
            entry_key.extend_from_slice(context_id);
            entry_key.extend_from_slice(&entry_key_bytes);

            let entry_value = db
                .get_cf(state_cf, &entry_key)
                .wrap_err("Failed to query entry")?
                .ok_or_else(|| eyre::eyre!("Entry not found"))?;

            // Decode the entry according to collection type
            match decode_collection_entry(
                &entry_value,
                field,
                collection,
                crdt_type,
                inner_type,
                manifest,
                Some((db, &entry_key)),
            ) {
                Ok(entry) => {
                    entries.push(json!({
                        "state_key": entry_state_key,
                        "entry": entry
                    }));
                }
                Err(e) => {
                    eprintln!("Failed to decode entry {}: {}", entry_state_key, e);
                }
            }
        }
    }

    Ok(entries)
}

/// Decode a single state field using the schema
/// This tries to match children of the root to fields by attempting to decode each child as each field type
/// The children of the root are collection root nodes, not individual entries
#[cfg(feature = "gui")]
fn decode_state_field(
    db: &DBWithThreadMode<SingleThreaded>,
    state_cf: &rocksdb::ColumnFamily,
    context_id: &[u8],
    field: &Field,
    field_type: &TypeRef,
    parent_index: &EntityIndex,
    manifest: &Manifest,
    element_to_state: &std::collections::HashMap<String, String>,
    used_children: &mut std::collections::HashSet<String>, // Track which children we've already matched
) -> Result<Value> {
    match field_type {
        TypeRef::Collection {
            collection,
            crdt_type,
            inner_type,
        } => {
            // Try to find a child that matches this field's collection root
            // The children of the root are collection root nodes
            let children = parent_index
                .children
                .as_ref()
                .map(|children| children.iter().collect::<Vec<_>>())
                .unwrap_or_default();

            let mut collection_root_id: Option<String> = None;

            // Try to match a child to this field by checking if it's a collection root
            for child_info in children {
                let child_element_id = hex::encode(child_info.id.as_bytes());

                // Skip if this child was already matched to another field
                if used_children.contains(&child_element_id) {
                    continue;
                }

                // Get the state key for this child
                let Some(child_state_key) = element_to_state.get(&child_element_id) else {
                    continue;
                };

                let child_key_bytes = hex::decode(child_state_key).wrap_err_with(|| {
                    format!("Failed to decode child_state_key: {}", child_state_key)
                })?;

                let mut child_key = Vec::with_capacity(64);
                child_key.extend_from_slice(context_id);
                child_key.extend_from_slice(&child_key_bytes);

                let child_value = match db.get_cf(state_cf, &child_key) {
                    Ok(Some(value)) => value,
                    Ok(None) => continue, // Child not found, skip
                    Err(e) => {
                        eprintln!("Error querying child {}: {}", child_state_key, e);
                        continue;
                    }
                };

                // Check if this child is a collection root (EntityIndex with children)
                if let Ok(child_index) = borsh::from_slice::<EntityIndex>(&child_value) {
                    // This is a collection root node - it matches this collection field
                    collection_root_id = Some(child_element_id.clone());
                    used_children.insert(child_element_id);

                    // Now get all entries in this collection by traversing the collection root's children
                    let mut entries = Vec::new();
                    if let Some(collection_children) = &child_index.children {
                        for entry_child in collection_children {
                            let entry_element_id = hex::encode(entry_child.id.as_bytes());
                            let Some(entry_state_key) = element_to_state.get(&entry_element_id)
                            else {
                                continue;
                            };

                            let entry_key_bytes =
                                hex::decode(entry_state_key).wrap_err_with(|| {
                                    format!("Failed to decode entry_state_key: {}", entry_state_key)
                                })?;

                            let mut entry_key = Vec::with_capacity(64);
                            entry_key.extend_from_slice(context_id);
                            entry_key.extend_from_slice(&entry_key_bytes);

                            let entry_value = match db.get_cf(state_cf, &entry_key) {
                                Ok(Some(value)) => value,
                                Ok(None) => continue,
                                Err(e) => {
                                    eprintln!("Error querying entry {}: {}", entry_state_key, e);
                                    continue;
                                }
                            };

                            // Decode the entry based on collection type
                            match decode_collection_entry(
                                &entry_value,
                                field,
                                collection,
                                crdt_type,
                                inner_type,
                                manifest,
                                Some((db, &entry_key)),
                            ) {
                                Ok(entry) => {
                                    entries.push(json!({
                                        "state_key": entry_state_key,
                                        "entry": entry
                                    }));
                                }
                                Err(e) => {
                                    eprintln!("Failed to decode entry {}: {}", entry_state_key, e);
                                }
                            }
                        }
                    }

                    return Ok(json!({
                        "field": field.name,
                        "type": format!("{:?}", collection),
                        "crdt_type": crdt_type.as_ref().map(|c| format!("{:?}", c)),
                        "collection_root": child_state_key,
                        "entries": entries,
                        "count": entries.len()
                    }));
                }
            }

            // No matching collection root found for this field
            Ok(json!({
                "field": field.name,
                "type": format!("{:?}", collection),
                "crdt_type": crdt_type.as_ref().map(|c| format!("{:?}", c)),
                "entries": [],
                "count": 0,
                "note": "Collection root not found or already matched to another field"
            }))
        }
        _ => {
            // For non-collection fields (scalars, records, etc.), they're stored directly in the state root
            // We would need to decode the state root itself to get these fields
            // For now, return a placeholder
            Ok(json!({
                "field": field.name,
                "type": "scalar_or_record",
                "note": "Non-collection fields are stored in the state root itself, not as separate entries"
            }))
        }
    }
}

/// Decode a collection entry based on its type
#[cfg(feature = "gui")]
fn decode_collection_entry(
    bytes: &[u8],
    field: &Field,
    collection: &CollectionType,
    crdt_type: &Option<CrdtCollectionType>,
    inner_type: &Option<Box<TypeRef>>,
    manifest: &Manifest,
    db_and_key: Option<(&DBWithThreadMode<SingleThreaded>, &[u8])>,
) -> Result<Value> {
    // Try to decode as EntityIndex first
    if let Ok(index) = borsh::from_slice::<EntityIndex>(bytes) {
        // If it's an EntityIndex, look up the actual entry data
        if let Some((db, current_key)) = db_and_key {
            if let Some(entry_data) =
                try_decode_collection_entry_from_index(&index, db, current_key, manifest)
            {
                return Ok(entry_data);
            }
        }

        // Return EntityIndex metadata if we can't decode the entry
        return Ok(json!({
            "type": "EntityIndex",
            "id": hex::encode(index.id.as_bytes()),
            "parent_id": index.parent_id.map(|id| hex::encode(id.as_bytes())),
        }));
    }

    // Try to decode directly based on collection type
    match collection {
        CollectionType::Map { key, value } => {
            let map_field = MapField {
                name: field.name.clone(),
                key_type: (**key).clone(),
                value_type: (**value).clone(),
            };
            decode_map_entry(bytes, &map_field, manifest)
                .map_err(|e| eyre::eyre!("Failed to decode map entry: {}", e))
        }
        CollectionType::List { items } => decode_list_entry(bytes, field, items, manifest)
            .map_err(|e| eyre::eyre!("Failed to decode list entry: {}", e)),
        CollectionType::Record { .. } => {
            // For RGA, individual entries are (CharKey, RgaChar) tuples, not full RGA structures
            // They should be handled by collect_rga_entries, not decoded individually
            if crdt_type.as_ref() == Some(&CrdtCollectionType::ReplicatedGrowableArray) {
                // Try to decode as (CharKey, RgaChar) tuple
                use std::io::Cursor;
                let mut cursor = Cursor::new(bytes);

                // Deserialize CharKey (CharId)
                if let (Ok(time), Ok(id), Ok(seq)) = (
                    u64::deserialize_reader(&mut cursor),
                    u128::deserialize_reader(&mut cursor),
                    u32::deserialize_reader(&mut cursor),
                ) {
                    let char_id = deserializer::CharIdData { time, id, seq };

                    // Deserialize RgaChar
                    if let (Ok(content), Ok(left_time), Ok(left_id), Ok(left_seq)) = (
                        u32::deserialize_reader(&mut cursor),
                        u64::deserialize_reader(&mut cursor),
                        u128::deserialize_reader(&mut cursor),
                        u32::deserialize_reader(&mut cursor),
                    ) {
                        let left = deserializer::CharIdData {
                            time: left_time,
                            id: left_id,
                            seq: left_seq,
                        };
                        let left_str = format!("{}#{}", left.time, left.seq);
                        let _rga_char = deserializer::RgaCharData { content, left };
                        let char_value = char::from_u32(content).unwrap_or('\u{FFFD}');

                        return Ok(json!({
                            "type": "RgaChar",
                            "char_id": format!("{}#{}", char_id.time, char_id.seq),
                            "char": char_value,
                            "content": content,
                            "left": left_str,
                        }));
                    }
                }

                // If deserialization fails, return raw bytes
                Ok(json!({
                    "type": "RgaChar",
                    "raw": hex::encode(bytes),
                    "note": "Failed to deserialize RGA character entry"
                }))
            } else {
                decode_record_entry(bytes, field, crdt_type, inner_type, manifest)
                    .map_err(|e| eyre::eyre!("Failed to decode record entry: {}", e))
            }
        }
    }
}

/// Recursively build tree structure from a given root hash with cycle detection
#[cfg(feature = "gui")]
fn build_tree_from_root(
    db: &DBWithThreadMode<SingleThreaded>,
    state_cf: &rocksdb::ColumnFamily,
    context_id: &[u8],
    node_id: &str,
    manifest: &Manifest,
    element_to_state: &std::collections::HashMap<String, String>,
    element_to_data: &std::collections::HashMap<String, Value>,
    visited: &mut std::collections::HashSet<String>,
) -> Result<Value> {
    // Detect cycles: if we've already visited this node, return an error
    if !visited.insert(node_id.to_string()) {
        return Ok(json!({
            "id": node_id,
            "type": "cycle_detected",
            "error": format!("Circular reference detected: node {} references an ancestor", node_id)
        }));
    }

    // Decode the node_id (state key) from hex string
    let state_key = hex::decode(node_id).wrap_err("Failed to decode node_id from hex")?;

    // Construct composite key: context_id (32 bytes) + state_key (32 bytes) = 64 bytes
    let mut key = Vec::with_capacity(64);
    key.extend_from_slice(context_id);
    key.extend_from_slice(&state_key);
    let key_for_lookup = key.clone(); // Clone for use in decode_state_entry
    let value_bytes = db
        .get_cf(state_cf, &key)
        .wrap_err("Failed to query State column")?;

    let Some(value_bytes) = value_bytes else {
        // Remove from visited before returning to allow siblings to visit this node
        visited.remove(node_id);
        return Ok(json!({
            "id": node_id,
            "type": "missing",
            "note": "Node not found in State column"
        }));
    };

    // Build the result based on the node type
    let result = if let Ok(index) = borsh::from_slice::<EntityIndex>(&value_bytes) {
        let children_info: Vec<Value> = if let Some(children) = &index.children {
            let mut child_nodes = Vec::new();
            for child in children {
                // Convert child element_id to hex string
                let child_element_id = hex::encode(child.id.as_bytes());

                // Look up the state_key for this element_id
                if let Some(child_state_key) = element_to_state.get(&child_element_id) {
                    let child_tree = build_tree_from_root(
                        db,
                        state_cf,
                        context_id,
                        child_state_key,
                        manifest,
                        element_to_state,
                        element_to_data,
                        visited,
                    )?;
                    child_nodes.push(child_tree);
                } else {
                    // Child element_id not found in mapping - it might be a data entry
                    // Try to look up this child as a data entry directly using the element_id as state_key
                    match hex::decode(&child_element_id) {
                        Ok(child_state_key_bytes) if child_state_key_bytes.len() == 32 => {
                            let mut child_key = Vec::with_capacity(64);
                            child_key.extend_from_slice(context_id);
                            child_key.extend_from_slice(&child_state_key_bytes);

                            if let Ok(Some(child_value)) = db.get_cf(state_cf, &child_key) {
                                // Try to decode as data entry
                                if let Some(decoded) = decode_state_entry(
                                    &child_value,
                                    manifest,
                                    Some((db, &child_key)),
                                ) {
                                    child_nodes.push(json!({
                                        "id": child_element_id,
                                        "type": decoded.get("type").and_then(|v| v.as_str()).unwrap_or("DataEntry"),
                                        "data": decoded
                                    }));
                                    continue;
                                }
                            }

                            child_nodes.push(json!({
                                "id": child_element_id,
                                "type": "missing",
                                "note": "Child element_id not found in state mapping"
                            }));
                        }
                        Ok(_) => {
                            child_nodes.push(json!({
                                "id": child_element_id,
                                "type": "error",
                                "note": "Child element_id has invalid length (expected 32 bytes)"
                            }));
                        }
                        Err(e) => {
                            child_nodes.push(json!({
                                "id": child_element_id,
                                "type": "error",
                                "note": format!("Failed to decode child element_id: {}", e)
                            }));
                        }
                    }
                }
            }
            child_nodes
        } else {
            Vec::new()
        };

        // Look up data entry associated with this EntityIndex using O(1) HashMap lookup
        let element_id_hex = hex::encode(index.id.as_bytes());
        let associated_data = element_to_data.get(&element_id_hex).cloned();

        json!({
            "id": node_id,
            "type": "EntityIndex",
            "parent_id": index.parent_id.map(|id| hex::encode(id.as_bytes())),
            "full_hash": hex::encode(index.full_hash),
            "own_hash": hex::encode(index.own_hash),
            "created_at": index.metadata.created_at,
            "updated_at": *index.metadata.updated_at,
            "deleted_at": index.deleted_at,
            "children": children_info,
            "children_count": children_info.len(),
            "data": associated_data
        })
    } else if let Some(decoded) =
        decode_state_entry(&value_bytes, manifest, Some((db, &key_for_lookup)))
    {
        // Try to decode as data entry
        json!({
            "id": node_id,
            "type": decoded.get("type").and_then(|v| v.as_str()).unwrap_or("DataEntry"),
            "data": decoded
        })
    } else if let Some(root_name) = manifest.state_root.as_ref() {
        // Fallback: try to deserialize directly as the state root type
        // This handles cases where the value is a raw CRDT collection or state struct
        match deserializer::deserialize_with_abi(&value_bytes, manifest, root_name) {
            Ok(parsed) => {
                json!({
                    "id": node_id,
                    "type": "StateRoot",
                    "parsed": parsed,
                    "raw": hex::encode(&value_bytes),
                    "size": value_bytes.len()
                })
            }
            Err(_) => {
                // Final fallback for unknown format
                json!({
                    "id": node_id,
                    "type": "Unknown",
                    "size": value_bytes.len(),
                    "raw": hex::encode(&value_bytes)
                })
            }
        }
    } else {
        // Fallback for unknown format (no state root available)
        json!({
            "id": node_id,
            "type": "Unknown",
            "size": value_bytes.len(),
            "raw": hex::encode(&value_bytes)
        })
    };

    // Remove from visited after processing to allow siblings to visit this node
    // This ensures cycle detection works (nodes in current path) while allowing
    // the same node to appear in different branches of the tree
    visited.remove(node_id);

    Ok(result)
}

/// Export data without state schema
#[cfg(feature = "gui")]
pub fn export_data_without_abi(
    db: &DBWithThreadMode<SingleThreaded>,
    columns: &[Column],
) -> Result<Value> {
    let mut data = serde_json::Map::new();

    for column in columns {
        let cf_name = column.as_str();
        let cf = db
            .cf_handle(cf_name)
            .ok_or_else(|| eyre::eyre!("Column family '{cf_name}' not found"))?;

        let mut entries = Vec::new();
        let iter = db.iterator_cf(&cf, IteratorMode::Start);

        for item in iter {
            let (key, value) = item
                .wrap_err_with(|| format!("Failed to read entry from column family '{cf_name}'"))?;

            let key_json = parse_key(*column, &key)
                .wrap_err_with(|| format!("Failed to parse key in column '{cf_name}'"))?;

            // For Generic column, try to parse ContextDagDelta even without ABI
            let value_json = if *column == Column::Generic {
                if let Ok(delta) = StoreContextDagDelta::try_from_slice(&value) {
                    let (timestamp_raw, hlc_json) = delta_hlc_snapshot(&delta);
                    json!({
                        "type": "context_dag_delta",
                        "delta_id": hex::encode(delta.delta_id),
                        "parents": delta.parents.iter().map(hex::encode).collect::<Vec<_>>(),
                        "actions": {
                            "raw": String::from_utf8_lossy(&delta.actions),
                            "note": "Unable to decode actions without ABI"
                        },
                        "timestamp": timestamp_raw,
                        "hlc": hlc_json,
                        "applied": delta.applied,
                        "expected_root_hash": hex::encode(delta.expected_root_hash)
                    })
                } else {
                    parse_value(*column, &value)
                        .wrap_err_with(|| format!("Failed to parse value in column '{cf_name}'"))?
                }
            } else {
                parse_value(*column, &value)
                    .wrap_err_with(|| format!("Failed to parse value in column '{cf_name}'"))?
            };

            entries.push(json!({
                "key": key_json,
                "value": value_json
            }));
        }

        drop(data.insert(
            cf_name.to_owned(),
            json!({
                "count": entries.len(),
                "entries": entries
            }),
        ));
    }

    Ok(json!({
        "database": "Calimero RocksDB Export",
        "exported_columns": columns.iter().map(Column::as_str).collect::<Vec<_>>(),
        "data": data
    }))
}
