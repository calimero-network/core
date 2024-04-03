use std::marker::PhantomData;

#[repr(C)]
#[derive(Eq, Ord, Copy, Hash, Clone, Debug, PartialEq, PartialOrd)]

pub struct PtrSized<T> {
    value: u64,
    _phantom: PhantomData<T>,
}

#[repr(C)]
#[derive(Eq, Ord, Copy, Hash, Clone, Debug, PartialEq, PartialOrd)]
pub struct Pointer<T>(PhantomData<T>);

impl<T> PtrSized<T> {
    pub const MAX: Self = Self {
        value: u64::MAX,
        _phantom: PhantomData,
    };
}

impl<T> PtrSized<Pointer<&T>> {
    pub fn new(ptr: *const T) -> Self {
        Self {
            value: ptr as _,
            _phantom: PhantomData,
        }
    }

    pub fn null() -> Self {
        Self::new(std::ptr::null())
    }
}

impl<T> PtrSized<Pointer<&mut T>> {
    pub fn new(ptr: *mut T) -> Self {
        Self {
            value: ptr as _,
            _phantom: PhantomData,
        }
    }
}

impl<T> From<PtrSized<Pointer<&T>>> for *const T {
    fn from(ptr: PtrSized<Pointer<&T>>) -> Self {
        ptr.value as _
    }
}

impl<T> From<PtrSized<Pointer<&mut T>>> for *mut T {
    fn from(ptr: PtrSized<Pointer<&mut T>>) -> Self {
        ptr.value as _
    }
}

impl<T> From<*const T> for PtrSized<Pointer<&T>> {
    fn from(ptr: *const T) -> Self {
        Self::new(ptr)
    }
}

impl<T> From<*mut T> for PtrSized<Pointer<&mut T>> {
    fn from(ptr: *mut T) -> Self {
        Self::new(ptr)
    }
}

#[derive(Eq, Ord, Copy, Hash, Clone, Debug, PartialEq, PartialOrd)]
pub enum Integer {}

impl PtrSized<Integer> {
    pub const fn new(value: usize) -> Self {
        Self {
            value: value as _,
            _phantom: PhantomData,
        }
    }

    pub const fn as_usize(self) -> usize {
        self.value as _
    }
}

impl From<usize> for PtrSized<Integer> {
    fn from(value: usize) -> Self {
        Self::new(value)
    }
}

impl From<PtrSized<Integer>> for usize {
    fn from(ptr: PtrSized<Integer>) -> Self {
        ptr.value as _
    }
}
