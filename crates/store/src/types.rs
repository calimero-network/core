use crate::entry::{DataType, Entry};
use crate::key;

mod application;
mod context;
mod generic;

pub use application::ApplicationMeta;
pub use context::{
    ContextIdentity, ContextMeta, ContextState, ContextTransaction, TransactionHash,
};
pub use generic::GenericData;

pub trait PredefinedEntry: key::AsKeyParts {
    type DataType<'a>: DataType<'a>;
}

impl<T: PredefinedEntry> Entry for T {
    type Key = T;
    type DataType<'a> = T::DataType<'a>;

    fn key(&self) -> &Self::Key {
        self
    }
}
