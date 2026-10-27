# calimero-sealed

The sealed transport (wire format v2), extracted from `calimero-server`'s `sealed`
module and `calimero-client`'s `tee/sealed` for the release-v2 cloud node.

- `server`: `SealedTransport` (per-process X25519 key, never persisted) and the
  `intercept` middleware. **Wrap the router from outside** (`tower::Layer::layer`
  on the router), never `Router::layer`: the opened request must be routed afresh.
- `client`: `SealedTransport` that seals `reqwest::Request`s; the key comes from
  `with_key` or an `Attest` implementation that verifies the node's quote.
- The wire format is byte-compatible with mero-js `src/sealed/`; change both or neither.
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
