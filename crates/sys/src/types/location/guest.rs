use core::panic;

use super::Location;
use crate::Buffer;

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
        panic::Location::caller().into()
    }

    #[inline]
    pub fn file(&self) -> &str {
        self.file
            .try_into()
            .expect("this should always be a valid utf8 string") // todo! test if this pulls in format code
    }
}

impl<'a> From<&'a panic::Location<'_>> for Location<'a> {
    #[inline]
    fn from(location: &'a panic::Location<'_>) -> Self {
        Location {
            file: Buffer::from(location.file()),
            line: location.line(),
            column: location.column(),
        }
    }
}

impl<'a> From<Option<&'a panic::Location<'_>>> for Location<'a> {
    #[inline]
    fn from(location: Option<&'a panic::Location<'_>>) -> Self {
        location.map_or_else(Location::unknown, Location::from)
    }
}
