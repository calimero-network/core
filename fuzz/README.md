# Fuzz targets

Coverage-guided fuzzing of the code a node runs on bytes a peer controls.
Each target calls the production entry point and asserts an invariant beyond "no panic".

```bash
scripts/fuzz.sh <target> [seconds]
```

The script installs nightly and `cargo-fuzz` if they are missing, and runs until a crash, or for `seconds`.
New inputs go to `fuzz/corpus/<target>/`, a crash to `fuzz/artifacts/<target>/`; neither is committed.
After one script run has copied the root lock in, replay a crash with `cargo +nightly fuzz run <target> <file>` from this directory.

| Target | Entry point | Invariant |
| --- | --- | --- |
| `bundle_open` | `VerifiedBundle::open`, then artifact extraction | an artifact is a verbatim regular-file slice of the archive; a read manifest is a recognised bundle |
| `crdt_sync` | `Root::sync` of a peer delta over a `RichText`, wrapped as the node wraps it | after a sync that succeeds, the text and formatting read back, a refused insert changes nothing, and the local replica can still format |
| `sealed_request` | `calimero_sealed::intercept` on a handshake or sealed request body, as every client's and peer's first bytes reach a node | no panic, and nothing reaches the router behind it unless an envelope opened |
| `passkey_verify` | WebAuthn `verify_registration` and `verify_assertion` (client data JSON, authenticator data, CBOR attestation object) and `claimed_origin` | no panic, and no assertion the fuzzer made verifies against a real credential |
| `session_token` | `peek_session` and `verify_session` on a token a request carries | no panic, and a token that verifies under the node's key verifies under no other |
| `quote_parse` | `quote_report_data` and `verify_evidence` on a TDX quote a node or peer presents (release build: no mock path) | no panic, and nothing verifies without collateral |

v1's `wire_decode`, `provider_record`, `signed_ops` and `governance_apply` fuzzed gossip, sync, blob-provider and governance-op decoding, which release-v2 deleted: the untrusted bytes a node reads now arrive through the sealed channel (`sealed_request`) and its authority is passkeys and sessions (`passkey_verify`, `session_token`) checked against attested peers (`quote_parse`).

Private entry points are reached through `fuzz_api` modules compiled only under `cfg(fuzzing)`; they wrap the real function and hold no logic.
Seeds, where a target has them, live in `fuzz/seeds/<target>/`; a target that guards a fixed finding includes its reproduction input.

CI (`.github/workflows/fuzz.yml`) runs `scripts/fuzz-ci.sh <target> 60` for each target whose sources a pull request changes, and every target for 30 minutes nightly with its corpus cached between runs.
`scripts/fuzz-ci.sh` is `scripts/fuzz.sh` with the fuzzer's output kept out of the job log, which is public.
A crash fails the run and prints only the target and the failing input's SHA-256; the input is neither shown nor uploaded, and nothing opens an issue, since it can be a working exploit.
To get the input, fuzz that target locally with `scripts/fuzz.sh`; the hash tells whether a local find is the same one.
