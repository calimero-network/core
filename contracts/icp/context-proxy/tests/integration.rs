use std::cell::RefCell;

use calimero_context_config::icp::repr::ICRepr;
use calimero_context_config::icp::types::{
    ICApplication, ICContextRequest, ICContextRequestKind, ICRequest, ICRequestKind, ICSigned,
};
use calimero_context_config::icp::{
    ICProposal, ICProposalAction, ICProposalApprovalWithSigner, ICProposalWithApprovals,
    ICProxyMutateRequest,
};
use calimero_context_config::repr::ReprTransmute;
use calimero_context_config::types::{ContextId, ContextIdentity};
use candid::Principal;
use ed25519_dalek::{Signer, SigningKey};
use ic_ledger_types::{AccountBalanceArgs, AccountIdentifier, Subaccount, Tokens};
use pocket_ic::{PocketIc, WasmResult};
use rand::Rng;

// Mock canister states
thread_local! {
    static MOCK_LEDGER_BALANCE: RefCell<u64> = RefCell::new(1_000_000_000);
    static MOCK_EXTERNAL_CALLS: RefCell<Vec<(String, Vec<u8>)>> = RefCell::new(Vec::new());
}

fn create_signed_request(
    signer_key: &SigningKey,
    request: ICProxyMutateRequest,
) -> ICSigned<ICProxyMutateRequest> {
    ICSigned::new(request, |bytes| signer_key.sign(bytes)).expect("Failed to create signed request")
}

fn create_signed_context_request(
    signer_key: &SigningKey,
    request: ICRequest,
) -> ICSigned<ICRequest> {
    ICSigned::new(request, |bytes| signer_key.sign(bytes)).expect("Failed to create signed request")
}

// Helper function to create a proposal and verify response
fn create_and_verify_proposal(
    pic: &PocketIc,
    canister: Principal,
    signer_sk: &SigningKey,
    proposal: ICProposal,
) -> Result<Option<ICProposalWithApprovals>, String> {
    let request = ICProxyMutateRequest::Propose { proposal };

    let signed_request = create_signed_request(signer_sk, request);
    let response = pic
        .update_call(
            canister,
            Principal::anonymous(),
            "mutate",
            candid::encode_one(signed_request).unwrap(),
        )
        .map_err(|e| format!("Failed to call canister: {}", e))?;

    match response {
        WasmResult::Reply(bytes) => {
            let result: Result<Option<ICProposalWithApprovals>, String> =
                candid::decode_one(&bytes)
                    .map_err(|e| format!("Failed to decode response: {}", e))?;

            match result {
                Ok(proposal_with_approvals) => Ok(proposal_with_approvals),
                Err(e) => Err(e),
            }
        }
        WasmResult::Reject(msg) => Err(format!("Canister rejected the call: {}", msg)),
    }
}

struct ProxyTestContext {
    pic: PocketIc,
    proxy_canister: Principal,
    context_canister: Principal,
    mock_ledger: Principal,
    mock_external: Principal,
    author_sk: SigningKey,
    context_id: ICRepr<ContextId>,
}

fn setup() -> ProxyTestContext {
    let pic = PocketIc::new();
    let mut rng = rand::thread_rng();

    // Setup context contract first
    let context_canister = pic.create_canister();
    pic.add_cycles(context_canister, 100_000_000_000_000_000);
    let context_wasm = std::fs::read("../context-config/res/calimero_context_config_icp.wasm")
        .expect("failed to read context wasm");
    pic.install_canister(context_canister, context_wasm, vec![], None);

    // Setup mock ledger
    let mock_ledger = pic.create_canister();
    pic.add_cycles(mock_ledger, 100_000_000_000_000);
    let mock_ledger_wasm = std::fs::read("mock/ledger/res/calimero_mock_ledger_icp.wasm")
        .expect("failed to read mock ledger wasm");
    pic.install_canister(mock_ledger, mock_ledger_wasm, vec![], None);

    // Set proxy code in context contract
    set_proxy_code(&pic, context_canister, mock_ledger).expect("Failed to set proxy code");

    // Setup mock external
    let mock_external = pic.create_canister();
    pic.add_cycles(mock_external, 100_000_000_000_000);
    let mock_external_wasm = std::fs::read("mock/external/res/calimero_mock_external_icp.wasm")
        .expect("failed to read mock external wasm");
    pic.install_canister(mock_external, mock_external_wasm, vec![], None);

    // Create initial author key
    let author_sk = SigningKey::from_bytes(&rng.gen());

    // Create context and get proxy canister
    let (proxy_canister, context_id) =
        create_context_with_proxy(&pic, context_canister, &author_sk)
            .expect("Failed to create context and proxy");

    ProxyTestContext {
        pic,
        proxy_canister,
        context_canister,
        mock_ledger,
        mock_external,
        author_sk,
        context_id,
    }
}

