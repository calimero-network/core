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
| `wire_decode` | the borsh and JSON decoders for a gossip message, a sync stream frame, and a blob request or announcement | no panic, and memory held while decoding stays proportional to the input |
| `provider_record` | `BlobProviderRecord::verify` | only the signer verifies, and a changed record never verifies with a claim it was not signed for |
| `signed_ops` | `NamespaceTopicMsg` off gossip, then `SignedNamespaceOp` and `SignedGroupOp` `validate` and `verify_signature` | only the target's key verifies, and a changed op verifies only if every signed field is unchanged |
| `governance_apply` | a sequence of signed group ops with chosen parents and arrival order, applied through `NamespaceGovernance::apply_signed_op` | a refused op changes no role or capability, a group that had an admin keeps one, and the outsider gains standing only from an op signed by someone with authority |

Private entry points are reached through `fuzz_api` modules compiled only under `cfg(fuzzing)`; they wrap the real function and hold no logic.
Seeds, where a target has them, live in `fuzz/seeds/<target>/`; a target that guards a fixed finding includes its reproduction input.

CI (`.github/workflows/fuzz.yml`) runs `scripts/fuzz-ci.sh <target> 60` for each target whose sources a pull request changes, and every target for 30 minutes nightly with its corpus cached between runs.
`scripts/fuzz-ci.sh` is `scripts/fuzz.sh` with the fuzzer's output kept out of the job log, which is public.
A crash fails the run and prints only the target and the failing input's SHA-256; the input is neither shown nor uploaded, and nothing opens an issue, since it can be a working exploit.
To get the input, fuzz that target locally with `scripts/fuzz.sh`; the hash tells whether a local find is the same one.
