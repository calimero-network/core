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

/// Max length of [`MetadataRecord::name`], in bytes.
pub const MAX_METADATA_NAME_LEN: usize = 64;
/// Max number of entries in [`MetadataRecord::data`].
pub const MAX_METADATA_DATA_ENTRIES: usize = 64;
/// Max length of a [`MetadataRecord::data`] key, in bytes.
pub const MAX_METADATA_DATA_KEY_LEN: usize = 64;
/// Max length of a [`MetadataRecord::data`] value, in bytes.
pub const MAX_METADATA_DATA_VALUE_LEN: usize = 4096;

/// Validate a metadata payload (`name` + `data`) against the hard protocol
/// limits. Enforced both at the HTTP entry point and on the `*MetadataSet`
/// op-apply path — gossip-replicated ops are checked too, so a peer can't
/// bloat the replicated governance state. Returns a human-readable reason on
/// violation. These limits are part of the protocol: changing them is a
/// breaking change.
pub fn validate_metadata_payload(
    name: Option<&str>,
    data: &BTreeMap<String, String>,
) -> Result<(), String> {
    if let Some(name) = name {
        if name.len() > MAX_METADATA_NAME_LEN {
            return Err(format!(
                "metadata name too long: {} bytes (max {MAX_METADATA_NAME_LEN})",
                name.len()
            ));
        }
    }
    if data.len() > MAX_METADATA_DATA_ENTRIES {
        return Err(format!(
            "metadata data has too many entries: {} (max {MAX_METADATA_DATA_ENTRIES})",
            data.len()
        ));
    }
    for (k, v) in data {
        if k.len() > MAX_METADATA_DATA_KEY_LEN {
            return Err(format!(
                "metadata data key too long: {} bytes (max {MAX_METADATA_DATA_KEY_LEN})",
                k.len()
            ));
        }
        if v.len() > MAX_METADATA_DATA_VALUE_LEN {
            return Err(format!(
                "metadata data value too long: {} bytes (max {MAX_METADATA_DATA_VALUE_LEN})",
                v.len()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        validate_metadata_payload, MetadataRecord, MAX_METADATA_DATA_ENTRIES,
        MAX_METADATA_DATA_VALUE_LEN, MAX_METADATA_NAME_LEN,
    };

    #[test]
    fn payload_validation_bounds() {
        assert!(validate_metadata_payload(Some("general"), &BTreeMap::new()).is_ok());
        assert!(validate_metadata_payload(None, &BTreeMap::new()).is_ok());

        let long_name = "x".repeat(MAX_METADATA_NAME_LEN + 1);
        assert!(validate_metadata_payload(Some(&long_name), &BTreeMap::new()).is_err());

        let mut too_many = BTreeMap::new();
        for i in 0..=MAX_METADATA_DATA_ENTRIES {
            let _ = too_many.insert(format!("k{i}"), "v".to_owned());
        }
        assert!(validate_metadata_payload(None, &too_many).is_err());

        let mut big_value = BTreeMap::new();
        let _ = big_value.insert("k".to_owned(), "v".repeat(MAX_METADATA_DATA_VALUE_LEN + 1));
        assert!(validate_metadata_payload(None, &big_value).is_err());
    }

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