// Helper function to set proxy code in context contract
fn set_proxy_code(
    pic: &PocketIc,
    context_canister: Principal,
    ledger_id: Principal,
) -> Result<(), String> {
    // Read proxy contract wasm
    let proxy_wasm =
        std::fs::read("res/calimero_context_proxy_icp.wasm").expect("failed to read proxy wasm");

    let response = pic.update_call(
        context_canister,
        Principal::anonymous(),
        "set_proxy_code",
        candid::encode_args((proxy_wasm, ledger_id)).unwrap(),
    );

    match response {
        Ok(WasmResult::Reply(bytes)) => {
            let result: Result<(), String> = candid::decode_one(&bytes)
                .map_err(|e| format!("Failed to decode response: {}", e))?;
            result
        }
        Ok(WasmResult::Reject(msg)) => Err(format!("Setting proxy code rejected: {}", msg)),
        Err(e) => Err(format!("Setting proxy code failed: {}", e)),
    }
}

// Helper function to create context and deploy proxy
fn create_context_with_proxy(
    pic: &PocketIc,
    context_canister: Principal,
    author_sk: &SigningKey,
) -> Result<(Principal, ICRepr<ContextId>), String> {
    let mut rng = rand::thread_rng();

    // Generate context ID
    let context_sk = SigningKey::from_bytes(&rng.gen());
    let context_pk = context_sk.verifying_key();
    let context_id = context_pk.rt().expect("infallible conversion");

    // Create author identity
    let author_pk = author_sk.verifying_key();
    let author_id = author_pk.rt().expect("infallible conversion");

    // Create context with initial application
    let create_request = ICRequest {
        kind: ICRequestKind::Context(ICContextRequest {
            context_id,
            kind: ICContextRequestKind::Add {
                author_id,
                application: ICApplication {
                    id: rng.gen::<[_; 32]>().rt().expect("infallible conversion"),
                    blob: rng.gen::<[_; 32]>().rt().expect("infallible conversion"),
                    size: 0,
                    source: String::new(),
                    metadata: vec![],
                },
            },
        }),
        signer_id: context_id.rt().expect("infallible conversion"),
        nonce: 0,
    };

    let signed_request = create_signed_context_request(&context_sk, create_request);
    let response = pic.update_call(
        context_canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );

    // Check if context creation succeeded
    match response {
        Ok(WasmResult::Reply(bytes)) => {
            let result: Result<(), String> = candid::decode_one(&bytes)
                .map_err(|e| format!("Failed to decode response: {}", e))?;
            result.map_err(|e| format!("Context creation failed: {}", e))?;
        }
        Ok(WasmResult::Reject(msg)) => return Err(format!("Context creation rejected: {}", msg)),
        Err(e) => return Err(format!("Context creation failed: {}", e)),
    }

    // Query for proxy canister ID
    let query_response = pic.query_call(
        context_canister,
        Principal::anonymous(),
        "proxy_contract",
        candid::encode_one(context_id).unwrap(),
    );

    match query_response {
        Ok(WasmResult::Reply(bytes)) => {
            let proxy_canister: Principal = candid::decode_one(&bytes)
                .map_err(|e| format!("Failed to decode proxy canister ID: {}", e))?;
            Ok((proxy_canister, context_id))
        }
        Ok(WasmResult::Reject(msg)) => Err(format!("Query rejected: {}", msg)),
        Err(e) => Err(format!("Query failed: {}", e)),
    }
}

// Helper function to add members to context
fn add_members_to_context(
    pic: &PocketIc,
    context_canister: Principal,
    context_id: ICRepr<ContextId>,
    author_sk: &SigningKey,
    members: Vec<ICRepr<ContextIdentity>>,
) -> Result<(), String> {
    let author_pk = author_sk.verifying_key();

    let request = ICRequest {
        kind: ICRequestKind::Context(ICContextRequest {
            context_id,
            kind: ICContextRequestKind::AddMembers { members },
        }),
        signer_id: author_pk.rt().expect("infallible conversion"),
        nonce: 0,
    };

    let signed_request = create_signed_context_request(author_sk, request);
    let response = pic.update_call(
        context_canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );

    match response {
        Ok(WasmResult::Reply(bytes)) => {
            candid::decode_one(&bytes).map_err(|e| format!("Failed to decode response: {}", e))
        }
        Ok(WasmResult::Reject(msg)) => Err(format!("Adding members rejected: {}", msg)),
        Err(e) => Err(format!("Adding members failed: {}", e)),
    }
}

