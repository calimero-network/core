# calimero-tee-attestation - TEE Attestation Generation and Verification

Generates and verifies Intel TDX attestation quotes, binding them to a nonce and an application/identity hash, with a mock path for non-Linux development.

## Package Identity

- **Crate**: `calimero-tee-attestation`
- **Entry**: `src/lib.rs`
- **Key deps**: `tdx-quote` (parses raw TDX quote bytes), `dcap-qvl` (Intel DCAP collateral fetch + cryptographic quote verification), `calimero-server-primitives` (the serializable `Quote`/`QuoteHeader`/`QuoteBody`/`CertificationData` types), `base64`/`hex` (encoding), `eyre` (internal error plumbing), `tracing`. Linux-only: `configfs-tsm` (real quote generation via the kernel TSM configfs interface), `tdx_workload_attestation` (MRTD/launch-measurement retrieval), `reqwest` (cloud metadata detection)

## Commands

```bash
# Build
cargo build -p calimero-tee-attestation

# Test (the `policy` module has unit tests; the rest is exercised via callers)
cargo test -p calimero-tee-attestation
```

### The `mock-attestation` feature (default OFF)

The crate has exactly one feature: **`mock-attestation`**, default **off**. It gates the entire mock path so it is *structurally absent* from production builds rather than merely runtime-guarded:

| Item | Without the feature |
| --- | --- |
| `MOCK_QUOTE_HEADER`, `generate_mock_attestation`, `is_mock_quote`, `create_mock_quote`, `verify_mock_attestation` | **Do not exist** (not compiled, not re-exported) |
| Non-Linux `generate_attestation` | Returns `AttestationError::QuoteGenerationFailed("mock attestation not compiled in; build with --features mock-attestation on non-TDX platforms")` instead of falling back to a mock quote |
| A `MOCK_TDX_QUOTE_V1` blob | Has no special handling: it just fails real DCAP parse and is rejected |

Platform selection is otherwise still `cfg(target_os = "linux")` vs not - real TDX quote generation only compiles on Linux. Callers own the policy decision (`mock_tee` / `accept_mock`) of whether a mock quote is acceptable; those policy *fields* remain regardless of the feature (they become inert when there is no mock code).

```bash
# Build/test the mock path (dev/CI/harness only)
cargo build -p calimero-tee-attestation --features mock-attestation
cargo test -p calimero-tee-attestation --features mock-attestation
```

Consumers propagate it explicitly: `calimero-server/mock-attestation`, `calimero-node/mock-attestation`, `merod/mock-attestation` all fan out to `calimero-tee-attestation/mock-attestation`. **No shipped crate lists it in default features**, and `.github/workflows/release.yml` fails the release if `cargo tree -e features -i calimero-tee-attestation -p merod` resolves it.

> **External consumers (mero-tee):** enabling `mock-attestation` re-exposes `is_mock_quote` / `verify_mock_attestation`. A consumer that calls those symbols must enable the feature explicitly; with it off they will not resolve.

## Public API

