//! The handshake that opens a sealed session, and the sessions it opens.

use core::time::Duration;
use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

use snow::params::NoiseParams;
use zeroize::Zeroizing;

use super::Refusal;

/// The Noise pattern, verbatim: the client knows the transport key in advance
/// (from the quote), the node's static key authenticates it, and both sides'
/// ephemeral keys give the session forward secrecy.
pub(super) const NOISE_PARAMS: &str = "Noise_NK_25519_AESGCM_SHA256";
/// Binds the handshake to this protocol, so a Noise NK handshake for anything
/// else never yields a sealed session.
pub(super) const PROLOGUE: &[u8] = b"calimero/sealed-http/v2";
/// Noise message 1 of NK: the client's ephemeral key and the tag of an empty
/// payload.
pub(super) const MESSAGE_1_LEN: usize = 32 + 16;
pub(super) const SESSION_ID_LEN: usize = 16;

/// How long a session's keys live, however busy it is. This is the forward
/// secrecy window: once a session is gone, nothing on the node can open what
/// was sent in it, including the transport key.
pub const SESSION_LIFETIME: Duration = Duration::from_secs(60 * 60);
/// How long an unused session's keys are kept.
const SESSION_IDLE: Duration = Duration::from_secs(10 * 60);
/// Sessions held at once. Opening one needs no credential, so beyond this the
/// least recently used session is dropped rather than a new one refused; its
/// client simply opens another.
const MAX_SESSIONS: usize = 1 << 14;
/// Request ids remembered per session. An id at or below the oldest one
/// forgotten is refused too, so a request can be replayed neither inside nor
/// below the window.
const REPLAY_WINDOW: usize = 4096;

pub(super) type SessionId = [u8; SESSION_ID_LEN];

/// The two AES-256-GCM keys the handshake splits into.
#[derive(Clone)]
pub(super) struct SessionKeys {
    /// Client to node: requests.
    pub request: Zeroizing<[u8; 32]>,
    /// Node to client: response frames.
    pub response: Zeroizing<[u8; 32]>,
}

/// Answer Noise message 1 with message 2, whose payload hands the client its
/// session id and lifetime, and return the session's keys.
pub(super) fn respond(
    transport_secret: &[u8; 32],
    message_1: &[u8],
    session_id: &SessionId,
) -> Result<(Vec<u8>, SessionKeys), Refusal> {
    let handshake = builder()
        .local_private_key(transport_secret.as_slice())
        .build_responder()
        .map_err(|_| undecryptable())?;
    respond_with(handshake, message_1, session_id)
}

pub(super) fn builder() -> snow::Builder<'static> {
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
pub(super) fn respond_with(
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
        return Err(super::malformed());
    }

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
    expires: Instant,
    idle_until: Instant,
    seen: ReplayWindow,
}

impl Session {
    fn is_live(&self, now: Instant) -> bool {
        now < self.expires && now < self.idle_until
    }
}

/// Every open session.
#[derive(Default)]
pub(super) struct Sessions {
    map: HashMap<SessionId, Session>,
}

impl Sessions {
    pub fn insert(&mut self, id: SessionId, keys: SessionKeys, now: Instant) {
        if self.map.len() >= MAX_SESSIONS {
            self.map.retain(|_, session| session.is_live(now));
        }
        if self.map.len() >= MAX_SESSIONS {
            let least_recent = self
                .map
                .iter()
                .min_by_key(|(_, session)| session.idle_until)
                .map(|(id, _)| *id);
            if let Some(id) = least_recent {
                let _dropped = self.map.remove(&id);
            }
        }
        let _previous = self.map.insert(
            id,
            Session {
                keys,
                expires: now + SESSION_LIFETIME,
                idle_until: now + SESSION_IDLE,
                seen: ReplayWindow::default(),
            },
        );
    }

    /// A live session's keys and the instant they expire.
    pub fn keys(
        &mut self,
        id: &SessionId,
        now: Instant,
    ) -> Result<(SessionKeys, Instant), Refusal> {
        let session = self.live(id, now)?;
        Ok((session.keys.clone(), session.expires))
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