#[test]
fn test_update_proxy_contract() {
    let mut rng = rand::thread_rng();

    let ProxyTestContext {
        pic,
        proxy_canister,
        context_canister,
        author_sk,
        context_id,
        ..
    } = setup();

    // First test: Try direct upgrade (should fail)
    let proxy_wasm =
        std::fs::read("res/calimero_context_proxy_icp.wasm").expect("failed to read proxy wasm");

    let unauthorized_result = pic.upgrade_canister(
        proxy_canister,
        proxy_wasm,
        candid::encode_one::<Vec<u8>>(vec![]).unwrap(),
        Some(Principal::anonymous()),
    );
    match unauthorized_result {
        Ok(_) => panic!("Direct upgrade should fail"),
        Err(e) => {
            println!("Got expected unauthorized error: {:?}", e);
        }
    }

    // Now continue with the rest of the test (authorized upgrade through context)
    let author_pk = author_sk.verifying_key();
    let author_id = author_pk.rt().expect("infallible conversion");
    let proposal_id = rng.gen::<[_; 32]>().rt().expect("infallible conversion");

    let proposal = ICProposal {
        id: proposal_id,
        author_id,
        actions: vec![ICProposalAction::Transfer {
            receiver_id: Principal::anonymous(),
            amount: 1000000,
        }],
    };

    create_and_verify_proposal(&pic, proxy_canister, &author_sk, proposal)
        .expect("Transfer proposal creation should succeed");

    // Query initial state - get the proposal
    let initial_proposal = pic
        .query_call(
            proxy_canister,
            Principal::anonymous(),
            "proposal",
            candid::encode_one(proposal_id).unwrap(),
        )
        .and_then(|r| match r {
            WasmResult::Reply(bytes) => {
                Ok(candid::decode_one::<Option<ICProposal>>(&bytes).unwrap())
            }
            _ => panic!("Unexpected response type"),
        })
        .expect("Query failed")
        .expect("Proposal not found");

    // Create update request to context contract
    let update_request = ICRequest {
        kind: ICRequestKind::Context(ICContextRequest {
            context_id,
            kind: ICContextRequestKind::UpdateProxyContract,
        }),
        signer_id: author_pk.rt().expect("infallible conversion"),
        nonce: 0,
    };

    let signed_update_request = create_signed_context_request(&author_sk, update_request);
    let response = pic.update_call(
        context_canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_update_request).unwrap(),
    );

    // Handle the response directly
    match response {
        Ok(WasmResult::Reply(bytes)) => {
            let result: Result<(), String> =
                candid::decode_one(&bytes).expect("Failed to decode response");
            assert!(result.is_ok(), "Context update should succeed");
        }
        Ok(WasmResult::Reject(msg)) => panic!("Context update was rejected: {}", msg),
        Err(e) => panic!("Context update failed: {}", e),
    }

    // Verify state was preserved after upgrade
    let final_proposal = pic
        .query_call(
            proxy_canister,
            Principal::anonymous(),
            "proposal",
            candid::encode_one(proposal_id).unwrap(),
        )
        .and_then(|r| match r {
            WasmResult::Reply(bytes) => {
                Ok(candid::decode_one::<Option<ICProposal>>(&bytes).unwrap())
            }
            _ => panic!("Unexpected response type"),
        })
        .expect("Query failed")
        .expect("Proposal not found");

    assert_eq!(
        initial_proposal, final_proposal,
        "Proposal state not preserved after upgrade"
    );
}

#[test]
fn test_create_proposal_transfer() {
    let mut rng = rand::thread_rng();

    let ProxyTestContext {
        pic,
        proxy_canister,
        author_sk,
        ..
    } = setup();

    let author_pk = author_sk.verifying_key();
    let author_id = author_pk.rt().expect("infallible conversion");
    let proposal_id = rng.gen::<[_; 32]>().rt().expect("infallible conversion");

    let proposal = ICProposal {
        id: proposal_id,
        author_id,
        actions: vec![ICProposalAction::Transfer {
            receiver_id: Principal::anonymous(),
            amount: 1000000,
        }],
    };

    create_and_verify_proposal(&pic, proxy_canister, &author_sk, proposal)
        .expect("Transfer proposal creation should succeed");
}

#[test]
fn test_create_proposal_external_call() {
    let mut rng = rand::thread_rng();

    let ProxyTestContext {
        pic,
        proxy_canister,
        author_sk,
        ..
    } = setup();

    let author_pk = author_sk.verifying_key();
    let author_id = author_pk.rt().expect("infallible conversion");
    let proposal_id = rng.gen::<[_; 32]>().rt().expect("infallible conversion");

    let proposal = ICProposal {
        id: proposal_id,
        author_id,
        actions: vec![ICProposalAction::ExternalFunctionCall {
            receiver_id: Principal::anonymous(),
            method_name: "test_method".to_string(),
            args: "deadbeef".to_string(),
            deposit: 0,
        }],
    };

    create_and_verify_proposal(&pic, proxy_canister, &author_sk, proposal)
        .expect("External call proposal creation should succeed");
}

#[test]
fn test_create_proposal_set_context() {
    let mut rng = rand::thread_rng();

    let ProxyTestContext {
        pic,
        proxy_canister,
        author_sk,
        ..
    } = setup();

    let author_pk = author_sk.verifying_key();
    let author_id = author_pk.rt().expect("infallible conversion");
    let proposal_id = rng.gen::<[_; 32]>().rt().expect("infallible conversion");

    let proposal = ICProposal {
        id: proposal_id,
        author_id,
        actions: vec![ICProposalAction::SetContextValue {
            key: vec![1, 2, 3],
            value: vec![4, 5, 6],
        }],
    };

    create_and_verify_proposal(&pic, proxy_canister, &author_sk, proposal)
        .expect("Setting context value should succeed");
}

