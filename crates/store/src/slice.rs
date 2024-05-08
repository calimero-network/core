#[derive(Debug)]
enum SliceInner<'a> {
    Ref(&'a [u8]),
    Box(Box<[u8]>),
    Any(Box<dyn buf::BufRef + 'a>),
}

#[derive(Debug)]
pub struct Slice<'a> {
    inner: SliceInner<'a>,
}

mod buf {
    use std::fmt;

    pub struct Buf<T>(pub T);

    pub trait BufRef {
        fn id(&self) -> usize;
        fn name(&self) -> &'static str {
            std::any::type_name::<Self>()
        }
        fn buf(&self) -> &[u8];
    }

    #[inline(never)]
    pub fn type_id_of<T: ?Sized>() -> usize {
        type_id_of::<T> as usize
    }

    impl<'a, T: AsRef<[u8]> + 'a> BufRef for Buf<T> {
        fn id(&self) -> usize {
            type_id_of::<T>()
        }

        fn buf(&self) -> &[u8] {
            self.0.as_ref()
        }
    }

    impl<'a> fmt::Debug for dyn BufRef + 'a {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.name())
        }
    }
}

impl<'a> Slice<'a> {
    /// Create a new `Slice` from an owned value.
    pub fn from_owned<T: AsRef<[u8]> + 'a>(inner: T) -> Self {
        Self {
            inner: SliceInner::Any(Box::new(buf::Buf(inner)) as _),
        }
    }

    /// Take the inner value if it is of the correct type passed in via `from_owned`.
    pub fn take_owned<T: AsRef<[u8]> + 'a>(self) -> Result<T, Self> {
        match self.inner {
            SliceInner::Any(inner) if inner.id() == buf::type_id_of::<T>() => {
                Ok(*unsafe { Box::from_raw(Box::into_raw(inner) as *mut T) })
            }
            _ => Err(self),
        }
    }

    pub fn into_owned(self) -> Box<[u8]> {
        match self.take_owned() {
            Ok(inner) => inner,
            Err(slice) => match slice.inner {
                SliceInner::Ref(inner) => inner.into(),
                SliceInner::Box(inner) => inner,
                SliceInner::Any(inner) => inner.buf().into(),
            },
        }
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
            inner: SliceInner::Box(inner.into()),
        }
    }
}

impl<'a> From<Slice<'a>> for Box<[u8]> {
    fn from(slice: Slice<'a>) -> Self {
        slice.into_owned()
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
        assert_eq!(&*slice.into_owned(), data);
    }

    #[test]
    fn test_slice_vec() {
        let data = vec![0; 5];
        let slice = Slice::from(data);

        assert_eq!(slice.as_ref(), [0; 5]);
        assert_eq!(&*slice.into_owned(), [0; 5]);
    }

    #[test]
    fn test_slice_box() {
        let data: Box<[u8]> = Box::new([0; 5]);
        let slice = Slice::from(data);

        assert_eq!(slice.as_ref(), [0; 5]);
        assert_eq!(&*slice.into_owned(), [0; 5]);
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
        assert_eq!(&*slice.into_owned(), b"hello");
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

        assert_eq!(slice, data);
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

        assert_eq!(slice, data);
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

        assert_eq!(slice, [0; 5]);
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
