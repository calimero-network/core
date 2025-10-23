use super::XCall;
use crate::Buffer;

impl<'a> XCall<'a> {
    #[inline]
    pub fn new(context_id: &'a [u8; 32], function: &'a str, params: &'a [u8]) -> Self {
        XCall {
            context_id: Buffer::new(context_id),
            function: Buffer::new(function),
            params: Buffer::new(params),
        }
    }

    #[inline]
    pub fn context_id(&self) -> &[u8; 32] {
        self.context_id
            .try_into()
            .expect("context_id should always be a 32-byte array")
    }

    #[inline]
    pub fn function(&self) -> &str {
        self.function
            .try_into()
            .expect("function should always be a valid utf8 string")
    }

    #[inline]
    pub fn params(&self) -> &[u8] {
        &self.params
    }
}

