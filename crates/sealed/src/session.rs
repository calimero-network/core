//! The handshake that opens a sealed session, and the sessions it opens.

use core::time::Duration;
use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

use curve25519_dalek::montgomery::MontgomeryPoint;
use snow::params::NoiseParams;
use zeroize::Zeroizing;

use crate::server::{malformed, Refusal};

/// The Noise pattern, verbatim: the client knows the transport key in advance
/// (from the quote), the node's static key authenticates it, and both sides'
/// ephemeral keys give the session forward secrecy.
pub(crate) const NOISE_PARAMS: &str = "Noise_NK_25519_AESGCM_SHA256";
/// The pattern of an identified session ([`crate::identity::IDENTIFIED_VERSION`]):
/// as NK, and the caller also proves its own static key (`s`, `ss`, `se`), in
/// the same one round trip.
pub(crate) const NOISE_PARAMS_IDENTIFIED: &str = "Noise_IK_25519_AESGCM_SHA256";
/// Binds the handshake to this protocol, so a Noise NK handshake for anything
/// else never yields a sealed session.
pub(crate) const PROLOGUE: &[u8] = b"calimero/sealed-http/v2";
/// Noise message 1 of NK: the client's ephemeral key and the tag of an empty
/// payload.
pub(crate) const MESSAGE_1_LEN: usize = 32 + 16;
/// Noise message 1 of IK: the caller's ephemeral key, its static key sealed
/// (32 + tag) and the tag of an empty payload.
pub(crate) const IDENTIFIED_MESSAGE_1_LEN: usize = 32 + 32 + 16 + 16;
pub(crate) const SESSION_ID_LEN: usize = 16;

/// How long a session's keys live, however busy it is. This is the forward
/// secrecy window: once a session is gone, nothing on the node can open what
/// was sent in it, including the transport key.
pub const SESSION_LIFETIME: Duration = Duration::from_secs(60 * 60);
/// How long an unused session's keys are kept.
const SESSION_IDLE: Duration = Duration::from_secs(10 * 60);
/// Sessions held at once. Opening one needs no credential, so beyond this a
/// session is dropped rather than a new one refused, and its client simply opens
/// another: first one that never carried a request, then the least recently
/// used. A flood of handshakes therefore displaces its own sessions before any
/// client's.
pub(crate) const MAX_SESSIONS: usize = 1 << 14;
/// Request ids remembered per session. An id at or below the oldest one
/// forgotten is refused too, so a request can be replayed neither inside nor
/// below the window.
const REPLAY_WINDOW: usize = 4096;

pub(crate) type SessionId = [u8; SESSION_ID_LEN];

/// The two AES-256-GCM keys the handshake splits into.
#[derive(Clone)]
pub(crate) struct SessionKeys {
    /// Client to node: requests.
    pub request: Zeroizing<[u8; 32]>,
    /// Node to client: response frames.
    pub response: Zeroizing<[u8; 32]>,
}

/// Answer Noise message 1 with message 2, whose payload hands the client its
/// session id and lifetime, and return the session's keys.
pub(crate) fn respond(
    transport_secret: &[u8; 32],
    message_1: &[u8],
    session_id: &SessionId,
) -> Result<(Vec<u8>, SessionKeys), Refusal> {
    // A low-order ephemeral key makes every Diffie-Hellman with it zero, keys
    // anybody can compute. Only the client that sent one could exploit that, but
    // there is no reason to open a session on it.
    let client_ephemeral = message_1.first_chunk::<32>().ok_or_else(malformed)?;
    let shared = Zeroizing::new(
        MontgomeryPoint(*client_ephemeral)
            .mul_clamped(*transport_secret)
            .0,
    );
    if shared.iter().all(|byte| *byte == 0) {
        return Err(malformed());
    }
    let handshake = builder()
        .local_private_key(transport_secret.as_slice())
        .build_responder()
        .map_err(|_| undecryptable())?;
    respond_with(handshake, message_1, session_id)
}

