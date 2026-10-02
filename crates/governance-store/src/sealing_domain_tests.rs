//! Key delivery and the app-facing `open_sealed` must not open each other's envelopes.

use calimero_context_client::local_governance::{EnvelopeRecipient, KeyEnvelope};
use calimero_context_config::types::ContextGroupId;
use calimero_crypto::{open_sealed, seal_to_root, Purpose, SealedEnvelope};
use calimero_primitives::identity::{PrivateKey, PublicKey};

use crate::{seal_tee_vault_key, GroupKeyring};

const GROUP_ID: [u8; 32] = [0x11; 32];
const GROUP_KEY: [u8; 32] = [0x22; 32];
const CONTEXT_ID: [u8; 32] = [0x33; 32];

/// What an app's `env::seal_to` / `env::open_sealed` uses for `recipient` in a context.
fn app(recipient: PublicKey) -> Purpose {
    Purpose::App {
        context_id: CONTEXT_ID,
        recipient,
    }
}

/// The bytes an app would hand `env::open_sealed` for a member-addressed key envelope.
fn as_app_envelope(envelope: &KeyEnvelope) -> SealedEnvelope {
    let EnvelopeRecipient::Member { ephemeral_pk, .. } = envelope.recipient else {
        panic!("wrap_for_member must produce a member-addressed envelope");
    };
    SealedEnvelope {
        ephemeral_public_key: ephemeral_pk,
        nonce: envelope.nonce,
        ciphertext: envelope.ciphertext.clone(),
    }
}

#[test]
fn a_group_key_envelope_does_not_open_through_the_app_sealing_path() {
    let admin = PrivateKey::from([0x01; 32]);
    let member = PrivateKey::from([0x02; 32]);
    let envelope =
        GroupKeyring::wrap_for_member(&admin, &member.public_key(), &GROUP_ID, &GROUP_KEY)
            .expect("wrap");
    assert_eq!(
        GroupKeyring::unwrap_for_recipient(&member, &GROUP_ID, None, &envelope).expect("unwrap"),
        GROUP_KEY,
        "the key-delivery path itself must still open it"
    );

    let opened = open_sealed(
        &member,
        &as_app_envelope(&envelope),
        app(member.public_key()),
    );
    assert!(
        opened.is_err(),
        "the app-facing open_sealed with the member's identity key revealed the group key"
    );
}

#[test]
fn an_app_sealed_envelope_does_not_open_as_a_key_delivery() {
    let sender = PrivateKey::from([0x01; 32]);
    let member = PrivateKey::from([0x02; 32]);
    let sealed = seal_to_root(
        &mut rand::rng(),
        &member.public_key(),
        GROUP_KEY.to_vec(),
        app(member.public_key()),
    )
    .expect("seal");

    let recipient = EnvelopeRecipient::Member {
        identity: member.public_key(),
        ephemeral_pk: sealed.ephemeral_public_key,
    };
    let payload = KeyEnvelope::signing_payload(
        &GROUP_ID,
        &recipient,
        &sender.public_key(),
        &sealed.nonce,
        &sealed.ciphertext,
    );
    let envelope = KeyEnvelope {
        recipient,
        sender: sender.public_key(),
        nonce: sealed.nonce,
        ciphertext: sealed.ciphertext,
        signature: sender.sign(&payload).expect("sign").to_bytes(),
    };

    assert!(
        GroupKeyring::unwrap_for_recipient(&member, &GROUP_ID, None, &envelope).is_err(),
        "an envelope minted by the app seal_to host function unwrapped as a group key"
    );
}

#[test]
fn a_tee_vault_key_delivery_does_not_open_through_the_app_sealing_path() {
    let vault = PrivateKey::from([0x03; 32]);
    let tee = PrivateKey::from([0x04; 32]);
    let namespace = ContextGroupId::from(GROUP_ID);
    let bytes = seal_tee_vault_key(&namespace, &vault, &tee.public_key()).expect("seal vault key");
    let envelope = SealedEnvelope::from_bytes(&bytes).expect("decode");

    assert!(
        open_sealed(&tee, &envelope, app(tee.public_key())).is_err(),
        "the app-facing open_sealed with the TEE's identity key revealed the namespace TEE key"
    );
}

#[test]
fn a_key_delivery_re_signed_by_another_sender_does_not_open() {
    let admin = PrivateKey::from([0x01; 32]);
    let other_admin = PrivateKey::from([0x05; 32]);
    let member = PrivateKey::from([0x02; 32]);
    let mut envelope =
        GroupKeyring::wrap_for_member(&admin, &member.public_key(), &GROUP_ID, &GROUP_KEY)
            .expect("wrap");

    envelope.sender = other_admin.public_key();
    let payload = KeyEnvelope::signing_payload(
        &GROUP_ID,
        &envelope.recipient,
        &envelope.sender,
        &envelope.nonce,
        &envelope.ciphertext,
    );
    envelope.signature = other_admin.sign(&payload).expect("sign").to_bytes();

    assert!(
        GroupKeyring::unwrap_for_recipient(&member, &GROUP_ID, None, &envelope).is_err(),
        "a delivery opened under a sender other than the one that sealed it"
    );
}