| Item | Kind | Purpose |
| --- | --- | --- |
| `build_report_data(nonce, app_hash)` | fn | Packs `nonce[32] \|\| app_hash[32]` (zero-filled if `app_hash` is `None`) into the 64-byte TDX report data field |
| `generate_attestation(report_data)` | fn | Linux+TDX: calls `configfs_tsm::create_tdx_quote`, parses with `tdx_quote::Quote`, converts to `Quote`. Non-Linux: falls back to `generate_mock_attestation` with a `warn!` **only under `mock-attestation`**; without the feature it returns `QuoteGenerationFailed` |
| `generate_mock_attestation(report_data)` | fn | **`mock-attestation` only.** Builds a syntactically valid but cryptographically invalid quote on any platform; `is_mock: true` |
| `is_mock_quote(quote_bytes)` | fn | **`mock-attestation` only.** Checks for the `MOCK_QUOTE_HEADER` magic prefix |
| `AttestationResult` | struct | `quote_bytes: Vec<u8>`, `quote_b64: String`, `quote: Quote`, `is_mock: bool` |
| `verify_attestation(quote_bytes, nonce, expected_app_hash)` | async fn | Real path: parses the quote, fetches Intel PCS collateral (`dcap_qvl::collateral::get_collateral_from_pcs`), runs `dcap_qvl::verify::verify`, checks nonce and app hash against `report_input_data()` |
| `verify_mock_attestation(quote_bytes, nonce, expected_app_hash)` | fn | **`mock-attestation` only.** Verifies a mock quote's embedded nonce/app hash; `quote_verified` is unconditionally `true` (there is no signature) |
| `VerificationResult` | struct | `quote_verified`, `nonce_verified`, `application_hash_verified: bool`; `tcb_status: Option<String>`; `advisory_ids: Vec<String>`; `quote: Quote` |
| `VerificationResult::is_valid()` | fn | `quote_verified && nonce_verified && application_hash_verified`. **Crypto-only — not an authorization verdict**: it ignores `tcb_status` and the measurement registers, so it returns `true` for a crypto-valid quote from an `OutOfDate`/`Revoked` platform or an unapproved workload. Never admit/release a key on this alone |
| `VerificationResult::policy_valid(&policy)` | fn | The safe-by-construction gate new callers should use: folds `is_valid()` together with a fail-closed TCB allowlist, the mock decision, the MRTD/RTMR allowlists and the app-hash binding. `Result<(), PolicyRejection>` |
| `VerifierPolicy` | struct | The policy `policy_valid` enforces: `allowed_tcb_statuses`, `allowed_mrtd`, `allowed_rtmr0..3`, `accept_mock`, `require_app_hash`. `VerifierPolicy::new(allowed_mrtd)` is the intended entry point. Every field fails closed (empty `allowed_mrtd` rejects; empty `allowed_tcb_statuses` enforces `DEFAULT_ALLOWED_TCB_STATUS`) — except `allowed_rtmr0..3`, where empty deliberately skips that register |
| `PolicyRejection` | enum | Typed reason: `NotCryptoValid`, `TcbRevoked`, `TcbNotAllowed { status }`, `MockNotAccepted`, `EmptyMrtdAllowlist`, `MeasurementMismatch { register }`, `AppHashNotBound` |
| `MeasurementRegister` | enum | `Mrtd`, `Rtmr0..3` — names the register in `MeasurementMismatch` |
| `DEFAULT_ALLOWED_TCB_STATUS` / `TCB_STATUS_REVOKED` / `TCB_STATUS_MOCK` | const | `"UpToDate"` / `"Revoked"` / `"Mock"`. **Not** gated by `mock-attestation`: `policy_valid` only compares the `tcb_status` *string*, so production builds must keep the ability to reject a mock quote |
| `get_tee_info()` | async fn | Returns `TeeInfo { cloud_provider, os_image, mrtd }`; on Linux reads MRTD via `tdx_workload_attestation` and probes the GCP metadata server for the OS image |
| `TeeInfo` | struct | `cloud_provider: String`, `os_image: String`, `mrtd: String` (hex) |
| `AttestationError` | enum | `NotSupported`, `QuoteGenerationFailed`, `QuoteParsingFailed`, `QuoteConversionFailed`, `QuoteVerificationFailed`, `CollateralFetchFailed`, `InvalidNonce`, `InvalidApplicationHash`, `NonceMismatch { expected, actual }`, `ApplicationHashMismatch { expected, actual }`, `InfoRetrievalFailed`, `SystemTimeError` - all carry `String` context, no source-error chaining |

`Quote`/`QuoteHeader`/`QuoteBody`/`CertificationData`/`QeReportCertificationDataInfo` are re-exported through `calimero_server_primitives::admin`, not defined in this crate - this crate only builds and consumes them (`Quote: TryFrom<tdx_quote::Quote>`).

## Mental Model: Generate vs Verify

Generation and verification are asymmetric and live in separate modules for a reason: generation only runs on the machine being attested (needs the TDX hardware path), verification runs on whichever peer is checking someone else's claim (needs only network access to Intel PCS, no TDX hardware).

**Generate** (`src/generate.rs`): report data (`nonce || app_hash`, 64 bytes) goes in, a `Quote` comes out. On Linux this is a real hardware call through `configfs-tsm` into the TDX module; the returned raw bytes are re-parsed with `tdx_quote::Quote::from_bytes` purely to convert them into the serializable `Quote` struct for transport/storage. Off Linux, `generate_attestation` silently degrades to `generate_mock_attestation` - same function signature, different guarantees, so callers must always check `result.is_mock` before trusting anything.

**Verify** (`src/verify.rs`): a `Quote`'s raw bytes go in, a `VerificationResult` comes out. Three independent checks compose into `is_valid()`:
1. `quote_verified` - cryptographic: DCAP verify() against collateral fetched from Intel's PCS, using current wall-clock time for freshness.
2. `nonce_verified` - `report_data[0..32] == nonce`, defeats replay of an old quote.
3. `application_hash_verified` - `report_data[32..64] == expected_app_hash`. This argument is **mandatory** (not `Option`) by design: an attestation that doesn't bind to a specific application/identity is meaningless as an authorization artifact, so there is no "skip the binding check" code path.

Mock quotes (`is_mock_quote`, magic header `MOCK_QUOTE_HEADER = b"MOCK_TDX_QUOTE_V1"`) exist purely so the generate/verify protocol flow can be exercised on non-TDX dev machines and in CI. `verify_mock_attestation` unconditionally sets `quote_verified = true` since there is no real signature to check - it only re-checks nonce and app hash. Never call it on a quote you haven't already confirmed with `is_mock_quote`; it errors out if the header doesn't match, but the caller is still responsible for deciding whether mock quotes are policy-acceptable at all (see below).

