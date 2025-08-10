use core::panic;

use super::Location;

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
    pub fn file(&self) -> &str {
        self.file
            .try_into()
            .expect("this should always be a valid utf8 string") // todo! test if this pulls in format code
    }
}

impl<'a> From<Option<&'a panic::Location<'_>>> for Location<'a> {
    #[inline]
    fn from(_location: Option<&'a panic::Location<'_>>) -> Self {
        unimplemented!("Slice construction is only permitted in wasm32")
    }
}
