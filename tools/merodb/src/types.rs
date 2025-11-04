#![allow(
    clippy::similar_names,
    reason = "Similar variable names are clear in context"
)]
#![allow(
    clippy::match_same_arms,
    reason = "Semantically different cases with same output"
)]
#![allow(
    clippy::missing_asserts_for_indexing,
    reason = "Bounds are checked with len()"
)]
#![allow(
    clippy::unnecessary_wraps,
    reason = "Consistent API for all parse functions"
)]
#![allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "Matches signature requirements"
)]

use borsh::BorshDeserialize;
use calimero_store::types::{
    ApplicationMeta as StoreApplicationMeta, BlobMeta as StoreBlobMeta,
    ContextConfig as StoreContextConfig, ContextDagDelta as StoreContextDagDelta,
    ContextIdentity as StoreContextIdentity, ContextMeta as StoreContextMeta,
};
use eyre::Result;
use serde::de::Error as DeError;
use serde::Deserialize;
use serde_json::{json, Value};

/// All column families in Calimero's RocksDB
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Meta,
    Config,
    Identity,
    State,
    Delta,
    Blobs,
    Application,
    Alias,
    Generic,
}

impl Column {
    /// Get all column families
    pub const fn all() -> &'static [Self] {
        &[
            Self::Meta,
            Self::Config,
            Self::Identity,
            Self::State,
            Self::Delta,
            Self::Blobs,
            Self::Application,
            Self::Alias,
            Self::Generic,
        ]
    }

    /// Get column family name as string
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Meta => "Meta",
            Self::Config => "Config",
            Self::Identity => "Identity",
            Self::State => "State",
            Self::Delta => "Delta",
            Self::Blobs => "Blobs",
            Self::Application => "Application",
            Self::Alias => "Alias",
            Self::Generic => "Generic",
        }
    }

    /// Get expected key size in bytes
    pub const fn key_size(&self) -> usize {
        match self {
            Self::Meta => 32,        // ContextId
            Self::Config => 32,      // ContextId
            Self::Identity => 64,    // ContextId + PublicKey
            Self::State => 64,       // ContextId + StateKey
            Self::Delta => 64,       // ContextId + DeltaId
            Self::Blobs => 32,       // BlobId
            Self::Application => 32, // ApplicationId
            Self::Alias => 83,       // Kind + Scope + Name
            Self::Generic => 0, // Variable: 48 bytes (Scope+Fragment) or 64 bytes (ContextId+DeltaId)
        }
    }

    /// Get key structure description
    pub const fn key_structure(&self) -> &'static str {
        match self {
            Self::Meta => "ContextId (32 bytes)",
            Self::Config => "ContextId (32 bytes)",
            Self::Identity => "ContextId (32 bytes) + PublicKey (32 bytes)",
            Self::State => "ContextId (32 bytes) + StateKey (32 bytes)",
            Self::Delta => "ContextId (32 bytes) + DeltaId (32 bytes)",
            Self::Blobs => "BlobId (32 bytes)",
            Self::Application => "ApplicationId (32 bytes)",
            Self::Alias => "Kind (1 byte) + Scope (32 bytes) + Name (50 bytes)",
            Self::Generic => "Scope (16 bytes) + Fragment (32 bytes) OR ContextId (32 bytes) + DeltaId (32 bytes) for backwards compatibility with older delta storage",
        }
    }

    /// Get value structure description
    pub const fn value_structure(&self) -> &'static str {
        match self {
            Self::Meta => "ContextMeta { application: ApplicationId, root_hash: Hash, dag_heads: Vec<Hash> }",
            Self::Config => "ContextConfig { protocol, network, contract, proxy_contract, application_revision, members_revision }",
            Self::Identity => "ContextIdentity { private_key: Option<[u8; 32]>, sender_key: Option<[u8; 32]> }",
            Self::State => "Raw bytes (application-specific state)",
            Self::Delta => "ContextDagDelta { delta_id, parents, actions, hlc, applied, expected_root_hash }",
            Self::Blobs => "BlobMeta { size: u64, hash: [u8; 32], links: Box<[BlobId]> }",
            Self::Application => "ApplicationMeta { bytecode: BlobId, size: u64, source: Box<str>, metadata: Box<[u8]>, compiled: BlobId, package: Box<str>, version: Box<str> }",
            Self::Alias => "Hash (32 bytes) - can point to ContextId, PublicKey, or ApplicationId",
            Self::Generic => "Raw bytes (generic key-value storage)",
        }
    }

    /// Parse a column from its canonical string representation.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "Meta" => Some(Self::Meta),
            "Config" => Some(Self::Config),
            "Identity" => Some(Self::Identity),
            "State" => Some(Self::State),
            "Delta" => Some(Self::Delta),
            "Blobs" => Some(Self::Blobs),
            "Application" => Some(Self::Application),
            "Alias" => Some(Self::Alias),
            "Generic" => Some(Self::Generic),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for Column {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as Deserialize>::deserialize(deserializer)?;
        Self::from_name(value.trim()).ok_or_else(|| {
            let expected = Self::all()
                .iter()
                .map(Self::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            DeError::custom(format!(
                "Unknown column family '{value}'. Expected one of: {expected}"
            ))
        })
    }
}

