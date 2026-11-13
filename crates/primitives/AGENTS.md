# calimero-primitives - Shared Ids and Wire Types

A leaf crate (no other Calimero crate as a dependency) holding the 32-byte ids
(`ApplicationId`, `BlobId`, `ContextId`, all over `Hash`), the outbound HTTPS
wire (`outbound`), the search vocabulary (`search`) and `utils::loggable_url`.
That is all of it: the context-era modules (identity keys, aliases, CRDT tags,
WebSocket events, build metadata) were removed once nothing used them.

## Package Identity

- **Crate**: `calimero-primitives`
- **Entry**: `src/lib.rs` (`pub mod` declarations only: no re-exports, no prelude; callers import from the module, `calimero_primitives::blobs::BlobId`)
- **Deps**: `sha2` (`Hash::new`), `hex` (the text form of every id), `serde`/`serde_json`, `borsh` (optional, feature `borsh`), `url` (`ApplicationSource`, `loggable_url`), `eyre` (`ApplicationId::for_bundle`), `thiserror`

## Commands

```bash
cargo build -p calimero-primitives --features borsh
cargo test -p calimero-primitives     # dev-deps reimport the crate with `borsh`, so tests cover it
cargo test -p calimero-primitives encoding_tests
```

There is no `default` feature; `borsh` is opt-in (`calimero-bundle`, `calimero-search` and `tools/search-bench` enable it).

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

Part of [crates/](../AGENTS.md).
