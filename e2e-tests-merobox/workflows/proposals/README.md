# Proposals Workflows

Comprehensive testing workflows for the Calimero proposals API.

## 📁 Workflows

### `near-proposals.yml`

**Comprehensive NEAR Blockchain Integration Proposals Test**

Tests the complete proposals lifecycle including:

- ✅ Proposal creation (multiple types)
- ✅ Proposals API methods
- ✅ Proposal messages
- ✅ Proposal approval flow
- ✅ Cross-node verification
- ✅ Batch proposal operations with `repeat`

**Steps**: 70+ comprehensive test steps  
**Nodes**: 3 nodes (1 inviter + 2 invitees)  
**Duration**: ~2-3 minutes

## 🚀 Quick Start

### Run NEAR Proposals Test

```bash
# Build binaries and run proposals test
cd /path/to/Calimero/core
./e2e-tests-merobox/run-local.sh --protocol near-proposals --build --build-apps
```

### Run with Auto-build

```bash
# Automatically builds merod, meroctl, and installs merobox in venv
./e2e-tests-merobox/run-local.sh --protocol near-proposals --build
```

### Run Directly with Merobox

```bash
# If you already have binaries built
merobox bootstrap run \
  e2e-tests-merobox/workflows/proposals/near-proposals.yml \
  --no-docker \
  --verbose
```

## 📊 What Gets Tested

### Phase 1: Setup (Steps 1-12)

- Install blockchain integration app from GitHub raw URL
- Create context with 3 nodes using the blockchain app
- Generate identities for nodes 2 & 3
- Invite and join all nodes to the context
- Wait for consensus

### Phase 2: Create Multiple Proposals (Steps 13-21)

Creates 4 different proposals:

1. **SetContextValue** - key: "test_key_1", value: "test_value_1"
2. **SetContextValue** - key: "test_key_2", value: "test_value_2"
3. **SetNumApprovals** - num_approvals: 2
4. **SetContextValue** - key: "test_key_4", value: "test_value_4" (for messages test)

> **Note**: ExternalFunctionCall is not tested in local environment as it requires a NEAR devnet. To test ExternalFunctionCall proposals, you would need to run a NEAR sandbox/devnet with deployed contracts.

### Phase 3: Test list_proposals API (Steps 22-24)

- List all proposals from Node 1
- List all proposals from Node 2
- List all proposals from Node 3
- **Verifies**: All nodes see the same proposals

### Phase 4: Test get_proposal API (Steps 25-28)

- Get details for Proposal 1 from Node 1
- Get details for Proposal 2 from Node 2
- Get details for Proposal 3 from Node 3
- Get details for Proposal 4 (ExternalFunctionCall) from Node 1
- **Verifies**: Proposal details are accessible from all nodes

### Phase 5: Test get_proposal_approvers API (Steps 29-32)

- Get approvers for Proposal 1
- Get approvers for Proposal 2
- Get approvers for Proposal 3
- **Verifies**: Initial approvers list (should include proposal creator)

### Phase 6: Test Proposal Messages (Steps 33-36)

- Send message to Proposal 1 from Node 1
- Wait for broadcast
- Get messages from Node 2
- Get messages from Node 3
- **Verifies**: Message propagation across all nodes

### Phase 7: Approve Proposals (Steps 37-42)

- Approve Proposal 1 from Node 2
- Approve Proposal 2 from Node 3
- Approve Proposal 4 from Node 2
- Approve Proposal 4 from Node 3 (second approval)
- **Verifies**: Multi-node approval flow

### Phase 8: Verify Approvers After Approval (Steps 43-44)

- Check Proposal 1 approvers (should now include Node 2)
- Check Proposal 4 approvers (should include Nodes 2 & 3)
- **Verifies**: Approvers list updates correctly

### Phase 9: Batch Operations with Repeat (Steps 45)

Creates 3 more proposals in a loop:

- For each iteration (0, 1, 2):
  - Create SetContextValue proposal with dynamic key/value
  - Wait for propagation
  - Get proposal details from different node
  - Check proposal approvers
- **Verifies**: Batch proposal creation and querying

### Phase 10: Final Verification (Steps 46-48)

- List all proposals from all 3 nodes
- **Verifies**: Final state consistency across all nodes

## 🎯 Proposal Types Tested

