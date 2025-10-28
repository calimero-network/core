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
use serde_json::{json, Value};

/// All column families in Calimero's RocksDB
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Meta,
    Config,
    Identity,
    State,
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
            Self::Blobs => 32,       // BlobId
            Self::Application => 32, // ApplicationId
            Self::Alias => 83,       // Kind + Scope + Name
            Self::Generic => 0, // Variable: 48 bytes (Scope+Fragment) OR 64 bytes (ContextId+DeltaId for DAG deltas)
        }
    }

    /// Get key structure description
    pub const fn key_structure(&self) -> &'static str {
        match self {
            Self::Meta => "ContextId (32 bytes)",
            Self::Config => "ContextId (32 bytes)",
            Self::Identity => "ContextId (32 bytes) + PublicKey (32 bytes)",
            Self::State => "ContextId (32 bytes) + StateKey (32 bytes)",
            Self::Blobs => "BlobId (32 bytes)",
            Self::Application => "ApplicationId (32 bytes)",
            Self::Alias => "Kind (1 byte) + Scope (32 bytes) + Name (50 bytes)",
            Self::Generic => "Scope (16 bytes) + Fragment (32 bytes) OR ContextId (32 bytes) + DeltaId (32 bytes) for DAG deltas",
        }
    }

    /// Get value structure description
    pub const fn value_structure(&self) -> &'static str {
        match self {
            Self::Meta => "ContextMeta { application: ApplicationId, root_hash: Hash, dag_heads: Vec<Hash> }",
            Self::Config => "ContextConfig { protocol, network, contract, proxy_contract, application_revision, members_revision }",
            Self::Identity => "ContextIdentity { private_key: Option<[u8; 32]>, sender_key: Option<[u8; 32]> }",
            Self::State => "Raw bytes (application-specific state)",
            Self::Blobs => "BlobMeta { size: u64, hash: [u8; 32], links: Box<[BlobId]> }",
            Self::Application => "ApplicationMeta { bytecode: BlobId, size: u64, source: Box<str>, metadata: Box<[u8]>, compiled: BlobId, package: Box<str>, version: Box<str> }",
            Self::Alias => "Hash (32 bytes) - can point to ContextId, PublicKey, or ApplicationId",
            Self::Generic => "Raw bytes (generic key-value storage) OR ContextDagDelta { delta_id, parents, actions, timestamp, applied } for DAG deltas",
        }
    }
}

/// Parse a key into a human-readable JSON representation
pub fn parse_key(column: Column, key: &[u8]) -> Result<Value> {
    match column {
        Column::Meta | Column::Config | Column::Blobs | Column::Application => {
            if key.len() != 32 {
                return Ok(json!({
                    "error": "Invalid key size",
                    "expected": 32,
                    "actual": key.len(),
                    "hex": hex::encode(key)
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
                    "hex": hex::encode(key)
                }));
            }
            Ok(json!({
                "context_id": String::from_utf8_lossy(&key[0..32]).to_string(),
                "public_key": hex::encode(&key[32..64])
            }))
        }
        Column::State => {
            if key.len() != 64 {
                return Ok(json!({
                    "error": "Invalid key size",
                    "expected": 64,
                    "actual": key.len(),
                    "hex": hex::encode(key)
                }));
            }
            Ok(json!({
                "context_id": String::from_utf8_lossy(&key[0..32]).to_string(),
                "state_key": hex::encode(&key[32..64])
            }))
        }
        Column::Alias => {
            if key.len() != 83 {
                return Ok(json!({
                    "error": "Invalid key size",
                    "expected": 83,
                    "actual": key.len(),
                    "hex": hex::encode(key)
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
                "scope": hex::encode(&key[1..33]),
                "name": name
            }))
        }
        Column::Generic => {
            // Generic column can contain two types:
            // 1. Regular generic keys: 48 bytes (Scope + Fragment)
            // 2. ContextDagDelta keys: 64 bytes (ContextId + DeltaId)
            match key.len() {
                48 => Ok(json!({
                    "type": "generic",
                    "scope": hex::encode(&key[0..16]),
                    "fragment": hex::encode(&key[16..48])
                })),
                64 => Ok(json!({
                    "type": "context_dag_delta",
                    "context_id": String::from_utf8_lossy(&key[0..32]).to_string(),
                    "delta_id": hex::encode(&key[32..64])
                })),
                _ => Ok(json!({
                    "error": "Invalid key size",
                    "expected": "48 (generic) or 64 (context_dag_delta)",
                    "actual": key.len(),
                    "hex": hex::encode(key)
                })),
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
            "hex": hex::encode(value),
            "size": value.len()
        })),
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
            "application_id": hex::encode(*meta.application.application_id()),
            "root_hash": hex::encode(meta.root_hash),
            "dag_heads": meta.dag_heads.iter().map(hex::encode).collect::<Vec<_>>()
        })),
        Err(e) => Ok(json!({
            "error": format!("Failed to parse ContextMeta: {e}"),
            "hex": hex::encode(data)
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
            "hex": hex::encode(data)
        })),
    }
}

