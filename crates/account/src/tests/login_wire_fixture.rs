//! The byte contract a non-Rust login signer must reproduce.
//!
//! A [`LoginStatement`] is signed by a device key to obtain a session, and the
//! holder of that key is typically not a Rust process — a browser tab is the
//! case the whole delegated-execution path exists for. Such a signer has to
//! reproduce two things exactly: the signing preimage, and the borsh wire
//! encoding.
//!
//! **Why this test lives in core rather than beside the JS implementation**, for
//! the same reason [`super::warrant_wire_fixture`] does: `AUTH_LOGIN_SIGN_DOMAIN`
//! is `pub(crate)` and the field order is implicit in a `derive`, so nothing on
//! this side forces anyone to notice another language depends on either. A
//! conformance test in the JS repository would be run by the repository that
//! *cannot* break the contract; this one is run by the repository that can.
//!
//! What a drift looks like without it: the other signer keeps producing
//! well-formed statements whose signatures verify nowhere, and the failure
//! arrives as a 401 at login — indistinguishable from a wrong key, and nowhere
//! near the change that caused it.
//!
//! **[`Audience`] is where a reimplementation will go wrong.** It is the only
//! field spelled differently in the two encodings, and neither spelling is
//! wrong-looking on its own:
//!
//! * in the **preimage** it contributes `tag ‖ body` as a single part, with no
//!   length inside it — [`calimero_primitives::identity::domain_hash`] prefixes
//!   each part it is handed, so a length here would be counted twice;
//! * in the **wire encoding** it is a borsh enum: `tag`, then for the two
//!   payload-carrying variants a borsh `String`, which is its own `u32` length
//!   then the UTF-8.
//!
//! So the same audience is `00 ‖ utf8` in one place and `00 ‖ u32 ‖ utf8` in the
//! other. All three variants are pinned below rather than one, because `Cli`
//! carries no payload and is the variant that makes a "just length-prefix it
//! everywhere" implementation look correct right up until someone logs in from a
//! browser.
//!
//! Regenerate these vectors only when the wire format is *deliberately*
//! changing, and treat that as the breaking change it is.

use calimero_primitives::identity::PublicKey;

use super::support::key;
use crate::account::borsh_bytes;
use crate::login::{Audience, LoginStatement};

/// The node the fixture statements are addressed to.
const NODE: [u8; 32] = [0x11; 32];
/// The challenge the node is pretending to have issued.
const CHALLENGE: [u8; 32] = [0x22; 32];
/// The ephemeral key the session would speak with.
const SESSION: [u8; 32] = [0x33; 32];
const ISSUED_AT: u64 = 1_700_000_000;
/// Five minutes later — distinct from `ISSUED_AT` so the two cannot be swapped
/// without the vectors moving.
const EXPIRES_AT: u64 = 1_700_000_300;

/// A browser origin with a port, so the colon and digits are covered rather than
/// just `[a-z.]`.
const WEB_ORIGIN: &str = "https://app.example:8443";
const CODE_SIGNING_ID: &str = "dev.calimero.client";

/// Signed with a different seed than the warrant fixture's, so a device key
/// copied from the wrong fixture is visible rather than coincidentally right.
fn statement(audience: Audience) -> LoginStatement {
    LoginStatement::sign(
        &key(9),
        PublicKey::from(NODE),
        audience,
        CHALLENGE,
        PublicKey::from(SESSION),
        ISSUED_AT,
        EXPIRES_AT,
    )
    .expect("sign")
}

/// The 32 bytes a device key actually signs, for each audience.
///
/// Pinned per variant because the audience is the only field whose contribution
/// varies in shape, and a signer that got the tag right but the length wrong
/// still produces a correct-looking preimage for `Cli`.
#[test]
fn the_signing_preimage_is_stable() {
    let preimage = |audience: Audience| {
        hex::encode(LoginStatement::signing_payload(
            &PublicKey::from(NODE),
            &audience,
            &CHALLENGE,
            &PublicKey::from(SESSION),
            &key(9).public_key(),
            ISSUED_AT,
            EXPIRES_AT,
        ))
    };

    assert_eq!(
        preimage(Audience::WebOrigin(WEB_ORIGIN.to_owned())),
        "78114dc2b03a0461cadc4596abdbb9a88d59c6d029687a03e48ac6679065e97e",
        "the preimage changed; every statement signed elsewhere now verifies \
         nowhere, and the failure shows up as a 401 at login",
    );
    assert_eq!(
        preimage(Audience::CodeSigningId(CODE_SIGNING_ID.to_owned())),
        "0093aaa7317be2e922363d051eabe2437532ff5f66ca00724dd1b8ab157485c4",
    );
    assert_eq!(
        preimage(Audience::Cli),
        "db187d3bf8180886092e31485f11b2648b335352df0cd0ea1d902564f201840a",
    );
}

