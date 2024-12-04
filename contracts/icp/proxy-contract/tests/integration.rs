#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::time::UNIX_EPOCH;

    use candid::Principal;
    use ed25519_dalek::{Signer, SigningKey};
    use ic_ledger_types::{AccountBalanceArgs, AccountIdentifier, Subaccount, Tokens};
    use pocket_ic::{PocketIc, WasmResult};
    use proxy_contract::types::{
        ICContextId, ICPSigned, ICProposal, ICProposalAction, ICProposalApprovalWithSigner,
        ICProposalId, ICProposalWithApprovals, ICRequest, ICRequestKind, ICSignerId,
    };
    use rand::Rng;

    // Mock canister states
    thread_local! {
        static MOCK_LEDGER_BALANCE: RefCell<u64> = RefCell::new(1_000_000_000);
        static MOCK_EXTERNAL_CALLS: RefCell<Vec<(String, Vec<u8>)>> = RefCell::new(Vec::new());
    }

    struct ProxyTestContext {
        pic: PocketIc,
        proxy_canister: Principal,
        mock_external: Principal,
        mock_ledger: Principal,
    }

    fn setup() -> ProxyTestContext {
        let pic = PocketIc::new();

        // Setup mock ledger first
        let mock_ledger = pic.create_canister();
        pic.add_cycles(mock_ledger, 100_000_000_000_000);
        let mock_ledger_wasm = std::fs::read("mock/ledger/res/mock_ledger.wasm")
            .expect("failed to read mock ledger wasm");
        pic.install_canister(mock_ledger, mock_ledger_wasm, vec![], None);

        // Setup proxy contract
        let wasm = std::fs::read("res/proxy_contract.wasm").expect("failed to read wasm");
        let proxy_canister = pic.create_canister();
        pic.add_cycles(proxy_canister, 100_000_000_000_000);

        // Create init arg with both context_id and ledger_id
        let context_id = ICContextId::new([0; 32]);
        let init_arg = candid::encode_args((context_id, mock_ledger)).unwrap();

        pic.install_canister(proxy_canister, wasm, init_arg, None);

        // Setup mock external
        let mock_external = pic.create_canister();
        pic.add_cycles(mock_external, 100_000_000_000_000);
        let mock_external_wasm = std::fs::read("mock/external/res/mock_external.wasm")
            .expect("failed to read mock external wasm");
        pic.install_canister(mock_external, mock_external_wasm, vec![], None);

        ProxyTestContext {
            pic,
            proxy_canister,
            mock_external,
            mock_ledger,
        }
    }

    fn create_signed_request(signer_key: &SigningKey, request: ICRequest) -> ICPSigned<ICRequest> {
        ICPSigned::new(request, |bytes| signer_key.sign(bytes))
            .expect("Failed to create signed request")
    }

    fn get_time_nanos(pic: &PocketIc) -> u64 {
        pic.get_time()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_nanos() as u64
    }

    // Helper function to create a proposal and verify response
    fn create_and_verify_proposal(
        pic: &PocketIc,
        canister: Principal,
        signer_sk: &SigningKey,
        signer_id: &ICSignerId,
        proposal: ICProposal,
    ) -> Result<ICProposalWithApprovals, String> {
        let request = ICRequest {
            signer_id: signer_id.clone(),
            timestamp_ms: get_time_nanos(pic),
            kind: ICRequestKind::Propose {
                proposal: proposal.clone(),
            },
        };

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
                    Ok(Some(proposal_with_approvals)) => Ok(proposal_with_approvals),
                    Ok(None) => Err("No proposal returned".to_string()),
                    Err(e) => Err(e),
                }
            }
            WasmResult::Reject(msg) => Err(format!("Canister rejected the call: {}", msg)),
        }
    }

    #[test]
    fn test_create_proposal() {
        let ProxyTestContext {
            pic,
            proxy_canister,
            ..
        } = setup();
        let mut rng = rand::thread_rng();

        let signer_sk = SigningKey::from_bytes(&rng.gen());
        let signer_pk = signer_sk.verifying_key();
        let signer_id = ICSignerId::new(signer_pk.to_bytes());

        let proposal = ICProposal {
            id: ICProposalId::new([0; 32]),
            author_id: signer_id.clone(),
            actions: vec![ICProposalAction::SetNumApprovals { num_approvals: 2 }],
        };

        let result =
            create_and_verify_proposal(&pic, proxy_canister, &signer_sk, &signer_id, proposal)
                .expect("Proposal creation should succeed");

        assert_eq!(result.proposal_id.0, [0; 32]);
    }

    #[test]
    fn test_create_proposal_set_num_approvals() {
        let ProxyTestContext {
            pic,
            proxy_canister,
            ..
        } = setup();
        let mut rng = rand::thread_rng();

        let signer_sk = SigningKey::from_bytes(&rng.gen());
        let signer_pk = signer_sk.verifying_key();
        let signer_id = ICSignerId::new(signer_pk.to_bytes());

        let proposal = ICProposal {
            id: ICProposalId::new([0; 32]),
            author_id: signer_id.clone(),
            actions: vec![ICProposalAction::SetNumApprovals { num_approvals: 2 }],
        };

        create_and_verify_proposal(&pic, proxy_canister, &signer_sk, &signer_id, proposal)
            .expect("Setting num approvals should succeed");
    }

    #[test]
    fn test_create_proposal_transfer() {
        let ProxyTestContext {
            pic,
            proxy_canister,
            ..
        } = setup();
        let mut rng = rand::thread_rng();

        let signer_sk = SigningKey::from_bytes(&rng.gen());
        let signer_pk = signer_sk.verifying_key();
        let signer_id = ICSignerId::new(signer_pk.to_bytes());

        let proposal = ICProposal {
            id: ICProposalId::new([1; 32]),
            author_id: signer_id.clone(),
            actions: vec![ICProposalAction::Transfer {
                receiver_id: Principal::anonymous(),
                amount: 1000000,
            }],
        };

        create_and_verify_proposal(&pic, proxy_canister, &signer_sk, &signer_id, proposal)
            .expect("Transfer proposal creation should succeed");
    }

    #[test]
    fn test_create_proposal_external_call() {
        let ProxyTestContext {
            pic,
            proxy_canister,
            ..
        } = setup();
        let mut rng = rand::thread_rng();

        let signer_sk = SigningKey::from_bytes(&rng.gen());
        let signer_pk = signer_sk.verifying_key();
        let signer_id = ICSignerId::new(signer_pk.to_bytes());

        let proposal = ICProposal {
            id: ICProposalId::new([3; 32]),
            author_id: signer_id.clone(),
            actions: vec![ICProposalAction::ExternalFunctionCall {
                receiver_id: Principal::anonymous(),
                method_name: "test_method".to_string(),
                args: "deadbeef".to_string(),
                deposit: 0,
            }],
        };

        create_and_verify_proposal(&pic, proxy_canister, &signer_sk, &signer_id, proposal)
            .expect("External call proposal creation should succeed");
    }

    #[test]
    fn test_create_proposal_set_context() {
        let ProxyTestContext {
            pic,
            proxy_canister,
            ..
        } = setup();
        let mut rng = rand::thread_rng();

        let signer_sk = SigningKey::from_bytes(&rng.gen());
        let signer_pk = signer_sk.verifying_key();
        let signer_id = ICSignerId::new(signer_pk.to_bytes());

        let proposal = ICProposal {
            id: ICProposalId::new([5; 32]),
            author_id: signer_id.clone(),
            actions: vec![ICProposalAction::SetContextValue {
                key: vec![1, 2, 3],
                value: vec![4, 5, 6],
            }],
        };

        create_and_verify_proposal(&pic, proxy_canister, &signer_sk, &signer_id, proposal)
            .expect("Setting context value should succeed");
    }

    #[test]
    fn test_create_proposal_multiple_actions() {
        let ProxyTestContext {
            pic,
            proxy_canister,
            ..
        } = setup();
        let mut rng = rand::thread_rng();

        let signer_sk = SigningKey::from_bytes(&rng.gen());
        let signer_pk = signer_sk.verifying_key();
        let signer_id = ICSignerId::new(signer_pk.to_bytes());

        let proposal = ICProposal {
            id: ICProposalId::new([6; 32]),
            author_id: signer_id.clone(),
            actions: vec![
                ICProposalAction::SetNumApprovals { num_approvals: 2 },
                ICProposalAction::SetActiveProposalsLimit {
                    active_proposals_limit: 5,
                },
            ],
        };

        create_and_verify_proposal(&pic, proxy_canister, &signer_sk, &signer_id, proposal)
            .expect("Multiple actions proposal creation should succeed");
    }

    #[test]
    fn test_create_proposal_invalid_transfer_amount() {
        let ProxyTestContext {
            pic,
            proxy_canister,
            ..
        } = setup();
        let mut rng = rand::thread_rng();

        let signer_sk = SigningKey::from_bytes(&rng.gen());
        let signer_pk = signer_sk.verifying_key();
        let signer_id = ICSignerId::new(signer_pk.to_bytes());

        let proposal = ICProposal {
            id: ICProposalId::new([8; 32]),
            author_id: signer_id.clone(),
            actions: vec![ICProposalAction::Transfer {
                receiver_id: Principal::anonymous(),
                amount: 0, // Invalid amount
            }],
        };

        let request = ICRequest {
            signer_id: signer_id.clone(),
            timestamp_ms: get_time_nanos(&pic),
            kind: ICRequestKind::Propose { proposal },
        };

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
        let ProxyTestContext {
            pic,
            proxy_canister,
            ..
        } = setup();
        let mut rng = rand::thread_rng();

        let signer_sk = SigningKey::from_bytes(&rng.gen());
        let signer_pk = signer_sk.verifying_key();
        let signer_id = ICSignerId::new(signer_pk.to_bytes());

        let proposal = ICProposal {
            id: ICProposalId::new([9; 32]),
            author_id: signer_id.clone(),
            actions: vec![ICProposalAction::ExternalFunctionCall {
                receiver_id: Principal::anonymous(),
                method_name: "".to_string(), // Invalid method name
                args: "deadbeef".to_string(),
                deposit: 0,
            }],
        };

        let request = ICRequest {
            signer_id: signer_id.clone(),
            timestamp_ms: get_time_nanos(&pic),
            kind: ICRequestKind::Propose { proposal },
        };

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
        let ProxyTestContext {
            pic,
            proxy_canister,
            ..
        } = setup();
        let mut rng = rand::thread_rng();

        let signer_sk = SigningKey::from_bytes(&rng.gen());
        let signer_pk = signer_sk.verifying_key();
        let signer_id = ICSignerId::new(signer_pk.to_bytes());

        // Create proposal
        let proposal = ICProposal {
            id: ICProposalId::new([10; 32]),
            author_id: signer_id.clone(),
            actions: vec![ICProposalAction::SetNumApprovals { num_approvals: 2 }],
        };

        let _ = create_and_verify_proposal(
            &pic,
            proxy_canister,
            &signer_sk,
            &signer_id,
            proposal.clone(),
        );

        // Try to approve own proposal
        let approval = ICProposalApprovalWithSigner {
            signer_id: signer_id.clone(),
            proposal_id: proposal.id,
            added_timestamp: get_time_nanos(&pic),
        };

        let request = ICRequest {
            signer_id: signer_id.clone(),
            timestamp_ms: get_time_nanos(&pic),
            kind: ICRequestKind::Approve { approval },
        };

        let signed_request = create_signed_request(&signer_sk, request);
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
        let ProxyTestContext {
            pic,
            proxy_canister,
            ..
        } = setup();
        let mut rng = rand::thread_rng();

        let signer_sk = SigningKey::from_bytes(&rng.gen());
        let signer_pk = signer_sk.verifying_key();
        let signer_id = ICSignerId::new(signer_pk.to_bytes());

        let approval = ICProposalApprovalWithSigner {
            signer_id: signer_id.clone(),
            proposal_id: ICProposalId::new([11; 32]),
            added_timestamp: get_time_nanos(&pic),
        };

        let request = ICRequest {
            signer_id: signer_id.clone(),
            timestamp_ms: get_time_nanos(&pic),
            kind: ICRequestKind::Approve { approval },
        };

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
        let ProxyTestContext {
            pic,
            proxy_canister,
            ..
        } = setup();
        let mut rng = rand::thread_rng();

        let signer_sk = SigningKey::from_bytes(&rng.gen());
        let signer_pk = signer_sk.verifying_key();
        let signer_id = ICSignerId::new(signer_pk.to_bytes());

        let proposal = ICProposal {
            id: ICProposalId::new([12; 32]),
            author_id: signer_id.clone(),
            actions: vec![], // Empty actions
        };

        let request = ICRequest {
            signer_id: signer_id.clone(),
            timestamp_ms: get_time_nanos(&pic),
            kind: ICRequestKind::Propose { proposal },
        };

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
        let ProxyTestContext {
            pic,
            proxy_canister,
            ..
        } = setup();
        let mut rng = rand::thread_rng();

        let signer_sk = SigningKey::from_bytes(&rng.gen());
        let signer_pk = signer_sk.verifying_key();
        let signer_id = ICSignerId::new(signer_pk.to_bytes());

        // Create max number of proposals
        for i in 0..10 {
            let proposal = ICProposal {
                id: ICProposalId::new([i as u8; 32]),
                author_id: signer_id.clone(),
                actions: vec![ICProposalAction::SetNumApprovals { num_approvals: 2 }],
            };
            let _ =
                create_and_verify_proposal(&pic, proxy_canister, &signer_sk, &signer_id, proposal);
        }

        // Try to create one more
        let proposal = ICProposal {
            id: ICProposalId::new([11; 32]),
            author_id: signer_id.clone(),
            actions: vec![ICProposalAction::SetNumApprovals { num_approvals: 2 }],
        };

        let request = ICRequest {
            signer_id: signer_id.clone(),
            timestamp_ms: get_time_nanos(&pic),
            kind: ICRequestKind::Propose { proposal },
        };

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
                    "Should not be able to exceed proposal limit"
                );
            }
            _ => panic!("Unexpected response type"),
        }
    }

    #[test]
    fn test_proposal_execution_transfer() {
        let ProxyTestContext {
            pic,
            proxy_canister,
            mock_ledger,
            ..
        } = setup();

        let mut rng = rand::thread_rng();

        let initial_balance = MOCK_LEDGER_BALANCE.with(|b| *b.borrow());

        // Setup signers
        let signer1_sk = SigningKey::from_bytes(&rng.gen());
        let signer1_pk = signer1_sk.verifying_key();
        let signer1_id = ICSignerId::new(signer1_pk.to_bytes());

        let signer2_sk = SigningKey::from_bytes(&rng.gen());
        let signer2_pk = signer2_sk.verifying_key();
        let signer2_id = ICSignerId::new(signer2_pk.to_bytes());

        let signer3_sk = SigningKey::from_bytes(&rng.gen());
        let signer3_pk = signer3_sk.verifying_key();
        let signer3_id = ICSignerId::new(signer3_pk.to_bytes());

        let transfer_amount = 1_000;

        let receiver_id = Principal::from_text("2vxsx-fae").unwrap();
        // Create transfer proposal
        let proposal = ICProposal {
            id: ICProposalId::new([14; 32]),
            author_id: signer1_id.clone(),
            actions: vec![ICProposalAction::Transfer {
                receiver_id,
                amount: transfer_amount,
            }],
        };

        // Create and verify initial proposal
        let _ = create_and_verify_proposal(
            &pic,
            proxy_canister,
            &signer1_sk,
            &signer1_id,
            proposal.clone(),
        );

        // Add approvals to trigger execution
        for (signer_sk, signer_id) in [(signer2_sk, signer2_id), (signer3_sk, signer3_id)] {
            let approval = ICProposalApprovalWithSigner {
                signer_id: signer_id.clone(),
                proposal_id: proposal.id.clone(),
                added_timestamp: get_time_nanos(&pic),
            };

            let request = ICRequest {
                signer_id,
                timestamp_ms: get_time_nanos(&pic),
                kind: ICRequestKind::Approve { approval },
            };

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
                                    candid::encode_one(proposal.id.clone()).unwrap(),
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
                        Err(e) => {
                            if e.contains("No route to canister") {
                                println!("Expected transfer error: {}", e);
                                // Test passed - we got the expected error
                            } else {
                                panic!("Unexpected error: {}", e);
                            }
                        }
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
        let ProxyTestContext {
            pic,
            proxy_canister,
            mock_external,
            ..
        } = setup();

        let mut rng = rand::thread_rng();

        // Setup signers
        let signer1_sk = SigningKey::from_bytes(&rng.gen());
        let signer1_pk = signer1_sk.verifying_key();
        let signer1_id = ICSignerId::new(signer1_pk.to_bytes());

        let signer2_sk = SigningKey::from_bytes(&rng.gen());
        let signer2_pk = signer2_sk.verifying_key();
        let signer2_id = ICSignerId::new(signer2_pk.to_bytes());

        let signer3_sk = SigningKey::from_bytes(&rng.gen());
        let signer3_pk = signer3_sk.verifying_key();
        let signer3_id = ICSignerId::new(signer3_pk.to_bytes());

        // Create external call proposal
        let test_args = "01020304".to_string(); // Test arguments as string
        let proposal = ICProposal {
            id: ICProposalId::new([14; 32]),
            author_id: signer1_id.clone(),
            actions: vec![ICProposalAction::ExternalFunctionCall {
                receiver_id: mock_external,
                method_name: "test_method".to_string(),
                args: test_args.clone(),
                deposit: 0,
            }],
        };

        // Create and verify initial proposal
        let _ = create_and_verify_proposal(
            &pic,
            proxy_canister,
            &signer1_sk,
            &signer1_id,
            proposal.clone(),
        );

        // Add approvals to trigger execution
        for (signer_sk, signer_id) in [(signer2_sk, signer2_id), (signer3_sk, signer3_id)] {
            let approval = ICProposalApprovalWithSigner {
                signer_id: signer_id.clone(),
                proposal_id: proposal.id.clone(),
                added_timestamp: get_time_nanos(&pic),
            };

            let request = ICRequest {
                signer_id,
                timestamp_ms: get_time_nanos(&pic),
                kind: ICRequestKind::Approve { approval },
            };

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
                                    candid::encode_one(proposal.id.clone()).unwrap(),
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
                let calls: Vec<Vec<u8>> =
                    candid::decode_one(&bytes).expect("Failed to decode calls");
                assert_eq!(calls.len(), 1, "Should have exactly one call");

                // Decode the Candid-encoded argument
                let received_args: String =
                    candid::decode_one(&calls[0]).expect("Failed to decode call arguments");
                assert_eq!(received_args, test_args, "Call arguments should match");
            }
            _ => panic!("Unexpected response type"),
        }
    }
}
