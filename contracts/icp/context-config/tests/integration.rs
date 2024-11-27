use std::time::{Duration, SystemTime, UNIX_EPOCH};

use candid::Principal;
use context_contract::types::{
    ContextRequest, ContextRequestKind, ICApplication, ICApplicationId, ICBlobId, ICCapability,
    ICContextId, ICContextIdentity, ICPSigned, ICSignerId, Request, RequestKind,
};
use ed25519_dalek::{Signer, SigningKey};
use pocket_ic::{PocketIc, WasmResult};
use rand::Rng;

fn setup() -> (PocketIc, Principal) {
    let pic = PocketIc::new();
    let wasm = std::fs::read(".dfx/local/canisters/context_contract/context_contract.wasm")
        .expect("failed to read wasm");
    let canister = pic.create_canister();
    pic.add_cycles(canister, 2_000_000_000_000);
    pic.install_canister(
        canister,
        wasm,
        vec![],
        None, // No controller
    );
    (pic, canister)
}

fn create_signed_request(signer_key: &SigningKey, request: Request) -> ICPSigned<Request> {
    // Serialize the request using candid (same as in verification)
    let message = candid::encode_one(&request).expect("Failed to serialize request");

    // Sign the serialized message
    let signature = signer_key.sign(&message);

    ICPSigned {
        payload: request,
        signature: signature.to_vec(),
    }
}

