use std::marker::PhantomData;
use std::ptr;

use crate::sys::PtrSizedInt;
 
#[repr(C)]
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
