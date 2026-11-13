# calimero-primitives - Shared Ids and Wire Types

A leaf crate (no other Calimero crate as a dependency) holding the 32-byte ids
(`ApplicationId`, `BlobId`, `ContextId`, all over `Hash`), the outbound HTTPS
wire (`outbound`), the search vocabulary (`search`) and `utils::loggable_url`.
Much of the rest is left from the context-era node (libp2p sync, CRDT storage,
WebSocket events) and nothing uses it; see "Unused (v1 leftovers)".

## Package Identity

- **Crate**: `calimero-primitives`
- **Entry**: `src/lib.rs` (`pub mod` declarations only: no re-exports, no prelude; callers import from the module, `calimero_primitives::blobs::BlobId`)
- **Deps**: `sha2` (`Hash::new`), `hex` (the text form of every id), `serde`/`serde_json`, `borsh` (optional, feature `borsh`), `url` (`ApplicationSource`, `loggable_url`), `eyre`, `thiserror`; `ed25519-dalek`, `zeroize`, `multiaddr` and `rand` (optional) serve only unused modules

## Commands

```bash
cargo build -p calimero-primitives --features borsh
cargo test -p calimero-primitives     # dev-deps reimport the crate with `borsh`, so tests cover it
cargo test -p calimero-primitives encoding_tests
```

There is no `default` feature; `borsh` is opt-in (`calimero-bundle`, `calimero-search` and `tools/search-bench` enable it). The optional `rand` dependency is never enabled by any crate.

## Who uses what

Every use outside the crate goes through a full path, so this list is what
`grep -rn calimero_primitives crates tools apps fuzz` finds (no app or fuzz
target depends on the crate):

