use std::io;

use libp2p::kad::store::MemoryStore;
use libp2p::kad::store::RecordStore;

use crate::types::DidDocument;

pub struct Dht<'a> {
    kad: &'a mut libp2p::kad::store::MemoryStore,
}

impl<'a> Dht<'a> {
    pub fn new(kad: &'a mut MemoryStore) -> Self {
        Dht { kad }
    }

    /// Write did in dht
    pub fn write_record(&mut self, did_document: DidDocument) -> Result<(), io::Error> {
        let key_id: Vec<u8> = did_document.clone().id.into();
        let key = libp2p::kad::RecordKey::from(key_id);
        let value: Vec<u8> = serde_json::to_vec(&did_document.clone())?;
        let record = libp2p::kad::Record::new(key, value.clone());
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
            let value = &result.value.clone();
            let data = String::from_utf8(value.to_vec())
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            let did_document: DidDocument = serde_json::from_str(&data)?;
            Ok(Some(did_document))
        } else {
            Ok(None)
        }
    }
}
