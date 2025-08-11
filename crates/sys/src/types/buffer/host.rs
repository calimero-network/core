use super::Slice;
use crate::{Buffer, Pointer};

impl<'a, T> Slice<'a, T> {
    #[inline]
    pub const fn ptr(&self) -> &Pointer<T> {
        &self.ptr
    }

    #[inline]
    pub const fn len(&self) -> u64 {
        self.len
    }

    #[inline]
    pub fn new<U: AsRef<[T]> + 'a>(_value: U) -> Self {
        unimplemented!("Slice construction is only permitted in wasm32")
    }

    #[inline]
    pub fn empty() -> Self {
        unimplemented!("Slice construction is only permitted in wasm32")
    }

    #[inline]
    pub(crate) fn as_slice(&self) -> &'a [T] {
        unimplemented!("Slice construction is only permitted in wasm32")
    }

    #[inline]
    pub(crate) fn as_mut_slice(&mut self) -> &'a mut [T] {
        unimplemented!("Slice construction is only permitted in wasm32")
    }
}

impl<'a> From<&'a str> for Buffer<'a> {
    #[inline]
    fn from(_buf: &'a str) -> Self {
        unimplemented!("Slice construction is only permitted in wasm32")
    }
}
