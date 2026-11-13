use super::ApplicationId;
use crate::hash::Hash;

// -----------------------------------------------------------------------------
// ApplicationId::for_bundle Tests
// -----------------------------------------------------------------------------

#[test]
fn for_bundle_matches_the_inline_derivation() {
    let package: Box<str> = "com.example.demo".into();
    let signer: Box<str> = "did:key:z6MkExample".into();
    let inline = ApplicationId::from(*Hash::hash_borsh(&(&package, &signer)).expect("hash"));
    assert_eq!(
        ApplicationId::for_bundle(&package, &signer).expect("helper"),
        inline
    );
}