/// [`respond`] for an identified session (Noise IK): also returns the static
/// key the caller proved it holds.
pub(crate) fn respond_identified(
    transport_secret: &[u8; 32],
    message_1: &[u8],
    session_id: &SessionId,
) -> Result<(Vec<u8>, SessionKeys, [u8; 32]), Refusal> {
    let client_ephemeral = message_1.first_chunk::<32>().ok_or_else(malformed)?;
    if low_order(client_ephemeral, transport_secret) {
        return Err(malformed());
    }
    let params: NoiseParams = NOISE_PARAMS_IDENTIFIED
        .parse()
        // SAFETY: a constant naming a pattern and primitives snow supports.
        .expect("valid Noise parameters");
    let mut handshake = snow::Builder::new(params)
        .prologue(PROLOGUE)
        .local_private_key(transport_secret.as_slice())
        .build_responder()
        .map_err(|_| undecryptable())?;
    let mut payload = [0u8; IDENTIFIED_MESSAGE_1_LEN];
    let payload_len = handshake
        .read_message(message_1, &mut payload)
        .map_err(|_| undecryptable())?;
    if payload_len != 0 {
        return Err(malformed());
    }
    let peer: [u8; 32] = handshake
        .get_remote_static()
        .and_then(|k| k.try_into().ok())
        .ok_or_else(malformed)?;
    // A low-order static key proves nothing: anyone computes its shared secrets.
    if low_order(&peer, transport_secret) {
        return Err(malformed());
    }
    let (message_2, keys) = write_message_2(handshake, session_id)?;
    Ok((message_2, keys, peer))
}

fn low_order(public: &[u8; 32], secret: &[u8; 32]) -> bool {
    let shared = Zeroizing::new(MontgomeryPoint(*public).mul_clamped(*secret).0);
    shared.iter().all(|byte| *byte == 0)
}

pub(crate) fn builder() -> snow::Builder<'static> {
    let params: NoiseParams = NOISE_PARAMS
        .parse()
        // SAFETY: a constant naming a pattern and primitives snow supports.
        .expect("valid Noise parameters");
    snow::Builder::new(params).prologue(PROLOGUE)
}

fn undecryptable() -> Refusal {
    Refusal::bad_request(
        "undecryptable",
        "the handshake did not complete under this node's transport key",
    )
}

/// [`respond`] on a responder already built, which the published test vectors
/// build with a fixed ephemeral key.
pub(crate) fn respond_with(
    mut handshake: snow::HandshakeState,
    message_1: &[u8],
    session_id: &SessionId,
) -> Result<(Vec<u8>, SessionKeys), Refusal> {
    let mut payload = [0u8; MESSAGE_1_LEN];
    let payload_len = handshake
        .read_message(message_1, &mut payload)
        .map_err(|_| undecryptable())?;
    // Nothing is sent before the ephemeral exchange completes: data under the
    // static key alone would have no forward secrecy.
    if payload_len != 0 {
        return Err(malformed());
    }
    write_message_2(handshake, session_id)
}

/// Message 2, whose payload hands the caller its session id and lifetime, and
/// the session's keys.
fn write_message_2(
    mut handshake: snow::HandshakeState,
    session_id: &SessionId,
) -> Result<(Vec<u8>, SessionKeys), Refusal> {
    let lifetime = u32::try_from(SESSION_LIFETIME.as_secs()).unwrap_or(u32::MAX);
    let mut payload = [0u8; SESSION_ID_LEN + 4];
    payload[..SESSION_ID_LEN].copy_from_slice(session_id);
    payload[SESSION_ID_LEN..].copy_from_slice(&lifetime.to_be_bytes());
    let mut message_2 = vec![0u8; 32 + payload.len() + 16];
    let len = handshake
        .write_message(&payload, &mut message_2)
        .map_err(|_| undecryptable())?;
    message_2.truncate(len);

    let (request, response) = handshake.dangerously_get_raw_split();
    Ok((
        message_2,
        SessionKeys {
            request: Zeroizing::new(request),
            response: Zeroizing::new(response),
        },
    ))
}

