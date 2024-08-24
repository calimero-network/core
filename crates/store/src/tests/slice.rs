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

    let slice = slice.take_owned::<Buf<'_>>().unwrap();

    assert_eq!(slice.0, b"hello");
}
