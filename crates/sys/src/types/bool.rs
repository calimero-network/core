#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Bool(u32);

impl TryFrom<Bool> for bool {
    type Error = u32;

    #[inline]
    fn try_from(value: Bool) -> Result<Self, Self::Error> {
        match value {
            Bool(0) => Ok(false),
            Bool(1) => Ok(true),
            Bool(x) => Err(x),
        }
    }
}
