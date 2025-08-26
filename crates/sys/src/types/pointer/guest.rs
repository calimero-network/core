use std::marker::PhantomData;

use super::{Pointer, PtrSizedInt};

impl<T> Pointer<T> {
    #[inline]
    pub fn new(ptr: *const T) -> Self {
        Self {
            value: PtrSizedInt::new(ptr as usize),
            _phantom: PhantomData,
        }
    }

    #[inline]
    pub const fn as_ptr(&self) -> *const T {
        self.value.as_usize() as *const T
    }

    #[inline]
    pub const fn as_mut_ptr(&self) -> *mut T {
        self.value.as_usize() as *mut T
    }
}
