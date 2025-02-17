extern crate alloc;
extern crate std;

use alloc::vec::Vec as StdVec;
use std::fs;

use calimero_context_config::stellar::{
    StellarProposal, StellarProposalAction, StellarProposalApprovalWithSigner,
    StellarProposalWithApprovals, StellarProxyError, StellarProxyMutateRequest,
};
// Local imports
use calimero_context_proxy_stellar::ContextProxyContractClient;
// Cryptographic imports
use ed25519_dalek::{Signer, SigningKey};
// Soroban SDK imports
use soroban_sdk::testutils::Address as _;
use soroban_sdk::token::{StellarAssetClient, TokenClient};
use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{log, vec, Address, Bytes, BytesN, Env, IntoVal, String, Symbol, Val, Vec};

// Import the context contract
mod context_contract {
    soroban_sdk::contractimport!(
        file = "../context-config/res/calimero_context_config_stellar.wasm"
    );
}

// Import mock external contract
mod mock_external {
    soroban_sdk::contractimport!(
        file = "../context-proxy/mock_external/res/calimero_mock_external_stellar.wasm"
    );
}

// For proxy contract operations (like creating proposals), use calimero_context_config types:
use calimero_context_config::stellar::stellar_types::{
    StellarSignedRequest as ProxySignedRequest,
    StellarSignedRequestPayload as ProxySignedRequestPayload,
};
// For context contract operations (like adding members), use context_contract types:
// These come from the contractimport! macro
use context_contract::{
    StellarSignedRequest as ContextSignedRequest,
    StellarSignedRequestPayload as ContextSignedRequestPayload,
};

/// Test context structure holding all necessary components for proxy contract tests
struct ProxyTestContext<'a> {
    env: Env,
    proxy_contract: Address,
    context_author_sk: SigningKey,
    context_author_id: BytesN<32>,
    signer2_sk: SigningKey,
    signer2_id: BytesN<32>,
    signer3_sk: SigningKey,
    signer3_id: BytesN<32>,
    token_client: TokenClient<'a>,
    xlm_token_address: Address,
}

/// Creates a signed request for proxy contract operations
/// # Arguments
/// * `env` - The Soroban environment
/// * `signer_key` - The signing key to use
/// * `payload` - The request payload to sign
fn create_signed_request(
    env: &Env,
    signer_key: &SigningKey,
    payload: ProxySignedRequestPayload,
) -> ProxySignedRequest {
    ProxySignedRequest::new(env, payload, |bytes| Ok(signer_key.sign(bytes)))
        .expect("Failed to create signed request")
}

/// Creates a signed request for context contract operations
/// # Arguments
/// * `env` - The Soroban environment
/// * `signer_key` - The signing key to use
/// * `payload` - The context request payload to sign
fn create_context_signed_request(
    env: &Env,
    signer_key: &SigningKey,
    payload: ContextSignedRequestPayload,
) -> ContextSignedRequest {
    let request_xdr = payload.clone().to_xdr(env);
    let message_to_sign: StdVec<u8> = request_xdr.into_iter().collect();
    let signature = signer_key.sign(&message_to_sign);

    ContextSignedRequest {
        payload,
        signature: BytesN::from_array(env, &signature.to_bytes()),
    }
}

/// Adds members to a context using the context contract
/// # Arguments
/// * `env` - The Soroban environment
/// * `context_client` - The context contract client
/// * `context_id` - The ID of the context
/// * `context_author_id` - The ID of the context author
/// * `context_author_sk` - The signing key of the context author
/// * `members` - The list of member IDs to add
fn add_members(
    env: &Env,
    context_client: &context_contract::Client,
    context_id: &BytesN<32>,
    context_author_id: &BytesN<32>,
    context_author_sk: &SigningKey,
    members: Vec<BytesN<32>>,
) {
    let add_members_request = context_contract::StellarRequest {
        signer_id: context_author_id.clone(),
        nonce: 0,
        kind: context_contract::StellarRequestKind::Context(
            context_contract::StellarContextRequest {
                context_id: context_id.clone(),
                kind: context_contract::StellarContextRequestKind::AddMembers(members),
            },
        ),
    };
    let add_members_req = ContextSignedRequestPayload::Context(add_members_request);

    // Create signed request
    let request_xdr = add_members_req.clone().to_xdr(env);
    let message_to_sign: StdVec<u8> = request_xdr.into_iter().collect();
    let signature = context_author_sk.sign(&message_to_sign);

    let signed_add_members = ContextSignedRequest {
        payload: add_members_req,
        signature: BytesN::from_array(env, &signature.to_bytes()),
    };

    context_client.mutate(&signed_add_members);
}

