//! A credential produced by the TypeScript SDK, verified here.
//!
//! This exists because its absence let a non-functional feature ship. mero-js
//! grew an offline device-certification path whose own tests assert the
//! credential's length and internal consistency — and pass identically whether
//! the bytes are ones core accepts or ones it rejects outright. They did the
//! latter for a full release: the SDK wrote `AccountGenesis` version 1 while
//! core requires 2, so every credential it produced failed `verify` on the first
//! field, and nothing on either side of the wire could tell.
//!
//! The bytes below are checked in deliberately rather than generated. A fixture
//! that regenerates from the current SDK would agree with whatever the SDK now
//! does, which is the property that failed to hold; agreeing with a recorded
//! artifact is the point.
//!
//! Regenerating (only when the credential format changes on purpose, and the
//! change is intended to be breaking):
//!
//! ```text
//! # in mero-js
//! node --import tsx -e "
//!   import { accountForRoot, mintDeviceId, signDeviceCert } from './src/device-cert/index.js';
//!   import { derivePublicKey, hex } from './src/crypto/internal.js';
//!   const root = '5c'.repeat(32), dev = '6d'.repeat(32);
//!   const signPk = hex(await derivePublicKey(dev));
//!   const account = await accountForRoot(root);
//!   const device = await mintDeviceId(account, new Uint8Array(16).fill(0xa1));
//!   console.log(account, await signDeviceCert({ rootSecret: root, device,
//!     signPublicKey: signPk, kemPublicKey: '3a'.repeat(32), deviceEpoch: 0 }));
//! "
//! ```

/// Inputs: root secret `5c`×32, device secret `6d`×32, kem pk `3a`×32,
/// device-id nonce `a1`×16, `device_epoch` 0.
const SDK_CREDENTIAL_HEX: &str = "02ed6a47a39da869b5446155e40b2d93f1e3f0167be26732bae7a3ef9d8e3a3fd300000000ca999783990fd7f4ea0c192135f78c17ac77745bf580b2ed20fea455a8133845044305da225179a277d6d96e07ff21ea2b3905c7e22b0b3625350f15c6f432938b237d788e8eaaef550c6d125823fa45f1fd5fc29b2c88bdf871119471fc13123a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a000000000000000058b84a4124014258b6548e2bc323bc6f42ea5dedfcc721f2501e22cfd3fc3e2e36ab9d76e648726379000a66f1061838673133a31b5516861811fcb0adaafe07";

/// The account that credential's genesis names.
const SDK_ACCOUNT_HEX: &str = "ca999783990fd7f4ea0c192135f78c17ac77745bf580b2ed20fea455a8133845";

/// The device signing key inside it — what a join op signed by this device must
/// carry as its `signer`.
const SDK_SIGN_PK_HEX: &str = "8b237d788e8eaaef550c6d125823fa45f1fd5fc29b2c88bdf871119471fc1312";

#[test]
fn a_credential_from_the_typescript_sdk_verifies() {
    let bytes = hex::decode(SDK_CREDENTIAL_HEX).expect("fixture is hex");
    let credential: crate::AccountProof<crate::DeviceCert> =
        borsh::from_slice(&bytes).expect("fixture decodes as AccountProof<DeviceCert>");
    let account: crate::AccountId = SDK_ACCOUNT_HEX.parse().expect("fixture account");

    // The genesis must name that account by itself. This is the check that
    // caught the version mismatch: a stale version tag derives a different
    // account rather than failing as a version error.
    assert_eq!(
        credential.genesis.account_id(),
        account,
        "the SDK's genesis no longer derives the account it claims"
    );

    // And the whole proof must verify — genesis, chain, and the root's signature
    // over the device certificate.
    credential
        .verify(account)
        .expect("core must accept a credential the SDK produced");
}

#[test]
fn the_fixture_carries_the_device_key_a_join_op_would_sign_with() {
    let bytes = hex::decode(SDK_CREDENTIAL_HEX).expect("fixture is hex");
    let credential: crate::AccountProof<crate::DeviceCert> =
        borsh::from_slice(&bytes).expect("fixture decodes");

    // A join op is applied only if `op.signer == credential.statement.sign_pk`.
    // Pinned here so a credential-format change that moved this field would fail
    // in this crate rather than as an unexplained refusal at apply time.
    assert_eq!(
        hex::encode(AsRef::<[u8]>::as_ref(&credential.statement.sign_pk)),
        SDK_SIGN_PK_HEX,
        "sign_pk moved within the credential"
    );
}
