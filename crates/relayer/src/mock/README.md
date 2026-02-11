# Mock Relayer

The mock relayer is an in-memory implementation of the Calimero relayer that simulates blockchain interactions without requiring actual blockchain infrastructure. It's designed for local development, testing, and rapid prototyping.

## Table of Contents

- [Overview](#overview)
- [Architecture](#architecture)
- [Feature Parity](#feature-parity)
- [What's Mocked](#whats-mocked)
- [Usage](#usage)
- [Limitations](#limitations)
- [Testing](#testing)

## Overview

The mock relayer provides a complete simulation of context configuration and proxy contract operations that would normally occur on NEAR. All state is maintained in-memory using Rust's standard collections (`BTreeMap`, `BTreeSet`).

### Key Benefits

- **Zero Setup**: No blockchain nodes, RPC endpoints, or credentials required
- **Instant Execution**: No transaction delays or gas costs
- **Deterministic**: Perfect for testing and CI/CD pipelines
- **Full API Compatibility**: Drop-in replacement for real relayer during development

## Architecture

The mock relayer consists of three main components:

```
mock/
├── mod.rs           # Main MockRelayer struct with request handling
├── handlers.rs      # Request routing and operation handlers
├── state.rs         # In-memory state management
└── tests.rs         # Unit tests demonstrating usage
```

### Request Flow

```
RelayRequest
    ↓
MockRelayer::handle_request()
    ↓
MockHandlers::handle_operation()
    ↓
├─ Operation::Read  → handle_read()  → Query handlers
└─ Operation::Write → handle_write() → Mutation handlers
    ↓
MockState (in-memory storage)
```

### State Management

[State structure](state.rs#L12-L18):
```rust
pub struct MockState {
    /// Context metadata indexed by context ID
    pub contexts: BTreeMap<ContextId, ContextData>,
    /// Nonce tracking per member per context
    pub nonces: BTreeMap<(ContextId, ContextIdentity), u64>,
}
```

[Each context stores](state.rs#L22-L42):
```rust
pub struct ContextData {
    pub application: Application<'static>,
    pub application_revision: Revision,
    pub members: BTreeSet<ContextIdentity>,
    pub members_revision: Revision,
    pub proxy_contract_id: String,
    pub proposals: BTreeMap<ProposalId, Proposal>,
    pub approvals: BTreeMap<ProposalId, Vec<ProposalApprovalWithSigner>>,
    pub storage: BTreeMap<Vec<u8>, Vec<u8>>,
    pub capabilities: BTreeMap<SignerId, Vec<Capability>>,
}
```

## Feature Parity

### ✅ Fully Implemented

#### Context Configuration Operations

| Operation | Method | Description | Implementation |
|-----------|--------|-------------|----------------|
| Add Context | `mutate` | Create new context with application and author | [handlers.rs:235-250](handlers.rs#L235-L250) |
| Update Application | `mutate` | Update application metadata | [handlers.rs:252-269](handlers.rs#L252-L269) |
| Add Members | `mutate` | Add members to context | [handlers.rs:270-279](handlers.rs#L270-L279) |
| Remove Members | `mutate` | Remove members from context | [handlers.rs:281-290](handlers.rs#L281-L290) |
| Grant Capabilities | `mutate` | Grant permissions to members | [handlers.rs:292-307](handlers.rs#L292-L307) |
| Revoke Capabilities | `mutate` | Revoke permissions from members | [handlers.rs:308-321](handlers.rs#L308-L321) |

#### Context Query Operations

| Operation | Method | Description | Implementation |
|-----------|--------|-------------|----------------|
| Query Application | `application` | Get application metadata | [handlers.rs:80-94](handlers.rs#L80-L94) |
| Query Application Revision | `application_revision` | Get application revision number | [handlers.rs:96-110](handlers.rs#L96-L110) |
| Query Members | `members` | List members (paginated) | [handlers.rs:112-136](handlers.rs#L112-L136) |
| Query Members Revision | `members_revision` | Get members revision number | [handlers.rs:138-152](handlers.rs#L138-L152) |
| Check Membership | `has_member` | Check if identity is a member | [handlers.rs:154-171](handlers.rs#L154-L171) |
| Query Privileges | `privileges` | Get member privileges | [handlers.rs:173-177](handlers.rs#L173-L177) |
| Get Proxy Contract | `get_proxy_contract` | Get proxy contract ID | [handlers.rs:179-193](handlers.rs#L179-L193) |
| Fetch Nonce | `fetch_nonce` | Get member nonce | [handlers.rs:195-213](handlers.rs#L195-L213) |

#### Proxy Contract Operations

| Operation | Method | Description | Implementation |
|-----------|--------|-------------|----------------|
| **Mutations** |
| Create Proposal | `proxy_mutate` (Propose) | Create governance proposal | [handlers.rs:533-543](handlers.rs#L533-L543) |
| Approve Proposal | `proxy_mutate` (Approve) | Add approval to proposal | [handlers.rs:545-562](handlers.rs#L545-L562) |
| **Queries** |
| List Proposals | `proposals` | Get proposals (paginated) | [handlers.rs:336-359](handlers.rs#L336-L359) |
| Get Proposal | `proposal` | Get specific proposal by ID | [handlers.rs:361-379](handlers.rs#L361-L379) |
| Active Proposal Count | `get_number_of_active_proposals` | Count active proposals | [handlers.rs:381-393](handlers.rs#L381-L393) |
| Approval Count | `get_number_of_proposal_approvals` | Get approval count for proposal | [handlers.rs:395-420](handlers.rs#L395-L420) |
| Proposal Approvers | `get_proposal_approvers` | List who approved proposal | [handlers.rs:422-439](handlers.rs#L422-L439) |
| Get Context Value | `get_context_value` | Read from context storage | [handlers.rs:441-459](handlers.rs#L441-L459) |
| Get Storage Entries | `get_context_storage_entries` | List storage entries (paginated) | [handlers.rs:461-486](handlers.rs#L461-L486) |

### ⚠️ Partially Implemented

| Feature | Status | Notes |
|---------|--------|-------|
| Proposal Execution | ❌ Not implemented | Proposals are stored but actions aren't executed |
| Threshold Logic | ❌ Not implemented | No automatic execution when approval threshold is met |
| Invitation System | ⚠️ No-op | `CommitOpenInvitation` and `RevealOpenInvitation` accepted but ignored |
| Update Proxy Contract | ⚠️ No-op | Proxy contract ID is deterministic, updates ignored |

### ❌ Not Implemented (By Design)

| Feature | Reason | Alternative |
|---------|--------|------------|
| Signature Verification | Mock mode skips crypto validation | Use integration tests with real chains |
| Transaction Costs | No gas/fees in mock mode | N/A |
| Blockchain Persistence | In-memory only | State cleared on restart |
| Network Delays | Instant execution | Add artificial delays in tests if needed |
| Transaction Finality | No consensus mechanism | All operations are immediately final |

## What's Mocked

### 1. Signature Verification

**Real Blockchain**: Validates cryptographic signatures using public keys
```rust
// Production: Verifies ED25519 signature
verify_signature(payload, signature, public_key)?;
```

**Mock Implementation** [handlers.rs:219](handlers.rs#L219):
```rust
// Skip signature verification in mock mode
// Deserialize directly from bytes
let kind: RequestKind<'_> = serde_json::from_slice(payload)?;
```

### 2. Proxy Contract ID Generation

**Real Blockchain**: Deployed contract has actual on-chain address

**Mock Implementation** [state.rs:71-76](state.rs#L71-L76):
```rust
fn generate_proxy_contract_id(context_id: &ContextId) -> String {
    let bytes = context_id.to_bytes();
    format!("mock-proxy-{}", bs58::encode(bytes).into_string())
}
```

**Result**: Deterministic IDs like `mock-proxy-3yZe7d7q9rW8J1V2X3bY4c5D6E7F8G9H`

### 3. State Persistence

**Real Blockchain**: State stored on distributed ledger

**Mock Implementation** [mod.rs:22-25](mod.rs#L22-L25):
```rust
pub struct MockRelayer {
    state: Arc<RwLock<MockState>>,
}
```

**Result**: All state in memory, cleared on restart. Thread-safe via `RwLock`.

### 4. Transaction Execution

**Real Blockchain**: Asynchronous with block confirmations

**Mock Implementation** [mod.rs:36-40](mod.rs#L36-L40):
```rust
pub async fn handle_request(&self, request: RelayRequest<'_>) -> EyreResult<Vec<u8>> {
    let mut state = self.state.write().await;
    MockHandlers::handle_operation(&mut state, &request.operation, &request.payload)
}
```

**Result**: Synchronous execution, instant "confirmation"

### 5. Nonce Management

**Real Blockchain**: Prevents replay attacks, enforced on-chain

**Mock Implementation** [state.rs:164-173](state.rs#L164-L173):
```rust
pub fn get_nonce(&mut self, context_id: &ContextId, member_id: &ContextIdentity) -> u64 {
    *self.nonces.entry((*context_id, *member_id)).or_insert(0)
}

pub fn increment_nonce(&mut self, context_id: &ContextId, member_id: &ContextIdentity) {
    let nonce = self.nonces.entry((*context_id, *member_id)).or_insert(0);
    *nonce += 1;
}
```

**Result**: Tracked in-memory, not enforced (callers must increment manually)

### 6. Proposal Actions

**Real Blockchain**: Executes `ProposalAction` variants when approved

**Mock Implementation**: Stores but doesn't execute
```rust
// Stored in context.proposals
pub proposals: BTreeMap<ProposalId, Proposal>,

// Actions like SetContextValue, DeleteContextValue are NOT executed
```

**Impact**: Proposals can be created and approved, but their effects aren't applied

## Usage

### Basic Setup

```rust
use calimero_relayer::mock::MockRelayer;
use calimero_context_config::client::relayer::RelayRequest;
use calimero_context_config::client::transport::Operation;

// Create mock relayer
let relayer = MockRelayer::new();

// Handle requests
let response = relayer.handle_request(relay_request).await?;
```

### Creating a Context

See [tests.rs:64-92](tests.rs#L64-L92) for complete example:

```rust
let request = RequestKind::Context(ContextRequest::new(
    Repr::new(context_id),
    ContextRequestKind::Add {
        author_id: Repr::new(author_id),
        application: application.clone(),
    },
));

let payload = serde_json::to_vec(&request).unwrap();
let operation = Operation::Write {
    method: Cow::Borrowed("mutate"),
};

let result = MockHandlers::handle_operation(&mut state, &operation, &payload);
```

### Creating and Approving Proposals

See [tests.rs:273-369](tests.rs#L273-L369) for complete examples:

```rust
// Create proposal
let proposal = Proposal {
    id: Repr::new(proposal_id),
    author_id: Repr::new(signer_id),
    actions: vec![ProposalAction::SetContextValue {
        key: vec![1, 2, 3].into_boxed_slice(),
        value: vec![4, 5, 6].into_boxed_slice(),
    }],
};

let request = ProxyMutateRequest::Propose {
    proposal: proposal.clone(),
};

// Approve proposal
let approval = ProposalApprovalWithSigner {
    proposal_id: Repr::new(proposal_id),
    signer_id: Repr::new(approver_id),
    added_timestamp: 1234567890,
};

let approve_request = ProxyMutateRequest::Approve { approval };
```

## Limitations

### 1. No Proposal Execution

**Issue**: Proposal actions aren't executed when approved.

**Example**:
```rust
// This proposal is stored but SetContextValue is NOT executed
ProposalAction::SetContextValue {
    key: b"config".to_vec(),
    value: b"new_value".to_vec(),
}
```

**Workaround**: Manually apply changes in tests, or extend mock to execute actions.

### 2. Simplified Context Binding

**Issue**: Proposals stored in first available context [handlers.rs:536](handlers.rs#L536).

**Impact**: Multi-context scenarios may behave differently than production.

**Workaround**: Test with single context, or modify mock to track context-proposal associations.

### 3. No Automatic Nonce Increment

**Issue**: Nonces must be manually incremented by callers.

**Example**:
```rust
// Manual increment required
state.increment_nonce(&context_id, &member_id);
```

**Impact**: Tests must manage nonces explicitly.

### 4. State is Ephemeral

**Issue**: All state lost when `MockRelayer` is dropped.

**Impact**: Cannot test restart/recovery scenarios.

**Workaround**: Implement state serialization if needed for specific tests.

### 5. No Concurrent Transaction Ordering

**Issue**: No block ordering or transaction sequencing.

**Impact**: Race conditions that would occur on-chain may not surface in tests.

**Workaround**: Use property-based testing or explicit sequencing in tests.

## Testing

The mock relayer includes comprehensive unit tests in [tests.rs](tests.rs). Run them with:

```bash
cargo test -p calimero-relayer --lib mock::tests
```

### Test Coverage

- ✅ Context creation and management
- ✅ Member addition/removal
- ✅ Capability grant/revoke
- ✅ Nonce tracking
- ✅ Proposal creation and approval
- ✅ Query operations (pagination, filtering)
- ✅ Error handling (context not found, etc.)

### Example Test Pattern

```rust
#[test]
fn test_add_members() {
    let mut state = MockState::new();
    let context_id = create_test_context_id();

    // Setup: Add context
    state.add_context(context_id, application, author_id);

    // Execute: Add members
    let request = ContextRequestKind::AddMembers { members };
    let result = MockHandlers::handle_operation(&mut state, &operation, &payload);

    // Verify: Check members were added
    assert!(result.is_ok());
    assert!(context.members.contains(&new_member));
    assert_eq!(context.members_revision, 2);
}
```

## CLI Integration

The mock relayer can be enabled via CLI flags:

```bash
# Run relayer in mock mode
mero-relayer --mock

# Or set via environment variable
export ENABLE_MOCK_RELAYER=true
mero-relayer
```

See the main [relayer README](../../README.md) for CLI flag documentation.

## Future Enhancements

Potential improvements to increase feature parity:

1. **Proposal Execution Engine**: Execute `ProposalAction` variants when threshold is met
2. **Threshold Configuration**: Support configurable approval thresholds per context
3. **State Persistence**: Optional file-based or database-backed storage
4. **Time Simulation**: Mock block times and time-dependent operations
5. **Event Emission**: Simulate blockchain events for subscriber testing
6. **Gas Metering**: Optional cost tracking for performance testing
7. **Network Simulation**: Introduce artificial latency/failures

## Contributing

When adding new operations to the relayer:

1. Add the handler in [handlers.rs](handlers.rs)
2. Update the routing in `handle_read()` or `handle_write()`
3. Add state tracking in [state.rs](state.rs) if needed
4. Write unit tests in [tests.rs](tests.rs)
5. Update this README's feature parity table

## Related Documentation

- [Main Relayer README](../../README.md) - Overall relayer architecture
- [Context Config Client](../../../context/config/README.md) - Client library using the relayer
- [Calimero Context Documentation](https://docs.calimero.network) - Conceptual overview
