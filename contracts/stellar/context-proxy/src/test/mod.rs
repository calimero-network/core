extern crate alloc;
use alloc::vec::Vec as StdVec;

use soroban_sdk::{
  log, testutils::{Address as _, MockAuth, MockAuthInvoke, }, token::{StellarAssetClient, TokenClient}, vec, xdr::ToXdr, Address, Bytes, BytesN, Env, IntoVal, String, Val
};
use soroban_env_common::Env as EnvCommon;
use ed25519_dalek::{Signer, SigningKey};
use calimero_context_config::stellar::{
  stellar_types::{
    StellarSignedRequest,
    StellarSignedRequestPayload
  }, StellarProposal, StellarProposalAction, StellarProposalApprovalWithSigner, StellarProxyMutateRequest
};

use crate::ContextProxyContractClient;

// Import the context contract
mod context_contract {
    soroban_sdk::contractimport!(
        file = "/Users/alen/www/calimero/core/contracts/stellar/context-config/res/calimero_context_config_stellar.wasm"
    );
}


struct ProxyTestContext<'a> {
    env: Env,
    proxy_contract: Address,
    context_contract_address: Address,
    context_id: BytesN<32>,
    mock_token: Address,
    mock_external: Address,
    context_author_sk: SigningKey,
    context_author_id: BytesN<32>,
    test_user: Address,
    xlm_token_admin: Address,
    xlm_token_address: Address,
    token_client: TokenClient<'a>,
    token_asset_client: StellarAssetClient<'a>,
}

fn create_signed_request(
  env: &Env,
  signer_key: &SigningKey,
  payload: StellarSignedRequestPayload,
) -> StellarSignedRequest {
    StellarSignedRequest::new(env, payload, |bytes| {
        Ok(signer_key.sign(bytes))
    }).expect("Failed to create signed request")
}

fn setup<'a>() -> ProxyTestContext<'a> {
    let env = Env::default();

    env.mock_all_auths();
    
    let context_owner = Address::generate(&env);
    let xlm_token_admin = Address::generate(&env);
    let xlm_token = env.register_stellar_asset_contract_v2(xlm_token_admin.clone());

    // Create token client and mint XLM to proxy contract
    let token_client = TokenClient::new(&env, &xlm_token.address());
    let token_asset_client = StellarAssetClient::new(&env, &xlm_token.address());
    token_asset_client.mint(&xlm_token_admin, &1_000_000_000);

    // Read wasm file
    let wasm = include_bytes!("../../../context-config/res/calimero_context_config_stellar.wasm");
    let contract_hash = env.deployer().upload_contract_wasm(*wasm);

    let salt =  BytesN::<32>::from_array(&env, &[0; 32]);
    let context_contract_address = env.deployer()
        .with_address(context_owner.clone(), salt)
        .deploy_v2(contract_hash, (&context_owner.clone(), &xlm_token.address()));
  
    let context_client = context_contract::Client::new(&env, &context_contract_address);

    // Set proxy contract code
    let proxy_wasm = include_bytes!("../../res/calimero_context_proxy_stellar.wasm");
    let proxy_wasm = Bytes::from_slice(&env, proxy_wasm);
    context_client.mock_all_auths().set_proxy_code(&proxy_wasm, &context_owner);

    // Generate context key and author key
    let context_bytes: BytesN<32> = env.as_contract(&context_contract_address, || {
        env.prng().gen()
    });
    let author_bytes: BytesN<32> = env.as_contract(&context_contract_address, || {
        env.prng().gen()
    });
    
    let context_sk = SigningKey::from_bytes(&context_bytes.to_array());
    let context_pk = context_sk.verifying_key();
    let context_id = BytesN::from_array(&env, &context_pk.to_bytes());

    let context_author_sk = SigningKey::from_bytes(&author_bytes.to_array());
    let context_author_pk = context_author_sk.verifying_key();
    let context_author_id = BytesN::from_array(&env, &context_author_pk.to_bytes());

    // Create initial application
    let app_id: BytesN<32> = env.as_contract(&context_contract_address, || {
        env.prng().gen()
    });
    let app_blob: BytesN<32> = env.as_contract(&context_contract_address, || {
        env.prng().gen()
    });

    let application = context_contract::StellarApplication {
        id: app_id,
        blob: app_blob,
        size: 0,
        source: String::from_str(&env, ""),
        metadata: Bytes::from_slice(&env, &[]),
    };

    // Create context request
    let context_request = context_contract::StellarRequest {
        kind: context_contract::StellarRequestKind::Context(
            context_contract::StellarContextRequest {
                context_id: context_id.clone(),
                kind: context_contract::StellarContextRequestKind::Add(
                    context_author_id.clone(),
                    application
                ),
            }
        ),
        signer_id: context_id.clone(),
        nonce: 0,
    };
    let context_req = context_contract::StellarSignedRequestPayload::Context(context_request);

    // Create signed request
    let request_xdr = context_req.clone().to_xdr(&env);
    let message_to_sign: StdVec<u8> = request_xdr.into_iter().collect();
    let signature = context_sk.sign(&message_to_sign);

    let signed_request = context_contract::StellarSignedRequest {
        payload: context_req,
        signature: BytesN::from_array(&env, &signature.to_bytes()),
    };

    context_client.mutate(&signed_request);

    // Get proxy contract address
    let proxy_contract = context_client.proxy_contract(&context_id);

    // Seed XLM to proxy contract
    token_asset_client.mint(&proxy_contract, &1_000_000_000);

    // Generate test addresses for mock contracts
    let test_user = Address::generate(&env);
    let mock_token = Address::generate(&env);
    let mock_external = Address::generate(&env);

    ProxyTestContext {
        env,
        proxy_contract,
        context_contract_address,
        context_id,
        mock_token,
        mock_external,
        context_author_sk,
        context_author_id,
        test_user,
        xlm_token_admin,
        xlm_token_address: xlm_token.address(),
        token_client,
        token_asset_client,
    }
}

