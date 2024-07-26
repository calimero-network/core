use super::PtrSizedInt;

#[repr(C)]
#[derive(Eq, Ord, Copy, Hash, Clone, Debug, PartialEq, PartialOrd)]
pub struct RegisterId(PtrSizedInt);

impl RegisterId {
    #[inline(always)]
    pub const fn new(value: usize) -> Self {
        Self(PtrSizedInt::new(value))
    }

    pub fn as_usize(&self) -> usize {
        self.0.as_usize()
    }
}

impl From<usize> for RegisterId {
    #[inline(always)]
    fn from(value: usize) -> Self {
        Self::new(value)
    }
}
