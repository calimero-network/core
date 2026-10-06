use camino::Utf8PathBuf;
use zeroize::Zeroizing;

#[derive(Debug)]
#[non_exhaustive]
pub struct StoreConfig {
    pub path: Utf8PathBuf,
    /// Optional encryption key for at-rest encryption.
    /// When set, all values stored in the database will be encrypted.
    ///
    /// Held in a [`Zeroizing`] wrapper so the key encryption key is wiped
    /// from the heap when the config is dropped instead of lingering for the
    /// whole process lifetime. The bytes are handed to the encryption layer's
    /// `KeyManager` (which is itself `ZeroizeOnDrop`) on open.
    pub encryption_key: Option<Zeroizing<Vec<u8>>>,
}

impl StoreConfig {
    #[must_use]
    pub fn new(path: Utf8PathBuf) -> Self {
        Self {
            path,
            encryption_key: None,
        }
    }

    /// Create a new config with encryption enabled.
    #[must_use]
    pub fn with_encryption(path: Utf8PathBuf, encryption_key: Zeroizing<Vec<u8>>) -> Self {
        Self {
            path,
            encryption_key: Some(encryption_key),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_omits_the_encryption_key() {
        let key = vec![0xa7; 32];
        let config = StoreConfig::with_encryption("db".into(), Zeroizing::new(key.clone()));
        let shown = format!("{config:?}");

        assert!(!shown.contains(&format!("{key:?}")), "{shown}");
        assert!(shown.contains("redacted"), "{shown}");
    }
}
