# calimero-sealed

The sealed transport (wire format v2) between clients and the node.

- `server`: `SealedTransport` (per-process X25519 key, never persisted) and the
  `intercept` middleware. **Wrap the router from outside** (`tower::Layer::layer`
  on the router), never `Router::layer`: the opened request must be routed afresh.
- `client`: `SealedTransport` that seals `reqwest::Request`s; the key comes from
  `with_key` or an `Attest` implementation that verifies the node's quote.
- The wire format is byte-compatible with mero-js `src/sealed/`; change both or neither.
- **Identified sessions** (handshake version 3, `Noise_IK_25519_AESGCM_SHA256`,
  `identity.rs`): a node calling a peer proves its own transport key
  (`client::SealedTransport::identified_as`, `server::SealedTransport::static_key`);
  the server puts the proven key on every request of that session as a
  `PeerKey` extension, set last so nothing upstream chooses it, and removes
  one on an anonymous session. A low-order static key is refused. No fallback
  to NK. Peers only: clients stay on NK, so mero-js is unaffected. Tests:
  `an_identified_session_names_its_caller`,
  `an_identified_handshake_with_another_nodes_key_is_refused`.
- A refused envelope (`Refusal`: too large, unknown session, busy) is answered in
  the clear, after the rest of its body is read and dropped (`discard`, up to
  `MAX_DISCARDED_BYTES`): a server that answers and closes with the body unread
  resets the connection, and a client still sending never sees the answer.
  Regression: `a_request_past_the_cap_is_answered_after_it_is_sent`.

Removed relative to the server copy: forward-auth, admin-path inner scopes and
prometheus metrics (plain atomic counters instead).

Tests: `cargo test -p calimero-sealed` (round trip, SSE frames, stale key, required mode,
an oversized request answered `413`).

Bench: `cargo bench -p calimero-sealed --bench sealed` (`benches/sealed.rs`): one
request sealed and opened both ways over loopback, at 1 KiB, 64 KiB and 1 MiB,
beside the same request sent plain to the same server (the envelope's cost, not
HTTP's). The session is opened before timing, as one lasts.
