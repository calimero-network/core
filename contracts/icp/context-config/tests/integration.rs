use std::time::{Duration, SystemTime, UNIX_EPOCH};

use calimero_context_config::icp::repr::ICRepr;
use calimero_context_config::icp::types::{
    ICApplication, ICCapability, ICContextRequest, ICContextRequestKind, ICRequest, ICRequestKind,
    ICSigned,
};
use calimero_context_config::repr::{ReprBytes, ReprTransmute};
use calimero_context_config::types::{ContextIdentity, SignerId};
use candid::Principal;
use ed25519_dalek::{Signer, SigningKey};
use pocket_ic::{PocketIc, UserError, WasmResult};
use rand::Rng;

fn setup() -> (PocketIc, Principal) {
    let pic = PocketIc::new();
    let wasm = std::fs::read("res/calimero_context_config_icp.wasm").expect("failed to read wasm");
    let canister = pic.create_canister();
    pic.add_cycles(canister, 1_000_000_000_000_000);
    pic.install_canister(
        canister,
        wasm,
        vec![],
        None, // No controller
    );

    // Set the proxy code
    let proxy_code = std::fs::read("../context-proxy/res/calimero_context_proxy_icp.wasm")
        .expect("failed to read proxy wasm");
    let ledger_id = Principal::from_text("ryjl3-tyaaa-aaaaa-aaaba-cai").unwrap();
    pic.update_call(
        canister,
        Principal::anonymous(),
        "set_proxy_code",
        candid::encode_args((proxy_code, ledger_id)).unwrap(),
    )
    .expect("Failed to set proxy code");

    (pic, canister)
}

fn create_signed_request(signer_key: &SigningKey, request: ICRequest) -> ICSigned<ICRequest> {
    ICSigned::new(request, |bytes| signer_key.sign(bytes)).expect("Failed to create signed request")
}

fn get_time_nanos(pic: &PocketIc) -> u64 {
    pic.get_time()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_nanos() as u64
}

fn handle_response(
    response: Result<WasmResult, UserError>,
    expected_success: bool,
    operation_name: &str,
) {
    match response {
        Ok(WasmResult::Reply(bytes)) => {
            let result: Result<(), String> = candid::decode_one(&bytes).unwrap_or_else(|e| {
                panic!("Failed to decode response for {}: {}", operation_name, e)
            });

            match (result, expected_success) {
                (Ok(_), true) => println!("{} succeeded as expected", operation_name),
                (Ok(_), false) => panic!("{} succeeded when it should have failed", operation_name),
                (Err(e), true) => panic!(
                    "{} failed when it should have succeeded: {}",
                    operation_name, e
                ),
                (Err(e), false) => println!("{} failed as expected: {}", operation_name, e),
            }
        }
        Ok(WasmResult::Reject(msg)) => {
            if expected_success {
                panic!("{}: Unexpected canister rejection: {}", operation_name, msg);
            } else {
                println!("{}: Expected canister rejection: {}", operation_name, msg);
            }
        }
        Err(e) => panic!("{}: Call failed: {:?}", operation_name, e),
    }
}