#[test]
fn test_create_proposal_multiple_actions() {
    let mut rng = rand::thread_rng();

    let ProxyTestContext {
        pic,
        proxy_canister,
        author_sk,
        ..
    } = setup();

    let author_pk = author_sk.verifying_key();
    let author_id = author_pk.rt().expect("infallible conversion");
    let proposal_id = rng.gen::<[_; 32]>().rt().expect("infallible conversion");

    let proposal = ICProposal {
        id: proposal_id,
        author_id,
        actions: vec![
            ICProposalAction::SetNumApprovals { num_approvals: 2 },
            ICProposalAction::SetActiveProposalsLimit {
                active_proposals_limit: 5,
            },
        ],
    };

    create_and_verify_proposal(&pic, proxy_canister, &author_sk, proposal)
        .expect("Multiple actions proposal creation should succeed");
}

#[test]
fn test_create_proposal_invalid_transfer_amount() {
    let mut rng = rand::thread_rng();

    let ProxyTestContext {
        pic,
        proxy_canister,
        author_sk,
        ..
    } = setup();

    let author_pk = author_sk.verifying_key();
    let author_id = author_pk.rt().expect("infallible conversion");
    let proposal_id = rng.gen::<[_; 32]>().rt().expect("infallible conversion");

    let proposal = ICProposal {
        id: proposal_id,
        author_id,
        actions: vec![ICProposalAction::Transfer {
            receiver_id: Principal::anonymous(),
            amount: 0, // Invalid amount
        }],
    };

    let request = ICProxyMutateRequest::Propose { proposal };

    let signed_request = create_signed_request(&author_sk, request);
    let response = pic.update_call(
        proxy_canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );

    match response {
        Ok(WasmResult::Reply(bytes)) => {
            let result: Result<Option<ICProposalWithApprovals>, String> =
                candid::decode_one(&bytes).expect("Failed to decode response");
            assert!(
                result.is_err(),
                "Expected error for invalid transfer amount"
            );
        }
        Ok(WasmResult::Reject(msg)) => {
            panic!("Canister rejected the call: {}", msg);
        }
        Err(err) => {
            panic!("Failed to call canister: {}", err);
        }
    }
}

#[test]
fn test_create_proposal_invalid_method_name() {
    let mut rng = rand::thread_rng();

    let ProxyTestContext {
        pic,
        proxy_canister,
        author_sk,
        ..
    } = setup();

    let author_pk = author_sk.verifying_key();
    let author_id = author_pk.rt().expect("infallible conversion");
    let proposal_id = rng.gen::<[_; 32]>().rt().expect("infallible conversion");

    let proposal = ICProposal {
        id: proposal_id,
        author_id,
        actions: vec![ICProposalAction::ExternalFunctionCall {
            receiver_id: Principal::anonymous(),
            method_name: "".to_string(), // Invalid method name
            args: "deadbeef".to_string(),
            deposit: 0,
        }],
    };

    let request = ICProxyMutateRequest::Propose { proposal };

    let signed_request = create_signed_request(&author_sk, request);
    let response = pic.update_call(
        proxy_canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );

    match response {
        Ok(WasmResult::Reply(bytes)) => {
            let result: Result<Option<ICProposalWithApprovals>, String> =
                candid::decode_one(&bytes).expect("Failed to decode response");
            assert!(result.is_err(), "Expected error for invalid method name");
        }
        Ok(WasmResult::Reject(msg)) => {
            panic!("Canister rejected the call: {}", msg);
        }
        Err(err) => {
            panic!("Failed to call canister: {}", err);
        }
    }
}

#[test]
fn test_approve_own_proposal() {
    let mut rng = rand::thread_rng();

    let ProxyTestContext {
        pic,
        proxy_canister,
        author_sk,
        ..
    } = setup();

    let author_pk = author_sk.verifying_key();
    let author_id = author_pk.rt().expect("infallible conversion");
    let proposal_id = rng.gen::<[_; 32]>().rt().expect("infallible conversion");

    // Create proposal
    let proposal = ICProposal {
        id: proposal_id,
        author_id,
        actions: vec![ICProposalAction::SetNumApprovals { num_approvals: 2 }],
    };

    let _ = create_and_verify_proposal(&pic, proxy_canister, &author_sk, proposal);

    // Try to approve own proposal
    let approval = ICProposalApprovalWithSigner {
        signer_id: author_id,
        proposal_id,
    };

    let request = ICProxyMutateRequest::Approve { approval };

    let signed_request = create_signed_request(&author_sk, request);
    let result = pic.update_call(
        proxy_canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );

    match result {
        Ok(WasmResult::Reply(bytes)) => {
            let result: Result<Option<ICProposalWithApprovals>, String> =
                candid::decode_one(&bytes).expect("Failed to decode response");
            assert!(
                matches!(result, Err(e) if e.contains("already approved")),
                "Should not be able to approve own proposal twice"
            );
        }
        _ => panic!("Unexpected response type"),
    }
}

