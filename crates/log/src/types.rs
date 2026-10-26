//! Raft type configuration shared by every group.

use std::fmt;
use std::io::Cursor;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A group's id. `0` is the system group; app groups get ids from it.
pub type GroupId = u64;

/// A node's id within a cluster.
pub type NodeId = u64;

/// The opaque bytes a group's state machine interprets. Serialized as base64 so
/// RPC bodies stay compact JSON.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Bytes(pub Vec<u8>);

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bytes({})", self.0.len())
    }
}

impl Serialize for Bytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        STANDARD
            .decode(s.as_bytes())
            .map(Bytes)
            .map_err(serde::de::Error::custom)
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(value: Vec<u8>) -> Self {
        Self(value)
    }
}

openraft::declare_raft_types!(
    /// The one Raft type configuration: commands and outputs are opaque bytes.
    pub TypeConfig:
        D = Bytes,
        R = Bytes,
        NodeId = NodeId,
        Node = openraft::BasicNode,
        Entry = openraft::Entry<TypeConfig>,
        SnapshotData = Cursor<Vec<u8>>,
);

pub type Raft = openraft::Raft<TypeConfig>;
pub type LogId = openraft::LogId<NodeId>;
pub type Vote = openraft::Vote<NodeId>;
pub type Entry = openraft::Entry<TypeConfig>;
pub type StoredMembership = openraft::StoredMembership<NodeId, openraft::BasicNode>;
pub type SnapshotMeta = openraft::SnapshotMeta<NodeId, openraft::BasicNode>;
pub type StorageError = openraft::StorageError<NodeId>;
