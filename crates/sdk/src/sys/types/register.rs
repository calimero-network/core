use super::{Integer, PtrSized};

#[repr(C)]
#[derive(Eq, Ord, Copy, Hash, Clone, Debug, PartialEq, PartialOrd)]
pub struct RegisterId(PtrSized<Integer>);

impl RegisterId {
    pub const fn new(value: usize) -> Self {
        Self(PtrSized::<Integer>::new(value))
    }
}

impl From<usize> for RegisterId {
    fn from(value: usize) -> Self {
        Self::new(value)
    }
}