/// Creates a test proposal with given parameters
/// # Arguments
/// * `env` - The Soroban environment
/// * `contract_address` - The address of the proxy contract
/// * `author_id` - The ID of the proposal author
/// * `actions` - The list of actions for the proposal
fn create_test_proposal(
    env: &Env,
    contract_address: &Address,
    author_id: &BytesN<32>,
    actions: Vec<StellarProposalAction>,
) -> (BytesN<32>, StellarProposal) {
    let proposal_id: BytesN<32> = env.as_contract(contract_address, || env.prng().gen());

    let proposal = StellarProposal {
        id: proposal_id.clone(),
        author_id: author_id.clone(),
        actions,
    };

    (proposal_id, proposal)
}

/// Deploys a mock external contract for testing
/// # Arguments
/// * `env` - The Soroban environment
/// * `xlm_token_address` - The address of the XLM token contract
fn deploy_mock_external<'a>(
    env: &'a Env,
    xlm_token_address: &Address,
) -> (Address, mock_external::Client<'a>) {
    let mock_owner = Address::generate(env);
    let mock_external_wasm = fs::read("./mock_external/res/calimero_mock_external_stellar.wasm")
        .expect("Failed to read mock external WASM file");
    let mock_external_hash = env
        .deployer()
        .upload_contract_wasm(Bytes::from_slice(&env, &mock_external_wasm));

    let salt = BytesN::<32>::from_array(env, &[0; 32]);
    let mock_external_address = env
        .deployer()
        .with_address(mock_owner, salt)
        .deploy_v2(mock_external_hash, (xlm_token_address,));

    let mock_external_client = mock_external::Client::new(env, &mock_external_address);

    (mock_external_address, mock_external_client)
}

