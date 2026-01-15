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
    /// Cache for the decrypted value to manage lifetimes.
    cached_decrypted: RwLock<Option<Vec<u8>>>,
}

impl<'a> DecryptingIter<'a> {
    /// Create a new decrypting iterator.
    pub fn new(inner: Box<dyn DBIter + 'a>, key_manager: Arc<RwLock<KeyManager>>) -> Self {
        Self {
            inner,
            key_manager,
            cached_decrypted: RwLock::new(None),
        }
    }
}

impl DBIter for DecryptingIter<'_> {
    fn seek(&mut self, key: Slice<'_>) -> Result<Option<Slice<'_>>> {
        // Invalidate cached value on seek
        if let Ok(mut cache) = self.cached_decrypted.write() {
            cache.take();
        }
        self.inner.seek(key)
    }

    fn next(&mut self) -> Result<Option<Slice<'_>>> {
        // Invalidate cached value on next
        if let Ok(mut cache) = self.cached_decrypted.write() {
            cache.take();
        }
        self.inner.next()
    }

    fn read(&self) -> Result<Slice<'_>> {
        let encrypted = self.inner.read()?;
        let decrypted = self
            .key_manager
            .write()
            .map_err(|e| eyre::eyre!("Lock poisoned: {e}"))?
            .decrypt(encrypted.as_ref())?;

        // Cache the decrypted value
        if let Ok(mut cache) = self.cached_decrypted.write() {
            *cache = Some(decrypted);
        }

        // Return slice from cache
        let cache = self
            .cached_decrypted
            .read()
            .map_err(|e| eyre::eyre!("Lock poisoned: {e}"))?;

        let bytes = cache.as_ref().expect("just set").as_slice();

        // Safety: The cached value lives as long as self. We transmute to extend
        // the lifetime since RwLockReadGuard's lifetime doesn't match what we need.
        let bytes: &[u8] = unsafe { std::mem::transmute::<&[u8], &[u8]>(bytes) };

        Ok(Slice::from(bytes))
    }
}
