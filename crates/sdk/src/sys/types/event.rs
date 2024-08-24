use super::Buffer;

#[repr(C)]
pub struct Event<'a> {
    kind: Buffer<'a>,
    data: Buffer<'a>,
}

impl<'a> Event<'a> {
    #[inline]
    pub fn new(kind: &'a str, data: &'a [u8]) -> Self {
        Event {
            kind: Buffer::new(kind),
            data: Buffer::new(data),
        }
    }

    #[inline]
    pub fn kind(&self) -> &str {
        self.kind
            .try_into()
            .expect("this should always be a valid utf8 string") // todo! test if this pulls in format code
    }

    #[inline]
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}
