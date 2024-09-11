use crate::entry::{Codec, Entry};
use crate::key::AsKeyParts;

mod application;
mod blobs;
mod context;
mod generic;

pub use application::ApplicationMeta;
pub use blobs::BlobMeta;
pub use context::{
    ContextConfig, ContextIdentity, ContextMeta, ContextState, ContextTransaction, TransactionHash,
};
pub use generic::GenericData;

pub trait PredefinedEntry: AsKeyParts {
    type Codec: for<'a> Codec<'a, Self::DataType<'a>>;
    type DataType<'a>;
}

impl<T: PredefinedEntry> Entry for T {
    type Key = T;
    type Codec = T::Codec;
    type DataType<'a> = T::DataType<'a>;

    fn key(&self) -> &Self::Key {
        self
    }
}
