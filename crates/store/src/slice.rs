use std::fmt;
use std::ops::Deref;
use std::rc::Rc;

use calimero_primitives::reflect::{DynReflect, Reflect, ReflectExt};

#[derive(Clone, Debug)]
enum SliceInner<'a> {
    Ref(&'a [u8]),
    Box(Rc<Box<[u8]>>),
    Any(Rc<dyn BufRef + 'a>),
}

#[derive(Clone, Debug)]
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

impl<'a> fmt::Debug for dyn BufRef + 'a {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.type_name())
    }
}

impl<'a> Slice<'a> {
    /// Create a new `Slice` from an owned value.
    pub fn from_owned<T: AsRef<[u8]> + 'a>(inner: T) -> Self {
        Self {
            inner: SliceInner::Any(Rc::new(inner) as _),
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

impl<'a> Deref for Slice<'a> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl<'a> AsRef<[u8]> for Slice<'a> {
    fn as_ref(&self) -> &[u8] {
        match &self.inner {
            SliceInner::Ref(inner) => inner,
            SliceInner::Box(inner) => inner.as_ref(),
            SliceInner::Any(inner) => inner.buf(),
        }
    }
}

impl<'a, T: AsRef<[u8]>> From<&'a T> for Slice<'a> {
    fn from(inner: &'a T) -> Self {
        Self {
            inner: SliceInner::Ref(inner.as_ref()),
        }
    }
}

impl<'a> From<&'a [u8]> for Slice<'a> {
    fn from(inner: &'a [u8]) -> Self {
        Self {
            inner: SliceInner::Ref(inner),
        }
    }
}

impl<'a> From<Box<[u8]>> for Slice<'a> {
    fn from(inner: Box<[u8]>) -> Self {
        Self {
            inner: SliceInner::Box(inner.into()),
        }
    }
}

impl<'a> From<Vec<u8>> for Slice<'a> {
    fn from(inner: Vec<u8>) -> Self {
        Self {
            inner: SliceInner::Box(Rc::new(inner.into())),
        }
    }
}

impl<'a> From<Rc<Box<[u8]>>> for Slice<'a> {
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

impl<'a> PartialEq for Slice<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.as_ref() == other.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slice_slice() {
        let data = b"hello";
        let slice = Slice::from(&data[..]);

        assert_eq!(slice.as_ref(), data);
        assert_eq!(&*slice.into_boxed(), data);
    }

    #[test]
    fn test_slice_vec() {
        let data = vec![0; 5];
        let slice = Slice::from(data);

        assert_eq!(slice.as_ref(), [0; 5]);
        assert_eq!(&*slice.into_boxed(), [0; 5]);
    }

    #[test]
    fn test_slice_box() {
        let data: Box<[u8]> = Box::new([0; 5]);
        let slice = Slice::from(data);

        assert_eq!(slice.as_ref(), [0; 5]);
        assert_eq!(&*slice.into_boxed(), [0; 5]);
    }

    #[test]
    fn test_slice_any() {
        struct Buf<'a>(&'a [u8]);

        impl AsRef<[u8]> for Buf<'_> {
            fn as_ref(&self) -> &[u8] {
                self.0
            }
        }

        let data = Buf(b"hello");
        let slice = Slice::from_owned(data);

        assert_eq!(slice.as_ref(), b"hello");
        assert_eq!(&*slice.into_boxed(), b"hello");
    }

    #[test]
    fn test_owned_slice() {
        let data = b"hello";
        let slice = Slice::from_owned(&data[..]);

        let slice = slice.take_owned::<[u8; 5]>().unwrap_err();
        let slice = slice.take_owned::<&[u8; 5]>().unwrap_err();
        let slice = slice.take_owned::<Vec<u8>>().unwrap_err();
        let slice = slice.take_owned::<Box<[u8]>>().unwrap_err();

        let slice = slice.take_owned::<&[u8]>().unwrap();

        assert_eq!(*slice, data);
    }

    #[test]
    fn test_owned_array() {
        let data = [0; 5];
        let slice = Slice::from_owned(data);

        let slice = slice.take_owned::<&[u8]>().unwrap_err();
        let slice = slice.take_owned::<&[u8; 5]>().unwrap_err();
        let slice = slice.take_owned::<Vec<u8>>().unwrap_err();
        let slice = slice.take_owned::<Box<[u8]>>().unwrap_err();

        let slice = slice.take_owned::<[u8; 5]>().unwrap();

        assert_eq!(*slice, data);
    }

    #[test]
    fn test_owned_vec() {
        let data = vec![0; 5];
        let slice = Slice::from_owned(data);

        let slice = slice.take_owned::<&[u8]>().unwrap_err();
        let slice = slice.take_owned::<&[u8; 5]>().unwrap_err();
        let slice = slice.take_owned::<[u8; 5]>().unwrap_err();
        let slice = slice.take_owned::<Box<[u8]>>().unwrap_err();

        let slice = slice.take_owned::<Vec<u8>>().unwrap();

        assert_eq!(*slice, [0; 5]);
    }

    #[test]
    fn test_owned_any() {
        struct Buf<'a>(&'a [u8]);

        impl AsRef<[u8]> for Buf<'_> {
            fn as_ref(&self) -> &[u8] {
                self.0
            }
        }

        let data = Buf(b"hello");
        let slice = Slice::from_owned(data);

        let slice = slice.take_owned::<Buf>().unwrap();

        assert_eq!(slice.0, b"hello");
    }
}
