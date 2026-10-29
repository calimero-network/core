//! A thread-local in-memory file store, for tests on the host target. A finished file is
//! visible at once and a delete takes effect at once: the host shows both only after the
//! run commits, and has soft errors for deleting a file made in the run or twice. Those need
//! the host core, which the guest-facing SDK does not depend on; `testing::TestApp` has them.

use std::cell::RefCell;
use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::files::FileInfo;

const NO_SUCH_FILE: &str = "no such file";

thread_local! {
    static FILES: RefCell<BTreeMap<String, Vec<u8>>> = const { RefCell::new(BTreeMap::new()) };
}

pub struct Writer(Vec<u8>);

pub struct Reader {
    bytes: Vec<u8>,
    at: usize,
}

pub fn create() -> Result<Writer, String> {
    Ok(Writer(Vec::new()))
}

pub fn open(id: &str) -> Result<Reader, String> {
    let bytes = FILES.with(|f| f.borrow().get(id).cloned());
    bytes
        .map(|bytes| Reader { bytes, at: 0 })
        .ok_or_else(|| NO_SUCH_FILE.to_owned())
}

pub fn info(id: &str) -> Option<FileInfo> {
    FILES.with(|f| {
        f.borrow().get(id).map(|bytes| FileInfo {
            size: bytes.len() as u64,
            sha256: Sha256::digest(bytes).to_vec(),
        })
    })
}

pub fn delete(id: &str) -> Result<(), String> {
    FILES
        .with(|f| f.borrow_mut().remove(id))
        .map(drop)
        .ok_or_else(|| NO_SUCH_FILE.to_owned())
}

impl Writer {
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.0.extend_from_slice(bytes);
        Ok(())
    }

    pub fn finish(self) -> Result<String, String> {
        let id = hex::encode(Sha256::digest(&self.0));
        FILES.with(|f| f.borrow_mut().insert(id.clone(), self.0));
        Ok(id)
    }
}

impl Reader {
    pub fn read(&mut self, max: u32) -> Result<Vec<u8>, String> {
        let end = self.at.saturating_add(max as usize).min(self.bytes.len());
        let chunk = self.bytes[self.at..end].to_vec();
        self.at = end;
        Ok(chunk)
    }
}
