use std::marker::PhantomData;

#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]

pub struct PtrSizedInt {
    value: u64,
}

impl PtrSizedInt {
    pub const MAX: Self = Self { value: u64::MAX };

    #[inline(always)]
    pub const fn new(value: usize) -> Self {
        Self { value: value as _ }
    }

    #[inline(always)]
    pub const fn as_usize(self) -> usize {
        self.value as _
    }
}

impl From<usize> for PtrSizedInt {
    #[inline(always)]
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
    #[inline(always)]
    pub fn new(ptr: *const T) -> Self {
        Self {
            value: PtrSizedInt::new(ptr as _),
            _phantom: PhantomData,
        }
    }

    #[inline(always)]
    pub fn null() -> Self {
        Self::new(std::ptr::null())
    }

    #[inline(always)]
    pub fn as_ptr(&self) -> *const T {
        self.value.as_usize() as _
    }

    #[inline(always)]
    pub fn as_mut_ptr(&self) -> *mut T {
        self.value.as_usize() as _
    }
}

impl<T> From<*const T> for Pointer<T> {
    #[inline(always)]
    fn from(ptr: *const T) -> Self {
        Self::new(ptr)
    }
}

impl<T> From<*mut T> for Pointer<T> {
    #[inline(always)]
    fn from(ptr: *mut T) -> Self {
        Self::new(ptr)
    }
}
