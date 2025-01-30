use calimero_context_config::stellar::{StellarProposal, StellarProposalAction, StellarProposalApprovalWithSigner, StellarProxyMutateRequest};
use ed25519_dalek::{Signer, SigningKey};
use soroban_sdk::{log, Symbol};
use soroban_sdk::token::TokenClient;
use soroban_sdk::{token::StellarAssetClient, vec, Address, BytesN, Env, String, Val, Vec, IntoVal};
use soroban_sdk::testutils::Address as _;
use crate::{ContextProxyContract, ContextProxyContractClient};

// For proxy contract operations (like creating proposals), use calimero_context_config types:
use calimero_context_config::stellar::stellar_types::{
  StellarSignedRequest as ProxySignedRequest,
  StellarSignedRequestPayload as ProxySignedRequestPayload,
};

fn create_signed_request(
  env: &Env,
  signer_key: &SigningKey,
  payload: ProxySignedRequestPayload,
) -> ProxySignedRequest {
  ProxySignedRequest::new(env, payload, |bytes| Ok(signer_key.sign(bytes)))
      .expect("Failed to create signed request")
}

mod mock_external {
  soroban_sdk::contractimport!(
      file = "/Users/alen/www/calimero/core/contracts/stellar/context-proxy/mock_external/res/calimero_mock_external_stellar.wasm"
  );
}

#[test]
fn test_cross_contract_call() {
    let env = Env::default();
    env.mock_all_auths();
    //Initialize token contract
    let xlm_token_admin = Address::generate(&env);
    let xlm_token = env.register_stellar_asset_contract_v2(xlm_token_admin.clone());
    
    // Read wasm file
    let wasm = include_bytes!("../../mock_external/res/calimero_mock_external_stellar.wasm");
    let contract_hash = env.deployer().upload_contract_wasm(*wasm);

    let salt = BytesN::<32>::from_array(&env, &[0; 32]);
    let mock_external_contract_address = env
        .deployer()
        .with_address(xlm_token_admin.clone(), salt)
        .deploy_v2(
            contract_hash,
            (&xlm_token.address(),),
        );
    // let token_client = TokenClient::new(&env, &xlm_token.address());
    let token_asset_client = StellarAssetClient::new(&env, &xlm_token.address());
    token_asset_client.mint(&xlm_token_admin, &1_000_000_000);
    
  
    let context_address = Address::generate(&env);
    
    let context_bytes: BytesN<32> = env.as_contract(&mock_external_contract_address, || env.prng().gen());
    let author_bytes: BytesN<32> = env.as_contract(&mock_external_contract_address, || env.prng().gen());

    let context_sk = SigningKey::from_bytes(&context_bytes.to_array());
    let context_pk = context_sk.verifying_key();
    let context_id = BytesN::from_array(&env, &context_pk.to_bytes());

    let context_author_sk = SigningKey::from_bytes(&author_bytes.to_array());
    let context_author_pk = context_author_sk.verifying_key();
    let context_author_id = BytesN::from_array(&env, &context_author_pk.to_bytes());

    let contract_id = env.register(ContextProxyContract, (&context_id, &context_address, &xlm_token.address()),);
    let client = ContextProxyContractClient::new(&env, &contract_id);

    token_asset_client.mint(&contract_id, &1_000_000_000);

    // Create two more signers
    let signer2_bytes: BytesN<32> = env.as_contract(&contract_id, || env.prng().gen());
    let signer3_bytes: BytesN<32> = env.as_contract(&contract_id, || env.prng().gen());

    let signer2_sk = SigningKey::from_bytes(&signer2_bytes.to_array());
    let signer3_sk = SigningKey::from_bytes(&signer3_bytes.to_array());

    let signer2_pk = signer2_sk.verifying_key();
    let signer3_pk = signer3_sk.verifying_key();

    let signer2_id = BytesN::from_array(&env, &signer2_pk.to_bytes());
    let signer3_id = BytesN::from_array(&env, &signer3_pk.to_bytes());


    // Generate proposal ID
    let proposal_id: BytesN<32> = env.as_contract(&contract_id, || env.prng().gen());

    // Create external call proposal with correct arguments
    let method_name = Symbol::new(&env, "deposit");
    let key = String::from_str(&env, "test_key");
    let value = String::from_str(&env, "test_value");
    let deposit: i128 = 100;

    // // Create args for external call - match the expected parameters of test_method
    let args: Vec<Val> = vec![
        &env,
        contract_id.to_val(),
        deposit.into_val(&env),
        key.to_val(),  // Just key and value parameters
        value.to_val(),
    ];
    // let deposit: i128 = 0;
    // // Create args for external call - match the expected parameters of test_method
    // let args: Vec<Val> = vec![
    //     &env,
    //     key.to_val(),  // Just key and value parameters
    //     value.to_val(),
    // ];

    let proposal = StellarProposal {
        id: proposal_id.clone(),
        author_id: context_author_id.clone(),
        actions: vec![
            &env,
            StellarProposalAction::ExternalFunctionCall(
                mock_external_contract_address.clone(),
                method_name,
                args,
                deposit,
            ),
        ],
    };

    let request = StellarProxyMutateRequest::Propose(proposal);
    let signed_request = create_signed_request(
        &env,
        &context_author_sk,
        ProxySignedRequestPayload::Proxy(request),
    );

    client.mutate(&signed_request);

    // Create and submit second approval for the verify proposal
    let second_approval = StellarProposalApprovalWithSigner {
        proposal_id: proposal_id.clone(),
        signer_id: signer2_id.clone(),
    };

    let second_approval_request = StellarProxyMutateRequest::Approve(second_approval);
    let signed_second_approval = create_signed_request(
        &env,
        &signer2_sk,
        ProxySignedRequestPayload::Proxy(second_approval_request),
    );

    // This should execute the proposal since we now only need 2 approvals
    let second_approval_result = client.mutate(&signed_second_approval);
    assert!(second_approval_result.is_some(), "Expected Some after second approval");
    let proposal_after_second = second_approval_result.unwrap();
    log!(&env, "proposal_after_second: {:?}", proposal_after_second);
    assert_eq!(proposal_after_second.num_approvals, 2);
    assert_eq!(proposal_after_second.proposal_id, proposal_id);

    // Create and submit third approval for the verify proposal
    let third_approval = StellarProposalApprovalWithSigner {
        proposal_id: proposal_id.clone(),
        signer_id: signer3_id.clone(),
    };

    let third_approval_request = StellarProxyMutateRequest::Approve(third_approval);
    let signed_third_approval = create_signed_request(
        &env,
        &signer3_sk,
        ProxySignedRequestPayload::Proxy(third_approval_request),
    );

    // This should execute the proposal since we now only need 2 approvals
    let third_approval_result = client.mutate(&signed_third_approval);
    assert!(third_approval_result.is_none(), "Expected None after third approval");

}
