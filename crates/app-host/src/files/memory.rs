//! An in-memory file store for tests: ids are the hex SHA-256 of the plaintext and the
//! ciphertext is the plaintext.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use sha2::{Digest, Sha256};

use super::{FileError, FileInfo, FileRead, FileWrite, Files, NewFile, ReadError, NO_SUCH_FILE};

/// Shared by every clone, so a test keeps a handle while the host core holds another.
#[derive(Clone, Default)]
pub struct MemoryFiles(Arc<Mutex<BTreeMap<String, Vec<u8>>>>);

impl MemoryFiles {
    /// Stores `bytes` and returns the id.
    pub fn insert(&self, bytes: &[u8]) -> String {
        let id = hex::encode(Sha256::digest(bytes));
        let _ = self.lock().insert(id.clone(), bytes.to_vec());
        id
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<Vec<u8>> {
        self.lock().get(id).cloned()
    }

    /// What the node does with an applied entry: keeps the new files, drops the deleted.
    pub fn apply(&self, new_files: &[NewFile], deleted: &[String]) {
        let mut files = self.lock();
        for file in new_files {
            let _ = files.insert(file.id.clone(), file.ciphertext.clone());
        }
        for id in deleted {
            let _ = files.remove(id);
        }
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, Vec<u8>>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

struct Reader {
    bytes: Vec<u8>,
    at: usize,
}

impl FileRead for Reader {
    fn read(&mut self, max: usize) -> Result<Vec<u8>, ReadError> {
        let end = self.at.saturating_add(max).min(self.bytes.len());
        let chunk = self.bytes[self.at..end].to_vec();
        self.at = end;
        Ok(chunk)
    }
}

struct Writer(Vec<u8>);

impl FileWrite for Writer {
    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.0.extend_from_slice(bytes);
        Ok(())
    }

    fn finish(self: Box<Self>) -> Result<NewFile, String> {
        let sha256 = Sha256::digest(&self.0).to_vec();
        Ok(NewFile {
            id: hex::encode(&sha256),
            size: self.0.len() as u64,
            sha256,
            wrapped_key: Vec::new(),
            ciphertext: self.0,
        })
    }
}

impl Files for MemoryFiles {
    fn open(&self, id: &str) -> Result<Box<dyn FileRead>, FileError> {
        let bytes = self
            .get(id)
            .ok_or_else(|| FileError::Failed(NO_SUCH_FILE.to_owned()))?;
        Ok(Box::new(Reader { bytes, at: 0 }))
    }

    fn info(&self, id: &str) -> Option<FileInfo> {
        let bytes = self.get(id)?;
        Some(FileInfo {
            size: bytes.len() as u64,
            sha256: Sha256::digest(&bytes).to_vec(),
        })
    }

    fn create(&self) -> Result<Box<dyn FileWrite>, FileError> {
        Ok(Box::new(Writer(Vec::new())))
    }
}