## dstack / Phala KMS Relationship

This crate has no dstack or Phala-specific code - it is a generic TDX quote generate/verify library. The Phala Cloud KMS integration lives in `crates/merod/src/kms/mod.rs`: `merod` calls `generate_attestation` to produce its own quote when authenticating to a Phala KMS endpoint, and calls `verify_attestation` / `verify_mock_attestation` to check quotes returned by that KMS (e.g. from its `/attest` endpoint) before trusting a fetched storage encryption key. The mock/real decision and the `accept_mock` policy enforcement (reject a mock quote unless explicitly allowed) is entirely the caller's responsibility - this crate just answers "is this quote's binding and signature valid," it never decides whether mock is acceptable.

## Key Files

| Path | What's there |
| --- | --- |
| `src/lib.rs` | Public re-exports and module-level docs; the module boundary is intentional (generate/verify/info/error are independently testable and have different platform `cfg`s) |
| `src/generate.rs` | `generate_attestation`, `generate_mock_attestation`, `is_mock_quote`, `create_mock_quote`, `build_report_data`, `MOCK_QUOTE_HEADER`, `AttestationResult` |
| `src/verify.rs` | `verify_attestation`, `verify_mock_attestation`, `VerificationResult` |
| `src/policy.rs` | `VerificationResult::policy_valid`, `VerifierPolicy`, `PolicyRejection`, `MeasurementRegister`, the TCB constants, and the policy unit tests. Deliberately mirrors `calimero_governance_store::membership::policy_rules::tcb_status_allowed` by value rather than importing it — this is a leaf `publish = true` crate and must not depend on the governance store, so the two MUST stay in sync |
| `src/info.rs` | `get_tee_info`, `TeeInfo`; MRTD retrieval and cloud-provider detection |
| `src/error.rs` | `AttestationError` |

## Invariants and Gotchas

- **App hash binding is not optional at verify time**: both `verify_attestation` and `verify_mock_attestation` take `expected_app_hash: &[u8; 32]` as a required argument, never `Option`. If you're tempted to add a "verify without app hash" convenience function, don't - that would let an attestation for one application be replayed to authorize a different one.
- **`is_mock` must be checked by every caller of `generate_attestation`**: *under the `mock-attestation` feature* the function silently returns a mock result on non-Linux instead of erroring, so code that assumes "if this returned Ok, it's a real TDX quote" is wrong on any non-Linux build with that feature on (dev laptops, the mock harness). Without the feature (the default, and every release build) the non-Linux body errors instead, so `is_mock` can never be `true` there - but keep the check: it is what makes the caller correct in both configurations.
- **Mock quotes are format-tagged, not crypto-tagged**: `is_mock_quote` only checks for a 17-byte magic prefix (`MOCK_TDX_QUOTE_V1`). There is no signature distinguishing a mock from a real quote beyond this header - do not rely on it as a security boundary, only as a routing signal for which verify function to call.
- **`verify_attestation` requires network access**: it calls out to Intel PCS (`get_collateral_from_pcs`) to fetch collateral on every call; there is no local/cached collateral path in this crate. A verifier with no internet access cannot verify real quotes.
- **`AttestationError` variants carry only `String`, not `Box<dyn Error>`**: underlying errors from `dcap_qvl`, `tdx_quote`, `configfs_tsm`, etc. are stringified with `{err:?}` at the call site and lose their original type; don't try to `downcast` an `AttestationError` to find a specific upstream failure.
- **`create_mock_quote` produces all-zero measurements**: `mrtd`, `mrseam`, RTMRs, etc. are all `"00..00"` hex strings, not derived from anything - a mock quote's measurement fields carry no information, only `reportdata` (the actual nonce/app_hash) is real.
- **Linux-only deps gate real generation and MRTD**: `configfs-tsm`, `tdx_workload_attestation`, and `reqwest` are only pulled in `cfg(target_os = "linux")`; `get_tee_info` and `generate_attestation` have entirely separate non-Linux bodies. The non-Linux `generate_attestation` splits again on `mock-attestation`: mock fallback with the feature, hard `QuoteGenerationFailed` without it.
- **Do not reintroduce an ungated reference to a mock symbol**: `is_mock_quote` / `verify_mock_attestation` / `generate_mock_attestation` / `MOCK_QUOTE_HEADER` only exist under `mock-attestation`. Every call site must sit under `#[cfg(feature = "mock-attestation")]` with a real-only counterpart under `#[cfg(not(...))]`, and the crate must stay clippy-clean in **both** configurations (that is the CI gate).

Part of [crates/](../AGENTS.md).
