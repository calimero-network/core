use super::Buffer;

#[repr(C)]
pub struct Location<'a> {
    file: Buffer<'a>,
    line: u32,
    column: u32,
}

impl<'a> Location<'a> {
    #[inline(always)]
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
        std::panic::Location::caller().into()
    }

    #[inline]
    pub fn file(&self) -> &str {
        self.file
            .try_into()
            .expect("this should always be a valid utf8 string") // todo! test if this pulls in format code
    }

    #[inline(always)]
    pub fn line(&self) -> u32 {
        self.line
    }

    #[inline(always)]
    pub fn column(&self) -> u32 {
        self.column
    }
}

impl<'a> From<&'a std::panic::Location<'_>> for Location<'a> {
    #[inline]
    fn from(location: &'a std::panic::Location<'_>) -> Self {
        Location {
            file: Buffer::from(location.file()),
            line: location.line(),
            column: location.column(),
        }
    }
}

impl<'a> From<Option<&'a std::panic::Location<'_>>> for Location<'a> {
    #[inline]
    fn from(location: Option<&'a std::panic::Location<'_>>) -> Self {
        location.map_or_else(Location::unknown, Location::from)
    }
}