/// Sets up the test environment with all necessary contracts and accounts
/// # Returns
/// Returns a ProxyTestContext containing all components needed for testing
fn setup<'a>() -> ProxyTestContext<'a> {
    let env = Env::default();
    env.mock_all_auths();

    // Setup token contract
    let xlm_token_admin = Address::generate(&env);
    let xlm_token = env.register_stellar_asset_contract_v2(xlm_token_admin.clone());
    let token_client = TokenClient::new(&env, &xlm_token.address());
    let token_asset_client = StellarAssetClient::new(&env, &xlm_token.address());
    token_asset_client.mint(&xlm_token_admin, &1_000_000_000);

    // Setup context contract - now using fs::read
    let context_owner = Address::generate(&env);
    let wasm = fs::read("../context-config/res/calimero_context_config_stellar.wasm")
        .expect("Failed to read context config WASM file");
    let contract_hash = env
        .deployer()
        .upload_contract_wasm(Bytes::from_slice(&env, &wasm));

    let salt = BytesN::<32>::from_array(&env, &[0; 32]);
    let context_contract_address = env
        .deployer()
        .with_address(context_owner.clone(), salt)
        .deploy_v2(contract_hash, (&context_owner, &xlm_token.address()));

    let context_client = context_contract::Client::new(&env, &context_contract_address);

    // Set proxy contract code
    let proxy_wasm = fs::read("./res/calimero_context_proxy_stellar.wasm")
        .expect("Failed to read proxy WASM file");
    context_client
        .mock_all_auths()
        .set_proxy_code(&Bytes::from_slice(&env, &proxy_wasm), &context_owner);

    // Generate context and author keys
    let context_bytes: BytesN<32> = env.as_contract(&context_contract_address, || env.prng().gen());
    let author_bytes: BytesN<32> = env.as_contract(&context_contract_address, || env.prng().gen());

    let context_sk = SigningKey::from_bytes(&context_bytes.to_array());
    let context_pk = context_sk.verifying_key();
    let context_id = BytesN::from_array(&env, &context_pk.to_bytes());

    let context_author_sk = SigningKey::from_bytes(&author_bytes.to_array());
    let context_author_pk = context_author_sk.verifying_key();
    let context_author_id = BytesN::from_array(&env, &context_author_pk.to_bytes());

    // Generate additional signers
    let signer2_bytes: BytesN<32> = env.as_contract(&context_contract_address, || env.prng().gen());
    let signer3_bytes: BytesN<32> = env.as_contract(&context_contract_address, || env.prng().gen());

    let signer2_sk = SigningKey::from_bytes(&signer2_bytes.to_array());
    let signer3_sk = SigningKey::from_bytes(&signer3_bytes.to_array());

    let signer2_pk = signer2_sk.verifying_key();
    let signer3_pk = signer3_sk.verifying_key();

    let signer2_id = BytesN::from_array(&env, &signer2_pk.to_bytes());
    let signer3_id = BytesN::from_array(&env, &signer3_pk.to_bytes());

    // Create initial application and context
    let app_id: BytesN<32> = env.as_contract(&context_contract_address, || env.prng().gen());
    let app_blob: BytesN<32> = env.as_contract(&context_contract_address, || env.prng().gen());

    let application = context_contract::StellarApplication {
        id: app_id,
        blob: app_blob,
        size: 0,
        source: String::from_str(&env, ""),
        metadata: Bytes::from_slice(&env, &[]),
    };

    let context_request = context_contract::StellarRequest {
        kind: context_contract::StellarRequestKind::Context(
            context_contract::StellarContextRequest {
                context_id: context_id.clone(),
                kind: context_contract::StellarContextRequestKind::Add(
                    context_author_id.clone(),
                    application,
                ),
            },
        ),
        signer_id: context_id.clone(),
        nonce: 0,
    };

    let context_req = ContextSignedRequestPayload::Context(context_request);
    let signed_request = create_context_signed_request(&env, &context_sk, context_req);
    context_client.mutate(&signed_request);

    // Get and deploy proxy contract
    let proxy_contract = context_client.proxy_contract(&context_id);
    // Fund the proxy contract
    token_asset_client.mint(&proxy_contract, &1_000_000_000);

    // Add signers as members
    add_members(
        &env,
        &context_client,
        &context_id,
        &context_author_id,
        &context_author_sk,
        vec![&env, signer2_id.clone(), signer3_id.clone()],
    );

    ProxyTestContext {
        env,
        proxy_contract,
        context_author_sk,
        context_author_id,
        signer2_sk,
        signer2_id,
        signer3_sk,
        signer3_id,
        token_client,
        xlm_token_address: xlm_token.address(),
    }
}

/// Helper function to submit and verify proposal approvals
/// # Arguments
/// * `env` - The Soroban environment
/// * `client` - The proxy contract client
/// * `proposal_id` - The ID of the proposal to approve
/// * `signer_id` - The ID of the approving signer
/// * `signer_sk` - The signing key of the approver
/// * `expected_approvals` - Expected number of approvals after submission
/// # Returns
/// Returns the proposal with approvals if not executed, None if executed
fn submit_approval(
    env: &Env,
    client: &ContextProxyContractClient,
    proposal_id: &BytesN<32>,
    signer_id: &BytesN<32>,
    signer_sk: &SigningKey,
    expected_approvals: u32,
) -> Option<StellarProposalWithApprovals> {
    let approval = StellarProposalApprovalWithSigner {
        proposal_id: proposal_id.clone(),
        signer_id: signer_id.clone(),
    };

    let request = StellarProxyMutateRequest::Approve(approval);
    let signed_request =
        create_signed_request(env, signer_sk, ProxySignedRequestPayload::Proxy(request));

    let result = client
        .mock_all_auths_allowing_non_root_auth()
        .mutate(&signed_request);

    if expected_approvals == 0 {
        assert!(result.is_none(), "Expected proposal to be executed");
        None
    } else {
        let proposal = result.expect("Expected proposal with approvals to be returned");
        assert_eq!(proposal.proposal_id, *proposal_id);
        assert_eq!(proposal.num_approvals, expected_approvals);
        Some(proposal)
    }
}

