use core::marker::PhantomData;
use core::ptr;

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

    // TODO: This converts from u64 to usize, which may fail on 32-bit systems,
    // TODO: but this function is meant to be infallible. That is a concern, as
    // TODO: we want to eliminate all potential panics. This needs future
    // TODO: assessment.
    #[expect(clippy::cast_possible_truncation, reason = "TODO: See above")]
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
    pub fn new(ptr: *const T) -> Self {
        Self {
            value: PtrSizedInt::new(ptr as usize),
            _phantom: PhantomData,
        }
    }

    #[inline]
    pub fn null() -> Self {
        Self::new(ptr::null())
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