#[test]
fn test_create_proposal() {
    let ProxyTestContext {
        env,
        proxy_contract,
        context_author_sk,
        mock_external,
        ..
    } =
     setup();
    
    let author_pk = context_author_sk.verifying_key();
    let author_id = BytesN::from_array(&env, &author_pk.to_bytes());
    
    // Generate proposal ID
    let proposal_id: BytesN<32> = env.as_contract(&proxy_contract, || {
        env.prng().gen()
    });

    let proposal = StellarProposal {
        id: proposal_id.clone(),
        author_id: author_id.clone(),
        actions: vec![&env, StellarProposalAction::Transfer(
            mock_external,
            1_000_000
        )],
    };

    let request = StellarProxyMutateRequest::Propose(proposal);
    let signed_request = create_signed_request(
        &env, 
        &context_author_sk, 
        StellarSignedRequestPayload::Proxy(request.clone())
    );

    let client = ContextProxyContractClient::new(&env, &proxy_contract);

    // Add authorization for the contract call
    env.mock_all_auths();

    let result = client.mutate(&signed_request);

    match result {
        Some(proposal_with_approvals) => {
            // Verify the returned proposal matches what we expect
            assert_eq!(proposal_with_approvals.proposal_id, proposal_id);
            assert_eq!(proposal_with_approvals.num_approvals, 1);
        }
        None => {
            log!(&env, "Operation completed but no proposal was returned");
        }
    }
}