struct Session {
    keys: SessionKeys,
    /// The static key an identified session's caller proved it holds.
    peer: Option<[u8; 32]>,
    expires: Instant,
    idle_until: Instant,
    /// Whether a request has opened under this session.
    used: bool,
    seen: ReplayWindow,
}

impl Session {
    fn is_live(&self, now: Instant) -> bool {
        now < self.expires && now < self.idle_until
    }
}

/// Every open session.
#[derive(Default)]
pub(crate) struct Sessions {
    map: HashMap<SessionId, Session>,
}

impl Sessions {
    pub fn insert(
        &mut self,
        id: SessionId,
        keys: SessionKeys,
        peer: Option<[u8; 32]>,
        now: Instant,
    ) {
        if self.map.len() >= MAX_SESSIONS {
            self.map.retain(|_, session| session.is_live(now));
        }
        if self.map.len() >= MAX_SESSIONS {
            let least_recent = self
                .map
                .iter()
                .min_by_key(|(_, session)| (session.used, session.idle_until))
                .map(|(id, _)| *id);
            if let Some(id) = least_recent {
                let _dropped = self.map.remove(&id);
            }
        }
        let _previous = self.map.insert(
            id,
            Session {
                keys,
                peer,
                expires: now + SESSION_LIFETIME,
                idle_until: now + SESSION_IDLE,
                used: false,
                seen: ReplayWindow::default(),
            },
        );
    }

    /// Drop every session past its lifetime or idle limit, and with it the
    /// only copy of its keys.
    pub fn expire(&mut self, now: Instant) {
        self.map.retain(|_, session| session.is_live(now));
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// A live session's keys, the instant they expire, and the static key its
    /// caller proved (identified sessions only).
    pub fn keys(
        &mut self,
        id: &SessionId,
        now: Instant,
    ) -> Result<(SessionKeys, Instant, Option<[u8; 32]>), Refusal> {
        let session = self.live(id, now)?;
        Ok((session.keys.clone(), session.expires, session.peer))
    }

    /// Record a request id as used. Called only once the request opened, so a
    /// forged request cannot spend an id its client has yet to use.
    pub fn admit(&mut self, id: &SessionId, request_id: u64, now: Instant) -> Result<(), Refusal> {
        let session = self.live(id, now)?;
        if !session.seen.insert(request_id) {
            return Err(Refusal::bad_request(
                "replayed_request",
                "this sealed request was already received",
            ));
        }
        session.idle_until = now + SESSION_IDLE;
        session.used = true;
        Ok(())
    }

    fn live(&mut self, id: &SessionId, now: Instant) -> Result<&mut Session, Refusal> {
        if self
            .map
            .get(id)
            .is_some_and(|session| !session.is_live(now))
        {
            let _expired = self.map.remove(id);
        }
        self.map.get_mut(id).ok_or(Refusal {
            status: axum::http::StatusCode::CONFLICT,
            code: "unknown_session",
            message: "no such sealed session on this node (it expired, or the node restarted); \
                      open a new one",
        })
    }
}

/// The request ids one session has used.
#[derive(Default)]
struct ReplayWindow {
    /// Every id at or below this is refused.
    floor: u64,
    seen: BTreeSet<u64>,
}

impl ReplayWindow {
    /// `false` if `id` was used before, or is too old to tell.
    fn insert(&mut self, id: u64) -> bool {
        if id <= self.floor || !self.seen.insert(id) {
            return false;
        }
        if self.seen.len() > REPLAY_WINDOW {
            if let Some(oldest) = self.seen.pop_first() {
                self.floor = oldest;
            }
        }
        true
    }
}