#[test]
fn test_approve_non_existent_proposal() {
    let mut rng = rand::thread_rng();

    let ProxyTestContext {
        pic,
        proxy_canister,
        author_sk: signer_sk,
        ..
    } = setup();

    let signer_pk = signer_sk.verifying_key();
    let signer_id = signer_pk.rt().expect("infallible conversion");
    let proposal_id = rng.gen::<[_; 32]>().rt().expect("infallible conversion");

    let approval = ICProposalApprovalWithSigner {
        signer_id,
        proposal_id,
    };

    let request = ICProxyMutateRequest::Approve { approval };

    let signed_request = create_signed_request(&signer_sk, request);
    let response = pic.update_call(
        proxy_canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );

    match response {
        Ok(WasmResult::Reply(bytes)) => {
            let result: Result<Option<ICProposalWithApprovals>, String> =
                candid::decode_one(&bytes).expect("Failed to decode response");
            assert!(
                result.is_err(),
                "Should not be able to approve non-existent proposal"
            );
        }
        _ => panic!("Unexpected response type"),
    }
}

#[test]
fn test_create_proposal_empty_actions() {
    let mut rng = rand::thread_rng();

    let ProxyTestContext {
        pic,
        proxy_canister,
        author_sk,
        ..
    } = setup();

    let author_pk = author_sk.verifying_key();
    let author_id = author_pk.rt().expect("infallible conversion");
    let proposal_id = rng.gen::<[_; 32]>().rt().expect("infallible conversion");

    let proposal = ICProposal {
        id: proposal_id,
        author_id,
        actions: vec![], // Empty actions
    };

    let request = ICProxyMutateRequest::Propose { proposal };

    let signed_request = create_signed_request(&author_sk, request);
    let response = pic.update_call(
        proxy_canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );

    match response {
        Ok(WasmResult::Reply(bytes)) => {
            let result: Result<Option<ICProposalWithApprovals>, String> =
                candid::decode_one(&bytes).expect("Failed to decode response");
            assert!(result.is_err(), "Should fail with empty actions");
            assert!(
                matches!(result, Err(e) if e.contains("empty actions")),
                "Error should mention empty actions"
            );
        }
        _ => panic!("Unexpected response type"),
    }
}

#[test]
fn test_create_proposal_exceeds_limit() {
    let mut rng = rand::thread_rng();

    let ProxyTestContext {
        pic,
        proxy_canister,
        author_sk,
        ..
    } = setup();

    let author_pk = author_sk.verifying_key();
    let author_id = author_pk.rt().expect("infallible conversion");

    // Create max number of proposals
    for _ in 0..10 {
        let proposal_id = rng.gen::<[_; 32]>().rt().expect("infallible conversion");

        let proposal = ICProposal {
            id: proposal_id,
            author_id,
            actions: vec![ICProposalAction::SetNumApprovals { num_approvals: 2 }],
        };

        let _ = create_and_verify_proposal(&pic, proxy_canister, &author_sk, proposal);
    }

    let proposal_id = rng.gen::<[_; 32]>().rt().expect("infallible conversion");

    // Try to create one more
    let proposal = ICProposal {
        id: proposal_id,
        author_id,
        actions: vec![ICProposalAction::SetNumApprovals { num_approvals: 2 }],
    };

    let request = ICProxyMutateRequest::Propose { proposal };

    let signed_request = create_signed_request(&author_sk, request);
    let response = pic.update_call(
        proxy_canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );

    match response {
        Ok(WasmResult::Reply(bytes)) => {
            let result: Result<Option<ICProposalWithApprovals>, String> =
                candid::decode_one(&bytes).expect("Failed to decode response");
            assert!(
                result.is_err(),
                "Should not be able to exceed proposal limit"
            );
        }
        _ => panic!("Unexpected response type"),
    }
}

