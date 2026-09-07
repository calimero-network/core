# calimero-app-downloader - Application Bytecode Acquisition

One route from "a group named a `bytecode_id`" to an installed, executable
application, from the single source `[registry] mode` selects - and no other.

## Package Identity

- **Crate**: `calimero-app-downloader`
- **Entry**: `src/lib.rs`
- **Consumers**: `calimero-node`, `calimero-node-primitives`,
  `calimero-context`, `calimero-context-primitives`,
  `calimero-governance-store`, `calimero-config`, `calimero-merod`

## Commands

```bash
cargo build -p calimero-app-downloader
cargo test -p calimero-app-downloader
```

## File Organization

```
src/
├── lib.rs          # Re-exports; the crate's public surface
├── downloader.rs   # ApplicationDownloader::download - the one entry point
├── port.rs         # ApplicationStore - the seam back into the node
├── registry.rs     # [registry] config + mode, package@version coords
├── source.rs       # AppSource + AppRequest + app_source - the one-source seam
├── source/http.rs  # HttpRegistry - the configured registry, addressed by coordinates
├── source/dht.rs   # DhtRegistry + PeerBlobs - peers, via blob share
└── http.rs         # The artifact client and the capped body read
tests/
├── single_source.rs   # The configured source is the only one contacted
└── source_contract.rs # What a source owes: bytes, Ok(None), or a real fault
```

## The Contract

`ApplicationDownloader::download` has ONE post-condition: on `Outcome::Installed`
and `Outcome::AlreadyInstalled`, the application row for `req.application_id`
names `req.bytecode_id` and that blob is local. Bytes on disk always mean an
executable application - never a downloaded blob with nothing bound to it.

- `Unavailable` is not a failure. The source had no bytes yet; the caller keeps
  the version it runs and retries on next access.
- `Err` is a real fault: the bytes would not verify, the bundle would not
  install, or storage failed.
- There is no second route. `app_source` picks one source at construction, and
  an `Http` node with no `base_url` fails there rather than falling through.

## The Sources

Each implements `AppSource::fetch`, which hands back *unverified* bytes:
`Ok(None)` means it simply had nothing yet, `Err` is a real fault. The blob-id
check, the install and the blob release stay in the downloader, so no source can
bypass the post-condition above.

`[registry] mode` picks exactly one, and `Http` is the default - an unset
`[registry]` section is an http node with no `base_url`, which fails at its first
fetch rather than reaching for peers. `Http` reaches the configured registry and
holds no peer handle at all; `Dht` reaches peers through `PeerBlobs` and never
dials a registry. An `Http` node also refuses to serve or announce application
bytecode, since it is not a source of it - see `NodeClient::may_share_blob`.

`Dht` needs a context to authorize against, so a bare install by coordinates
(`install_by_coords`, no context and no `bytecode_id`) yields `Ok(None)` there:
a dht node gets applications from governance or a local `.mpk`, never by name.

## The Port

`ApplicationStore` is one trait with one implementation
(`NodeClient`, in `calimero-node-primitives`). It exists only to keep this
crate a leaf: the node client depends on the downloader, so the downloader
cannot name the node client back. Do not grow it into a general node facade.

## Common Gotchas

- `bytecode_id` is a `BlobId` (sha256 over chunk ids), never a `ContentHash`
  (sha256 over raw bytes). Never pass one where the other is expected - the
  content hash would reject every correct artifact.
- The per-kind application-id rule lives behind `bind_application`: a signed
  bundle **derives** `ApplicationId::for_bundle(package, signer_id)` and must
  equal the id governance named; raw wasm **adopts** that id and never
  re-derives it, because a raw-wasm id folds in per-node source and metadata.
- The only URL this crate ever fetches is the operator's own
  `[registry] base_url` plus coordinates, so no host guard applies - private and
  air-gapped registries are the point. `is_safe_coord` is the boundary that
  keeps a coordinate from walking out of that base.
- Add a source by implementing `AppSource` and giving it a `RegistryMode`, never
  by fetching inline at a call site or chaining a fallback behind an existing one.
