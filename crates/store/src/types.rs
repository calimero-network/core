use crate::entry::{DataType, Entry};
use crate::key;

mod context;
mod generic;

pub use context::{ContextIdentity, ContextMeta, ContextState, ContextTransaction};
pub use generic::GenericData;

pub trait PredefinedEntry: key::AsKeyParts {
    type DataType: DataType;
}

impl<T: PredefinedEntry> Entry for T {
    type Key = T;
    type DataType = T::DataType;

    fn key(&self) -> &Self::Key {
        self
    }
}
