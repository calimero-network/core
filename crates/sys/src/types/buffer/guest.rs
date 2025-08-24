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

    #[inline]
    pub(crate) fn as_slice(&self) -> &'a [T] {
        unsafe { from_raw_parts(self.ptr.as_ptr(), self.len()) }
    }

    #[inline]
    pub(crate) fn as_mut_slice(&mut self) -> &'a mut [T] {
        unsafe { from_raw_parts_mut(self.ptr.as_mut_ptr(), self.len()) }
    }
}

impl<'a> From<&'a str> for Buffer<'a> {
    #[inline]
    fn from(buf: &'a str) -> Self {
        Self::new(buf)
    }
}
