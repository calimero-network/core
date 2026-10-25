# Outbound Probe

The E2E fixture for outbound HTTPS requests (`env::http_request`). Each method
asks for a request from a different kind of run, so the `outbound` merobox
scenario can show every gate the node puts in front of one:

- `probe(path, body)`: a write that POSTs under the one declared prefix,
  `https://echo.outbound.test/v1/`, naming the `ECHO_KEY` secret in a header and
  in the body, and stores what came back.
- `probe_url(url)`: a write that GETs any URL, so a path or host the gates
  refuse comes back as a reason.
- `probe_twice()`: a write that makes two requests; the node traps the second.
- `probe_in_view()`: a view asking for a request; the node traps it.
- `last()`: what the last write stored.

`.test` hosts exist only in the test build (`merod --features
mock-attestation`), whose `[context.outbound.testing]` answers them in-process
with an echo of the request the node sent. See `workflows/outbound.yml`, and the
guide `docs/src/content/docs/build/guides/outbound.mdx`.