#[test]
fn test_proxy_management() {
    let (pic, canister) = setup();
    let mut rng = rand::thread_rng();

    // Advance IC time
    let current_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    pic.advance_time(Duration::from_nanos(current_nanos));

    // Create test identities
    let context_sk = SigningKey::from_bytes(&rng.gen());
    let context_pk = context_sk.verifying_key();
    let context_id = context_pk.rt().expect("infallible conversion");

    let alice_sk = SigningKey::from_bytes(&rng.gen());
    let alice_pk = alice_sk.verifying_key();
    let alice_id = alice_pk.rt().expect("infallible conversion");

    // Create context with initial application
    let create_request = ICRequest {
        kind: ICRequestKind::Context(ICContextRequest {
            context_id,
            kind: ICContextRequestKind::Add {
                author_id: alice_id,
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
        timestamp_ms: get_time_nanos(&pic),
        nonce: 0,
    };

    let signed_request = create_signed_request(&context_sk, create_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    handle_response(response, true, "mutate");

    // Try to update proxy contract without Proxy capability (should fail)
    let bob_sk = SigningKey::from_bytes(&rng.gen());
    let bob_pk = bob_sk.verifying_key();
    let update_request = ICRequest {
        kind: ICRequestKind::Context(ICContextRequest {
            context_id,
            kind: ICContextRequestKind::UpdateProxyContract,
        }),
        signer_id: bob_pk.rt().expect("infallible conversion"),
        timestamp_ms: get_time_nanos(&pic),
        nonce: 0,
    };

    let signed_request = create_signed_request(&bob_sk, update_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    handle_response(response, false, "mutate");

    // Update proxy contract with proper capability (Alice has it by default)
    let update_request = ICRequest {
        kind: ICRequestKind::Context(ICContextRequest {
            context_id,
            kind: ICContextRequestKind::UpdateProxyContract,
        }),
        signer_id: (alice_pk.to_bytes().rt().expect("infallible conversion")),
        timestamp_ms: get_time_nanos(&pic),
        nonce: 0,
    };

    let signed_request = create_signed_request(&alice_sk, update_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    handle_response(response, true, "mutate");
}

#[test]
fn test_mutate_success_cases() {
    let (pic, canister) = setup();
    let mut rng = rand::thread_rng();

    // Advance IC time to current time
    let current_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    pic.advance_time(Duration::from_nanos(current_nanos));

    // Create context keys and ID
    let context_sk = SigningKey::from_bytes(&rng.gen());
    let context_pk = context_sk.verifying_key();
    let context_id = context_pk.rt().expect("infallible conversion");

    // Get current IC time in nanoseconds
    let current_time = get_time_nanos(&pic);

    // Create the request with IC time in nanoseconds
    let request = ICRequest {
        kind: ICRequestKind::Context(ICContextRequest {
            context_id,
            kind: ICContextRequestKind::Add {
                author_id: rng.gen::<[_; 32]>().rt().expect("infallible conversion"),
                application: ICApplication {
                    id: rng.gen::<[_; 32]>().rt().expect("infallible conversion"),
                    blob: rng.gen::<[_; 32]>().rt().expect("infallible conversion"),
                    size: 0,
                    source: String::new(),
                    metadata: vec![],
                },
            },
        }),
        signer_id: (context_id.as_bytes().rt().expect("infallible conversion")),
        timestamp_ms: current_time,
        nonce: 0,
    };

    let signed_request = create_signed_request(&context_sk, request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    handle_response(response, true, "Context creation");
}

#[test]
fn test_member_management() {
    let (pic, canister) = setup();
    let mut rng = rand::thread_rng();

    // Advance IC time
    let current_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    pic.advance_time(Duration::from_nanos(current_nanos));

    // Create test identities
    let context_sk = SigningKey::from_bytes(&rng.gen());
    let context_pk = context_sk.verifying_key();
    let context_id = context_pk.rt().expect("infallible conversion");

    let alice_sk = SigningKey::from_bytes(&rng.gen());
    let alice_pk = alice_sk.verifying_key();
    let alice_id = alice_pk.rt().expect("infallible conversion");

    let bob_sk = SigningKey::from_bytes(&rng.gen());
    let bob_pk = bob_sk.verifying_key();
    let bob_id = bob_pk.rt().expect("infallible conversion");

    // First create the context with Alice as author
    let create_request = ICRequest {
        kind: ICRequestKind::Context(ICContextRequest {
            context_id,
            kind: ICContextRequestKind::Add {
                author_id: alice_id,
                application: ICApplication {
                    id: rng.gen::<[_; 32]>().rt().expect("infallible conversion"),
                    blob: rng.gen::<[_; 32]>().rt().expect("infallible conversion"),
                    size: 0,
                    source: String::new(),
                    metadata: vec![],
                },
            },
        }),
        signer_id: (context_id.as_bytes().rt().expect("infallible conversion")),
        timestamp_ms: get_time_nanos(&pic),
        nonce: 0,
    };

    let signed_request = create_signed_request(&context_sk, create_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    handle_response(response, true, "Context creation");

    // Add Bob as a member (signed by Alice who has management rights)
    let add_member_request = ICRequest {
        kind: ICRequestKind::Context(ICContextRequest {
            context_id,
            kind: ICContextRequestKind::AddMembers {
                members: vec![bob_id],
            },
        }),
        signer_id: (alice_pk.rt().expect("infallible conversion")),
        timestamp_ms: get_time_nanos(&pic),
        nonce: 0,
    };

    let signed_request = create_signed_request(&alice_sk, add_member_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    handle_response(response, true, "Member addition");

    // Verify members through query call
    let query_response = pic.query_call(
        canister,
        Principal::anonymous(),
        "members",
        candid::encode_args((context_id, 0_usize, 10_usize)).unwrap(),
    );

    match query_response {
        Ok(WasmResult::Reply(bytes)) => {
            let members: Vec<ICRepr<ContextIdentity>> = candid::decode_one(&bytes).unwrap();
            assert_eq!(
                members.len(),
                2,
                "Should have both Alice and Bob as members"
            );
            assert!(members.contains(&alice_id), "Alice should be a member");
            assert!(members.contains(&bob_id), "Bob should be a member");
        }
        Ok(_) => panic!("Unexpected response format"),
        Err(err) => panic!("Failed to query members: {}", err),
    }

    // Try to remove Bob (signed by Alice)
    let remove_member_request = ICRequest {
        kind: ICRequestKind::Context(ICContextRequest {
            context_id,
            kind: ICContextRequestKind::RemoveMembers {
                members: vec![bob_id],
            },
        }),
        signer_id: (alice_pk.rt().expect("infallible conversion")),
        timestamp_ms: get_time_nanos(&pic),
        nonce: 0,
    };

    let signed_request = create_signed_request(&alice_sk, remove_member_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    handle_response(response, true, "Member removal");

    // Verify members again
    let query_response = pic.query_call(
        canister,
        Principal::anonymous(),
        "members",
        candid::encode_args((context_id, 0_usize, 10_usize)).unwrap(),
    );

    if let Ok(WasmResult::Reply(bytes)) = query_response {
        let members: Vec<ICRepr<ContextIdentity>> = candid::decode_one(&bytes).unwrap();
        assert_eq!(members.len(), 1, "Should have one member (Alice)");
        assert!(
            members.contains(&alice_id),
            "Alice should still be a member"
        );
        assert!(!members.contains(&bob_id), "Bob should not be a member");
    } else {
        panic!("Failed to query members");
    }
}

#[test]
fn test_capability_management() {
    let (pic, canister) = setup();
    let mut rng = rand::thread_rng();

    // Advance IC time
    let current_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    pic.advance_time(Duration::from_nanos(current_nanos));

    // Create test identities
    let context_sk = SigningKey::from_bytes(&rng.gen());
    let context_pk = context_sk.verifying_key();
    let context_id = context_pk.rt().expect("infallible conversion");

    let alice_sk = SigningKey::from_bytes(&rng.gen());
    let alice_pk = alice_sk.verifying_key();
    let alice_id = alice_pk.rt().expect("infallible conversion");

    let bob_sk = SigningKey::from_bytes(&rng.gen());
    let bob_pk = bob_sk.verifying_key();
    let bob_id = bob_pk.rt().expect("infallible conversion");

    // First create the context with Alice as author
    let create_request = ICRequest {
        kind: ICRequestKind::Context(ICContextRequest {
            context_id,
            kind: ICContextRequestKind::Add {
                author_id: alice_id,
                application: ICApplication {
                    id: rng.gen::<[_; 32]>().rt().expect("infallible conversion"),
                    blob: rng.gen::<[_; 32]>().rt().expect("infallible conversion"),
                    size: 0,
                    source: String::new(),
                    metadata: vec![],
                },
            },
        }),
        signer_id: (context_id.as_bytes().rt().expect("infallible conversion")),
        timestamp_ms: get_time_nanos(&pic),
        nonce: 0,
    };

    let signed_request = create_signed_request(&context_sk, create_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    handle_response(response, true, "Context creation");

    // Add Bob as a member before granting capabilities
    let add_member_request = ICRequest {
        kind: ICRequestKind::Context(ICContextRequest {
            context_id,
            kind: ICContextRequestKind::AddMembers {
                members: vec![bob_id],
            },
        }),
        signer_id: (alice_pk.to_bytes().rt().expect("infallible conversion")),
        timestamp_ms: get_time_nanos(&pic),
        nonce: 0,
    };

    let signed_request = create_signed_request(&alice_sk, add_member_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    handle_response(response, true, "Member addition");

    // Grant capabilities to Bob
    let grant_request = ICRequest {
        kind: ICRequestKind::Context(ICContextRequest {
            context_id,
            kind: ICContextRequestKind::Grant {
                capabilities: vec![(bob_id, ICCapability::ManageMembers)],
            },
        }),
        signer_id: (alice_pk.to_bytes().rt().expect("infallible conversion")),
        timestamp_ms: get_time_nanos(&pic),
        nonce: 0,
    };

    let signed_request = create_signed_request(&alice_sk, grant_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    handle_response(response, true, "Capability granting");

    // Verify Bob's capabilities
    let query_response = pic.query_call(
        canister,
        Principal::anonymous(),
        "privileges",
        candid::encode_one((context_id, vec![bob_id])).unwrap(),
    );

    if let Ok(WasmResult::Reply(bytes)) = query_response {
        let privileges: std::collections::BTreeMap<ICRepr<SignerId>, Vec<ICCapability>> =
            candid::decode_one(&bytes).unwrap();

        let bob_id: SignerId = bob_pk.to_bytes().rt().expect("infallible conversion");

        let bob_capabilities = privileges
            .get(&bob_id)
            .expect("Bob should have capabilities");
        assert_eq!(bob_capabilities, &[ICCapability::ManageMembers]);
    }

    // Revoke Bob's capabilities
    let revoke_request = ICRequest {
        kind: ICRequestKind::Context(ICContextRequest {
            context_id,
            kind: ICContextRequestKind::Revoke {
                capabilities: vec![(bob_id, ICCapability::ManageMembers)],
            },
        }),
        signer_id: (alice_pk.to_bytes().rt().expect("infallible conversion")),
        timestamp_ms: get_time_nanos(&pic),
        nonce: 0,
    };

    let signed_request = create_signed_request(&alice_sk, revoke_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    handle_response(response, true, "Capability revoking");

    // Verify Bob's capabilities are gone
    let query_response = pic.query_call(
        canister,
        Principal::anonymous(),
        "privileges",
        candid::encode_one((context_id, vec![bob_id])).unwrap(),
    );

    if let Ok(WasmResult::Reply(bytes)) = query_response {
        let privileges: std::collections::BTreeMap<ICRepr<SignerId>, Vec<ICCapability>> =
            candid::decode_one(&bytes).unwrap();

        let bob_id: SignerId = bob_pk.to_bytes().rt().expect("infallible conversion");

        assert!(privileges.get(&bob_id).is_none() || privileges.get(&bob_id).unwrap().is_empty());
    }
}

#[test]
fn test_application_update() {
    let (pic, canister) = setup();
    let mut rng = rand::thread_rng();

    // Advance IC time
    let current_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    pic.advance_time(Duration::from_nanos(current_nanos));

    // Create test identities
    let context_sk = SigningKey::from_bytes(&rng.gen());
    let context_pk = context_sk.verifying_key();
    let context_id = context_pk.to_bytes().rt().expect("infallible conversion");

    let alice_sk = SigningKey::from_bytes(&rng.gen());
    let alice_pk = alice_sk.verifying_key();
    let alice_id = alice_pk.to_bytes().rt().expect("infallible conversion");

    let bob_sk = SigningKey::from_bytes(&rng.gen());
    let bob_pk = bob_sk.verifying_key();
    // let bob_id = ICRepr<ContextIdentity>::new(bob_pk.to_bytes());

    // Initial application IDs
    let initial_app_id = rng.gen::<[_; 32]>().rt().expect("infallible conversion");
    let initial_blob_id = rng.gen::<[_; 32]>().rt().expect("infallible conversion");

    // Create context with initial application
    let create_request = ICRequest {
        kind: ICRequestKind::Context(ICContextRequest {
            context_id,
            kind: ICContextRequestKind::Add {
                author_id: alice_id,
                application: ICApplication {
                    id: initial_app_id,
                    blob: initial_blob_id,
                    size: 0,
                    source: String::new(),
                    metadata: vec![],
                },
            },
        }),
        signer_id: (context_id.as_bytes().rt().expect("infallible conversion")),
        timestamp_ms: get_time_nanos(&pic),
        nonce: 0,
    };

    let signed_request = create_signed_request(&context_sk, create_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    handle_response(response, true, "Context creation");

    // Verify initial application state
    let query_response = pic.query_call(
        canister,
        Principal::anonymous(),
        "application",
        candid::encode_one(context_id).unwrap(),
    );

    if let Ok(WasmResult::Reply(bytes)) = query_response {
        let app: ICApplication = candid::decode_one(&bytes).unwrap();
        assert_eq!(app.id, initial_app_id);
        assert_eq!(app.blob, initial_blob_id);
    }

    // Check initial application revision
    let query_response = pic.query_call(
        canister,
        Principal::anonymous(),
        "application_revision",
        candid::encode_one(context_id).unwrap(),
    );

    if let Ok(WasmResult::Reply(bytes)) = query_response {
        let revision: u64 = candid::decode_one(&bytes).unwrap();
        assert_eq!(revision, 0, "Initial application revision should be 0");
    }

    // Try unauthorized application update (Bob)
    let new_app_id = rng.gen::<[_; 32]>().rt().expect("infallible conversion");
    let new_blob_id = rng.gen::<[_; 32]>().rt().expect("infallible conversion");

    let update_request = ICRequest {
        kind: ICRequestKind::Context(ICContextRequest {
            context_id,
            kind: ICContextRequestKind::UpdateApplication {
                application: ICApplication {
                    id: new_app_id,
                    blob: new_blob_id,
                    size: 0,
                    source: String::new(),
                    metadata: vec![],
                },
            },
        }),
        signer_id: (bob_pk.to_bytes().rt().expect("infallible conversion")),
        timestamp_ms: get_time_nanos(&pic),
        nonce: 0,
    };

    let signed_request = create_signed_request(&bob_sk, update_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );

    match response {
        Ok(WasmResult::Reply(bytes)) => {
            let result: Result<(), String> = candid::decode_one(&bytes).unwrap();
            assert!(
                result.is_err(),
                "Unauthorized application update should fail"
            );
        }
        Ok(_) => panic!("Expected Reply variant"),
        Err(err) => panic!("Unexpected error: {}", err),
    }

    // Verify application hasn't changed
    let query_response = pic.query_call(
        canister,
        Principal::anonymous(),
        "application",
        candid::encode_one(context_id).unwrap(),
    );

    if let Ok(WasmResult::Reply(bytes)) = query_response {
        let app: ICApplication = candid::decode_one(&bytes).unwrap();
        assert_eq!(app.id, initial_app_id);
        assert_eq!(app.blob, initial_blob_id);
    }

    // Authorized application update (Alice)
    let update_request = ICRequest {
        kind: ICRequestKind::Context(ICContextRequest {
            context_id,
            kind: ICContextRequestKind::UpdateApplication {
                application: ICApplication {
                    id: new_app_id,
                    blob: new_blob_id,
                    size: 0,
                    source: String::new(),
                    metadata: vec![],
                },
            },
        }),
        signer_id: (alice_pk.to_bytes().rt().expect("infallible conversion")),
        timestamp_ms: get_time_nanos(&pic),
        nonce: 0,
    };

    let signed_request = create_signed_request(&alice_sk, update_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    handle_response(response, true, "Authorized update");

    // Verify application has been updated
    let query_response = pic.query_call(
        canister,
        Principal::anonymous(),
        "application",
        candid::encode_one(context_id).unwrap(),
    );

    if let Ok(WasmResult::Reply(bytes)) = query_response {
        let app: ICApplication = candid::decode_one(&bytes).unwrap();
        assert_eq!(app.id, new_app_id);
        assert_eq!(app.blob, new_blob_id);
    }

    // Check final application revision
    let query_response = pic.query_call(
        canister,
        Principal::anonymous(),
        "application_revision",
        candid::encode_one(context_id).unwrap(),
    );

    if let Ok(WasmResult::Reply(bytes)) = query_response {
        let revision: u64 = candid::decode_one(&bytes).unwrap();
        assert_eq!(revision, 1, "Application revision should be 1 after update");
    }
}

#[test]
fn test_edge_cases() {
    let (pic, canister) = setup();
    let mut rng = rand::thread_rng();

    // Setup context and identities
    let context_sk = SigningKey::from_bytes(&rng.gen());
    let context_pk = context_sk.verifying_key();
    let context_id = context_pk.to_bytes().rt().expect("infallible conversion");
    let alice_sk = SigningKey::from_bytes(&rng.gen());
    let alice_pk = alice_sk.verifying_key();
    let alice_id = alice_pk.to_bytes().rt().expect("infallible conversion");

    // Create initial context
    let create_request = ICRequest {
        kind: ICRequestKind::Context(ICContextRequest {
            context_id,
            kind: ICContextRequestKind::Add {
                author_id: alice_id,
                application: ICApplication {
                    id: rng.gen::<[_; 32]>().rt().expect("infallible conversion"),
                    blob: rng.gen::<[_; 32]>().rt().expect("infallible conversion"),
                    size: 0,
                    source: String::new(),
                    metadata: vec![],
                },
            },
        }),
        signer_id: (context_id.as_bytes().rt().expect("infallible conversion")),
        timestamp_ms: get_time_nanos(&pic),
        nonce: 0,
    };

    let signed_request = create_signed_request(&context_sk, create_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    handle_response(response, true, "Context creation");

    // Test 1: Adding empty member list
    let add_empty_members = ICRequest {
        kind: ICRequestKind::Context(ICContextRequest {
            context_id,
            kind: ICContextRequestKind::AddMembers { members: vec![] },
        }),
        signer_id: (alice_pk.to_bytes().rt().expect("infallible conversion")),
        timestamp_ms: get_time_nanos(&pic),
        nonce: 0,
    };

    let signed_request = create_signed_request(&alice_sk, add_empty_members);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    handle_response(response, true, "Empty member list addition");

    // Test 2: Adding duplicate members
    let bob_id = rng.gen::<[_; 32]>().rt().expect("infallible conversion");
    let add_duplicate_members = ICRequest {
        kind: ICRequestKind::Context(ICContextRequest {
            context_id,
            kind: ICContextRequestKind::AddMembers {
                members: vec![bob_id, bob_id],
            },
        }),
        signer_id: (alice_pk.to_bytes().rt().expect("infallible conversion")),
        timestamp_ms: get_time_nanos(&pic),
        nonce: 0,
    };

    let signed_request = create_signed_request(&alice_sk, add_duplicate_members);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    handle_response(response, true, "Duplicate member addition");

    // Verify only one instance was added
    let query_response = pic.query_call(
        canister,
        Principal::anonymous(),
        "members",
        candid::encode_one((context_id, 0_usize, 10_usize)).unwrap(),
    );

    if let Ok(WasmResult::Reply(bytes)) = query_response {
        let members: Vec<ICRepr<ContextIdentity>> = candid::decode_one(&bytes).unwrap();
        assert_eq!(
            members.iter().filter(|&m| m == &bob_id).count(),
            1,
            "Member should only appear once"
        );
    }
}

#[ignore = "we're deprecating timestamp checks, in favor of nonce checks"]
#[test]
fn test_timestamp_scenarios() {
    let (pic, canister) = setup();
    let mut rng = rand::thread_rng();

    // Setup initial context
    let context_sk = SigningKey::from_bytes(&rng.gen());
    let context_pk = context_sk.verifying_key();
    let context_id = context_pk.to_bytes().rt().expect("infallible conversion");
    let alice_sk = SigningKey::from_bytes(&rng.gen());
    let alice_pk = alice_sk.verifying_key();
    let alice_id = alice_pk.to_bytes().rt().expect("infallible conversion");

    // Create initial context with current timestamp
    let current_time = get_time_nanos(&pic);
    let create_request = ICRequest {
        kind: ICRequestKind::Context(ICContextRequest {
            context_id,
            kind: ICContextRequestKind::Add {
                author_id: alice_id,
                application: ICApplication {
                    id: rng.gen::<[_; 32]>().rt().expect("infallible conversion"),
                    blob: rng.gen::<[_; 32]>().rt().expect("infallible conversion"),
                    size: 0,
                    source: String::new(),
                    metadata: vec![],
                },
            },
        }),
        signer_id: (context_id.as_bytes().rt().expect("infallible conversion")),
        timestamp_ms: current_time,
        nonce: 0,
    };

    let signed_request = create_signed_request(&context_sk, create_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    handle_response(response, true, "Context creation");

    // Try with expired timestamp (more than 5 seconds old)
    let expired_request = ICRequest {
        kind: ICRequestKind::Context(ICContextRequest {
            context_id,
            kind: ICContextRequestKind::AddMembers {
                members: vec![rng.gen::<[_; 32]>().rt().expect("infallible conversion")],
            },
        }),
        signer_id: (alice_pk.to_bytes().rt().expect("infallible conversion")),
        timestamp_ms: current_time - 6_000_000_000, // 6 seconds ago
        nonce: 0,
    };

    let signed_request = create_signed_request(&alice_sk, expired_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    handle_response(response, false, "Expired timestamp request");
}

#[test]
fn test_concurrent_operations() {
    let (pic, canister) = setup();
    let mut rng = rand::thread_rng();

    // Setup initial context
    let context_sk = SigningKey::from_bytes(&rng.gen());
    let context_pk = context_sk.verifying_key();
    let context_id = context_pk.to_bytes().rt().expect("infallible conversion");
    let alice_sk = SigningKey::from_bytes(&rng.gen());
    let alice_pk = alice_sk.verifying_key();
    let alice_id = alice_pk.to_bytes().rt().expect("infallible conversion");

    // Create initial context
    let create_request = ICRequest {
        kind: ICRequestKind::Context(ICContextRequest {
            context_id,
            kind: ICContextRequestKind::Add {
                author_id: alice_id,
                application: ICApplication {
                    id: rng.gen::<[_; 32]>().rt().expect("infallible conversion"),
                    blob: rng.gen::<[_; 32]>().rt().expect("infallible conversion"),
                    size: 0,
                    source: String::new(),
                    metadata: vec![],
                },
            },
        }),
        signer_id: (context_id.as_bytes().rt().expect("infallible conversion")),
        timestamp_ms: get_time_nanos(&pic),
        nonce: 0,
    };

    let signed_request = create_signed_request(&context_sk, create_request);
    pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    )
    .expect("Context creation should succeed");

    // Create multiple member additions with same timestamp
    let timestamp = get_time_nanos(&pic);
    let mut requests = Vec::new();
    for _ in 0..3 {
        let new_member = rng.gen::<[_; 32]>().rt().expect("infallible conversion");
        let request = ICRequest {
            kind: ICRequestKind::Context(ICContextRequest {
                context_id,
                kind: ICContextRequestKind::AddMembers {
                    members: vec![new_member],
                },
            }),
            signer_id: (alice_pk.to_bytes().rt().expect("infallible conversion")),
            timestamp_ms: timestamp,
            nonce: 0,
        };
        requests.push(create_signed_request(&alice_sk, request));
    }

    // Submit requests "concurrently"
    let responses: Vec<_> = requests
        .into_iter()
        .map(|signed_request| {
            pic.update_call(
                canister,
                Principal::anonymous(),
                "mutate",
                candid::encode_one(signed_request).unwrap(),
            )
        })
        .collect();

    // Verify all operations succeeded
    assert!(
        responses.iter().all(|r| r.is_ok()),
        "All concurrent operations should succeed"
    );

    // Verify final state
    let query_response = pic.query_call(
        canister,
        Principal::anonymous(),
        "members",
        candid::encode_one((context_id, 0_usize, 10_usize)).unwrap(),
    );

    if let Ok(WasmResult::Reply(bytes)) = query_response {
        let members: Vec<ICRepr<ContextIdentity>> = candid::decode_one(&bytes).unwrap();
        assert_eq!(
            members.len(),
            4,
            "Should have all members (Alice + 3 new members)"
        );
    }
}