fn get_time_nanos(pic: &PocketIc) -> u64 {
    pic.get_time()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_nanos() as u64
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
    let context_id = ICContextId::new(context_pk.to_bytes());

    // Get current IC time in nanoseconds
    let current_time = get_time_nanos(&pic);
    println!("Current IC time (nanos): {}", current_time);

    // Create the request with IC time in nanoseconds
    let request = Request {
        kind: RequestKind::Context(ContextRequest {
            context_id: context_id.clone(),
            kind: ContextRequestKind::Add {
                author_id: ICContextIdentity::new(rng.gen()),
                application: ICApplication {
                    id: ICApplicationId::new(rng.gen()),
                    blob: ICBlobId::new(rng.gen()),
                    size: 0,
                    source: String::new(),
                    metadata: vec![],
                },
            },
        }),
        signer_id: ICSignerId::new(context_id.0.clone()),
        timestamp_ms: current_time, // Now using nanoseconds
    };

    let signed_request = create_signed_request(&context_sk, request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );

    println!("Raw response: {:?}", response);
    if let Ok(WasmResult::Reply(bytes)) = &response {
        let decoded: Result<(), String> = candid::decode_one(bytes).unwrap();
        println!("Decoded response: {:?}", decoded);
    }

    assert!(response.is_ok(), "Context addition should succeed");
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
    let context_id = ICContextId::new(context_pk.to_bytes());

    let alice_sk = SigningKey::from_bytes(&rng.gen());
    let alice_pk = alice_sk.verifying_key();
    let alice_id = ICContextIdentity::new(alice_pk.to_bytes());

    let bob_sk = SigningKey::from_bytes(&rng.gen());
    let bob_pk = bob_sk.verifying_key();
    let bob_id = ICContextIdentity::new(bob_pk.to_bytes());

    // First create the context with Alice as author
    let create_request = Request {
        kind: RequestKind::Context(ContextRequest {
            context_id: context_id.clone(),
            kind: ContextRequestKind::Add {
                author_id: alice_id.clone(),
                application: ICApplication {
                    id: ICApplicationId::new(rng.gen()),
                    blob: ICBlobId::new(rng.gen()),
                    size: 0,
                    source: String::new(),
                    metadata: vec![],
                },
            },
        }),
        signer_id: ICSignerId::new(context_id.0.clone()),
        timestamp_ms: get_time_nanos(&pic),
    };

    let signed_request = create_signed_request(&context_sk, create_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    assert!(response.is_ok(), "Context creation should succeed");

    // Add Bob as a member (signed by Alice who has management rights)
    let add_member_request = Request {
        kind: RequestKind::Context(ContextRequest {
            context_id: context_id.clone(),
            kind: ContextRequestKind::AddMembers {
                members: vec![bob_id.clone()],
            },
        }),
        signer_id: ICSignerId::new(alice_pk.to_bytes()),
        timestamp_ms: get_time_nanos(&pic),
    };

    let signed_request = create_signed_request(&alice_sk, add_member_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    assert!(response.is_ok(), "Adding member should succeed");

    // Verify members through query call
    let query_response = pic.query_call(
        canister,
        Principal::anonymous(),
        "members",
        candid::encode_args((context_id.clone(), 0_usize, 10_usize)).unwrap(),
    );

    match query_response {
        Ok(WasmResult::Reply(bytes)) => {
            let members: Vec<ICContextIdentity> = candid::decode_one(&bytes).unwrap();
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
    let remove_member_request = Request {
        kind: RequestKind::Context(ContextRequest {
            context_id: context_id.clone(),
            kind: ContextRequestKind::RemoveMembers {
                members: vec![bob_id.clone()],
            },
        }),
        signer_id: ICSignerId::new(alice_pk.to_bytes()),
        timestamp_ms: get_time_nanos(&pic),
    };

    let signed_request = create_signed_request(&alice_sk, remove_member_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    assert!(response.is_ok(), "Removing member should succeed");

    // Verify members again
    let query_response = pic.query_call(
        canister,
        Principal::anonymous(),
        "members",
        candid::encode_args((context_id.clone(), 0_usize, 10_usize)).unwrap(),
    );

    if let Ok(WasmResult::Reply(bytes)) = query_response {
        let members: Vec<ICContextIdentity> = candid::decode_one(&bytes).unwrap();
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
    let context_id = ICContextId::new(context_pk.to_bytes());

    let alice_sk = SigningKey::from_bytes(&rng.gen());
    let alice_pk = alice_sk.verifying_key();
    let alice_id = ICContextIdentity::new(alice_pk.to_bytes());

    let bob_sk = SigningKey::from_bytes(&rng.gen());
    let bob_pk = bob_sk.verifying_key();
    let bob_id = ICContextIdentity::new(bob_pk.to_bytes());

    // First create the context with Alice as author
    let create_request = Request {
        kind: RequestKind::Context(ContextRequest {
            context_id: context_id.clone(),
            kind: ContextRequestKind::Add {
                author_id: alice_id.clone(),
                application: ICApplication {
                    id: ICApplicationId::new(rng.gen()),
                    blob: ICBlobId::new(rng.gen()),
                    size: 0,
                    source: String::new(),
                    metadata: vec![],
                },
            },
        }),
        signer_id: ICSignerId::new(context_id.0.clone()),
        timestamp_ms: get_time_nanos(&pic),
    };

    let signed_request = create_signed_request(&context_sk, create_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    assert!(response.is_ok(), "Context creation should succeed");

    // Grant capabilities to Bob
    let grant_request = Request {
        kind: RequestKind::Context(ContextRequest {
            context_id: context_id.clone(),
            kind: ContextRequestKind::Grant {
                capabilities: vec![(bob_id.clone(), ICCapability::ManageMembers)],
            },
        }),
        signer_id: ICSignerId::new(alice_pk.to_bytes()),
        timestamp_ms: get_time_nanos(&pic),
    };

    let signed_request = create_signed_request(&alice_sk, grant_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    assert!(response.is_ok(), "Granting capability should succeed");

    // Verify Bob's capabilities
    let query_response = pic.query_call(
        canister,
        Principal::anonymous(),
        "privileges",
        candid::encode_one((context_id.clone(), vec![bob_id.clone()])).unwrap(),
    );

    if let Ok(WasmResult::Reply(bytes)) = query_response {
        let privileges: std::collections::BTreeMap<ICSignerId, Vec<ICCapability>> =
            candid::decode_one(&bytes).unwrap();
        let bob_capabilities = privileges
            .get(&ICSignerId::new(bob_pk.to_bytes()))
            .expect("Bob should have capabilities");
        assert_eq!(bob_capabilities, &[ICCapability::ManageMembers]);
    }

    // Revoke Bob's capabilities
    let revoke_request = Request {
        kind: RequestKind::Context(ContextRequest {
            context_id: context_id.clone(),
            kind: ContextRequestKind::Revoke {
                capabilities: vec![(bob_id.clone(), ICCapability::ManageMembers)],
            },
        }),
        signer_id: ICSignerId::new(alice_pk.to_bytes()),
        timestamp_ms: get_time_nanos(&pic),
    };

    let signed_request = create_signed_request(&alice_sk, revoke_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    assert!(response.is_ok(), "Revoking capability should succeed");

    // Verify Bob's capabilities are gone
    let query_response = pic.query_call(
        canister,
        Principal::anonymous(),
        "privileges",
        candid::encode_one((context_id.clone(), vec![bob_id.clone()])).unwrap(),
    );

    if let Ok(WasmResult::Reply(bytes)) = query_response {
        let privileges: std::collections::BTreeMap<ICSignerId, Vec<ICCapability>> =
            candid::decode_one(&bytes).unwrap();
        assert!(
            privileges
                .get(&ICSignerId::new(bob_pk.to_bytes()))
                .is_none()
                || privileges
                    .get(&ICSignerId::new(bob_pk.to_bytes()))
                    .unwrap()
                    .is_empty()
        );
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
    let context_id = ICContextId::new(context_pk.to_bytes());

    let alice_sk = SigningKey::from_bytes(&rng.gen());
    let alice_pk = alice_sk.verifying_key();
    let alice_id = ICContextIdentity::new(alice_pk.to_bytes());

    let bob_sk = SigningKey::from_bytes(&rng.gen());
    let bob_pk = bob_sk.verifying_key();
    // let bob_id = ICContextIdentity::new(bob_pk.to_bytes());

    // Initial application IDs
    let initial_app_id = ICApplicationId::new(rng.gen());
    let initial_blob_id = ICBlobId::new(rng.gen());

    // Create context with initial application
    let create_request = Request {
        kind: RequestKind::Context(ContextRequest {
            context_id: context_id.clone(),
            kind: ContextRequestKind::Add {
                author_id: alice_id.clone(),
                application: ICApplication {
                    id: initial_app_id.clone(),
                    blob: initial_blob_id.clone(),
                    size: 0,
                    source: String::new(),
                    metadata: vec![],
                },
            },
        }),
        signer_id: ICSignerId::new(context_id.0.clone()),
        timestamp_ms: get_time_nanos(&pic),
    };

    let signed_request = create_signed_request(&context_sk, create_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    assert!(response.is_ok(), "Context creation should succeed");

    // Verify initial application state
    let query_response = pic.query_call(
        canister,
        Principal::anonymous(),
        "application",
        candid::encode_one(context_id.clone()).unwrap(),
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
        candid::encode_one(context_id.clone()).unwrap(),
    );

    if let Ok(WasmResult::Reply(bytes)) = query_response {
        let revision: u64 = candid::decode_one(&bytes).unwrap();
        assert_eq!(revision, 0, "Initial application revision should be 0");
    }

    // Try unauthorized application update (Bob)
    let new_app_id = ICApplicationId::new(rng.gen());
    let new_blob_id = ICBlobId::new(rng.gen());

    let update_request = Request {
        kind: RequestKind::Context(ContextRequest {
            context_id: context_id.clone(),
            kind: ContextRequestKind::UpdateApplication {
                application: ICApplication {
                    id: new_app_id.clone(),
                    blob: new_blob_id.clone(),
                    size: 0,
                    source: String::new(),
                    metadata: vec![],
                },
            },
        }),
        signer_id: ICSignerId::new(bob_pk.to_bytes()),
        timestamp_ms: get_time_nanos(&pic),
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
        candid::encode_one(context_id.clone()).unwrap(),
    );

    if let Ok(WasmResult::Reply(bytes)) = query_response {
        let app: ICApplication = candid::decode_one(&bytes).unwrap();
        assert_eq!(app.id, initial_app_id);
        assert_eq!(app.blob, initial_blob_id);
    }

    // Authorized application update (Alice)
    let update_request = Request {
        kind: RequestKind::Context(ContextRequest {
            context_id: context_id.clone(),
            kind: ContextRequestKind::UpdateApplication {
                application: ICApplication {
                    id: new_app_id.clone(),
                    blob: new_blob_id.clone(),
                    size: 0,
                    source: String::new(),
                    metadata: vec![],
                },
            },
        }),
        signer_id: ICSignerId::new(alice_pk.to_bytes()),
        timestamp_ms: get_time_nanos(&pic),
    };

    let signed_request = create_signed_request(&alice_sk, update_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    assert!(
        response.is_ok(),
        "Authorized application update should succeed"
    );

    // Verify application has been updated
    let query_response = pic.query_call(
        canister,
        Principal::anonymous(),
        "application",
        candid::encode_one(context_id.clone()).unwrap(),
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
        candid::encode_one(context_id.clone()).unwrap(),
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
    let context_id = ICContextId::new(context_pk.to_bytes());
    let alice_sk = SigningKey::from_bytes(&rng.gen());
    let alice_pk = alice_sk.verifying_key();
    let alice_id = ICContextIdentity::new(alice_pk.to_bytes());

    // Create initial context
    let create_request = Request {
        kind: RequestKind::Context(ContextRequest {
            context_id: context_id.clone(),
            kind: ContextRequestKind::Add {
                author_id: alice_id.clone(),
                application: ICApplication {
                    id: ICApplicationId::new(rng.gen()),
                    blob: ICBlobId::new(rng.gen()),
                    size: 0,
                    source: String::new(),
                    metadata: vec![],
                },
            },
        }),
        signer_id: ICSignerId::new(context_id.0.clone()),
        timestamp_ms: get_time_nanos(&pic),
    };

    let signed_request = create_signed_request(&context_sk, create_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    assert!(response.is_ok(), "Context creation should succeed");

    // Test 1: Adding empty member list
    let add_empty_members = Request {
        kind: RequestKind::Context(ContextRequest {
            context_id: context_id.clone(),
            kind: ContextRequestKind::AddMembers { members: vec![] },
        }),
        signer_id: ICSignerId::new(alice_pk.to_bytes()),
        timestamp_ms: get_time_nanos(&pic),
    };

    let signed_request = create_signed_request(&alice_sk, add_empty_members);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    assert!(
        response.is_ok(),
        "Empty member list should be handled gracefully"
    );

    // Test 2: Adding duplicate members
    let bob_id = ICContextIdentity::new(rng.gen());
    let add_duplicate_members = Request {
        kind: RequestKind::Context(ContextRequest {
            context_id: context_id.clone(),
            kind: ContextRequestKind::AddMembers {
                members: vec![bob_id.clone(), bob_id.clone()],
            },
        }),
        signer_id: ICSignerId::new(alice_pk.to_bytes()),
        timestamp_ms: get_time_nanos(&pic),
    };

    let signed_request = create_signed_request(&alice_sk, add_duplicate_members);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    assert!(
        response.is_ok(),
        "Duplicate members should be handled gracefully"
    );

    // Verify only one instance was added
    let query_response = pic.query_call(
        canister,
        Principal::anonymous(),
        "members",
        candid::encode_one((context_id.clone(), 0_usize, 10_usize)).unwrap(),
    );

    if let Ok(WasmResult::Reply(bytes)) = query_response {
        let members: Vec<ICContextIdentity> = candid::decode_one(&bytes).unwrap();
        assert_eq!(
            members.iter().filter(|&m| m == &bob_id).count(),
            1,
            "Member should only appear once"
        );
    }
}

#[test]
fn test_timestamp_scenarios() {
    let (pic, canister) = setup();
    let mut rng = rand::thread_rng();

    // Setup initial context
    let context_sk = SigningKey::from_bytes(&rng.gen());
    let context_pk = context_sk.verifying_key();
    let context_id = ICContextId::new(context_pk.to_bytes());
    let alice_sk = SigningKey::from_bytes(&rng.gen());
    let alice_pk = alice_sk.verifying_key();
    let alice_id = ICContextIdentity::new(alice_pk.to_bytes());

    // Create initial context with current timestamp
    let current_time = get_time_nanos(&pic);
    let create_request = Request {
        kind: RequestKind::Context(ContextRequest {
            context_id: context_id.clone(),
            kind: ContextRequestKind::Add {
                author_id: alice_id.clone(),
                application: ICApplication {
                    id: ICApplicationId::new(rng.gen()),
                    blob: ICBlobId::new(rng.gen()),
                    size: 0,
                    source: String::new(),
                    metadata: vec![],
                },
            },
        }),
        signer_id: ICSignerId::new(context_id.0.clone()),
        timestamp_ms: current_time,
    };

    let signed_request = create_signed_request(&context_sk, create_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );
    assert!(response.is_ok(), "Context creation should succeed");

    // Try with expired timestamp (more than 5 seconds old)
    let expired_request = Request {
        kind: RequestKind::Context(ContextRequest {
            context_id: context_id.clone(),
            kind: ContextRequestKind::AddMembers {
                members: vec![ICContextIdentity::new(rng.gen())],
            },
        }),
        signer_id: ICSignerId::new(alice_pk.to_bytes()),
        timestamp_ms: current_time - 6_000_000_000, // 6 seconds ago
    };

    let signed_request = create_signed_request(&alice_sk, expired_request);
    let response = pic.update_call(
        canister,
        Principal::anonymous(),
        "mutate",
        candid::encode_one(signed_request).unwrap(),
    );

    match response {
        Ok(WasmResult::Reply(bytes)) => {
            let result: Result<(), String> = candid::decode_one(&bytes).unwrap();
            assert!(result.is_err(), "Expired timestamp should be rejected");
            assert!(
                result.unwrap_err().contains("expired"),
                "Should contain 'expired' in error message"
            );
        }
        _ => panic!("Expected error response for expired timestamp"),
    }
}

#[test]
fn test_concurrent_operations() {
    let (pic, canister) = setup();
    let mut rng = rand::thread_rng();

    // Setup initial context
    let context_sk = SigningKey::from_bytes(&rng.gen());
    let context_pk = context_sk.verifying_key();
    let context_id = ICContextId::new(context_pk.to_bytes());
    let alice_sk = SigningKey::from_bytes(&rng.gen());
    let alice_pk = alice_sk.verifying_key();
    let alice_id = ICContextIdentity::new(alice_pk.to_bytes());

    // Create initial context
    let create_request = Request {
        kind: RequestKind::Context(ContextRequest {
            context_id: context_id.clone(),
            kind: ContextRequestKind::Add {
                author_id: alice_id.clone(),
                application: ICApplication {
                    id: ICApplicationId::new(rng.gen()),
                    blob: ICBlobId::new(rng.gen()),
                    size: 0,
                    source: String::new(),
                    metadata: vec![],
                },
            },
        }),
        signer_id: ICSignerId::new(context_id.0.clone()),
        timestamp_ms: get_time_nanos(&pic),
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
        let new_member = ICContextIdentity::new(rng.gen());
        let request = Request {
            kind: RequestKind::Context(ContextRequest {
                context_id: context_id.clone(),
                kind: ContextRequestKind::AddMembers {
                    members: vec![new_member],
                },
            }),
            signer_id: ICSignerId::new(alice_pk.to_bytes()),
            timestamp_ms: timestamp,
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
        candid::encode_one((context_id.clone(), 0_usize, 10_usize)).unwrap(),
    );

    if let Ok(WasmResult::Reply(bytes)) = query_response {
        let members: Vec<ICContextIdentity> = candid::decode_one(&bytes).unwrap();
        assert_eq!(
            members.len(),
            4,
            "Should have all members (Alice + 3 new members)"
        );
    }
}
