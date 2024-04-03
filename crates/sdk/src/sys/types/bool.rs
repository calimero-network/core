#[repr(C)]
#[derive(Eq, Ord, Copy, Hash, Clone, Debug, PartialEq, PartialOrd)]
pub struct Bool(u8);

impl Bool {
    pub const fn as_bool(self) -> Option<bool> {
        match self {
            Self(0) => Some(false),
            Self(1) => Some(true),
            _ => None,
        }
    }
}
