use std::marker::PhantomData;

use super::Pointer;

#[repr(C)]
#[derive(Eq, Copy, Clone, Debug, PartialEq)]
pub struct Slice<'a, T> {
    ptr: Pointer<T>,
    len: u64,
    _phantom: PhantomData<&'a T>,
}

impl<'a, T> Slice<'a, T> {
    pub fn new<U: AsRef<[T]> + 'a>(value: U) -> Self {
        let slice = value.as_ref();
        Self {
            ptr: Pointer::new(slice.as_ptr()),
            len: slice.len() as _,
            _phantom: PhantomData,
        }
    }

    pub fn empty() -> Self {
        Self {
            ptr: Pointer::null(),
            len: 0,
            _phantom: PhantomData,
        }
    }

    pub fn len(&self) -> usize {
        self.len as _
    }

    fn as_slice(&self) -> &'a [T] {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len()) }
    }

    fn as_mut_slice(&mut self) -> &'a mut [T] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_mut_ptr(), self.len()) }
    }
}

impl<'a, T> AsRef<[T]> for Slice<'a, T> {
    fn as_ref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<'a, T> AsMut<[T]> for Slice<'a, T> {
    fn as_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<'a, T> From<&'a [T]> for Slice<'a, T> {
    fn from(buf: &'a [T]) -> Self {
        Self::new(buf)
    }
}

impl<'a, T> From<&'a mut [T]> for Slice<'a, T> {
    fn from(buf: &'a mut [T]) -> Self {
        Self::new(buf)
    }
}

impl<'a, T> std::ops::Deref for Slice<'a, T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl<'a, T> std::ops::DerefMut for Slice<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

pub type Buffer<'a> = Slice<'a, u8>;
pub type BufferMut<'a> = Buffer<'a>;

impl<'a> From<&'a str> for Buffer<'a> {
    fn from(buf: &'a str) -> Self {
        Self::new(buf)
    }
}

impl<'a> TryFrom<Buffer<'a>> for &'a str {
    type Error = std::str::Utf8Error;

    fn try_from(buf: Buffer<'a>) -> Result<Self, Self::Error> {
        std::str::from_utf8(buf.as_slice())
    }
}