| Proposal Type            | Description                             | Tested in Local           |
| ------------------------ | --------------------------------------- | ------------------------- |
| **SetContextValue**      | Set a key-value pair in context storage | ✅                        |
| **SetNumApprovals**      | Change number of required approvals     | ✅                        |
| **ExternalFunctionCall** | Call function on external NEAR contract | ❌ (requires NEAR devnet) |

## 🔌 API Methods Tested

| Method                   | Description                      | Test Count                             |
| ------------------------ | -------------------------------- | -------------------------------------- |
| `create_new_proposal`    | Create a new proposal            | 7 (4 direct + 3 in repeat)             |
| `list_proposals`         | List all proposals in context    | 6 (3 + 3 final)                        |
| `get_proposal`           | Get details of specific proposal | 7 (4 + 3 in repeat)                    |
| `get_proposal_approvers` | Get list of approvers            | 6 (3 + 1 after approval + 2 in repeat) |
| `send_proposal_messages` | Send message to proposal         | 1                                      |
| `get_proposal_messages`  | Get messages for proposal        | 2                                      |
| `approve_proposal`       | Approve a proposal               | 4                                      |

## 📈 Expected Results

### Success Criteria

- ✅ All 70+ steps complete without errors
- ✅ All nodes can create proposals
- ✅ All nodes can list and query proposals
- ✅ Messages propagate to all nodes
- ✅ Approval flow works correctly
- ✅ Approvers list updates after approvals
- ✅ Repeat operations create proposals dynamically

### Output Location

Results saved to: `e2e-tests-merobox/results/near-proposals/`

### Logs

Node logs available at: `~/.merobox/logs/near-prop-{1,2,3}.log`

## 🔍 Debugging

### Enable Verbose Output

```bash
./e2e-tests-merobox/run-local.sh --protocol near-proposals --verbose
```

### Check Individual Node Logs

```bash
tail -f ~/.merobox/logs/near-prop-1.log
tail -f ~/.merobox/logs/near-prop-2.log
tail -f ~/.merobox/logs/near-prop-3.log
```

### View Test Results

```bash
cat e2e-tests-merobox/results/near-proposals/summary.json | jq
```

## 🐛 Troubleshooting

### Issue: Proposals not propagating

**Solution**: Increase wait times in the workflow YAML

### Issue: Want to test ExternalFunctionCall proposals

**Solution**: ExternalFunctionCall proposals require a running NEAR devnet with deployed contracts. For local testing, we use SetContextValue and SetNumApprovals which don't require external blockchain state. To test ExternalFunctionCall:

1. Set up NEAR sandbox locally
2. Deploy a test contract
3. Update the workflow to use the contract address
4. Ensure nodes can access the NEAR RPC endpoint

### Issue: Message not received on other nodes

**Solution**: Check logs for consensus/broadcast issues. Increase wait times after `send_proposal_messages`.

## 🔗 Related Tests

- **KV Store Tests**: Basic consensus and state propagation
  - `workflows/kv-store/near.yml`
  - `workflows/kv-store/icp.yml`
  - `workflows/kv-store/ethereum.yml`

## 📚 Resources

- **Merobox Documentation**: https://github.com/calimero-network/merobox
- **Calimero Proposals API**: Check the main Calimero docs
- **Original Rust Test**: `e2e-tests/config/protocols/near/demo-blockchain-integrations.json`

## 🎓 Learning Path

1. **Start with KV Store tests** to understand basic merobox workflow
2. **Run this proposals test** to see complete proposals lifecycle
3. **Study the workflow YAML** to understand step structure
4. **Modify wait times** to optimize for your environment
5. **Add more proposal types** as needed for your testing

## 🚧 Future Enhancements

### High Priority

- [ ] **Add ExternalFunctionCall proposal testing** (requires NEAR devnet setup)
  - See: `e2e-tests-merobox/.github-issues/add-external-function-call-proposals-testing.md`
  - Need to set up NEAR sandbox/devnet in CI
  - Deploy test contracts for external state verification
  - Add external state verification steps

### Medium Priority

- [ ] Add ICP proposals workflow
- [ ] Add Ethereum proposals workflow
- [ ] Add proposal rejection testing
- [ ] Add proposal expiration testing

### Low Priority

- [ ] Add concurrent proposal creation testing
- [ ] Add stress testing for large proposal batches
- [ ] Add proposal conflict resolution testing

---

**Status**: ✅ Ready for Testing  
**Last Updated**: October 2025  
**Maintainers**: Calimero Core Team
