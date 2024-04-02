use super::{Pointer, PtrSized};

#[repr(C)]
#[derive(Eq, Copy, Clone, Debug, PartialEq)]
pub struct Buffer<'a> {
    len: u64,
    ptr: PtrSized<Pointer<&'a u8>>,
}

impl<'a> Buffer<'a> {
    pub fn new(len: usize, ptr: PtrSized<Pointer<&'a u8>>) -> Self {
        Self { len: len as _, ptr }
    }

    pub fn len(&self) -> usize {
        self.len as _
    }

    pub fn ptr(&self) -> *const u8 {
        self.ptr.into()
    }
}

impl From<&[u8]> for Buffer<'_> {
    fn from(slice: &[u8]) -> Self {
        Self::new(slice.len(), slice.as_ptr().into())
    }
}

impl From<&str> for Buffer<'_> {
    fn from(string: &str) -> Self {
        Self::from(string.as_bytes())
    }
}
