use super::{Pointer, PtrSized};

#[repr(C)]
#[derive(Eq, Copy, Clone, Debug, PartialEq)]
pub struct Buffer<'a> {
    ptr: PtrSized<Pointer<&'a u8>>,
    len: u64,
}

impl<'a> Buffer<'a> {
    pub fn new(ptr: PtrSized<Pointer<&'a u8>>, len: usize) -> Self {
        Self { ptr, len: len as _ }
    }

    pub fn len(&self) -> usize {
        self.len as _
    }

    pub fn ptr(&self) -> *const u8 {
        self.ptr.into()
    }

    pub fn empty() -> Self {
        Self::new(PtrSized::null(), 0)
    }
}

impl From<&[u8]> for Buffer<'_> {
    fn from(slice: &[u8]) -> Self {
        Self::new(slice.as_ptr().into(), slice.len())
    }
}

impl From<Buffer<'_>> for &[u8] {
    fn from(buffer: Buffer<'_>) -> Self {
        unsafe { std::slice::from_raw_parts(buffer.ptr(), buffer.len()) }
    }
}

impl From<&str> for Buffer<'_> {
    fn from(string: &str) -> Self {
        Self::from(string.as_bytes())
    }
}

impl TryFrom<Buffer<'_>> for &str {
    type Error = std::str::Utf8Error;

    fn try_from(buffer: Buffer<'_>) -> Result<Self, Self::Error> {
        std::str::from_utf8(buffer.into())
    }
}
