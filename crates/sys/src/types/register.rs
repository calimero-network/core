use super::PtrSizedInt;

#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RegisterId(PtrSizedInt);

impl RegisterId {
    #[inline]
    pub const fn new(value: usize) -> Self {
        Self(PtrSizedInt::new(value))
    }

    pub const fn as_usize(self) -> usize {
        self.0.as_usize()
    }
}

impl From<usize> for RegisterId {
    #[inline]
    fn from(value: usize) -> Self {
        Self::new(value)
    }
}
