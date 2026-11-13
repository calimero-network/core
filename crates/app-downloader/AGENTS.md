# calimero-app-downloader - Fetching Releases from a Registry

Fetches a signed release (`.mpk`) by `package@version` from the operator's
package registry. The node uses one piece of it: `HttpRegistry`, behind
`POST /packages/install` and the installs `POST /apps` and upgrade runs make
(`crates/node/src/packages.rs`, `Node::registry` and `fetch_from_registry`).
Verification, the install and the release record are the node's
(`calimero_bundle::VerifiedBundle`, `InstallPackage`), not this crate's.

## Package Identity

- **Crate**: `calimero-app-downloader`
- **Entry**: `src/lib.rs`
- **Consumer**: `calimero-node` only (`packages.rs`: `HttpRegistry`, `AppRequest`, `AppSource`)
- **Deps**: `reqwest`, `url`, `async-trait`, `calimero-primitives` (`ApplicationId`, `BlobId`, `ContextId`, `ApplicationSource`, `utils::loggable_url`)

## Commands

```bash
cargo build -p calimero-app-downloader
cargo test -p calimero-app-downloader
```

## File Organization

```
src/
├── lib.rs          # Re-exports; the crate's public surface
├── source.rs       # AppSource + AppRequest + app_source
├── source/http.rs  # HttpRegistry, its artifact client and the capped body read
├── source/dht.rs   # DhtRegistry + PeerBlobs (unused, see below)
├── registry.rs     # RegistryConfig/RegistryMode, RegistryCoords::artifact_url, is_safe_coord
├── downloader.rs   # ApplicationDownloader::download (unused, see below)
└── port.rs         # ApplicationStore (unused, see below)
tests/
├── single_source.rs   # app_source + ApplicationDownloader: the configured source is the only one contacted
└── source_contract.rs # What a source owes: bytes, Ok(None), or a real fault
```

## What the node uses

The node builds an `HttpRegistry` from `--registry-url` (`Config::registry`;
none answers `400 NO_REGISTRY`) and calls `AppSource::fetch` with an
`AppRequest { package, version }` and no ids. The answer is the *unverified*
bytes:

- `Ok(Some(bytes))`: the artifact at
  `{base}/artifacts/{package}/{version}/{package}-{version}.mpk`.
- `Ok(None)`: the registry answered `404` (not published there); the node
  answers `404`.
- `Err`: a real fault (unreachable, other status, too large, unaddressable
  coordinates); the node answers `503`.

The node then verifies the publisher signature and every digest itself, and
fetches a registry release again from the registry (never a peer) when it runs
one it does not hold (`Node::release_wasm`).

## Unused (v1 leftovers)

Nothing outside this crate's own tests names these; they are the context-era
download path (a group naming a `bytecode_id`, an application row, peers via
blob share), and are candidates for deletion:

- `ApplicationDownloader`, `Outcome` (`downloader.rs`) and the
  `ApplicationStore`/`InstalledApplication` port (`port.rs`): its one
  implementation (`NodeClient` in `calimero-node-primitives`) is gone.
- `DhtRegistry`, `PeerBlobs` (`source/dht.rs`), `app_source`, `RegistryConfig`,
  `RegistryMode`, `RegistryCoordsBuf`, `stored_coords`,
  `PENDING_BLOB_SHARE_SOURCE` (`registry.rs`): the node reads `--registry-url`
  itself, and has no `[registry] mode` and no peer route.
- `AppRequest::application_id`, `bytecode_id` and `context_id`: the node always
  passes `None`.

Evidence: `grep -rn calimero_app_downloader crates tools` finds only
`crates/node/src/packages.rs`, importing `source::http::HttpRegistry`,
`AppRequest` and `AppSource`.

## Common Gotchas

- `HttpRegistry::new` checks the base once: only `http`/`https`, and plain
  `http` only for `localhost` or a loopback, private, unique-local or
  link-local IP literal (`is_local_or_private`).
- There is no host guard beyond that: the base is the operator's own,
  routinely private or air-gapped. `is_safe_coord` (non-empty, at most
  `MAX_COORD_LEN`, `[A-Za-z0-9._-]`, no `..`) is what keeps a coordinate from
  walking out of the base; an unaddressable coordinate is an `Err`, never
  `Ok(None)`.
- The artifact client follows no redirect (`Policy::none()`), times out
  (15 s connect, 300 s total) and caps the body at `MAX_ARTIFACT_BYTES`
  (512 MiB) while reading, whatever `Content-Length` says.
- URLs are logged through `calimero_primitives::utils::loggable_url`, which
  drops credentials and the query.

Part of [crates/](../AGENTS.md).
