#![no_main]
//! Noise message 1: the first bytes of a sealed session, which a node reads
//! under its transport key before it knows who sent them (NK from a client,
//! IK from a peer, which also proves its own static key).
//!
//! The first input byte picks NK or IK, and whether the rest is message 1
//! itself or the keys an honest initiator writes one with (its ephemeral and
//! static secrets, a payload, and a byte to flip). A message the fuzzer wrote
//! never opens a session. An honest one with no payload always does, its
//! initiator reads message 2 to the session id and the node's keys, and a
//! peer is told the static key it proved; a payload or one flipped bit is
//! refused.

use calimero_sealed::fuzz_api::{respond, respond_identified};
use curve25519_dalek::montgomery::MontgomeryPoint;
use libfuzzer_sys::fuzz_target;

const TRANSPORT_SECRET: [u8; 32] = [0x2a; 32];
const SESSION_ID: [u8; 16] = [0x51; 16];
const PROLOGUE: &[u8] = b"calimero/sealed-http/v2";
const NK: &str = "Noise_NK_25519_AESGCM_SHA256";
const IK: &str = "Noise_IK_25519_AESGCM_SHA256";

fn public(secret: &[u8; 32]) -> [u8; 32] {
    MontgomeryPoint::mul_base_clamped(*secret).to_bytes()
}

fuzz_target!(|data: &[u8]| {
    let Some((&pick, rest)) = data.split_first() else {
        return;
    };
    let identified = pick & 1 == 1;
    if pick & 2 == 0 {
        // As the fuzzer wrote it: it cannot compute the Diffie-Hellman that opens one.
        let opened = if identified {
            respond_identified(&TRANSPORT_SECRET, rest, &SESSION_ID).is_some()
        } else {
            respond(&TRANSPORT_SECRET, rest, &SESSION_ID).is_some()
        };
        assert!(!opened, "a message 1 the fuzzer wrote opened a session");
        return;
    }
    honest(identified, rest);
});

fn honest(identified: bool, data: &[u8]) {
    let Some((ephemeral, data)) = data.split_first_chunk::<32>() else {
        return;
    };
    let Some((static_secret, data)) = data.split_first_chunk::<32>() else {
        return;
    };
    let Some((&[flip_at, flip_bit], payload)) = data.split_first_chunk::<2>() else {
        return;
    };
    let payload = &payload[..payload.len().min(64)];
    let transport = public(&TRANSPORT_SECRET);
    let builder = snow::Builder::new(if identified { IK } else { NK }.parse().expect("params"))
        .prologue(PROLOGUE)
        .remote_public_key(&transport)
        .fixed_ephemeral_key_for_testing_only(ephemeral);
    let builder = if identified {
        builder.local_private_key(static_secret)
    } else {
        builder
    };
    let mut initiator = builder.build_initiator().expect("an initiator");
    let mut message_1 = vec![0u8; 256];
    let len = initiator
        .write_message(payload, &mut message_1)
        .expect("message 1 writes");
    message_1.truncate(len);

    // A flip at an index past the end leaves the message as written.
    let flipped = usize::from(flip_at) < message_1.len();
    if flipped {
        message_1[usize::from(flip_at)] ^= 1 << (flip_bit % 8);
    }
    let answer = if identified {
        respond_identified(&TRANSPORT_SECRET, &message_1, &SESSION_ID)
            .map(|(m2, req, resp, peer)| (m2, req, resp, Some(peer)))
    } else {
        respond(&TRANSPORT_SECRET, &message_1, &SESSION_ID)
            .map(|(m2, req, resp)| (m2, req, resp, None))
    };
    if flipped || !payload.is_empty() {
        assert!(
            answer.is_none(),
            "an altered message 1 or one with a payload opened a session"
        );
        return;
    }
    let (message_2, request, response, peer) = answer.expect("an honest message 1 opens a session");
    if identified {
        assert_eq!(
            peer,
            Some(public(static_secret)),
            "a peer was told another static key"
        );
    }
    let mut read = [0u8; 64];
    let len = initiator
        .read_message(&message_2, &mut read)
        .expect("the initiator reads message 2");
    assert_eq!(&read[..16], SESSION_ID.as_slice());
    assert_eq!(len, 20, "message 2 carries the session id and its lifetime");
    let (initiator_request, initiator_response) = initiator.dangerously_get_raw_split();
    assert_eq!((initiator_request, initiator_response), (request, response));
    assert_ne!(request, response, "one key both ways");
}