/// Tests proposal execution for token transfer functionality
///
/// This test verifies:
/// - Proposal creation for token transfer
/// - Multi-signature approval process
/// - Token transfer execution
/// - Balance updates for both sender and receiver
#[test]
fn test_execute_proposal_transfer() {
    let ProxyTestContext {
        env,
        proxy_contract,
        context_author_sk,
        context_author_id,
        signer2_sk,
        signer2_id,
        signer3_sk,
        signer3_id,
        token_client,
        ..
    } = setup();

    // Verify initial balance
    let initial_balance = token_client.balance(&proxy_contract);
    let test_user = Address::generate(&env);

    // Create and submit transfer proposal
    let transfer_amount = 100_000;
    let (proposal_id, proposal) = create_test_proposal(
        &env,
        &proxy_contract,
        &context_author_id,
        vec![
            &env,
            StellarProposalAction::Transfer(test_user.clone(), transfer_amount),
        ],
    );

    let client = ContextProxyContractClient::new(&env, &proxy_contract);
    let request = StellarProxyMutateRequest::Propose(proposal);
    let signed_request = create_signed_request(
        &env,
        &context_author_sk,
        ProxySignedRequestPayload::Proxy(request),
    );

    // Verify proposal creation
    let result = client.mutate(&signed_request);
    let proposal_after_first = result.expect("Expected proposal with approvals to be returned");
    assert_eq!(proposal_after_first.proposal_id, proposal_id);
    assert_eq!(proposal_after_first.num_approvals, 1);
    assert_eq!(
        token_client.balance(&proxy_contract),
        initial_balance,
        "Balance shouldn't change after first approval"
    );

    // Submit second approval
    submit_approval(&env, &client, &proposal_id, &signer2_id, &signer2_sk, 2);

    // Submit final approval which executes the proposal
    submit_approval(&env, &client, &proposal_id, &signer3_id, &signer3_sk, 0);

    // Verify the transfer was executed
    let final_proxy_balance = token_client.balance(&proxy_contract);
    let final_recipient_balance = token_client.balance(&test_user);

    assert_eq!(
        final_proxy_balance,
        initial_balance - transfer_amount,
        "Proxy contract balance should decrease after execution"
    );
    assert_eq!(
        final_recipient_balance, transfer_amount,
        "Recipient should receive the transfer amount after execution"
    );
}

/// Tests proposal execution for changing the required number of approvals
///
/// This test verifies:
/// - Proposal creation for changing num_approvals
/// - Execution of approval change
/// - Verification of new approval requirement with a subsequent proposal
/// - Storage updates with new approval requirement
///
/// Test flow:
/// 1. Create and execute proposal to change num_approvals to 2
/// 2. Verify change by creating a new proposal
/// 3. Confirm new proposal executes with only 2 approvals
/// 4. Verify storage updates
#[test]
fn test_execute_proposal_set_num_approvals() {
    let ProxyTestContext {
        env,
        proxy_contract,
        context_author_sk,
        context_author_id,
        signer2_sk,
        signer2_id,
        signer3_sk,
        signer3_id,
        ..
    } = setup();

    let client = ContextProxyContractClient::new(&env, &proxy_contract);

    // Create proposal to change num_approvals to 2
    let (proposal_id, proposal) = create_test_proposal(
        &env,
        &proxy_contract,
        &context_author_id,
        vec![&env, StellarProposalAction::SetNumApprovals(2)],
    );

    // Submit initial proposal
    let request = StellarProxyMutateRequest::Propose(proposal);
    let signed_request = create_signed_request(
        &env,
        &context_author_sk,
        ProxySignedRequestPayload::Proxy(request),
    );

    let result = client.mutate(&signed_request);
    let proposal_after_first = result.expect("Expected proposal with approvals to be returned");
    assert_eq!(proposal_after_first.num_approvals, 1);

    // Submit second approval
    submit_approval(&env, &client, &proposal_id, &signer2_id, &signer2_sk, 2);

    // Submit final approval which executes the proposal
    submit_approval(&env, &client, &proposal_id, &signer3_id, &signer3_sk, 0);

    // Verify the change by creating a new proposal
    let (verify_proposal_id, verify_proposal) = create_test_proposal(
        &env,
        &proxy_contract,
        &context_author_id,
        vec![
            &env,
            StellarProposalAction::SetContextValue(
                Bytes::from_slice(&env, "test".as_bytes()),
                Bytes::from_slice(&env, "value".as_bytes()),
            ),
        ],
    );

    let verify_request = StellarProxyMutateRequest::Propose(verify_proposal);
    let signed_verify_request = create_signed_request(
        &env,
        &context_author_sk,
        ProxySignedRequestPayload::Proxy(verify_request),
    );

    let result = client.mutate(&signed_verify_request);
    let verify_proposal_result = result.expect("Expected proposal with approvals to be returned");
    assert_eq!(verify_proposal_result.num_approvals, 1);

    // Second approval should now execute the proposal since num_approvals is 2
    let result = submit_approval(
        &env,
        &client,
        &verify_proposal_id,
        &signer2_id,
        &signer2_sk,
        0,
    );
    assert!(result.is_none(), "Expected None after proposal execution");

    // Verify the context value was set
    let test_key = Bytes::from_slice(&env, "test".as_bytes());
    let test_value = Bytes::from_slice(&env, "value".as_bytes());
    let stored_value = client.get_context_value(&test_key);
    assert_eq!(
        stored_value,
        Some(test_value.clone()),
        "Context value was not set correctly"
    );

    // Verify the context value was set using context_storage_entries
    let storage_entries = client.context_storage_entries(&0, &10);
    assert_eq!(storage_entries.len(), 1, "Expected one storage entry");
    assert_eq!(
        storage_entries.get(0),
        Some((test_key, test_value)),
        "Context value was not set correctly"
    );
}

