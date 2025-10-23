use super::XCall;
use crate::Buffer;

impl<'a> XCall<'a> {
    #[inline]
    pub fn new(_context_id: &'a [u8; 32], _function: &'a str, _params: &'a [u8]) -> Self {
        unimplemented!("XCall construction is only permitted in wasm32")
    }

    #[inline]
    pub fn context_id(&self) -> &Buffer<'a> {
        &self.context_id
    }

    #[inline]
    pub fn function(&self) -> &Buffer<'a> {
        &self.function
    }

    #[inline]
    pub fn params(&self) -> &Buffer<'a> {
        &self.params
    }
}

