use super::{Pointer, PtrSizedInt};

impl<T> Pointer<T> {
    #[inline]
    pub fn new(_ptr: *const T) -> Self {
        unimplemented!("Pointer construction is only permitted in wasm32")
    }

    #[inline]
    pub fn as_ptr(&self) -> *const T {
        unimplemented!("Pointer construction is only permitted in wasm32")
    }

    #[inline]
    pub fn as_mut_ptr(&self) -> *mut T {
        unimplemented!("Pointer construction is only permitted in wasm32")
    }

    #[inline]
    pub const fn value(&self) -> PtrSizedInt {
        self.value
    }
}