/// Tests proposal execution for changing the active proposals limit
///
/// This test verifies:
/// - Proposal creation for changing active proposals limit
/// - Multi-signature approval process
/// - Successful update of the limit
/// - Verification of new limit value
#[test]
fn test_execute_proposal_set_active_proposals_limit() {
    let ProxyTestContext {
        env,
        proxy_contract,
        context_author_sk,
        context_author_id,
        signer2_sk,
        signer2_id,
        signer3_sk,
        signer3_id,
        ..
    } = setup();

    let client = ContextProxyContractClient::new(&env, &proxy_contract);
    let new_limit = 5;

    // Create and submit proposal
    let (proposal_id, proposal) = create_test_proposal(
        &env,
        &proxy_contract,
        &context_author_id,
        vec![
            &env,
            StellarProposalAction::SetActiveProposalsLimit(new_limit),
        ],
    );

    let request = StellarProxyMutateRequest::Propose(proposal);
    let signed_request = create_signed_request(
        &env,
        &context_author_sk,
        ProxySignedRequestPayload::Proxy(request),
    );

    // Submit initial proposal
    let result = client.mutate(&signed_request);
    let proposal_after_first = result.expect("Expected proposal with approvals to be returned");
    assert_eq!(proposal_after_first.num_approvals, 1);
    assert_eq!(proposal_after_first.proposal_id, proposal_id);

    // Submit second approval
    submit_approval(&env, &client, &proposal_id, &signer2_id, &signer2_sk, 2);

    // Submit final approval which executes the proposal
    submit_approval(&env, &client, &proposal_id, &signer3_id, &signer3_sk, 0);

    // Verify the limit was updated
    let updated_limit = client.get_active_proposals_limit();
    assert_eq!(
        updated_limit, new_limit,
        "Active proposals limit not updated correctly"
    );
}

