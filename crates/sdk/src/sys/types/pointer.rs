use std::marker::PhantomData;

#[repr(C)]
#[derive(Eq, Ord, Copy, Hash, Clone, Debug, PartialEq, PartialOrd)]

pub struct PtrSizedInt {
    value: u64,
}

impl PtrSizedInt {
    pub const MAX: Self = Self { value: u64::MAX };

    pub const fn new(value: usize) -> Self {
        Self { value: value as _ }
    }

    pub const fn as_usize(self) -> usize {
        self.value as _
    }
}

impl From<usize> for PtrSizedInt {
    fn from(value: usize) -> Self {
        Self::new(value)
    }
}

#[repr(C)]
#[derive(Eq, Ord, Copy, Hash, Clone, Debug, PartialEq, PartialOrd)]
pub struct Pointer<T> {
    value: PtrSizedInt,
    _phantom: PhantomData<T>,
}

impl<T> Pointer<T> {
    pub fn new(ptr: *const T) -> Self {
        Self {
            value: PtrSizedInt::new(ptr as _),
            _phantom: PhantomData,
        }
    }

    pub fn null() -> Self {
        Self::new(std::ptr::null())
    }

    pub fn as_ptr(&self) -> *const T {
        self.value.as_usize() as _
    }

    pub fn as_mut_ptr(&self) -> *mut T {
        self.value.as_usize() as _
    }
}

impl<T> From<*const T> for Pointer<T> {
    fn from(ptr: *const T) -> Self {
        Self::new(ptr)
    }
}

impl<T> From<*mut T> for Pointer<T> {
    fn from(ptr: *mut T) -> Self {
        Self::new(ptr)
    }
}