/// Parse a key into a human-readable JSON representation
#[expect(
    clippy::too_many_lines,
    reason = "Branch-heavy decoding logic mirrors database schema"
)]
pub fn parse_key(column: Column, key: &[u8]) -> Result<Value> {
    match column {
        Column::Meta | Column::Config | Column::Blobs | Column::Application => {
            if key.len() != 32 {
                return Ok(json!({
                    "error": "Invalid key size",
                    "expected": 32,
                    "actual": key.len(),
                    "raw": String::from_utf8_lossy(key)
                }));
            }
            Ok(json!({
                "id": hex::encode(key)
            }))
        }
        Column::Identity => {
            if key.len() != 64 {
                return Ok(json!({
                    "error": "Invalid key size",
                    "expected": 64,
                    "actual": key.len(),
                    "raw": String::from_utf8_lossy(key)
                }));
            }
            Ok(json!({
                "context_id": hex::encode(&key[0..32]),
                "public_key": String::from_utf8_lossy(&key[32..64])
            }))
        }
        Column::Delta => {
            if key.len() != 64 {
                return Ok(json!({
                    "error": "Invalid key size",
                    "expected": 64,
                    "actual": key.len(),
                    "raw": String::from_utf8_lossy(key)
                }));
            }
            Ok(json!({
                "context_id": hex::encode(&key[0..32]),
                "delta_id": hex::encode(&key[32..64])
            }))
        }
        Column::State => {
            if key.len() != 64 {
                return Ok(json!({
                    "error": "Invalid key size",
                    "expected": 64,
                    "actual": key.len(),
                    "raw": String::from_utf8_lossy(key)
                }));
            }
            Ok(json!({
                "context_id": hex::encode(&key[0..32]),
                "state_key": hex::encode(&key[32..64])
            }))
        }
        Column::Alias => {
            if key.len() != 83 {
                return Ok(json!({
                    "error": "Invalid key size",
                    "expected": 83,
                    "actual": key.len(),
                    "raw": String::from_utf8_lossy(key)
                }));
            }
            let kind = match key[0] {
                1 => "ContextId",
                2 => "PublicKey",
                3 => "ApplicationId",
                _ => "Unknown",
            };
            let name_bytes = &key[33..83];
            let name = String::from_utf8_lossy(name_bytes)
                .trim_end_matches('\0')
                .to_owned();
            Ok(json!({
                "kind": kind,
                "scope": String::from_utf8_lossy(&key[1..33]),
                "name": name
            }))
        }
        Column::Generic => {
            // Generic column can contain two types:
            // 1. Regular generic keys: 48 bytes (Scope + Fragment)
            // 2. ContextDagDelta keys: 64 bytes (ContextId + DeltaId) - for backwards compatibility
            match key.len() {
                48 => Ok(json!({
                    "type": "generic",
                    "scope": String::from_utf8_lossy(&key[0..16]),
                    "fragment": String::from_utf8_lossy(&key[16..48])
                })),
                64 => Ok(json!({
                    "type": "context_dag_delta",
                    "context_id": hex::encode(&key[0..32]),
                    "delta_id": hex::encode(&key[32..64])
                })),
                _ => {
                    // Unknown key size - still parse it for debugging
                    Ok(json!({
                        "type": "unknown",
                        "size": key.len(),
                        "raw_hex": hex::encode(key)
                    }))
                }
            }
        }
    }
}

/// Parse a value into a human-readable JSON representation
pub fn parse_value(column: Column, value: &[u8]) -> Result<Value> {
    match column {
        Column::Meta => parse_context_meta(value),
        Column::Config => parse_context_config(value),
        Column::Identity => parse_context_identity(value),
        Column::State => Ok(json!({
            "raw": String::from_utf8_lossy(value),
            "size": value.len()
        })),
        Column::Delta => parse_dag_delta(value),
        Column::Blobs => parse_blob_meta(value),
        Column::Application => parse_application_meta(value),
        Column::Alias => parse_alias_target(value),
        Column::Generic => parse_generic_value(value),
    }
}

// Parse functions using imported Calimero types

fn parse_context_meta(data: &[u8]) -> Result<Value> {
    match StoreContextMeta::try_from_slice(data) {
        Ok(meta) => Ok(json!({
            "application_id": String::from_utf8_lossy(meta.application.application_id().as_ref()),
            "root_hash": String::from_utf8_lossy(&meta.root_hash),
            "dag_heads": meta.dag_heads.iter().map(hex::encode).collect::<Vec<_>>()
        })),
        Err(e) => Ok(json!({
            "error": format!("Failed to parse ContextMeta: {e}"),
            "raw": String::from_utf8_lossy(data)
        })),
    }
}

