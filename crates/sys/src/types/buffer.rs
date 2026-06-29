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

impl<T> AsRef<[T]> for Slice<'_, T> {
    #[inline]
    fn as_ref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T> AsMut<[T]> for Slice<'_, T> {
    #[inline]
    fn as_mut(&mut self) -> &mut [T] {
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

impl<T> Deref for Slice<'_, T> {
    type Target = [T];

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl<T> DerefMut for Slice<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

impl<'a> TryFrom<Buffer<'a>> for &'a str {
    type Error = Utf8Error;

    fn try_from(buf: Buffer<'a>) -> Result<Self, Self::Error> {
        // The descriptor is consumed by value, so the `'a` slice cannot alias a
        // live borrow of `buf`. `into_slice` is the only path that hands out the
        // full `'a` lifetime; the borrowing accessors stay tied to `self`.
        from_utf8(buf.into_slice())
    }
}