#[test]
fn test_proposal_execution_transfer() {
    let mut rng = rand::thread_rng();

    let ProxyTestContext {
        pic,
        proxy_canister,
        mock_external,
        mock_ledger,
        author_sk,
        context_canister,
        context_id,
        ..
    } = setup();

    let initial_balance = MOCK_LEDGER_BALANCE.with(|b| *b.borrow());

    // Setup signers
    let author_pk = author_sk.verifying_key();
    let author_id = author_pk.rt().expect("infallible conversion");

    let signer2_sk = SigningKey::from_bytes(&rng.gen());
    let signer2_pk = signer2_sk.verifying_key();
    let signer2_id = signer2_pk.rt().expect("infallible conversion");

    let signer3_sk = SigningKey::from_bytes(&rng.gen());
    let signer3_pk = signer3_sk.verifying_key();
    let signer3_id = signer3_pk.rt().expect("infallible conversion");

    let transfer_amount = 1_000;

    let proposal_id = rng.gen::<[_; 32]>().rt().expect("infallible conversion");

    // let receiver_id = Principal::from_text("bnz7o-iuaaa-aaaaa-qaaaa-cai").unwrap();
    // Create transfer proposal
    let proposal = ICProposal {
        id: proposal_id,
        author_id,
        actions: vec![ICProposalAction::Transfer {
            receiver_id: mock_external,
            amount: transfer_amount,
        }],
    };

    // Create and verify initial proposal
    let _ = create_and_verify_proposal(&pic, proxy_canister, &author_sk, proposal);

    let context_members = vec![
        signer2_pk.rt().expect("infallible conversion"),
        signer3_pk.rt().expect("infallible conversion"),
    ];

    let _ = add_members_to_context(
        &pic,
        context_canister,
        context_id,
        &author_sk,
        context_members,
    );

    // Add approvals to trigger execution
    for (signer_sk, signer_id) in [(signer2_sk, signer2_id), (signer3_sk, signer3_id)] {
        let approval = ICProposalApprovalWithSigner {
            signer_id,
            proposal_id,
        };

        let request = ICProxyMutateRequest::Approve { approval };

        let signed_request = create_signed_request(&signer_sk, request);
        let response = pic.update_call(
            proxy_canister,
            Principal::anonymous(),
            "mutate",
            candid::encode_one(signed_request).unwrap(),
        );

        // Last approval should trigger execution
        match response {
            Ok(WasmResult::Reply(bytes)) => {
                let result: Result<Option<ICProposalWithApprovals>, String> =
                    candid::decode_one(&bytes).expect("Failed to decode response");
                match result {
                    Ok(Some(_proposal_with_approvals)) => {}
                    Ok(None) => {
                        // Proposal was executed and removed
                        // Verify proposal no longer exists
                        let query_response = pic
                            .query_call(
                                proxy_canister,
                                Principal::anonymous(),
                                "proposal",
                                candid::encode_one(proposal_id).unwrap(),
                            )
                            .expect("Query failed");

                        match query_response {
                            WasmResult::Reply(bytes) => {
                                let stored_proposal: Option<ICProposal> =
                                    candid::decode_one(&bytes)
                                        .expect("Failed to decode stored proposal");
                                assert!(
                                    stored_proposal.is_none(),
                                    "Proposal should be removed after execution"
                                );
                            }
                            WasmResult::Reject(msg) => {
                                panic!("Query rejected: {}", msg);
                            }
                        }
                    }
                    Err(e) => panic!("Unexpected error: {}", e),
                }
            }
            _ => panic!("Unexpected response type"),
        }
    }

    let args = AccountBalanceArgs {
        account: AccountIdentifier::new(&Principal::anonymous(), &Subaccount([0; 32])),
    };

    let response = pic
        .query_call(
            mock_ledger,
            Principal::anonymous(),
            "account_balance",
            candid::encode_one(args).unwrap(),
        )
        .expect("Failed to query balance");

    match response {
        WasmResult::Reply(bytes) => {
            let balance: Tokens = candid::decode_one(&bytes).expect("Failed to decode balance");
            let final_balance = balance.e8s();
            // Verify the transfer was executed
            assert_eq!(
                final_balance,
                initial_balance
                    .saturating_sub(transfer_amount as u64)
                    .saturating_sub(10_000), // Subtract both transfer amount and fee
                "Transfer amount should be deducted from ledger balance"
            );
        }
        _ => panic!("Unexpected response type"),
    }
}

