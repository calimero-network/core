pub struct Slice<'a> {
    inner: Box<dyn AsRef<[u8]> + 'a>,
}

impl<'a> Slice<'a> {
    pub fn as_ref(&self) -> &[u8] {
        (*self.inner).as_ref()
    }
}

impl<'a, T: AsRef<[u8]> + 'a> From<T> for Slice<'a> {
    fn from(inner: T) -> Self {
        Self {
            inner: Box::new(inner),
        }
    }
}

impl<'a> TryFrom<Slice<'a>> for Box<[u8]> {
    type Error = Slice<'a>;

    fn try_from(value: Slice<'a>) -> Result<Self, Self::Error> {
        // todo! if the inner is a Box<[u8]>, downcast it to reuse the allocation
        Ok(value.as_ref().to_vec().into_boxed_slice())
    }
}