fn parse_context_config(data: &[u8]) -> Result<Value> {
    match StoreContextConfig::try_from_slice(data) {
        Ok(config) => Ok(json!({
            "protocol": config.protocol.as_ref(),
            "network": config.network.as_ref(),
            "contract": config.contract.as_ref(),
            "proxy_contract": config.proxy_contract.as_ref(),
            "application_revision": config.application_revision,
            "members_revision": config.members_revision
        })),
        Err(e) => Ok(json!({
            "error": format!("Failed to parse ContextConfig: {e}"),
            "raw": String::from_utf8_lossy(data)
        })),
    }
}

fn parse_context_identity(data: &[u8]) -> Result<Value> {
    match StoreContextIdentity::try_from_slice(data) {
        Ok(identity) => {
            let mut result = serde_json::Map::new();
            if let Some(private_key) = identity.private_key {
                drop(result.insert(
                    "private_key".to_owned(),
                    json!(String::from_utf8_lossy(&private_key)),
                ));
            }
            if let Some(sender_key) = identity.sender_key {
                drop(result.insert(
                    "sender_key".to_owned(),
                    json!(String::from_utf8_lossy(&sender_key)),
                ));
            }
            Ok(json!(result))
        }
        Err(e) => Ok(json!({
            "error": format!("Failed to parse ContextIdentity: {e}"),
            "raw": String::from_utf8_lossy(data)
        })),
    }
}

fn parse_blob_meta(data: &[u8]) -> Result<Value> {
    match StoreBlobMeta::try_from_slice(data) {
        Ok(meta) => Ok(json!({
            "size": meta.size,
            "hash": String::from_utf8_lossy(&meta.hash),
            "links_count": meta.links.len()
        })),
        Err(e) => Ok(json!({
            "error": format!("Failed to parse BlobMeta: {e}"),
            "raw": String::from_utf8_lossy(data)
        })),
    }
}

fn parse_application_meta(data: &[u8]) -> Result<Value> {
    match StoreApplicationMeta::try_from_slice(data) {
        Ok(meta) => Ok(json!({
            "bytecode": String::from_utf8_lossy(meta.bytecode.blob_id().as_ref()),
            "size": meta.size,
            "source": meta.source.as_ref(),
            "metadata": String::from_utf8_lossy(&meta.metadata),
            "compiled": String::from_utf8_lossy(meta.compiled.blob_id().as_ref()),
            "package": meta.package.as_ref(),
            "version": meta.version.as_ref()
        })),
        Err(e) => Ok(json!({
            "error": format!("Failed to parse ApplicationMeta: {e}"),
            "raw": String::from_utf8_lossy(data)
        })),
    }
}

fn parse_alias_target(data: &[u8]) -> Result<Value> {
    if data.len() == 32 {
        Ok(json!({
            "hash": String::from_utf8_lossy(data)
        }))
    } else {
        Ok(json!({
            "error": "Invalid alias hash size",
            "expected": 32,
            "actual": data.len(),
            "raw": String::from_utf8_lossy(data)
        }))
    }
}

fn parse_dag_delta(data: &[u8]) -> Result<Value> {
    match StoreContextDagDelta::try_from_slice(data) {
        Ok(delta) => {
            let (timestamp_raw, hlc_json) = delta_hlc_snapshot(&delta);
            Ok(json!({
                "type": "context_dag_delta",
                "delta_id": hex::encode(delta.delta_id),
                "parents": delta.parents.iter().map(hex::encode).collect::<Vec<_>>(),
                "actions_size": delta.actions.len(),
                "timestamp": timestamp_raw,
                "hlc": hlc_json,
                "applied": delta.applied,
                "expected_root_hash": hex::encode(delta.expected_root_hash)
            }))
        }
        Err(e) => Ok(json!({
            "error": format!("Failed to parse ContextDagDelta: {e}"),
            "raw": String::from_utf8_lossy(data),
            "size": data.len()
        })),
    }
}

fn parse_generic_value(data: &[u8]) -> Result<Value> {
    // Try to parse as ContextDagDelta first (for backwards compatibility)
    match StoreContextDagDelta::try_from_slice(data) {
        Ok(delta) => {
            let (timestamp_raw, hlc_json) = delta_hlc_snapshot(&delta);
            Ok(json!({
                "type": "context_dag_delta",
                "delta_id": hex::encode(delta.delta_id),
                "parents": delta.parents.iter().map(hex::encode).collect::<Vec<_>>(),
                "actions_size": delta.actions.len(),
                "timestamp": timestamp_raw,
                "hlc": hlc_json,
                "applied": delta.applied
            }))
        }
        Err(_) => {
            // Fall back to raw bytes for generic values
            Ok(json!({
                "type": "generic",
                "raw": String::from_utf8_lossy(data),
                "size": data.len()
            }))
        }
    }
}

fn delta_hlc_snapshot(delta: &StoreContextDagDelta) -> (u64, Value) {
    let timestamp = delta.hlc.inner();
    let raw_time = timestamp.get_time().as_u64();
    let id_hex = format!("{:032x}", u128::from(*timestamp.get_id()));
    let physical_seconds = (raw_time >> 32_u32) as u32;
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
