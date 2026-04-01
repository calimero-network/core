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
}

impl ApplicationMeta {
    #[must_use]
    pub const fn new(
        bytecode: key::BlobMeta,
        size: u64,
        source: Box<str>,
        metadata: Box<[u8]>,
        compiled: key::BlobMeta,
        package: Box<str>,
        version: Box<str>,
        signer_id: Box<str>,
    ) -> Self {
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
