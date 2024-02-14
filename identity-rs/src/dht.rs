use std::io;

use libp2p::kad::store::{MemoryStore, RecordStore};

use crate::types::DidDocument;

pub struct Dht<'a> {
    kad: &'a mut MemoryStore,
}

impl<'a> Dht<'a> {
    pub fn new(kad: &'a mut MemoryStore) -> Self {
        Dht { kad }
    }

    /// Write did in dht
    pub fn write_record(&mut self, did_document: DidDocument) -> Result<(), io::Error> {
        let key_id = did_document.id.as_bytes().to_owned();
        let key = libp2p::kad::RecordKey::from(key_id);
        let value = serde_json::to_vec(&did_document)?;
        let record = libp2p::kad::Record::new(key, value);
        self.kad
            .put(record)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        Ok(())
    }

    /// read did document per did id from dht
    pub fn read_record(&self, did: String) -> Result<Option<DidDocument>, io::Error> {
        let key_id: Vec<u8> = did.into();
        let key = libp2p::kad::RecordKey::from(key_id);

        if let Some(result) = self.kad.get(&key) {
            let data = std::str::from_utf8(&result.value)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            let did_document: DidDocument = serde_json::from_str(data)?;
            Ok(Some(did_document))
        } else {
            Ok(None)
        }
    }
}
