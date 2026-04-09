use std::io::Read;

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

#[derive(BorshSerialize, Clone, Debug, Eq, PartialEq)]
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

// Custom deserialization: handle backwards compatibility for old data
// that doesn't have the `services` field (added in rc.19).
impl BorshDeserialize for ApplicationMeta {
    fn deserialize_reader<R: Read>(reader: &mut R) -> std::io::Result<Self> {
        let bytecode = key::BlobMeta::deserialize_reader(reader)?;
        let size = u64::deserialize_reader(reader)?;
        let source = Box::<str>::deserialize_reader(reader)?;
        let metadata = Box::<[u8]>::deserialize_reader(reader)?;
        let compiled = key::BlobMeta::deserialize_reader(reader)?;
        let package = Box::<str>::deserialize_reader(reader)?;
        let version = Box::<str>::deserialize_reader(reader)?;
        let signer_id = Box::<str>::deserialize_reader(reader)?;

        // `services` was added after the initial schema. Old records end after `signer_id`.
        // Try to read it; if there's no more data, default to an empty Vec.
        let services = match Vec::<ServiceMeta>::deserialize_reader(reader) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Vec::new(),
            Err(e)
                if e.kind() == std::io::ErrorKind::InvalidData
                    && e.to_string().contains("Unexpected length") =>
            {
                Vec::new()
            }
            Err(e) => return Err(e),
        };

        Ok(Self {
            bytecode,
            size,
            source,
            metadata,
            compiled,
            package,
            version,
            signer_id,
            services,
        })
    }
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
