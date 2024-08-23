use core::panic::Location as PanicLocation;

use super::Buffer;

#[repr(C)]
pub struct Location<'a> {
    file: Buffer<'a>,
    line: u32,
    column: u32,
}

impl Location<'_> {
    #[inline]
    pub fn unknown() -> Self {
        Location {
            file: Buffer::empty(),
            line: 0,
            column: 0,
        }
    }

    #[track_caller]
    #[inline]
    pub fn caller() -> Self {
        PanicLocation::caller().into()
    }

    #[inline]
    pub fn file(&self) -> &str {
        self.file
            .try_into()
            .expect("this should always be a valid utf8 string") // todo! test if this pulls in format code
    }

    #[inline]
    pub const fn line(&self) -> u32 {
        self.line
    }

    #[inline]
    pub const fn column(&self) -> u32 {
        self.column
    }
}

impl<'a> From<&'a PanicLocation<'_>> for Location<'a> {
    #[inline]
    fn from(location: &'a PanicLocation<'_>) -> Self {
        Location {
            file: Buffer::from(location.file()),
            line: location.line(),
            column: location.column(),
        }
    }
}

impl<'a> From<Option<&'a PanicLocation<'_>>> for Location<'a> {
    #[inline]
    fn from(location: Option<&'a PanicLocation<'_>>) -> Self {
        location.map_or_else(Location::unknown, Location::from)
    }
}