#[test]
fn test_proposal_execution_external_call() {
    let mut rng = rand::thread_rng();

    let ProxyTestContext {
        pic,
        proxy_canister,
        mock_external,
        author_sk,
        context_canister,
        context_id,
        ..
    } = setup();

    let author_pk = author_sk.verifying_key();
    let author_id = author_pk.rt().expect("infallible conversion");

    let signer2_sk = SigningKey::from_bytes(&rng.gen());
    let signer2_pk = signer2_sk.verifying_key();
    let signer2_id = signer2_pk.rt().expect("infallible conversion");

    let signer3_sk = SigningKey::from_bytes(&rng.gen());
    let signer3_pk = signer3_sk.verifying_key();
    let signer3_id = signer3_pk.rt().expect("infallible conversion");

    let proposal_id = rng.gen::<[_; 32]>().rt().expect("infallible conversion");

    // Create external call proposal
    let test_args = "01020304".to_string(); // Test arguments as string
    let proposal = ICProposal {
        id: proposal_id,
        author_id,
        actions: vec![ICProposalAction::ExternalFunctionCall {
            receiver_id: mock_external,
            method_name: "test_method".to_string(),
            args: test_args.clone(),
            deposit: 0,
        }],
    };

    // Create and verify initial proposal
    let _ = create_and_verify_proposal(&pic, proxy_canister, &author_sk, proposal);

    let context_members = vec![
        signer2_pk.rt().expect("infallible conversion"),
        signer3_pk.rt().expect("infallible conversion"),
    ];

    let _ = add_members_to_context(
        &pic,
        context_canister,
        context_id,
        &author_sk,
        context_members,
    );

    // Add approvals to trigger execution
    for (signer_sk, signer_id) in [(signer2_sk, signer2_id), (signer3_sk, signer3_id)] {
        let approval = ICProposalApprovalWithSigner {
            signer_id,
            proposal_id,
        };

        let request = ICProxyMutateRequest::Approve { approval };

        let signed_request = create_signed_request(&signer_sk, request);
        let response = pic.update_call(
            proxy_canister,
            Principal::anonymous(),
            "mutate",
            candid::encode_one(signed_request).unwrap(),
        );

        // Last approval should trigger execution
        match response {
            Ok(WasmResult::Reply(bytes)) => {
                let result: Result<Option<ICProposalWithApprovals>, String> =
                    candid::decode_one(&bytes).expect("Failed to decode response");
                match result {
                    Ok(Some(_proposal_with_approvals)) => {}
                    Ok(None) => {
                        // Proposal was executed and removed
                        // Verify proposal no longer exists
                        let query_response = pic
                            .query_call(
                                proxy_canister,
                                Principal::anonymous(),
                                "proposal",
                                candid::encode_one(proposal_id).unwrap(),
                            )
                            .expect("Query failed");

                        match query_response {
                            WasmResult::Reply(bytes) => {
                                let stored_proposal: Option<ICProposal> =
                                    candid::decode_one(&bytes)
                                        .expect("Failed to decode stored proposal");
                                assert!(
                                    stored_proposal.is_none(),
                                    "Proposal should be removed after execution"
                                );
                            }
                            WasmResult::Reject(msg) => {
                                panic!("Query rejected: {}", msg);
                            }
                        }
                    }
                    Err(e) => panic!("Unexpected error: {}", e),
                }
            }
            _ => panic!("Unexpected response type"),
        }
    }

    // Verify the external call was executed
    let response = pic
        .query_call(
            mock_external,
            Principal::anonymous(),
            "get_calls",
            candid::encode_args(()).unwrap(),
        )
        .expect("Query failed");

    match response {
        WasmResult::Reply(bytes) => {
            let calls: Vec<Vec<u8>> = candid::decode_one(&bytes).expect("Failed to decode calls");
            assert_eq!(calls.len(), 1, "Should have exactly one call");

            // Decode the Candid-encoded argument
            let received_args: String =
                candid::decode_one(&calls[0]).expect("Failed to decode call arguments");
            assert_eq!(received_args, test_args, "Call arguments should match");
        }
        _ => panic!("Unexpected response type"),
    }
}

