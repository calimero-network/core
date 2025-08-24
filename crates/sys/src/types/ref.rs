use std::marker::PhantomData;
use std::ptr;

use crate::PtrSizedInt;

/// Abstraction type that encodes the pointer to any aggregate type T
#[repr(C)]
#[derive(Debug)]
pub struct Ref<T> {
    ptr: PtrSizedInt,
    phantom: PhantomData<T>,
}

impl<T> Ref<T> {
    pub fn new(val: &T) -> Self {
        Ref {
            ptr: PtrSizedInt::new(ptr::from_ref(val).addr()),
            phantom: PhantomData,
        }
    }
}

impl<T> From<T> for Ref<T> {
    fn from(value: T) -> Self {
        Ref::new(&value)
    }
}
