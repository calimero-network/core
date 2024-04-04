use super::{Pointer, PtrSized};

#[repr(C)]
#[derive(Eq, Copy, Clone, Debug, PartialEq)]
pub struct Slice<T> {
    ptr: PtrSized<Pointer<T>>,
    len: u64,
}

pub type Buffer<'a> = Slice<&'a u8>;
pub type BufferMut<'a> = Slice<&'a mut u8>;

impl<T> Slice<T> {
    pub fn new(ptr: PtrSized<Pointer<T>>, len: usize) -> Self {
        Self { ptr, len: len as _ }
    }

    pub fn len(&self) -> usize {
        self.len as _
    }

    pub fn empty() -> Self {
        Self::new(PtrSized::null(), 0)
    }
}

impl Buffer<'_> {
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr.as_ptr()
    }
}

impl BufferMut<'_> {
    pub fn as_ptr(&mut self) -> *const u8 {
        self.ptr.as_ptr()
    }

    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.ptr.as_mut_ptr()
    }
}

impl<T: AsRef<[u8]>> From<T> for Buffer<'_> {
    fn from(value: T) -> Self {
        let slice = value.as_ref();
        Self::new(slice.as_ptr().into(), slice.len())
    }
}

impl<T: AsMut<[u8]>> From<T> for BufferMut<'_> {
    fn from(mut value: T) -> Self {
        let slice = value.as_mut();
        Self::new(slice.as_mut_ptr().into(), slice.len())
    }
}

impl<'a> TryFrom<Buffer<'a>> for &'a str {
    type Error = std::str::Utf8Error;

    fn try_from(buf: Buffer<'a>) -> Result<Self, Self::Error> {
        let buf = unsafe { std::mem::transmute(&*buf) };
        std::str::from_utf8(buf)
    }
}

impl<T> std::ops::Deref for Slice<&T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len()) }
    }
}

impl<'a, T> std::ops::Deref for Slice<&'a mut T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        let downgrade = Slice {
            ptr: self.ptr.as_ptr().into(),
            len: self.len,
        };

        unsafe { std::mem::transmute(&*downgrade) }
    }
}

impl<'a, T> std::ops::DerefMut for Slice<&'a mut T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_mut_ptr(), self.len()) }
    }
}