#[test]
fn test_proposal_execution_external_call_with_deposit() {
    let mut rng = rand::thread_rng();

    let ProxyTestContext {
        pic,
        proxy_canister,
        mock_external,
        author_sk,
        context_canister,
        context_id,
        mock_ledger,
        ..
    } = setup();

    let author_pk = author_sk.verifying_key();
    let author_id = author_pk.rt().expect("infallible conversion");

    let signer2_sk = SigningKey::from_bytes(&rng.gen());
    let signer2_pk = signer2_sk.verifying_key();
    let signer2_id = signer2_pk.rt().expect("infallible conversion");

    let signer3_sk = SigningKey::from_bytes(&rng.gen());
    let signer3_pk = signer3_sk.verifying_key();
    let signer3_id = signer3_pk.rt().expect("infallible conversion");

    let proposal_id = rng.gen::<[_; 32]>().rt().expect("infallible conversion");

    // Create external call proposal
    let deposit_amount = 1_000_000;
    let test_args = "01020304".to_string(); // Test arguments as string
    let proposal = ICProposal {
        id: proposal_id,
        author_id,
        actions: vec![ICProposalAction::ExternalFunctionCall {
            receiver_id: mock_external,
            method_name: "test_method".to_string(),
            args: test_args.clone(),
            deposit: deposit_amount,
        }],
    };

    // Create and verify initial proposal
    let _ = create_and_verify_proposal(&pic, proxy_canister, &author_sk, proposal);

    let context_members = vec![
        signer2_pk.rt().expect("infallible conversion"),
        signer3_pk.rt().expect("infallible conversion"),
    ];

    let _ = add_members_to_context(
        &pic,
        context_canister,
        context_id,
        &author_sk,
        context_members,
    );

    // Add approvals to trigger execution
    for (signer_sk, signer_id) in [(signer2_sk, signer2_id), (signer3_sk, signer3_id)] {
        let approval = ICProposalApprovalWithSigner {
            signer_id,
            proposal_id,
        };

        let request = ICProxyMutateRequest::Approve { approval };
        let signed_request = create_signed_request(&signer_sk, request);

        let response = pic
            .update_call(
                proxy_canister,
                Principal::anonymous(),
                "mutate",
                candid::encode_one(signed_request).unwrap(),
            )
            .expect("Failed to approve proposal");

        match response {
            WasmResult::Reply(bytes) => {
                let result: Result<Option<ICProposalWithApprovals>, String> =
                    candid::decode_one(&bytes).expect("Failed to decode response");

                if let Ok(None) = result {
                    // Proposal was executed, verify it's gone
                    let query_response = pic
                        .query_call(
                            proxy_canister,
                            Principal::anonymous(),
                            "proposal",
                            candid::encode_one(proposal_id).unwrap(),
                        )
                        .expect("Query failed");

                    match query_response {
                        WasmResult::Reply(bytes) => {
                            let stored_proposal: Option<ICProposal> = candid::decode_one(&bytes)
                                .expect("Failed to decode stored proposal");
                            assert!(
                                stored_proposal.is_none(),
                                "Proposal should be removed after execution"
                            );
                        }
                        WasmResult::Reject(msg) => panic!("Query rejected: {}", msg),
                    }
                }
            }
            WasmResult::Reject(msg) => panic!("Approval rejected: {}", msg),
        }
    }

    // Verify the transfer was executed by checking mock ledger balance
    let args = AccountBalanceArgs {
        account: AccountIdentifier::new(&mock_external, &Subaccount([0; 32])),
    };

    let response = pic
        .query_call(
            mock_ledger,
            Principal::anonymous(),
            "account_balance",
            candid::encode_one(args).unwrap(),
        )
        .expect("Failed to query balance");

    match response {
        WasmResult::Reply(bytes) => {
            let balance: Tokens = candid::decode_one(&bytes).expect("Failed to decode balance");
            let gas_fee = 10_000;
            assert_eq!(
                balance.e8s(),
                MOCK_LEDGER_BALANCE.with(|b| *b.borrow()) - deposit_amount as u64 - gas_fee as u64,
                "External contract should have received the deposit"
            );
        }
        WasmResult::Reject(msg) => panic!("Balance query rejected: {}", msg),
    }

    // Verify the external call was executed
    let response = pic
        .query_call(
            mock_external,
            Principal::anonymous(),
            "get_calls",
            candid::encode_args(()).unwrap(),
        )
        .expect("Query failed");

    match response {
        WasmResult::Reply(bytes) => {
            let calls: Vec<Vec<u8>> = candid::decode_one(&bytes).expect("Failed to decode calls");
            assert_eq!(calls.len(), 1, "Should have exactly one call");

            // Decode the Candid-encoded argument
            let received_args: String =
                candid::decode_one(&calls[0]).expect("Failed to decode call arguments");
            assert_eq!(received_args, test_args, "Call arguments should match");
        }
        _ => panic!("Unexpected response type"),
    }
}

#[test]
fn test_delete_proposal() {
    let mut rng = rand::thread_rng();

    let ProxyTestContext {
        pic,
        proxy_canister,
        author_sk,
        ..
    } = setup();

    let author_pk = author_sk.verifying_key();
    let author_id = author_pk.rt().expect("infallible conversion");

    // First create a proposal that we'll want to delete
    let target_proposal_id = rng.gen::<[_; 32]>().rt().expect("infallible conversion");
    let target_proposal = ICProposal {
        id: target_proposal_id,
        author_id,
        actions: vec![ICProposalAction::SetNumApprovals { num_approvals: 2 }],
    };

    // Create and verify target proposal
    let target_proposal_result =
        create_and_verify_proposal(&pic, proxy_canister, &author_sk, target_proposal)
            .expect("Target proposal creation should succeed");
    assert!(
        target_proposal_result.is_some(),
        "Target proposal should be created"
    );

    // Create delete proposal
    let delete_proposal_id = rng.gen::<[_; 32]>().rt().expect("infallible conversion");
    let delete_proposal = ICProposal {
        id: delete_proposal_id,
        author_id,
        actions: vec![ICProposalAction::DeleteProposal {
            proposal_id: target_proposal_id,
        }],
    };

    // Execute delete proposal immediately
    let delete_proposal_result =
        create_and_verify_proposal(&pic, proxy_canister, &author_sk, delete_proposal)
            .expect("Delete proposal execution should succeed");
    assert!(
        delete_proposal_result.is_none(),
        "Delete proposal should execute immediately"
    );

    // Verify target proposal no longer exists
    let query_response = pic
        .query_call(
            proxy_canister,
            Principal::anonymous(),
            "proposal",
            candid::encode_one(target_proposal_id).unwrap(),
        )
        .expect("Query failed");

    match query_response {
        WasmResult::Reply(bytes) => {
            let stored_proposal: Option<ICProposal> =
                candid::decode_one(&bytes).expect("Failed to decode stored proposal");
            assert!(
                stored_proposal.is_none(),
                "Target proposal should be deleted"
            );
        }
        WasmResult::Reject(msg) => panic!("Query rejected: {}", msg),
    }
}
