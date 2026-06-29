use core::panic;

use super::Location;
use crate::Buffer;

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

    // Returns a borrow (not an owned descriptor): `Slice` is move-only, so the
    // field cannot be copied out of `&self`.
    #[inline]
    pub fn file(&self) -> &Buffer<'_> {
        &self.file
    }
}

impl<'a> From<Option<&'a panic::Location<'_>>> for Location<'a> {
    #[inline]
    fn from(_location: Option<&'a panic::Location<'_>>) -> Self {
        unimplemented!("Slice construction is only permitted in wasm32")
    }
}
