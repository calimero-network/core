# mero-sign

The signing library for bundle manifests: Ed25519 over the manifest's RFC 8785
(JCS) canonical form, the `did:key` signer id, key files written owner-only, and
the ABI check against the last release. `meroctl` signs with it; the `mero-sign`
binary it used to ship is gone.

| What | `meroctl` | Library |
| --- | --- | --- |
| A new key, written owner-only | `meroctl app signer new KEY` | `generate_key` |
| The key's signer id (`did:key:z6Mk...`) | `meroctl app signer id KEY` | `derive_signer_id` |
| Sign a `manifest.json` in place | `meroctl app sign MANIFEST (--key KEY \| --dev)` | `sign_manifest`, `load_signing_key`, `dev_signing_key` |
| Write, sign and pack the `.mpk` | `meroctl app pack ... [--key KEY]` | the above, then `calimero-bundle` |
| Check the ABI against the last release first | `--previous OLD` on `sign` and `pack` | `check_abi` |

A package's identity is its package id and its signer: keep the key safe, and sign
every release of a package with the same one. The development key (`--dev`, and
`app pack` without `--key`) has a public seed, so a public registry and a node
without `--allow-unsigned-bundles` refuse what it signed
(`calimero_bundle::DEV_SIGNER_ID`).

The format itself (the canonical bytes, the signing domain, the hash) is
`calimero-bundle`'s (`crates/bundle/AGENTS.md`); this crate only adds keys and the
command-line workflow's checks. The guide is
[Packaging & signing](../../docs/src/content/docs/build/packaging-signing.mdx).

```bash
cargo test -p mero-sign
```
