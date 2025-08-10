use super::Event;
use crate::Buffer;

impl<'a> Event<'a> {
    #[inline]
    pub fn new(_kind: &'a str, _data: &'a [u8]) -> Self {
        unimplemented!("Slice construction is only permitted in wasm32")
    }

    #[inline]
    pub fn kind(&self) -> &Buffer<'a> {
        &self.kind
    }

    #[inline]
    pub fn data(&self) -> &Buffer<'a> {
        &self.data
    }
}
