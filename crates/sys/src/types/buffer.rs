use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::str::{from_utf8, Utf8Error};

use super::Pointer;

#[cfg(target_arch = "wasm32")]
mod guest;

#[cfg(not(target_arch = "wasm32"))]
mod host;

// Deliberately NOT `Copy`/`Clone`. The descriptor owns a raw pointer plus a
// lifetime, and `as_mut_slice` hands out `&mut [T]` over that pointer. If the
// descriptor were duplicable, safe code could copy it and call `as_mut_slice`
// on each copy to obtain two independent `&mut [T]` aliasing the same memory
// (or a shared `into_slice` alongside a `&mut`) — UB the per-borrow lifetimes
// alone cannot prevent. Being move-only makes "I hold this descriptor" unique,
// so the borrow checker can actually serialize access through it.
#[repr(C)]
#[derive(Debug, Eq, PartialEq)]
pub struct Slice<'a, T> {
    ptr: Pointer<T>,
    len: u64,
    _phantom: PhantomData<&'a T>,
}

pub type Buffer<'a> = Slice<'a, u8>;
pub type BufferMut<'a> = Buffer<'a>;

impl Buffer<'_> {
    /// Borrow the buffer's bytes as a UTF-8 `str`, tied to this borrow of
    /// `self`. The sound replacement for the by-value `TryFrom<Buffer> for &str`
    /// when you only have a `&Buffer` (e.g. a field of `Event`/`Location`).
    #[inline]
    pub fn as_str(&self) -> Result<&str, Utf8Error> {
        from_utf8(self.as_slice())
    }
}

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
        // Sound because `Slice` is move-only: consuming `buf` by value leaves no
        // other live descriptor that could hand out a `&mut` aliasing this `'a`
        // slice. `into_slice` is the only path that yields the full `'a`
        // lifetime; the borrowing accessors stay tied to `self`. For a `&Buffer`
        // (no ownership to consume), use [`Buffer::as_str`] instead.
        from_utf8(buf.into_slice())
    }
}
