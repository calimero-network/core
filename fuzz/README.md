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
| `sealed_request` | `calimero_sealed::intercept` on a handshake or sealed request body, as every client's and peer's first bytes reach a node | no panic, and nothing reaches the router behind it unless an envelope opened |
| `passkey_verify` | WebAuthn `verify_registration` and `verify_assertion` (client data JSON, authenticator data, CBOR attestation object) and `claimed_origin` | no panic, and no assertion the fuzzer made verifies against a real credential |
| `session_token` | `peek_session` and `verify_session` on a token a request carries | no panic, and a token that verifies under the node's key verifies under no other |
| `quote_parse` | `quote_report_data` and `verify_evidence` on a TDX quote a node or peer presents (release build: no mock path) | no panic, and nothing verifies without collateral |
| `job_wire` | a job node's `Assignment` and a leader's `Report` decoded from JSON, `sign_*`/`verify_assignment`/`verify_report` (`calimero_node::job_wire`) | what decodes verifies under the key that signed it only, survives a JSON round trip, and two different documents never sign the same bytes; a signature the fuzzer wrote never verifies |
| `log_entry` | a log entry decoded as a component's `ComponentCmd` (after `entry_format`) and the system group's `SystemCmd` | what decodes encodes, decodes again and encodes to the same bytes, with no format marker |
| `outbound_request` | `OutboundHost::send` on the borsh `OutboundRequest` a run hands its node, through a recording transport | only an https request to the allowed host under the declared and approved prefix, with headers that cannot split a line, reaches the network, and a secret never comes back to an app that did not send it |
| `pipeline_compile` | `calimero_pipeline::compile` on a member's YAML, then `check` and the worst-case estimates | a compiled pipeline decodes from the log to the same canonical bytes |
| `noise_handshake` | Noise message 1 (NK and IK) read under the node's transport key (`session::respond`, `respond_identified`) | a message the fuzzer wrote never opens a session; an honest one always does, to the keys and session id its initiator reads, and a peer is told the static key it proved; a payload or a flipped bit is refused |
| `sealed_file` | `FileOpener` on a sealed file's chunk stream, fed in fuzzer-sized pieces | a stream opens to its plaintext exactly when nothing was altered (a flipped bit, a truncation, an inserted byte), and the pieces change nothing |
| `release_policy` | `NodeRelease::from_published_mrtds` and `kms-client`'s `parse_policy_json` on a release's signed measurements | what parses is for the release and profile asked for, and admits only on pinned 48-byte registers in the form a quote renders them |

v1's `wire_decode`, `provider_record`, `signed_ops` and `governance_apply` fuzzed gossip, sync, blob-provider and governance-op decoding, which are gone with v1's node: the untrusted bytes a node reads now arrive through the sealed channel (`sealed_request`) and its authority is passkeys and sessions (`passkey_verify`, `session_token`) checked against attested peers (`quote_parse`).

Private entry points are reached through `fuzz_api` modules compiled only under `cfg(fuzzing)`; they wrap the real function and hold no logic.
Seeds, where a target has them, live in `fuzz/seeds/<target>/`, taken from the crates' own test fixtures; a target that guards a fixed finding includes its reproduction input.
Dependencies name their versions as the root `Cargo.toml` does, never `*` (`deny.toml` refuses a wildcard; the fuzz crate is outside the workspace it checks, so this is by hand).

CI (`.github/workflows/fuzz.yml`) runs `scripts/fuzz-ci.sh <target> 60` for each target whose sources a pull request changes, and every target for 30 minutes nightly with its corpus cached between runs.
`scripts/fuzz-ci.sh` is `scripts/fuzz.sh` with the fuzzer's output kept out of the job log, which is public.
A crash fails the run and prints only the target and the failing input's SHA-256; the input is neither shown nor uploaded, and nothing opens an issue, since it can be a working exploit.
To get the input, fuzz that target locally with `scripts/fuzz.sh`; the hash tells whether a local find is the same one.