| Module | Items | Used by |
| --- | --- | --- |
| `outbound` | `OutboundMethod`, `OutboundRequest`, `OutboundResponse`, the `MAX_*`/`TIMEOUT_MS`/`SECRET_*` limits, `OutboundRefusal::{NotApproved, AwaitingApproval}`, `OutboundSelection` | `calimero-outbound` (the gate), `calimero-node` (`NodeHttp` encodes a component's request; `OutboundSelection` in `api/approvals.rs`, `api.rs`, `system.rs`, `consent_page.rs`) |
| `search` | `SearchDoc`, `SearchFieldKind`, `SearchFieldSchema`, `SearchIndexSchema`, `SearchValue`, `SearchMode`, `SearchOrder`, `SearchFilter`, `SearchRequest`, `SearchResponse`, `SearchHit` | `calimero-search` (`index.rs`, its tests), `calimero-node` (`component/search.rs`), `tools/search-bench` |
| `blobs` | `BlobId` | `calimero-node` (`api.rs`, `api/files.rs`: parse and print a blob id), `calimero-app-downloader` |
| `application` | `ApplicationId`, `ApplicationSource`, `ApplicationId::for_bundle` | `calimero-app-downloader` (`ApplicationId` in `AppRequest`; `ApplicationSource` only in its unused downloader), `calimero-bundle` tests (`ApplicationId::for_bundle`) |
| `context` | `ContextId` | `calimero-app-downloader` (`AppRequest::context_id`, always `None` from the node) |
| `utils` | `loggable_url` | `calimero-kms-client`, `calimero-app-downloader` |
| `hash`, `common` | `Hash`, `DIGEST_SIZE` | inside the crate only, under the id newtypes |

## Live types

| Type | Module | Purpose | Encoding |
| --- | --- | --- | --- |
| `Hash` | `hash` | 32-byte digest (`Hash::new` = SHA-256; `hash_json`, `hash_borsh`) | 64 lowercase hex over `Display`/`FromStr`/serde; raw 32 bytes over borsh; `Copy` |
| `ApplicationId`, `BlobId`, `ContextId` | `application`, `blobs`, `context` | Newtypes over `Hash`, so one cannot be passed for another | as `Hash` |
| `ApplicationSource` | `application` | A `Url` newtype | serde as a URL string |
| `OutboundRequest`/`OutboundResponse`/`OutboundMethod` | `outbound` | What a run asks the node to send, and what it gets back (status and body only) | borsh between the node and `calimero-outbound`; `OutboundMethod`'s borsh tag is its index, append only |
| `OutboundRefusal` | `outbound` | The words (`NotApproved`, `AwaitingApproval`) a refusal's reason starts with, which apps match on | text |
| `OutboundSelection` | `outbound` | What an approver approves: `"manifest"` or an array of prefixes | hand-written serde |
| `Search*` | `search` | Index schema, documents, query, hits | serde and borsh |

## Invariants and Gotchas

- **Every 32-byte id renders and parses as 64 hex**, and refuses base58 (`tests/encoding.rs`, a loop over the id types). Hex digits are a subset of base58's alphabet, so a parser taking both would decode some strings to the wrong id instead of failing. A new id type goes into that loop.
- **`Hash` caches no string**: `Display` encodes on demand, keeping the struct 32 bytes and `Copy`.
- **`ApplicationId::for_bundle(package, signer_id)`** is `Hash::hash_borsh(&(package, signer_id))` (feature `borsh`): the id is stable across versions of one package and key.
- **Outbound limits are the wire's** (`MAX_REQUEST_BODY` 64 KiB, `MAX_URL` 2048, `MAX_HEADERS` 8, `MAX_HEADER_BYTES` 1024, `MAX_RESPONSE_BODY` 1 MiB, `TIMEOUT_MS` 10 s); a longer response is refused, never truncated. `OutboundRefusal::word` is stable: the outbound crate writes it (`NOT_APPROVED`, `AWAITING_APPROVAL`) and apps match on it.
- **`loggable_url`** drops userinfo, the query (marked `?[redacted]`) and the fragment: use it for every URL logged at `info` and above.

## Unused (v1 leftovers)

Nothing outside the crate names these (the grep above), and no live module of
the crate depends on them. They describe systems that are gone (libp2p sync,
the causal DAG, CRDT merge storage, WebSocket events, namespaces with TEE
roles, the 0.x SDK's search exports) and are candidates for deletion; their doc
comments still describe those systems as current and were left as they are:

| Module or item | What it was |
| --- | --- |
| `events` (`NodeEvent`, `ContextEvent`, `ContextEventPayload`, ...) | the WebSocket event envelope; events now stream over SSE from the node's own types |
| `sync_status` (`SyncState`) | the `sync_status` RPC and its WS event |
| `crdt` (`CrdtType`, `LwwKind`) | `calimero-storage`'s merge-semantics tag |
| `alias` (`Alias<T>`, `ScopedAlias`) | context/application/member aliases; account aliases are the node's own (`Account::aliases`) |
| `metadata` (`MetadataRecord`, `validate_metadata_payload`) | group/context/member metadata |
| `identity` (`PrivateKey`, `PublicKey`, `AccountId`, `DeviceId`, `MemberIdentity`, `domain_hash`, ...) | context members, CRDT replica ids, namespace TEE authoring |
| `content_hash` (`ContentHash`) | a file's `sha256(content)`, distinct from `BlobId` |
| `version` (`Version`, `MAX_VERSION_STRING_LEN`) | build metadata sent in the libp2p handshake |
| `reflect` (`Reflect`, `ReflectExt`) | type-erasure helpers for the actix actors (`calimero-utils-actix`, gone) |
| `utils::compact_path`, `utils::prefix_upper_bound`, `utils::loggable_url_str` | logging and store-scan helpers |
| `common::{ZERO_HASH, bool_true, ResultAlt, multiaddr_to_url}` | the only reason for the `multiaddr` dependency |
| `context::{Context, ContextConfigParams, GroupMemberRole}`, `ContextId::from_seed` (feature `rand`, never enabled) | a context's DAG heads and root hash; namespace roles including `ReadOnlyTee`/`RelayTee` |
| `application::{Application, ApplicationBlob, SignerId, Version, ZERO_APPLICATION_ID}` | the context node's application rows |
| `blobs::{BlobInfo, BlobMetadata}` | the old blob API's answers |
| `outbound::{SealedSecret, SealedSecretPayload, SEALED_PREFIX, MAX_SEALED_ENVELOPE, MAX_REQUESTS_PER_RUN}`, `OutboundRefusal::SealedUnavailable` | secrets sealed to a namespace TEE key; the one-request bound (the per-run bound is the component host's `MAX_HTTP_REQUESTS`, 8) |
| `search::{SCHEMA_EXPORT, EXTRACT_EXPORT, SCAN_EXPORT, is_export, ExtractRequest, ExtractResponse, ScanRequest, ScanResponse}` | SDK 0.12's search exports; a component's search fields come from its ABI |

Deleting `identity` also drops `ed25519-dalek`, `zeroize` and `rand`; deleting
`common::multiaddr_to_url` drops `multiaddr`.

Part of [crates/](../AGENTS.md).
