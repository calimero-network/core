//! Identity newtypes describe themselves for the ABI, replacing the name
//! matching the syn normalizer does for types outside an app's own source.

use calimero_wasm_abi::abi_type::{AbiType, TypeRegistry};
use calimero_wasm_abi::schema::TypeRef;

use crate::application::ApplicationId;
use crate::blobs::BlobId;
use crate::context::ContextId;
use crate::identity::PublicKey;

macro_rules! bytes32 {
    ($($ty:ty),* $(,)?) => {
        $(impl AbiType for $ty {
            fn type_ref(_reg: &mut TypeRegistry) -> TypeRef {
                TypeRef::bytes_with_size(32, None)
            }
        })*
    };
}

bytes32!(ApplicationId, BlobId, ContextId, PublicKey);
