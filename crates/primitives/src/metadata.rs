use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::identity::PublicKey;

/// App-extensible metadata for a group, a group member, or a context
/// registered in a group. A namespace is a root group, so the group variant
/// covers it.
///
/// `data` is **opaque to core** — core stores and replicates it verbatim and
/// never reads or interprets any key in it. (A future per-namespace
/// name-uniqueness policy will live in a typed field or a separate op, never
/// inside `data`.)
///
/// `updated_at` is stamped by the *applier* at apply time, so peers may
/// disagree by a few millis; that is acceptable because metadata is
/// deliberately excluded from `compute_group_state_hash` (exactly as the
/// former alias rows were) — it is replicated governance state but not
/// consensus-relevant state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshSerialize, borsh::BorshDeserialize)
)]
pub struct MetadataRecord {
    /// The entity's human-readable name (the field formerly called `alias`).
    /// `None` means "no name set".
    pub name: Option<String>,
    /// Arbitrary application-defined properties. Stored and replicated
    /// verbatim; never inspected by core.
    pub data: BTreeMap<String, String>,
    /// Wall-clock millis when the most recent `*MetadataSet` op was applied
    /// locally. Informational only.
    pub updated_at: u64,
    /// Public key of the signer of the most recent `*MetadataSet` op.
    pub updated_by: PublicKey,
}

impl Default for MetadataRecord {
    fn default() -> Self {
        Self {
            name: None,
            data: BTreeMap::new(),
            updated_at: 0,
            updated_by: PublicKey::from([0_u8; 32]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MetadataRecord;

    #[test]
    fn default_is_empty() {
        let rec = MetadataRecord::default();
        assert!(rec.name.is_none());
        assert!(rec.data.is_empty());
        assert_eq!(rec.updated_at, 0);
    }

    #[cfg(feature = "borsh")]
    #[test]
    fn borsh_roundtrips() {
        use std::collections::BTreeMap;

        let mut data = BTreeMap::new();
        let _ = data.insert("topic".to_owned(), "general chatter".to_owned());
        let _ = data.insert("color".to_owned(), "#3366ff".to_owned());
        let rec = MetadataRecord {
            name: Some("general".to_owned()),
            data,
            updated_at: 1_700_000_000_000,
            updated_by: [7_u8; 32].into(),
        };
        let bytes = borsh::to_vec(&rec).expect("serialize");
        let back: MetadataRecord = borsh::from_slice(&bytes).expect("deserialize");
        assert_eq!(back, rec);
    }
}
