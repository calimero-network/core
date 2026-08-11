use borsh::{BorshDeserialize, BorshSerialize};

use crate::entry::Borsh;
use crate::key;
use crate::types::PredefinedEntry;

/// A named service within a multi-service application bundle.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
pub struct ServiceMeta {
    pub name: Box<str>,
    pub bytecode: key::BlobMeta,
    pub compiled: key::BlobMeta,
}

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ApplicationMeta {
    pub bytecode: key::BlobMeta,
    pub size: u64,
    pub source: Box<str>,
    pub metadata: Box<[u8]>,
    pub compiled: key::BlobMeta,
    pub package: Box<str>,
    pub version: Box<str>,
    pub signer_id: Box<str>,
    /// Named services within this application. Empty for single-service apps.
    /// When non-empty, `bytecode`/`compiled` above point to the first (default) service.
    pub services: Vec<ServiceMeta>,
    /// Max ABI state version across this application's services, `0` when none
    /// exposes a readable ABI. What the migration rollup compares, not `version`.
    pub state_version: u32,
}

/// Identifying fields of an [`ApplicationMeta`]: who published it, what semver
/// it claims, and what state version its ABI declares.
#[derive(Debug, Clone)]
pub struct PackageInfo {
    pub package: Box<str>,
    pub version: Box<str>,
    pub signer_id: Box<str>,
    pub state_version: u32,
}

impl ApplicationMeta {
    #[must_use]
    pub fn new(
        bytecode: key::BlobMeta,
        size: u64,
        source: Box<str>,
        metadata: Box<[u8]>,
        compiled: key::BlobMeta,
        info: PackageInfo,
    ) -> Self {
        let PackageInfo {
            package,
            version,
            signer_id,
            state_version,
        } = info;
        Self {
            bytecode,
            size,
            source,
            metadata,
            compiled,
            package,
            version,
            signer_id,
            services: Vec::new(),
            state_version,
        }
    }

    /// Resolve a service's bytecode blob by name.
    /// Returns None if not found. For single-service apps, returns
    /// the default bytecode when service_name is None.
    pub fn resolve_service(
        &self,
        service_name: Option<&str>,
    ) -> Option<(key::BlobMeta, key::BlobMeta)> {
        match service_name {
            None if self.services.is_empty() => Some((self.bytecode, self.compiled)),
            None if self.services.len() == 1 => {
                let svc = &self.services[0];
                Some((svc.bytecode, svc.compiled))
            }
            None => None,
            Some(name) => self
                .services
                .iter()
                .find(|s| &*s.name == name)
                .map(|s| (s.bytecode, s.compiled)),
        }
    }
}

impl PredefinedEntry for key::ApplicationMeta {
    type Codec = Borsh;
    type DataType<'a> = ApplicationMeta;
}

/// Value for [`key::ApplicationPreviousBlob`]: the bytecode blob that an
/// in-place (same-id) bundle install overwrote. Node-local breadcrumb — lets
/// a logically-aborted migration pin its context back to the pre-upgrade
/// code, and gives the L1 downgrade gate a pre-install ABI to compare. A
/// brand-new key, so a missing row reads as `None` (no prior in-place
/// install on record).
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "single breadcrumb value; additions would need a migration"
)]
pub struct ApplicationPreviousBlob {
    pub bytecode: [u8; 32],
}

impl PredefinedEntry for key::ApplicationPreviousBlob {
    type Codec = Borsh;
    type DataType<'a> = ApplicationPreviousBlob;
}

#[cfg(test)]
mod application_meta_tests {
    use borsh::BorshDeserialize;
    use calimero_primitives::blobs::BlobId;

    use super::ApplicationMeta;
    use crate::key;

    fn sample() -> ApplicationMeta {
        ApplicationMeta {
            bytecode: key::BlobMeta::new(BlobId::from([1; 32])),
            size: 10,
            source: "test".into(),
            metadata: Box::new([]),
            compiled: key::BlobMeta::new(BlobId::from([0; 32])),
            package: "com.example.app".into(),
            version: "10.1.3".into(),
            signer_id: "did:key:zTest".into(),
            services: Vec::new(),
            state_version: 2,
        }
    }

    #[test]
    fn application_meta_roundtrips_state_version() {
        let meta = sample();

        let bytes = borsh::to_vec(&meta).expect("serialize");
        let back = ApplicationMeta::try_from_slice(&bytes).expect("deserialize");

        assert_eq!(
            back.state_version, 2,
            "state_version must survive a round trip"
        );
        assert_eq!(back.version.as_ref(), "10.1.3");
    }

    /// Pre-`state_version` records must fail loud rather than decode a default.
    /// Both legacy shapes: missing the trailing `u32`, and missing `services` too.
    #[test]
    fn rejects_records_written_before_state_version() {
        let bytes = borsh::to_vec(&sample()).expect("serialize");

        for drop in [4, 8] {
            let truncated = &bytes[..bytes.len() - drop];
            let _err = ApplicationMeta::try_from_slice(truncated)
                .expect_err("a record short of the full layout must not decode");
        }
    }
}
