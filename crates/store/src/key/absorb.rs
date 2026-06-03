use core::convert::Infallible;
use core::fmt::{self, Debug, Formatter};

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use generic_array::sequence::Concat;
use generic_array::typenum::{U1, U32};
use generic_array::GenericArray;

use crate::db::Column;
use crate::key::component::KeyComponent;
use crate::key::{AsKeyParts, FromKeyParts, Key};

/// Prefix byte for the absorb buffer (PR-6b straggler safety).
///
/// Lives in its own [`Column::AbsorbBuffer`] CF, so the byte only has to be
/// distinct within that CF — `0x4A` is kept for grep-ability (the `Group` CF
/// prefixes occupy a contiguous `0x20`–`0x3C` band, so `0x4A` is well clear).
pub const ABSORB_BUFFER_PREFIX: u8 = 0x4A;

#[derive(Clone, Copy, Debug)]
pub struct AbsorbPrefix;

impl KeyComponent for AbsorbPrefix {
    type LEN = U1;
}

#[derive(Clone, Copy, Debug)]
pub struct ContextIdComponent;

impl KeyComponent for ContextIdComponent {
    type LEN = U32;
}

#[derive(Clone, Copy, Debug)]
pub struct AppKeyComponent;

impl KeyComponent for AppKeyComponent {
    type LEN = U32;
}

#[derive(Clone, Copy, Debug)]
pub struct DeltaIdComponent;

impl KeyComponent for DeltaIdComponent {
    type LEN = U32;
}

/// Key for a buffered (absorbed) straggler delta:
/// `prefix(1) ‖ context_id(32) ‖ producing_app_key(32) ‖ delta_id(32)` = 97 bytes.
///
/// The `delta_id` lives in the key so a re-delivered straggler delta overwrites
/// rather than duplicates (idempotent absorb). The `context_id` prefix makes the
/// per-context recovery scan a contiguous range walk.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct AbsorbBufferKey(
    Key<(
        AbsorbPrefix,
        ContextIdComponent,
        AppKeyComponent,
        DeltaIdComponent,
    )>,
);

impl AbsorbBufferKey {
    #[must_use]
    pub fn new(context_id: [u8; 32], producing_app_key: [u8; 32], delta_id: [u8; 32]) -> Self {
        Self(Key(GenericArray::from([ABSORB_BUFFER_PREFIX])
            .concat(GenericArray::from(context_id))
            .concat(GenericArray::from(producing_app_key))
            .concat(GenericArray::from(delta_id))))
    }

    #[must_use]
    pub fn context_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 97]>::as_ref(&self.0)[1..33]);
        id
    }

    #[must_use]
    pub fn producing_app_key(&self) -> [u8; 32] {
        let mut key = [0; 32];
        key.copy_from_slice(&AsRef::<[_; 97]>::as_ref(&self.0)[33..65]);
        key
    }

    #[must_use]
    pub fn delta_id(&self) -> [u8; 32] {
        let mut id = [0; 32];
        id.copy_from_slice(&AsRef::<[_; 97]>::as_ref(&self.0)[65..97]);
        id
    }
}

impl AsKeyParts for AbsorbBufferKey {
    type Components = (
        AbsorbPrefix,
        ContextIdComponent,
        AppKeyComponent,
        DeltaIdComponent,
    );

    fn column() -> Column {
        Column::AbsorbBuffer
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for AbsorbBufferKey {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for AbsorbBufferKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("AbsorbBufferKey")
            .field("context_id", &self.context_id())
            .field("producing_app_key", &self.producing_app_key())
            .field("delta_id", &self.delta_id())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absorb_key_round_trips_three_components() {
        let k = AbsorbBufferKey::new([1; 32], [2; 32], [3; 32]);
        assert_eq!(k.context_id(), [1; 32]);
        assert_eq!(k.producing_app_key(), [2; 32]);
        assert_eq!(k.delta_id(), [3; 32]);
        assert_eq!(AbsorbBufferKey::column(), Column::AbsorbBuffer);
    }
}