#[test]
fn test_execute_proposal_transfer() {
    let ProxyTestContext {
        env,
        proxy_contract,
        context_author_sk,
        context_contract_address,
        test_user,
        context_id,
        context_author_id,
        token_client,
        ..
    } = setup();

    // Create two more signers
    let signer2_bytes: BytesN<32> = env.as_contract(&proxy_contract, || {
        env.prng().gen()
    });
    let signer3_bytes: BytesN<32> = env.as_contract(&proxy_contract, || {
        env.prng().gen()
    });
    
    let signer2_sk = SigningKey::from_bytes(&signer2_bytes.to_array());
    let signer3_sk = SigningKey::from_bytes(&signer3_bytes.to_array());
    
    let signer2_pk = signer2_sk.verifying_key();
    let signer3_pk = signer3_sk.verifying_key();
    
    let signer2_id = BytesN::from_array(&env, &signer2_pk.to_bytes());
    let signer3_id = BytesN::from_array(&env, &signer3_pk.to_bytes());

    // Add signer2 and signer3 as context members
    let context_client = context_contract::Client::new(&env, &context_contract_address);
    
    let add_members_request = context_contract::StellarRequest {
        signer_id: context_author_id.clone(),
        nonce: 0,
        kind: context_contract::StellarRequestKind::Context(
            context_contract::StellarContextRequest {
                context_id: context_id.clone(),
                kind: context_contract::StellarContextRequestKind::AddMembers(
                    vec![&env, signer2_id.clone(), signer3_id.clone()]
                ),
            }
        ),
    };
    let add_members_req = context_contract::StellarSignedRequestPayload::Context(add_members_request);
    let request_xdr = add_members_req.clone().to_xdr(&env);
    let message_to_sign: StdVec<u8> = request_xdr.into_iter().collect();
    let signature = context_author_sk.sign(&message_to_sign);

    let signed_add_members = context_contract::StellarSignedRequest {
        payload: add_members_req,
        signature: BytesN::from_array(&env, &signature.to_bytes()),
    };

    context_client.mutate(&signed_add_members);

    // Verify initial balance
    let initial_balance = token_client.balance(&proxy_contract);

    // Generate proposal ID
    let proposal_id: BytesN<32> = env.as_contract(&proxy_contract, || {
        env.prng().gen()
    });

    // Create transfer proposal
    let transfer_amount = 100_000;
    let proposal = StellarProposal {
        id: proposal_id.clone(),
        author_id: context_author_id.clone(),
        actions: vec![
            &env,
            StellarProposalAction::Transfer(
                test_user.clone(),
                transfer_amount
            )
        ],
    };

    let client = ContextProxyContractClient::new(&env, &proxy_contract);

    // Submit the initial proposal (first approval)
    let request = StellarProxyMutateRequest::Propose(proposal);
    let signed_request = create_signed_request(
        &env,
        &context_author_sk,
        StellarSignedRequestPayload::Proxy(request)
    );
    
    let result = client.mutate(&signed_request);

    // Verify first approval result
    let proposal_after_first = result.expect("Expected proposal with approvals to be returned");
    assert_eq!(proposal_after_first.proposal_id, proposal_id);
    assert_eq!(proposal_after_first.num_approvals, 1);

    // Check that transfer hasn't happened yet
    assert_eq!(
        token_client.balance(&proxy_contract),
        initial_balance,
        "Balance shouldn't change after first approval"
    );

    // Create and submit second approval
    let second_approval = StellarProposalApprovalWithSigner {
        proposal_id: proposal_id.clone(),
        signer_id: signer2_id.clone(),
    };

    let second_approval_request = StellarProxyMutateRequest::Approve(second_approval);
    let signed_second_approval = create_signed_request(
        &env,
        &signer2_sk,
        StellarSignedRequestPayload::Proxy(second_approval_request)
    );

    let result = client.mutate(&signed_second_approval);

    // Verify second approval result
    let proposal_after_second = result.expect("Expected proposal with approvals to be returned");
    assert_eq!(proposal_after_second.proposal_id, proposal_id);
    assert_eq!(proposal_after_second.num_approvals, 2);

    // Check that transfer still hasn't happened
    assert_eq!(
        token_client.balance(&proxy_contract),
        initial_balance,
        "Balance shouldn't change after second approval"
    );

    // Create and submit third approval
    let third_approval = StellarProposalApprovalWithSigner {
        proposal_id: proposal_id.clone(),
        signer_id: signer3_id.clone(),
    };

    let third_approval_request = StellarProxyMutateRequest::Approve(third_approval);
    let signed_third_approval = create_signed_request(
        &env,
        &signer3_sk,
        StellarSignedRequestPayload::Proxy(third_approval_request)
    );

    // Execute the proposal with third approval
    let result = client.mutate(&signed_third_approval);
    assert!(result.is_none(), "Expected None after proposal execution");

    // Verify the transfer was executed
    let final_proxy_balance = token_client.balance(&proxy_contract);
    let final_recipient_balance = token_client.balance(&test_user);

    log!(&env, "Final proxy balance: {}", final_proxy_balance);
    log!(&env, "Final recipient balance: {}", final_recipient_balance);
    assert_eq!(
        final_proxy_balance,
        initial_balance - transfer_amount,
        "Proxy contract balance should decrease after execution"
    );
    assert_eq!(
        final_recipient_balance,
        transfer_amount,
        "Recipient should receive the transfer amount after execution"
    );
}

