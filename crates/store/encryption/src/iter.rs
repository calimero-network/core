//! Decrypting iterator wrapper for encrypted database.

use std::sync::{Arc, RwLock};

use calimero_store::iter::DBIter;
use calimero_store::slice::Slice;
use eyre::Result;

use crate::key_manager::KeyManager;

/// An iterator wrapper that decrypts values on read.
///
/// This wraps an inner database iterator and transparently decrypts
/// values when `read()` is called. Keys are returned unchanged since
/// they are not encrypted.
pub struct DecryptingIter<'a> {
    inner: Box<dyn DBIter + 'a>,
    key_manager: Arc<RwLock<KeyManager>>,
}

impl<'a> DecryptingIter<'a> {
    /// Create a new decrypting iterator.
    pub fn new(inner: Box<dyn DBIter + 'a>, key_manager: Arc<RwLock<KeyManager>>) -> Self {
        Self { inner, key_manager }
    }
}

impl DBIter for DecryptingIter<'_> {
    fn seek(&mut self, key: Slice<'_>) -> Result<Option<Slice<'_>>> {
        self.inner.seek(key)
    }

    fn next(&mut self) -> Result<Option<Slice<'_>>> {
        self.inner.next()
    }

    fn read(&self) -> Result<Slice<'_>> {
        let encrypted = self.inner.read()?;
        let decrypted = self
            .key_manager
            .write()
            .map_err(|e| eyre::eyre!("Lock poisoned: {e}"))?
            .decrypt(encrypted.as_ref())?;

        // Return an owned Slice from the decrypted data
        Ok(Slice::from(decrypted.into_boxed_slice()))
    }
}