/// Tests the execution of a proposal with an external contract call with deposit
///
/// This test verifies:
/// - Proposal creation and approval process
/// - External contract interaction with deposit
/// - State updates in external contract
#[test]
fn test_execute_proposal_external_call_deposit() {
    let ProxyTestContext {
        env,
        proxy_contract,
        context_author_sk,
        context_author_id,
        signer2_sk,
        signer2_id,
        signer3_sk,
        signer3_id,
        token_client,
        xlm_token_address,
        ..
    } = setup();

    let (mock_external_address, mock_external_client) =
        deploy_mock_external(&env, &xlm_token_address);

    // Create external call proposal with deposit
    let method_name = Symbol::new(&env, "deposit");
    let key = String::from_str(&env, "test_key");
    let value = String::from_str(&env, "test_value");
    let deposit: i128 = 1_000;

    // Create args for external call
    let args: Vec<Val> = vec![
        &env,
        proxy_contract.to_val(),
        deposit.into_val(&env),
        key.to_val(),
        value.to_val(),
    ];

    let (proposal_id, proposal) = create_test_proposal(
        &env,
        &proxy_contract,
        &context_author_id,
        vec![
            &env,
            StellarProposalAction::ExternalFunctionCall(
                mock_external_address.clone(),
                method_name,
                args,
                deposit,
            ),
        ],
    );

    let client = ContextProxyContractClient::new(&env, &proxy_contract);
    env.mock_all_auths();

    let initial_proxy_balance = token_client.balance(&proxy_contract);
    let initial_external_balance = token_client.balance(&mock_external_address);

    // Submit initial proposal
    let request = StellarProxyMutateRequest::Propose(proposal);
    let signed_request = create_signed_request(
        &env,
        &context_author_sk,
        ProxySignedRequestPayload::Proxy(request),
    );

    let result = client.mock_all_auths().mutate(&signed_request);
    let proposal_after_first = result.expect("Expected proposal with approvals to be returned");
    assert_eq!(proposal_after_first.num_approvals, 1);

    // Submit second approval
    submit_approval(&env, &client, &proposal_id, &signer2_id, &signer2_sk, 2);
    // First, enable non-root auth mocking
    env.mock_all_auths_allowing_non_root_auth();

    // Submit final approval which executes the proposal
    submit_approval(&env, &client, &proposal_id, &signer3_id, &signer3_sk, 0);

    // Verify execution
    let stored_value = mock_external_client.get_value(&key);
    assert_eq!(stored_value, Some(value), "Value not stored correctly");

    let final_proxy_balance = token_client.balance(&proxy_contract);
    let final_external_balance = token_client.balance(&mock_external_address);

    assert_eq!(
        final_proxy_balance,
        initial_proxy_balance - deposit,
        "Proxy balance not decreased correctly"
    );
    assert_eq!(
        final_external_balance,
        initial_external_balance + deposit,
        "External contract balance not increased correctly"
    );

    let final_state = mock_external_client.get_state();
    assert_eq!(
        final_state.total_deposits, deposit,
        "Total deposits not updated correctly"
    );
}

/// Tests proposal execution for an external contract call without deposit
///
/// This test verifies:
/// - Proposal creation and approval process
/// - External contract interaction without token transfer
/// - State updates in external contract
#[test]
fn test_execute_proposal_external_call_no_deposit() {
    let ProxyTestContext {
        env,
        proxy_contract,
        context_author_sk,
        context_author_id,
        signer2_sk,
        signer2_id,
        signer3_sk,
        signer3_id,
        xlm_token_address,
        ..
    } = setup();

    let (mock_external_address, mock_external_client) =
        deploy_mock_external(&env, &xlm_token_address);

    // Create external call proposal without deposit
    let method_name = Symbol::new(&env, "no_deposit");
    let key = String::from_str(&env, "test_key");
    let value = String::from_str(&env, "test_value");

    let args: Vec<Val> = vec![&env, key.to_val(), value.to_val()];

    let (proposal_id, proposal) = create_test_proposal(
        &env,
        &proxy_contract,
        &context_author_id,
        vec![
            &env,
            StellarProposalAction::ExternalFunctionCall(
                mock_external_address.clone(),
                method_name,
                args,
                0,
            ),
        ],
    );

    let client = ContextProxyContractClient::new(&env, &proxy_contract);
    env.mock_all_auths();

    // Submit initial proposal
    let request = StellarProxyMutateRequest::Propose(proposal);
    let signed_request = create_signed_request(
        &env,
        &context_author_sk,
        ProxySignedRequestPayload::Proxy(request),
    );

    let result = client.mutate(&signed_request);
    let proposal_after_first = result.expect("Expected proposal with approvals to be returned");
    assert_eq!(proposal_after_first.num_approvals, 1);

    // Submit second approval
    submit_approval(&env, &client, &proposal_id, &signer2_id, &signer2_sk, 2);

    // Submit final approval which executes the proposal
    submit_approval(&env, &client, &proposal_id, &signer3_id, &signer3_sk, 0);

    // Verify execution
    let stored_value = mock_external_client.get_value(&key);
    assert_eq!(stored_value, Some(value), "Value not stored correctly");
}

