#[cfg(test)]
#[path = "tests/slice.rs"]
mod tests;

use core::borrow::Borrow;
use core::cmp::Ordering;
use core::fmt::{self, Debug, Formatter};
use core::ops::Deref;
use std::rc::Rc;

use calimero_primitives::reflect::{DynReflect, Reflect, ReflectExt};

#[derive(Clone, Debug)]
enum SliceInner<'a> {
    Ref(&'a [u8]),
    Box(Rc<Box<[u8]>>),
    Any(Rc<dyn BufRef + 'a>),
}

#[derive(Clone)]
pub struct Slice<'a> {
    inner: SliceInner<'a>,
}

trait BufRef: Reflect {
    fn buf(&self) -> &[u8];
}

impl<'a, T: AsRef<[u8]> + 'a> BufRef for T {
    fn buf(&self) -> &[u8] {
        self.as_ref()
    }
}

impl Debug for dyn BufRef + '_ {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.type_name())
    }
}

impl<'a> Slice<'a> {
    /// Create a new `Slice` from an owned value.
    pub fn from_owned<T: AsRef<[u8]> + 'a>(inner: T) -> Self {
        Self {
            inner: SliceInner::Any(Rc::new(inner)),
        }
    }

    pub fn into_boxed(self) -> Box<[u8]> {
        let ref_boxed = match self.inner {
            SliceInner::Ref(inner) => return inner.into(),
            SliceInner::Box(inner) => inner,
            SliceInner::Any(inner) => match inner.with_rc(<dyn Reflect>::downcast_rc) {
                Ok(inner) => inner,
                Err(inner) => return inner.buf().into(),
            },
        };

        Rc::try_unwrap(ref_boxed).unwrap_or_else(|inner| (*inner).clone())
    }

    #[must_use]
    pub fn owned_ref<T: AsRef<[u8]>>(&'a self) -> Option<&'a T> {
        if let SliceInner::Any(inner) = &self.inner {
            if let Some(inner) = inner.as_dyn().downcast_ref::<T>() {
                return Some(inner);
            }
        }
        None
    }

    /// Take the inner value if it is of the correct type passed in via `from_owned`.
    pub fn take_owned<T: AsRef<[u8]> + 'a>(self) -> Result<Rc<T>, Self> {
        if let SliceInner::Any(inner) = self.inner {
            return match inner.with_rc(<dyn Reflect>::downcast_rc) {
                Ok(inner) => Ok(inner),
                Err(inner) => Err(Self {
                    inner: SliceInner::Any(inner),
                }),
            };
        };

        Err(self)
    }
}

impl Deref for Slice<'_> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl AsRef<[u8]> for Slice<'_> {
    fn as_ref(&self) -> &[u8] {
        match &self.inner {
            SliceInner::Ref(inner) => inner,
            SliceInner::Box(inner) => inner.as_ref(),
            SliceInner::Any(inner) => inner.buf(),
        }
    }
}

impl<'a, T: AsRef<[u8]> + ?Sized> From<&'a T> for Slice<'a> {
    fn from(inner: &'a T) -> Self {
        Self {
            inner: SliceInner::Ref(inner.as_ref()),
        }
    }
}

impl From<Box<[u8]>> for Slice<'_> {
    fn from(inner: Box<[u8]>) -> Self {
        Self {
            inner: SliceInner::Box(inner.into()),
        }
    }
}

impl From<Vec<u8>> for Slice<'_> {
    fn from(inner: Vec<u8>) -> Self {
        Self {
            inner: SliceInner::Box(Rc::new(inner.into())),
        }
    }
}

impl From<Rc<Box<[u8]>>> for Slice<'_> {
    fn from(inner: Rc<Box<[u8]>>) -> Self {
        Self {
            inner: SliceInner::Box(inner),
        }
    }
}

impl<'a> From<Slice<'a>> for Box<[u8]> {
    fn from(slice: Slice<'a>) -> Self {
        slice.into_boxed()
    }
}

impl Eq for Slice<'_> {}
impl<T: AsRef<[u8]> + ?Sized> PartialEq<T> for Slice<'_> {
    fn eq(&self, other: &T) -> bool {
        self.as_ref() == other.as_ref()
    }
}

impl Ord for Slice<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_ref().cmp(other.as_ref())
    }
}

impl<'a> PartialOrd<Slice<'a>> for Slice<'_> {
    fn partial_cmp(&self, other: &Slice<'a>) -> Option<Ordering> {
        self.as_ref().partial_cmp(other.as_ref())
    }
}

impl Debug for Slice<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            f.debug_tuple("Slice").field(&self.inner).finish()
        } else {
            write!(f, "{:?}", self.as_ref())
        }
    }
}

impl Borrow<[u8]> for Slice<'_> {
    fn borrow(&self) -> &[u8] {
        self
    }
}
