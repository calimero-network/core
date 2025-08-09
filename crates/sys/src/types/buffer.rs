use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::slice::{from_raw_parts, from_raw_parts_mut};
use core::str::{from_utf8, Utf8Error};

use super::Pointer;

// TODO: It does not make sense to have Slice.len as a u64 internally, and then
// TODO: cast to usize everywhere, especially as this may fail on 32-bit
// TODO: systems. Therefore, at some point this should be assessed, and ideally
// TODO: the type used would be unified.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Slice<'a, T> {
    ptr: Pointer<T>,
    len: u64,
    _phantom: PhantomData<&'a T>,
}

impl<'a, T> Slice<'a, T> {
    #[inline]
    pub fn new<U: AsRef<[T]> + 'a>(value: U) -> Self {
        let slice = value.as_ref();
        Self {
            ptr: Pointer::new(slice.as_ptr()),
            len: slice.len() as u64,
            _phantom: PhantomData,
        }
    }

    #[inline]
    pub fn empty() -> Self {
        Self {
            ptr: Pointer::null(),
            len: 0,
            _phantom: PhantomData,
        }
    }

    // TODO: This converts from u64 to usize, which may fail on 32-bit systems,
    // TODO: but this function is meant to be infallible. That is a concern, as
    // TODO: we want to eliminate all potential panics. This needs future
    // TODO: assessment.
    #[expect(clippy::cast_possible_truncation, reason = "TODO: See above")]
    #[inline]
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    #[inline]
    const fn as_slice(&self) -> &'a [T] {
        unsafe { from_raw_parts(self.ptr.as_ptr(), self.len()) }
    }

    #[inline]
    fn as_mut_slice(&mut self) -> &'a mut [T] {
        unsafe { from_raw_parts_mut(self.ptr.as_mut_ptr(), self.len()) }
    }
}

impl<'a, T> AsRef<[T]> for Slice<'a, T> {
    #[inline]
    fn as_ref(&self) -> &'a [T] {
        self.as_slice()
    }
}

impl<'a, T> AsMut<[T]> for Slice<'a, T> {
    #[inline]
    fn as_mut(&mut self) -> &'a mut [T] {
        self.as_mut_slice()
    }
}

impl<'a, T> From<&'a [T]> for Slice<'a, T> {
    #[inline]
    fn from(buf: &'a [T]) -> Self {
        Self::new(buf)
    }
}

impl<'a, T> From<&'a mut [T]> for Slice<'a, T> {
    #[inline]
    fn from(buf: &'a mut [T]) -> Self {
        Self::new(buf)
    }
}

impl<'a, T> Deref for Slice<'a, T> {
    type Target = [T];

    #[inline]
    fn deref(&self) -> &'a Self::Target {
        self.as_slice()
    }
}

impl<'a, T> DerefMut for Slice<'a, T> {
    #[inline]
    fn deref_mut(&mut self) -> &'a mut Self::Target {
        self.as_mut_slice()
    }
}

pub type Buffer<'a> = Slice<'a, u8>;
pub type BufferMut<'a> = Buffer<'a>;

impl<'a> From<&'a str> for Buffer<'a> {
    #[inline]
    fn from(buf: &'a str) -> Self {
        Self::new(buf)
    }
}

impl<'a> TryFrom<Buffer<'a>> for &'a str {
    type Error = Utf8Error;

    fn try_from(buf: Buffer<'a>) -> Result<Self, Self::Error> {
        from_utf8(buf.as_slice())
    }
}
