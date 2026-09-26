# calimero-tee-release - Signed mero-tee Release Assets

Downloads a mero-tee release asset with its detached cosign signature and
Sigstore bundle, and verifies both against the GitHub Actions workflow that must
have signed it. Two consumers:

- **merod** checks the KMS it takes its storage key from against the
  `Release mero-kms` workflow's `kms-phala-attestation-policy.json`
  (`fetch_verified_asset` + `KMS_RELEASE_IDENTITY`, in `merod/src/kms_policy.rs`).
- **`admit_tee_node`** (calimero-context) checks a joining TEE under a
  signed-release admission policy against the `Release mero-tee` workflow's
  `published-mrtds.json` for the release the TEE names (`fetch_node_release`).

## Package Identity

- **Crate**: `calimero-tee-release`
- **Entry**: `src/lib.rs`
- **Key deps**: `sigstore` (keyless verification: Fulcio certificate, Rekor
  inclusion, workflow identity extensions), `reqwest` (release downloads),
  `x509-cert`, `tokio` (the per-process release cache)

## Commands

```bash
cargo test -p calimero-tee-release
```

The tests are offline: they cover version handling, `published-mrtds.json`
parsing and profile matching, and bundle decoding. Nothing here fetches from
GitHub under test.

## Files

| File | What it holds |
| --- | --- |
| `src/version.rs` | `normalize_release_version` (strips a tag prefix, validates semver shape), `compare_release_versions` (pre-release before release, build metadata ignored) |
| `src/sigstore_verify.rs` | `WorkflowIdentity`, the two identities, `verify_signed_asset` |
| `src/fetch.rs` | `fetch_verified_asset`: asset + `.sig` + `.bundle.json`, bounded retries on transient errors only |
| `src/node.rs` | `NodeRelease` / `ProfileMeasurements`, `matching_profile`, `fetch_node_release` (cached, successes only) |

## Gotchas

- **The identity is the check.** A valid signature from any other workflow,
  repository or ref is refused. The node-release identity does not pin the
  workflow trigger, because mero-tee releases are also dispatched by hand.
- **The file must name its release.** `NodeRelease::from_published_mrtds`
  refuses a file whose `tag` is not the requested version, so one release's
  signed file cannot be served under another's tag.
- **A profile missing MRTD, RTMR1, RTMR2 or RTMR3 is dropped**, never matched on
  fewer registers: RTMR3 names the image only when the kernel and initrd before
  it are pinned. RTMR0 is compared only when the profile pins it.
- **Releases without a `.bundle.json` cannot be verified.** mero-tee started
  publishing bundles for node releases after 2.3.71.
- **The cache is bounded (32) and holds only successes**: the version comes
  from whoever asks to be admitted, and a GitHub outage is not a property of a
  release.
