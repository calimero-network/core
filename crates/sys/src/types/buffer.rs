use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::str::{from_utf8, Utf8Error};

use super::Pointer;

#[cfg(target_arch = "wasm32")]
mod guest;

#[cfg(not(target_arch = "wasm32"))]
mod host;

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Slice<'a, T> {
    ptr: Pointer<T>,
    len: u64,
    _phantom: PhantomData<&'a T>,
}

pub type Buffer<'a> = Slice<'a, u8>;
pub type BufferMut<'a> = Buffer<'a>;

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

impl<'a> TryFrom<Buffer<'a>> for &'a str {
    type Error = Utf8Error;

    fn try_from(buf: Buffer<'a>) -> Result<Self, Self::Error> {
        from_utf8(buf.as_slice())
    }
}
