# Bundle Installation Invitation Flow Test Plan

## Overview
This document outlines the test plan for verifying that bundle installations work correctly in the invitation flow, where User 1 installs a bundle, creates a context, and invites User 2, who should automatically receive and install the bundle.

## Current Test Coverage

### ✅ `test_bundle_blob_sharing_integration`
**Status**: Implemented  
**What it tests**: 
- User 1 installs bundle
- User 2 receives bundle blob (simulating blob sharing)
- User 2 installs from bundle blob using `install_application_from_bundle_blob`
- Verifies `ApplicationId` consistency across nodes

**What it doesn't test**:
- Full invitation flow through `sync_context_config`
- External config client integration
- Context creation and invitation

## Full Invitation Flow Test Plan

### Test Scenario: User 1 Installs Bundle → Invites User 2 → User 2 Gets Bundle

**Steps**:
1. **User 1 Setup**:
   - User 1 installs bundle via `install_application_from_path`
   - User 1 creates context with bundle application
   - User 1 stores context config on external chain/contract (via `add_context`)
   - User 1 invites User 2 (adds User 2 to members list)

2. **User 2 Setup**:
   - User 2 receives invitation
   - User 2 receives bundle blob via blob sharing (DHT discovery)
   - User 2 calls `sync_context_config` with context ID

3. **Expected Behavior**:
   - `sync_context_config` fetches context config from external client
   - `sync_context_config` detects application is not installed locally
   - `sync_context_config` checks if blob exists locally (it should, via blob sharing)
   - `sync_context_config` detects blob is a bundle via `is_bundle_blob`
   - `sync_context_config` calls `install_application_from_bundle_blob`
   - User 2 can now use the context with the bundle application

4. **Verification**:
   - User 2 has the application installed
   - `ApplicationId` matches User 1's `ApplicationId`
   - User 2 can execute methods from the bundle WASM
   - Bundle artifacts are extracted correctly on User 2's node

## Implementation Requirements

### Dependencies Needed
1. **ContextClient Setup**:
   - Requires `Store`, `NodeClient`, `ExternalClient`, `LazyRecipient<ContextMessage>`
   - External client needs to be mockable or use a test transport

2. **External Config Client Mocking**:
   - Mock `application()` - returns bundle application with blob_id, size, source, metadata
   - Mock `application_revision()` - returns revision number
   - Mock `members_revision()` - returns revision number
   - Mock `members()` - returns list of public keys

3. **Blob Sharing Simulation**:
   - User 2 needs to receive the bundle blob before calling `sync_context_config`
   - This can be simulated by manually adding the blob to User 2's blobstore

### Test Implementation Strategy

**Option 1: Unit Test with Mocks** (Recommended for now)
- Create mock external config client
- Test `sync_context_config` directly with mocked external client
- Verify bundle installation logic is called correctly

**Option 2: Integration Test** (Future work)
- Set up full ContextClient instances
- Use test transport for external client (e.g., in-memory mock)
- Test end-to-end flow

**Option 3: E2E Test** (Future work)
- Use actual test network (e.g., NEAR sandbox)
- Deploy context config contract
- Test real invitation flow

## Current Status

### ✅ What's Tested
- Bundle installation from path
- Bundle installation from URL
- Bundle blob detection (`is_bundle_blob`)
- Bundle installation from blob (`install_application_from_bundle_blob`)
- Bundle extraction and deduplication
- `ApplicationId` consistency across nodes
- WASM loading from bundles (`get_application_bytes`)

### ⚠️ What's Partially Tested
- Blob sharing integration (tested manually, not through `sync_context_config`)

### ❌ What's Not Tested
- Full invitation flow through `sync_context_config`
- External config client integration
- Context creation with bundles
- Invitation acceptance flow

## Next Steps

1. ✅ **Short Term**: Document current test coverage (this document)
2. ✅ **Short Term**: Create e2e workflow test using existing e2e-tests infrastructure
3. **Medium Term**: Add unit test for `sync_context_config` bundle installation logic with mocked external client (optional, e2e test covers this)
4. **Long Term**: Add more e2e test scenarios (bundle updates, multiple versions, etc.)

## E2E Test Implementation

### Created Files
- **Workflow**: `apps/kv-store/workflows/bundle-invitation-test.yml`
  - Tests full invitation flow: User 1 installs bundle → creates context → invites User 2 → User 2 gets bundle automatically
  - Verifies bundle installation via method execution (if User 2 can execute methods, bundle was installed correctly)
  
- **Build Script**: `apps/kv-store/build-bundle.sh`
  - Creates `.mpk` bundle from WASM and ABI files
  - Generates `manifest.json` with package, version, and artifact metadata
  - Packages everything into a tar.gz archive with `.mpk` extension

### Running the Test

```bash
# 1. Build the bundle
cd apps/kv-store
./build-bundle.sh

# 2. Build binaries
cargo build -p merod
cargo build -p meroctl

# 3. Run e2e test
cargo run -p e2e-tests -- \
  --input-dir ./apps/kv-store/workflows \
  --output-dir ./e2e-tests/corpus \
  --merod-binary ./target/debug/merod \
  --meroctl-binary ./target/debug/meroctl
```

### Test Flow
1. User 1 installs bundle from `.mpk` file
2. User 1 creates context with bundle application
3. User 1 invites User 2
4. User 2 joins context (triggers `sync_context_config`)
5. `sync_context_config` detects bundle blob exists locally
6. `sync_context_config` calls `install_application_from_bundle_blob`
7. User 2 can execute methods from bundle WASM
8. State sync works correctly between nodes

## Code Locations

- Bundle installation: `crates/node/primitives/src/client/application.rs`
- Context sync: `crates/context/primitives/src/client/sync.rs`
- Tests: `crates/node/primitives/tests/bundle_installation.rs`
- External config client: `crates/context/primitives/src/client/external/config.rs`