/// Tests proposal limits and deletion functionality
///
/// This test verifies:
/// - Setting proposal limits
/// - Enforcing maximum proposal count per author
/// - Proposal deletion
/// - Creating new proposals after deletion
///
/// Test flow:
/// 1. Set proposal limit to 2
/// 2. Create two proposals successfully
/// 3. Verify third proposal fails
/// 4. Delete a proposal
/// 5. Verify new proposal can be created
#[test]
fn test_proposal_limits_and_deletion() {
    let ProxyTestContext {
        env,
        proxy_contract,
        context_author_sk,
        context_author_id,
        signer2_sk,
        signer2_id,
        signer3_sk,
        signer3_id,
        ..
    } = setup();

    let client = ContextProxyContractClient::new(&env, &proxy_contract);
    let new_limit = 2;

    // First set the active proposals limit to 2
    let (limit_proposal_id, limit_proposal) = create_test_proposal(
        &env,
        &proxy_contract,
        &context_author_id,
        vec![
            &env,
            StellarProposalAction::SetActiveProposalsLimit(new_limit),
        ],
    );

    // Submit and approve limit change proposal
    let request = StellarProxyMutateRequest::Propose(limit_proposal);
    let signed_request = create_signed_request(
        &env,
        &context_author_sk,
        ProxySignedRequestPayload::Proxy(request),
    );
    client.mutate(&signed_request);
    submit_approval(
        &env,
        &client,
        &limit_proposal_id,
        &signer2_id,
        &signer2_sk,
        2,
    );
    submit_approval(
        &env,
        &client,
        &limit_proposal_id,
        &signer3_id,
        &signer3_sk,
        0,
    );

    // Verify limit was set
    assert_eq!(client.get_active_proposals_limit(), new_limit);
    // Create first proposal
    let (proposal1_id, proposal1) = create_test_proposal(
        &env,
        &proxy_contract,
        &context_author_id,
        vec![
            &env,
            StellarProposalAction::SetContextValue(
                Bytes::from_slice(&env, "key".as_bytes()),
                Bytes::from_slice(&env, "value".as_bytes()),
            ),
        ],
    );
    let request = StellarProxyMutateRequest::Propose(proposal1);
    let signed_request = create_signed_request(
        &env,
        &context_author_sk,
        ProxySignedRequestPayload::Proxy(request),
    );
    client.mutate(&signed_request);

    // Create second proposal
    let (_, proposal2) = create_test_proposal(
        &env,
        &proxy_contract,
        &context_author_id,
        vec![
            &env,
            StellarProposalAction::SetContextValue(
                Bytes::from_slice(&env, "key".as_bytes()),
                Bytes::from_slice(&env, "value".as_bytes()),
            ),
        ],
    );
    let request = StellarProxyMutateRequest::Propose(proposal2);
    let signed_request = create_signed_request(
        &env,
        &context_author_sk,
        ProxySignedRequestPayload::Proxy(request),
    );
    client.mutate(&signed_request);

    // Try to create third proposal - should fail with HostError
    let (_, proposal3) = create_test_proposal(
        &env,
        &proxy_contract,
        &context_author_id,
        vec![&env, StellarProposalAction::SetActiveProposalsLimit(3)],
    );
    let request = StellarProxyMutateRequest::Propose(proposal3.clone());
    let signed_request = create_signed_request(
        &env,
        &context_author_sk,
        ProxySignedRequestPayload::Proxy(request),
    );
    let result = client.try_mutate(&signed_request);
    match result {
        Ok(_) => panic!("Expected error for exceeding proposal limit"),
        Err(e) => match e {
            Ok(StellarProxyError::TooManyActiveProposals) => {
                // This is what we expect
                log!(&env, "Got expected TooManyActiveProposals error");
            }
            other => panic!("Got unexpected error: {:?}", other),
        },
    }

    // Delete first proposal to free up space
    let (_, delete_proposal) = create_test_proposal(
        &env,
        &proxy_contract,
        &context_author_id,
        vec![&env, StellarProposalAction::DeleteProposal(proposal1_id)],
    );

    let request = StellarProxyMutateRequest::Propose(delete_proposal);
    let signed_request = create_signed_request(
        &env,
        &context_author_sk,
        ProxySignedRequestPayload::Proxy(request),
    );
    client.mutate(&signed_request);

    let current_proposals = client.proposals(&0, &10);
    assert_eq!(
        current_proposals.len(),
        1,
        "Should have exactly 1 active proposal"
    );

    // Now we should be able to create another proposal
    let request = StellarProxyMutateRequest::Propose(proposal3);
    let signed_request = create_signed_request(
        &env,
        &context_author_sk,
        ProxySignedRequestPayload::Proxy(request),
    );
    let result = client.mutate(&signed_request);
    assert!(
        result.is_some(),
        "Should allow new proposal after deleting previous one"
    );

    // Verify proposal counts
    let active_proposals = client.proposals(&0, &10);
    assert_eq!(
        active_proposals.len(),
        2,
        "Should have exactly 2 active proposals"
    );
}
