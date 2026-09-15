# calimero-client-stub

A stub node serving the delegated-execution client contracts, so a client can be
built and tested before the node side lands.

```bash
cargo run -p calimero-client-stub -- --port 8080
cargo run -p calimero-client-stub -- --refuse-authorship
```

The contracts it serves are specified in
[`docs/src/content/docs/build/delegated-execution-client.mdx`](../../docs/src/content/docs/build/delegated-execution-client.mdx).

**This is not a security boundary and must never be deployed as one.** It verifies
no signatures and accepts any well-formed proof. It checks what a client can get
wrong on its own side — encodings, and whether a warrant's `intent_hash` commits to
the method and arguments it arrived with.
