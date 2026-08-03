use calimero_primitives::application::ApplicationId;
use calimero_sdk::abi::{AbiType, ScalarType, TypeRef, TypeRegistry};
use calimero_sdk::{BlobId, ContextId, PublicKey};

fn ref_of<T: AbiType>() -> TypeRef {
    let mut reg = TypeRegistry::new();
    <T as AbiType>::type_ref(&mut reg)
}

#[test]
fn identity_newtypes_are_32_byte_fixed_bytes() {
    for r in [
        ref_of::<BlobId>(),
        ref_of::<ContextId>(),
        ref_of::<ApplicationId>(),
        ref_of::<PublicKey>(),
    ] {
        let TypeRef::Scalar(ScalarType::Bytes { size, .. }) = r else {
            panic!("expected fixed-size bytes, got {r:?}");
        };
        assert_eq!(size, Some(32));
    }
}
