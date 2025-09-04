use core::panic;

use super::Location;
use crate::Slice;

impl Location<'_> {
    #[inline]
    pub const fn line(&self) -> u32 {
        self.line
    }

    #[inline]
    pub const fn column(&self) -> u32 {
        self.column
    }
}

impl Location<'_> {
    #[track_caller]
    #[inline]
    pub fn caller() -> Self {
        unimplemented!("Slice construction is only permitted in wasm32")
    }

    #[inline]
    pub fn file(&self) -> Slice<'_, u8> {
        self.file
    }
}

impl<'a> From<Option<&'a panic::Location<'_>>> for Location<'a> {
    #[inline]
    fn from(_location: Option<&'a panic::Location<'_>>) -> Self {
        unimplemented!("Slice construction is only permitted in wasm32")
    }
}
