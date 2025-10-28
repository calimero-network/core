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
            Self::Delta => 72,       // ContextId + PublicKey + Height
            Self::Blobs => 32,       // BlobId
            Self::Application => 32, // ApplicationId
            Self::Alias => 83,       // Kind + Scope + Name
            Self::Generic => 48,     // Scope + Fragment
        }
    }

    /// Get key structure description
    pub const fn key_structure(&self) -> &'static str {
        match self {
            Self::Meta => "ContextId (32 bytes)",
            Self::Config => "ContextId (32 bytes)",
            Self::Identity => "ContextId (32 bytes) + PublicKey (32 bytes)",
            Self::State => "ContextId (32 bytes) + StateKey (32 bytes)",
            Self::Delta => "ContextId (32 bytes) + PublicKey (32 bytes) + Height (8 bytes)",
            Self::Blobs => "BlobId (32 bytes)",
            Self::Application => "ApplicationId (32 bytes)",
            Self::Alias => "Kind (1 byte) + Scope (32 bytes) + Name (50 bytes)",
            Self::Generic => "Scope (16 bytes) + Fragment (32 bytes)",
        }
    }

    /// Get value structure description
    pub const fn value_structure(&self) -> &'static str {
        match self {
            Self::Meta => "ContextMeta { application: ApplicationId, root_hash: Hash }",
            Self::Config => "ContextConfig { protocol, network, contract, proxy_contract, application_revision, members_revision }",
            Self::Identity => "ContextIdentity { private_key: Option<[u8; 32]>, sender_key: Option<[u8; 32]> }",
            Self::State => "Raw bytes (application-specific state)",
            Self::Delta => "ContextDagDelta { delta_id, parents, actions, timestamp, applied }",
            Self::Blobs => "BlobMeta { size: u64, hash: [u8; 32], links: Box<[BlobId]> }",
            Self::Application => "ApplicationMeta { bytecode: BlobId, size: u64, source: Box<str>, metadata: Box<[u8]>, compiled: BlobId }",
            Self::Alias => "AliasTarget (ContextId, PublicKey, or ApplicationId)",
            Self::Generic => "Raw bytes (generic key-value storage)",
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
        Column::Identity | Column::State => {
            if key.len() != 64 {
                return Ok(json!({
                    "error": "Invalid key size",
                    "expected": 64,
                    "actual": key.len(),
                    "hex": hex::encode(key)
                }));
            }
            Ok(json!({
                "context_id": hex::encode(&key[0..32]),
                "second_part": hex::encode(&key[32..64])
            }))
        }
        Column::Delta => {
            if key.len() != 72 {
                return Ok(json!({
                    "error": "Invalid key size",
                    "expected": 72,
                    "actual": key.len(),
                    "hex": hex::encode(key)
                }));
            }
            let height = u64::from_le_bytes(key[64..72].try_into().unwrap_or([0; 8]));
            Ok(json!({
                "context_id": hex::encode(&key[0..32]),
                "public_key": hex::encode(&key[32..64]),
                "height": height
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
            if key.len() != 48 {
                return Ok(json!({
                    "error": "Invalid key size",
                    "expected": 48,
                    "actual": key.len(),
                    "hex": hex::encode(key)
                }));
            }
            Ok(json!({
                "scope": hex::encode(&key[0..16]),
                "fragment": hex::encode(&key[16..48])
            }))
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
        Column::Delta => parse_context_delta(value),
        Column::Blobs => parse_blob_meta(value),
        Column::Application => parse_application_meta(value),
        Column::Alias => parse_alias_target(value),
        Column::Generic => Ok(json!({
            "hex": hex::encode(value),
            "size": value.len()
        })),
    }
}

// Parse functions using imported Calimero types

fn parse_context_meta(data: &[u8]) -> Result<Value> {
    match StoreContextMeta::try_from_slice(data) {
        Ok(meta) => Ok(json!({
            "application_id": hex::encode(*meta.application.application_id()),
            "root_hash": hex::encode(meta.root_hash)
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

fn parse_context_delta(data: &[u8]) -> Result<Value> {
    match StoreContextDagDelta::try_from_slice(data) {
        Ok(delta) => Ok(json!({
            "delta_id": hex::encode(delta.delta_id),
            "parents": delta.parents.iter().map(hex::encode).collect::<Vec<_>>(),
            "actions_size": delta.actions.len(),
            "timestamp": delta.timestamp,
            "applied": delta.applied
        })),
        Err(e) => Ok(json!({
            "error": format!("Failed to parse ContextDagDelta: {e}"),
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
            "compiled": hex::encode(*meta.compiled.blob_id())
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
            "target_id": hex::encode(data)
        }))
    } else {
        Ok(json!({
            "error": "Invalid alias target size",
            "expected": 32,
            "actual": data.len(),
            "hex": hex::encode(data)
        }))
    }
}