/// Two audiences differing only in their tag must not share a preimage.
///
/// This is what the tag byte in [`Audience::signing_bytes`] buys, and it is
/// invisible in the vectors above: they differ, but so would two unrelated
/// strings. Here the body is held equal so the tag is the only thing left.
#[test]
fn the_audience_tag_separates_two_variants_with_the_same_body() {
    let same_body = "dev.calimero.client";
    let preimage = |audience: Audience| {
        LoginStatement::signing_payload(
            &PublicKey::from(NODE),
            &audience,
            &CHALLENGE,
            &PublicKey::from(SESSION),
            &key(9).public_key(),
            ISSUED_AT,
            EXPIRES_AT,
        )
    };

    assert_ne!(
        preimage(Audience::WebOrigin(same_body.to_owned())),
        preimage(Audience::CodeSigningId(same_body.to_owned())),
        "an origin and a code-signing id spelled the same way hash alike, so a \
         session minted for one surface can be presented from the other",
    );
}

/// The full wire encoding, for the variant that carries a payload.
#[test]
fn the_web_origin_wire_encoding_is_stable() {
    let bytes = borsh_bytes(&statement(Audience::WebOrigin(WEB_ORIGIN.to_owned())));

    assert_eq!(
        bytes.len(),
        237,
        "32 node + (1 tag + 4 + 24) audience + 32 challenge + 32 session + 32 \
         device + 8 + 8 + 64 signature. A different length means a field \
         changed shape, and a signer in another language is now wrong",
    );
    // Split by field so a reviewer can check the boundaries without counting
    // hex, and a diff names which field moved.
    assert_eq!(
        hex::encode(&bytes),
        concat!(
            // node
            "1111111111111111111111111111111111111111111111111111111111111111",
            // audience: borsh enum tag 0 = WebOrigin...
            "00",
            // ...then a borsh String: u32 LE length 24, then the UTF-8. The
            // length is HERE and not in the preimage.
            "18000000",
            "68747470733a2f2f6170702e6578616d706c653a38343433",
            // challenge
            "2222222222222222222222222222222222222222222222222222222222222222",
            // session_key
            "3333333333333333333333333333333333333333333333333333333333333333",
            // device_key (derived from key(9))
            "fd1724385aa0c75b64fb78cd602fa1d991fdebf76b13c58ed702eac835e9f618",
            // issued_at 1_700_000_000, u64 LITTLE-endian
            "00f1536500000000",
            // expires_at 1_700_000_300
            "2cf2536500000000",
            // signature
            "8302e61afe8f61c8471bc5f8ae9ff13cdd5b3e9bcd793cf8c46acb3ff9592aa4",
            "37c1aae1b1ee9c9514f29b2340d13a547ea5e6cd4b4b65fbf09bafb55f4c7e00",
        ),
    );
}

/// The payload-free variant, whose encoding is the one a length-prefix-everything
/// implementation gets wrong.
///
/// `Cli` is a bare tag: no count, no body. An implementation that writes a `u32`
/// zero here produces 213 bytes where a node expects 209, and the four extra
/// bytes are read as the start of the challenge.
#[test]
fn the_cli_wire_encoding_is_stable() {
    let bytes = borsh_bytes(&statement(Audience::Cli));

    assert_eq!(bytes.len(), 209, "a bare tag, with no length and no body");
    assert_eq!(
        hex::encode(&bytes),
        concat!(
            // node
            "1111111111111111111111111111111111111111111111111111111111111111",
            // audience: tag 2 = Cli, and nothing follows it
            "02",
            // challenge
            "2222222222222222222222222222222222222222222222222222222222222222",
            // session_key
            "3333333333333333333333333333333333333333333333333333333333333333",
            // device_key
            "fd1724385aa0c75b64fb78cd602fa1d991fdebf76b13c58ed702eac835e9f618",
            "00f1536500000000",
            "2cf2536500000000",
            // signature
            "085b4ee049f7f268a35ec1bfdfe779b94f3bda66cbbb48937735f9ab10c0ef71",
            "cad6f5bbb9afc4b2b87b5ee87d06284e3c5bc77a6c541c56e62aa65ace8fdb0b",
        ),
    );
}

/// The remaining variant, pinned by length and signature so all three tags are
/// covered.
#[test]
fn the_code_signing_wire_encoding_is_stable() {
    let bytes = borsh_bytes(&statement(Audience::CodeSigningId(
        CODE_SIGNING_ID.to_owned(),
    )));

    assert_eq!(bytes.len(), 232);
    // Tag 1, then a borsh String of length 19.
    assert!(
        hex::encode(&bytes).contains("01130000006465762e63616c696d65726f2e636c69656e74"),
        "the CodeSigningId tag and its length-prefixed body moved",
    );
}

/// A statement round-trips through borsh, so the vectors above describe bytes a
/// node can actually read back.
#[test]
fn the_encoding_round_trips() {
    for audience in [
        Audience::WebOrigin(WEB_ORIGIN.to_owned()),
        Audience::CodeSigningId(CODE_SIGNING_ID.to_owned()),
        Audience::Cli,
    ] {
        let signed = statement(audience);
        let bytes = borsh_bytes(&signed);
        let decoded: LoginStatement = borsh::from_slice(&bytes).expect("decode");
        assert_eq!(decoded, signed);
        decoded
            .verify_signature()
            .expect("the decoded statement verifies");
    }
}
