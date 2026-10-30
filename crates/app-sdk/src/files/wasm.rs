//! The guest side of `files`: calls through the `calimero:app/files` import.

use crate::bindings::calimero::app::files;
use crate::files::FileInfo;

pub use files::{Reader, Writer};

pub fn create() -> Result<Writer, String> {
    files::create()
}

pub fn open(id: &str) -> Result<Reader, String> {
    files::open(id)
}

pub fn info(id: &str) -> Option<FileInfo> {
    files::info(id).map(|i| FileInfo {
        size: i.size,
        sha256: i.sha256,
    })
}

pub fn delete(id: &str) -> Result<(), String> {
    files::delete(id)
}
