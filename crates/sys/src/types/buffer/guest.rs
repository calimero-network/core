use core::marker::PhantomData;
use core::slice::{from_raw_parts, from_raw_parts_mut};

use super::Slice;
use crate::{Buffer, Pointer};

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

    #[inline]
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    // The borrowing accessors deliberately tie the returned slice to the borrow
    // of `self`, NOT to `'a`. Returning `&'a`/`&'a mut` here would let safe SDK
    // code mint two aliasing `&mut [T]` (two `as_mut` calls), or a `&[T]` and a
    // `&mut [T]` over the same bytes (`as_ref` then `as_mut`) — instant UB. With
    // the borrow tied to `self` AND `Slice` being move-only, the borrow checker
    // serialises every access through the one live descriptor.
    #[inline]
    pub(crate) fn as_slice(&self) -> &[T] {
        // SAFETY: a `Slice` is only ever constructed from a live `&[T]`/`&mut [T]`
        // (`Slice::new`/`From`) or the empty/null descriptor, so `ptr` is non-null
        // and valid for `len` elements; the returned borrow is tied to `self`, so
        // it cannot outlive that backing storage.
        unsafe { from_raw_parts(self.ptr.as_ptr(), self.len()) }
    }

    #[inline]
    pub(crate) fn as_mut_slice(&mut self) -> &mut [T] {
        // SAFETY: as in `as_slice`, `ptr` is valid for `len` elements; the
        // descriptor was built from a `&mut [T]` so the bytes are uniquely owned,
        // and `&mut self` (on a move-only `Slice`) guarantees this is the only
        // live handle, so the returned `&mut` does not alias.
        unsafe { from_raw_parts_mut(self.ptr.as_mut_ptr(), self.len()) }
    }

    // Consumes the descriptor to yield the full-`'a` (shared, read-only) slice.
    // Sound because `Slice` is move-only: taking `self` by value leaves no other
    // live descriptor over the same memory, and only a shared reference is handed
    // out — the one place that genuinely needs the `'a` lifetime.
    #[inline]
    pub(crate) fn into_slice(self) -> &'a [T] {
        // SAFETY: same validity invariant as `as_slice`; `self` is consumed, so
        // the `'a` borrow it yields cannot coexist with any other handle.
        unsafe { from_raw_parts(self.ptr.as_ptr(), self.len()) }
    }
}

impl<'a> From<&'a str> for Buffer<'a> {
    #[inline]
    fn from(buf: &'a str) -> Self {
        Self::new(buf)
    }
}
