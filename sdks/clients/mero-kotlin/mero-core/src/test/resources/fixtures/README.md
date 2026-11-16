# Captured fixtures

Response bodies captured **verbatim** from a live node, not hand-written.

A hand-written fixture that matches the model can only confirm the model agrees
with itself — which is how both the rc.25 `groupId`→`namespaceId` rename and the
invitation field drop got past a fully green suite. These are the real bytes.

| file | captured from | route |
|---|---|---|
| `invitation-rc32.json` | `merod 0.11.0-rc.32` (0.x) | a 0.x invitation, kept as a realistic payload for `InviteCodec`'s base58+deflate round trip |
| `sealed/mero-tee-v2.3.8{6,7}.*`, `sealed/mero-kms-v2.3.87.*` | the mero-tee / mero-kms GitHub releases (file, cosign bundle, `.sig`), via mero-js `src/sealed/fixtures/` | signed release assets; `…tampered…` has one byte changed |
| `sealed/tdx_quote.bin`, `sealed/tdx_quote_collateral.json` | `@phala/dcap-qvl`'s sample TDX quote and its Intel-signed collateral, via mero-js | the input of `Dcap.verify` |
| `agent-gate/rust-sealed-bundle.json` | mero-agent-gate (Rust, the `hpke` crate), via mero-js `src/agent-gate/fixtures/` | an HPKE Auth-mode `/provision` message |