#[test]
fn test_execute_proposal_set_num_approvals() {
    let ProxyTestContext {
        env,
        proxy_contract,
        context_contract_address,
        context_author_sk,
        context_author_id,
        context_id,
        ..
    } = setup();

    // Create two more signers
    let signer2_bytes: BytesN<32> = env.as_contract(&proxy_contract, || {
        env.prng().gen()
    });
    let signer3_bytes: BytesN<32> = env.as_contract(&proxy_contract, || {
        env.prng().gen()
    });
    
    let signer2_sk = SigningKey::from_bytes(&signer2_bytes.to_array());
    let signer3_sk = SigningKey::from_bytes(&signer3_bytes.to_array());
    
    let signer2_pk = signer2_sk.verifying_key();
    let signer3_pk = signer3_sk.verifying_key();
    
    let signer2_id = BytesN::from_array(&env, &signer2_pk.to_bytes());
    let signer3_id = BytesN::from_array(&env, &signer3_pk.to_bytes());

    // Add signer2 and signer3 as context members
    let context_client = context_contract::Client::new(&env, &context_contract_address);
    
    let add_members_request = context_contract::StellarRequest {
        signer_id: context_author_id.clone(),
        nonce: 0,
        kind: context_contract::StellarRequestKind::Context(
            context_contract::StellarContextRequest {
                context_id: context_id.clone(),
                kind: context_contract::StellarContextRequestKind::AddMembers(
                    vec![&env, signer2_id.clone(), signer3_id.clone()]
                ),
            }
        ),
    };

    let signed_add_members = create_signed_request(
        &env,
        &context_author_sk,
        context_contract::StellarSignedRequestPayload::Context(add_members_request)
    );

    env.mock_all_auths();
    context_client.mutate(&signed_add_members);

    // Generate proposal ID
    let proposal_id: BytesN<32> = env.as_contract(&proxy_contract, || {
        env.prng().gen()
    });

    // Create proposal to change num_approvals to 2
    let new_num_approvals = 2;
    let proposal = StellarProposal {
        id: proposal_id.clone(),
        author_id: context_author_id.clone(),
        actions: vec![
            &env,
            StellarProposalAction::SetNumApprovals(new_num_approvals)
        ],
    };

    let client = ContextProxyContractClient::new(&env, &proxy_contract);

    // Submit the initial proposal
    let request = StellarProxyMutateRequest::Propose(proposal);
    let signed_request = create_signed_request(
        &env,
        &context_author_sk,
        StellarSignedRequestPayload::Proxy(request)
    );
    
    let result = client.mutate(&signed_request);
    let proposal_after_first = result.expect("Expected proposal with approvals to be returned");
    assert_eq!(proposal_after_first.num_approvals, 1);

    // Create and submit second approval
    let second_approval = StellarProposalApprovalWithSigner {
        proposal_id: proposal_id.clone(),
        signer_id: signer2_id.clone(),
    };

    let second_approval_request = StellarProxyMutateRequest::Approve(second_approval);
    let signed_second_approval = create_signed_request(
        &env,
        &signer2_sk,
        StellarSignedRequestPayload::Proxy(second_approval_request)
    );

    let result = client.mutate(&signed_second_approval);
    let proposal_after_second = result.expect("Expected proposal with approvals to be returned");
    assert_eq!(proposal_after_second.num_approvals, 2);

    // Create and submit third approval
    let third_approval = StellarProposalApprovalWithSigner {
        proposal_id: proposal_id.clone(),
        signer_id: signer3_id.clone(),
    };

    let third_approval_request = StellarProxyMutateRequest::Approve(third_approval);
    let signed_third_approval = create_signed_request(
        &env,
        &signer3_sk,
        StellarSignedRequestPayload::Proxy(third_approval_request)
    );

    let result = client.mutate(&signed_third_approval);
    assert!(result.is_none(), "Expected None after proposal execution");

    // Create a new proposal to verify the num_approvals was changed
    let verify_proposal_id: BytesN<32> = env.as_contract(&proxy_contract, || {
        env.prng().gen()
    });

    let verify_proposal = StellarProposal {
        id: verify_proposal_id.clone(),
        author_id: context_author_id.clone(),
        actions: vec![&env, StellarProposalAction::SetContextValue(
            String::from_str(&env, "test"),
            String::from_str(&env, "value")
        )],
    };

    let verify_request = StellarProxyMutateRequest::Propose(verify_proposal);
    let signed_verify_request = create_signed_request(
        &env,
        &context_author_sk,
        StellarSignedRequestPayload::Proxy(verify_request)
    );
    
    let result = client.mutate(&signed_verify_request);
    let verify_proposal_result = result.expect("Expected proposal with approvals to be returned");
    
    // Verify that the new proposal requires only 2 approvals
    assert_eq!(verify_proposal_result.num_approvals, 1);
    assert_eq!(verify_proposal_result.required_approvals, new_num_approvals);
}