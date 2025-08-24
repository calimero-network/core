use core::marker::PhantomData;
use core::ptr;

#[cfg(target_arch = "wasm32")]
mod guest;

#[cfg(not(target_arch = "wasm32"))]
mod host;

#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PtrSizedInt {
    value: u64,
}

impl PtrSizedInt {
    pub const MAX: Self = Self { value: u64::MAX };

    #[inline]
    pub const fn new(value: usize) -> Self {
        Self {
            value: value as u64,
        }
    }

    #[inline]
    pub const fn as_usize(self) -> usize {
        self.value as usize
    }
}

impl From<usize> for PtrSizedInt {
    #[inline]
    fn from(value: usize) -> Self {
        Self::new(value)
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Pointer<T> {
    value: PtrSizedInt,
    _phantom: PhantomData<T>,
}

impl<T> Pointer<T> {
    #[inline]
    pub fn null() -> Self {
        Self::new(ptr::null())
    }
}

impl<T> From<*const T> for Pointer<T> {
    #[inline]
    fn from(ptr: *const T) -> Self {
        Self::new(ptr)
    }
}

impl<T> From<*mut T> for Pointer<T> {
    #[inline]
    fn from(ptr: *mut T) -> Self {
        Self::new(ptr)
    }
}