fn parse_context_identity(data: &[u8]) -> Result<Value> {
    match StoreContextIdentity::try_from_slice(data) {
        Ok(identity) => {
            let mut result = serde_json::Map::new();
            if let Some(private_key) = identity.private_key {
                drop(result.insert("private_key".to_owned(), json!(hex::encode(private_key))));
            }
            if let Some(sender_key) = identity.sender_key {
                drop(result.insert("sender_key".to_owned(), json!(hex::encode(sender_key))));
            }
            Ok(json!(result))
        }
        Err(e) => Ok(json!({
            "error": format!("Failed to parse ContextIdentity: {e}"),
            "hex": hex::encode(data)
        })),
    }
}

fn parse_blob_meta(data: &[u8]) -> Result<Value> {
    match StoreBlobMeta::try_from_slice(data) {
        Ok(meta) => Ok(json!({
            "size": meta.size,
            "hash": hex::encode(meta.hash),
            "links_count": meta.links.len()
        })),
        Err(e) => Ok(json!({
            "error": format!("Failed to parse BlobMeta: {e}"),
            "hex": hex::encode(data)
        })),
    }
}

fn parse_application_meta(data: &[u8]) -> Result<Value> {
    match StoreApplicationMeta::try_from_slice(data) {
        Ok(meta) => Ok(json!({
            "bytecode": hex::encode(*meta.bytecode.blob_id()),
            "size": meta.size,
            "source": meta.source.as_ref(),
            "metadata": hex::encode(&meta.metadata),
            "compiled": hex::encode(*meta.compiled.blob_id()),
            "package": meta.package.as_ref(),
            "version": meta.version.as_ref()
        })),
        Err(e) => Ok(json!({
            "error": format!("Failed to parse ApplicationMeta: {e}"),
            "hex": hex::encode(data)
        })),
    }
}

fn parse_alias_target(data: &[u8]) -> Result<Value> {
    if data.len() == 32 {
        Ok(json!({
            "hash": hex::encode(data)
        }))
    } else {
        Ok(json!({
            "error": "Invalid alias hash size",
            "expected": 32,
            "actual": data.len(),
            "hex": hex::encode(data)
        }))
    }
}

fn parse_generic_value(data: &[u8]) -> Result<Value> {
    // Try to parse as ContextDagDelta first
    match StoreContextDagDelta::try_from_slice(data) {
        Ok(delta) => Ok(json!({
            "type": "context_dag_delta",
            "delta_id": hex::encode(delta.delta_id),
            "parents": delta.parents.iter().map(hex::encode).collect::<Vec<_>>(),
            "actions_size": delta.actions.len(),
            "timestamp": delta.timestamp,
            "applied": delta.applied
        })),
        Err(_) => {
            // Fall back to raw bytes for generic values
            Ok(json!({
                "type": "generic",
                "hex": hex::encode(data),
                "size": data.len()
            }))
        }
    }
}
