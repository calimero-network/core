extern crate alloc;
use alloc::vec::Vec;

use calimero_context_config::stellar::stellar_types::{
    StellarApplication, StellarCapability, StellarContextRequest, StellarContextRequestKind,
    StellarRequest, StellarRequestKind, StellarSignedRequest,
};
use ed25519_dalek::{Signer, SigningKey};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{log, vec, Address, Bytes, BytesN, Env, IntoVal};

// use crate::types::{
//     StellarApplication, StellarCapability, StellarContextRequest, StellarContextRequestKind,
//     StellarRequest, StellarRequestKind, StellarSignedRequest,
// };
use crate::{ContextContract, ContextContractClient};

fn create_signed_request(
    signer_key: &SigningKey,
    request: StellarRequest,
    env: &Env,
) -> StellarSignedRequest {
    let request_xdr = request.clone().to_xdr(&env);
    let std_vec: Vec<u8> = request_xdr.into_iter().collect();
    let signature = signer_key.sign(&std_vec);

    let signature_bytes: BytesN<64> = BytesN::from_array(env, &signature.to_bytes());
    StellarSignedRequest {
        payload: request,
        signature: signature_bytes,
    }
}

#[test]
fn test_add_context() {
    let env = Env::default();
    let contract_id = env.register(ContextContract, ());
    let client = ContextContractClient::new(&env, &contract_id);

    // Initialize contract
    let owner = Address::generate(&env);
    client.mock_all_auths().initialize(&owner);

    let wasm = include_bytes!("../../mock_proxy/res/calimero_mock_proxy_stellar.wasm");
    let proxy_wasm = Bytes::from_slice(&env, wasm);
    let wasm_hash = client.mock_all_auths().set_proxy_code(&proxy_wasm, &owner);
    log!(&env, "Proxy code hash: {:?}", wasm_hash);

    let random_bytes: BytesN<32> = env.as_contract(&contract_id, || env.prng().gen());
    let signing_key = SigningKey::from_bytes(&random_bytes.to_array());
    let public_key = signing_key.verifying_key();

    let context_id = BytesN::from_array(&env, &public_key.to_bytes());
    let author_id = BytesN::from_array(&env, &[2u8; 32]);

    let application = StellarApplication {
        id: BytesN::from_array(&env, &[3u8; 32]),
        blob: BytesN::from_array(&env, &[4u8; 32]),
        size: 100,
        source: "test_app".into_val(&env),
        metadata: Bytes::from_array(&env, &[6u8; 32]),
    };

    let request = StellarRequest {
        signer_id: context_id.clone(),
        nonce: 0,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: context_id.clone(),
            kind: StellarContextRequestKind::Add(author_id.clone(), application.clone()),
        }),
    };

    let signed_request = create_signed_request(&signing_key, request, &env);
    client.mutate(&signed_request);

    let app = client.application(&context_id);
    if app.id == application.id {
        log!(
            &env,
            "Found application: id={:?}, source={}",
            app.id,
            app.source
        );
    } else {
        log!(&env, "No application found for context_id={:?}", context_id);
    }
}

#[test]
fn test_member_management() {
    let env = Env::default();
    let contract_id = env.register(ContextContract, ());
    let client = ContextContractClient::new(&env, &contract_id);

    // Initialize contract
    let owner = Address::generate(&env);
    client.mock_all_auths().initialize(&owner);

    // Set up proxy code
    let wasm = include_bytes!("../../mock_proxy/res/calimero_mock_proxy_stellar.wasm");
    let proxy_wasm = Bytes::from_slice(&env, wasm);
    client.mock_all_auths().set_proxy_code(&proxy_wasm, &owner);

    // Generate context and member keys
    let context_key =
        SigningKey::from_bytes(&env.as_contract(&contract_id, || env.prng().gen::<[u8; 32]>()));
    let alice_key =
        SigningKey::from_bytes(&env.as_contract(&contract_id, || env.prng().gen::<[u8; 32]>()));
    let bob_key =
        SigningKey::from_bytes(&env.as_contract(&contract_id, || env.prng().gen::<[u8; 32]>()));
    let charlie_key =
        SigningKey::from_bytes(&env.as_contract(&contract_id, || env.prng().gen::<[u8; 32]>()));

    let context_id = BytesN::from_array(&env, &context_key.verifying_key().to_bytes());
    let alice_id = BytesN::from_array(&env, &alice_key.verifying_key().to_bytes());
    let bob_id = BytesN::from_array(&env, &bob_key.verifying_key().to_bytes());
    let charlie_id = BytesN::from_array(&env, &charlie_key.verifying_key().to_bytes());

    // Create context with Alice as author
    let application = StellarApplication {
        id: BytesN::from_array(&env, &[1u8; 32]),
        blob: BytesN::from_array(&env, &[2u8; 32]),
        size: 100,
        source: "test_app".into_val(&env),
        metadata: Bytes::from_array(&env, &[3u8; 32]),
    };

    let create_request = StellarRequest {
        signer_id: context_id.clone(),
        nonce: 0,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: context_id.clone(),
            kind: StellarContextRequestKind::Add(alice_id.clone(), application),
        }),
    };

    let signed_request = create_signed_request(&context_key, create_request, &env);
    client.mutate(&signed_request);

    // Add Bob as member (signed by Alice - authorized)
    let add_member_request = StellarRequest {
        signer_id: alice_id.clone(),
        nonce: 0,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: context_id.clone(),
            kind: StellarContextRequestKind::AddMembers(vec![&env, bob_id.clone()]),
        }),
    };

    let signed_request = create_signed_request(&alice_key, add_member_request, &env);
    client.mutate(&signed_request);

    // Verify members after authorized addition
    let members = client.members(&context_id, &0u32, &10u32);
    log!(&env, "Members after authorized addition: {:?}", members);
    assert_eq!(
        members.len(),
        2,
        "Should have both Alice and Bob as members"
    );
    assert!(members.contains(&alice_id), "Alice should be a member");
    assert!(members.contains(&bob_id), "Bob should be a member");

    // Try to add Charlie as member (signed by Bob - unauthorized)
    let unauthorized_request = StellarRequest {
        signer_id: bob_id.clone(),
        nonce: 0,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: context_id.clone(),
            kind: StellarContextRequestKind::AddMembers(vec![&env, charlie_id.clone()]),
        }),
    };

    let signed_request = create_signed_request(&bob_key, unauthorized_request, &env);

    // This should fail with Unauthorized error
    let result = client.try_mutate(&signed_request);
    assert!(result.is_err(), "Unauthorized request should fail");

    // Verify members haven't changed
    let members = client.members(&context_id, &0u32, &10u32);
    log!(&env, "Final members list: {:?}", members);
    assert_eq!(
        members.len(),
        2,
        "Should still have only Alice and Bob as members"
    );
    assert!(
        members.contains(&alice_id),
        "Alice should still be a member"
    );
    assert!(members.contains(&bob_id), "Bob should still be a member");
    assert!(
        !members.contains(&charlie_id),
        "Charlie should not be a member"
    );

    // Try unauthorized removal (Bob trying to remove Alice)
    let unauthorized_remove = StellarRequest {
        signer_id: bob_id.clone(),
        nonce: 1,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: context_id.clone(),
            kind: StellarContextRequestKind::RemoveMembers(vec![&env, alice_id.clone()]),
        }),
    };

    let signed_request = create_signed_request(&bob_key, unauthorized_remove, &env);

    // This should fail with Unauthorized error
    let result = client.try_mutate(&signed_request);
    assert!(result.is_err(), "Unauthorized removal should fail");

    // Verify members haven't changed after failed removal
    let members = client.members(&context_id, &0u32, &10u32);
    log!(&env, "Members after failed removal attempt: {:?}", members);
    assert_eq!(members.len(), 2, "Should still have both members");
    assert!(
        members.contains(&alice_id),
        "Alice should still be a member"
    );
    assert!(members.contains(&bob_id), "Bob should still be a member");

    // Test authorized removal (Alice removing Bob)
    let authorized_remove = StellarRequest {
        signer_id: alice_id.clone(),
        nonce: 1,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: context_id.clone(),
            kind: StellarContextRequestKind::RemoveMembers(vec![&env, bob_id.clone()]),
        }),
    };

    let signed_request = create_signed_request(&alice_key, authorized_remove, &env);
    client.mutate(&signed_request);

    // Verify final membership after successful removal
    let members = client.members(&context_id, &0u32, &10u32);
    log!(
        &env,
        "Final members after authorized removal: {:?}",
        members
    );
    assert_eq!(members.len(), 1, "Should have only Alice as member");
    assert!(
        members.contains(&alice_id),
        "Alice should still be a member"
    );
    assert!(!members.contains(&bob_id), "Bob should have been removed");

    log!(&env, "Member management test completed successfully");
}

#[test]
fn test_capability_management() {
    let env = Env::default();
    let contract_id = env.register(ContextContract, ());
    let client = ContextContractClient::new(&env, &contract_id);

    // Initialize contract
    let owner = Address::generate(&env);
    client.mock_all_auths().initialize(&owner);

    // Set up proxy code
    let wasm = include_bytes!("../../mock_proxy/res/calimero_mock_proxy_stellar.wasm");
    let proxy_wasm = Bytes::from_slice(&env, wasm);
    client.mock_all_auths().set_proxy_code(&proxy_wasm, &owner);

    // Generate keys for context, Alice, and Bob
    let context_key =
        SigningKey::from_bytes(&env.as_contract(&contract_id, || env.prng().gen::<[u8; 32]>()));
    let alice_key =
        SigningKey::from_bytes(&env.as_contract(&contract_id, || env.prng().gen::<[u8; 32]>()));
    let bob_key =
        SigningKey::from_bytes(&env.as_contract(&contract_id, || env.prng().gen::<[u8; 32]>()));

    let context_id = BytesN::from_array(&env, &context_key.verifying_key().to_bytes());
    let alice_id = BytesN::from_array(&env, &alice_key.verifying_key().to_bytes());
    let bob_id = BytesN::from_array(&env, &bob_key.verifying_key().to_bytes());

    // Create context with Alice as author
    let application = StellarApplication {
        id: BytesN::from_array(&env, &[1u8; 32]),
        blob: BytesN::from_array(&env, &[2u8; 32]),
        size: 100,
        source: "test_app".into_val(&env),
        metadata: Bytes::from_array(&env, &[3u8; 32]),
    };

    let create_request = StellarRequest {
        signer_id: context_id.clone(),
        nonce: 0,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: context_id.clone(),
            kind: StellarContextRequestKind::Add(alice_id.clone(), application),
        }),
    };

    let signed_request = create_signed_request(&context_key, create_request, &env);
    client.mutate(&signed_request);

    // Add Bob as member
    let add_member_request = StellarRequest {
        signer_id: alice_id.clone(),
        nonce: 0,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: context_id.clone(),
            kind: StellarContextRequestKind::AddMembers(vec![&env, bob_id.clone()]),
        }),
    };

    let signed_request = create_signed_request(&alice_key, add_member_request, &env);
    client.mutate(&signed_request);

    // Grant ManageMembers capability to Bob
    let grant_request = StellarRequest {
        signer_id: alice_id.clone(),
        nonce: 1,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: context_id.clone(),
            kind: StellarContextRequestKind::Grant(vec![
                &env,
                (bob_id.clone(), StellarCapability::ManageMembers),
            ]),
        }),
    };

    let signed_request = create_signed_request(&alice_key, grant_request, &env);
    client.mutate(&signed_request);

    // Verify Bob's capabilities
    let bob_privileges = client.privileges(&context_id, &vec![&env, bob_id.clone()]);
    log!(&env, "Bob's privileges: {:?}", bob_privileges);

    // Bob should now be able to add members
    let charlie_key =
        SigningKey::from_bytes(&env.as_contract(&contract_id, || env.prng().gen::<[u8; 32]>()));
    let charlie_id = BytesN::from_array(&env, &charlie_key.verifying_key().to_bytes());

    let add_member_request = StellarRequest {
        signer_id: bob_id.clone(),
        nonce: 0,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: context_id.clone(),
            kind: StellarContextRequestKind::AddMembers(vec![&env, charlie_id.clone()]),
        }),
    };

    let signed_request = create_signed_request(&bob_key, add_member_request, &env);
    // This should succeed now that Bob has the ManageMembers capability
    client.mutate(&signed_request);

    // Verify Charlie was added
    let members = client.members(&context_id, &0u32, &10u32);
    assert!(
        members.contains(&charlie_id),
        "Charlie should have been added by Bob"
    );

    // Now revoke Bob's ManageMembers capability
    let revoke_request = StellarRequest {
        signer_id: alice_id.clone(),
        nonce: 2,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: context_id.clone(),
            kind: StellarContextRequestKind::Revoke(vec![
                &env,
                (bob_id.clone(), StellarCapability::ManageMembers),
            ]),
        }),
    };

    let signed_request = create_signed_request(&alice_key, revoke_request, &env);
    client.mutate(&signed_request);

    // Verify Bob's capabilities are gone
    let bob_privileges = client.privileges(&context_id, &vec![&env, bob_id.clone()]);
    log!(
        &env,
        "Bob's privileges after revocation: {:?}",
        bob_privileges
    );
    assert!(
        bob_privileges.is_empty() || !bob_privileges.contains_key(bob_id.clone()),
        "Bob should have no capabilities after revocation"
    );

    // Try to add another member with Bob (should fail now)
    let david_key =
        SigningKey::from_bytes(&env.as_contract(&contract_id, || env.prng().gen::<[u8; 32]>()));
    let david_id = BytesN::from_array(&env, &david_key.verifying_key().to_bytes());

    let unauthorized_add = StellarRequest {
        signer_id: bob_id.clone(),
        nonce: 1,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: context_id.clone(),
            kind: StellarContextRequestKind::AddMembers(vec![&env, david_id.clone()]),
        }),
    };

    let signed_request = create_signed_request(&bob_key, unauthorized_add, &env);
    let result = client.try_mutate(&signed_request);
    assert!(
        result.is_err(),
        "Bob should not be able to add members after capability revocation"
    );

    // Verify David was not added
    let members = client.members(&context_id, &0u32, &10u32);
    assert!(
        !members.contains(&david_id),
        "David should not have been added"
    );

    log!(&env, "Capability management test completed successfully");
}

#[test]
fn test_application_update() {
    let env = Env::default();
    let contract_id = env.register(ContextContract, ());
    let client = ContextContractClient::new(&env, &contract_id);

    // Initialize contract
    let owner = Address::generate(&env);
    client.mock_all_auths().initialize(&owner);

    // Set up proxy code
    let wasm = include_bytes!("../../mock_proxy/res/calimero_mock_proxy_stellar.wasm");
    let proxy_wasm = Bytes::from_slice(&env, wasm);
    client.mock_all_auths().set_proxy_code(&proxy_wasm, &owner);

    // Generate keys
    let context_key =
        SigningKey::from_bytes(&env.as_contract(&contract_id, || env.prng().gen::<[u8; 32]>()));
    let alice_key =
        SigningKey::from_bytes(&env.as_contract(&contract_id, || env.prng().gen::<[u8; 32]>()));
    let bob_key =
        SigningKey::from_bytes(&env.as_contract(&contract_id, || env.prng().gen::<[u8; 32]>()));

    let context_id = BytesN::from_array(&env, &context_key.verifying_key().to_bytes());
    let alice_id = BytesN::from_array(&env, &alice_key.verifying_key().to_bytes());
    let bob_id = BytesN::from_array(&env, &bob_key.verifying_key().to_bytes());

    // Create initial application
    let initial_app = StellarApplication {
        id: BytesN::from_array(&env, &[1u8; 32]),
        blob: BytesN::from_array(&env, &[2u8; 32]),
        size: 100,
        source: "initial_app".into_val(&env),
        metadata: Bytes::from_array(&env, &[3u8; 32]),
    };

    // Create context with Alice as author
    let create_request = StellarRequest {
        signer_id: context_id.clone(),
        nonce: 0,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: context_id.clone(),
            kind: StellarContextRequestKind::Add(alice_id.clone(), initial_app.clone()),
        }),
    };

    let signed_request = create_signed_request(&context_key, create_request, &env);
    client.mutate(&signed_request);

    // Verify initial application state
    let app = client.application(&context_id);
    log!(&env, "Initial application: {:?}", app);
    assert_eq!(app.id, initial_app.id, "Initial application ID mismatch");
    assert_eq!(
        app.blob, initial_app.blob,
        "Initial application blob mismatch"
    );

    // Create updated application
    let updated_app = StellarApplication {
        id: BytesN::from_array(&env, &[4u8; 32]),
        blob: BytesN::from_array(&env, &[5u8; 32]),
        size: 200,
        source: "updated_app".into_val(&env),
        metadata: Bytes::from_array(&env, &[6u8; 32]),
    };

    // Try unauthorized update (Bob)
    let unauthorized_update = StellarRequest {
        signer_id: bob_id.clone(),
        nonce: 0,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: context_id.clone(),
            kind: StellarContextRequestKind::UpdateApplication(updated_app.clone()),
        }),
    };

    let signed_request = create_signed_request(&bob_key, unauthorized_update, &env);
    let result = client.try_mutate(&signed_request);
    assert!(result.is_err(), "Unauthorized update should fail");

    // Verify application hasn't changed
    let app = client.application(&context_id);
    log!(&env, "Application after failed update: {:?}", app);
    assert_eq!(
        app.id, initial_app.id,
        "Application should not have changed"
    );
    assert_eq!(
        app.blob, initial_app.blob,
        "Application should not have changed"
    );

    // Authorized update (Alice)
    let authorized_update = StellarRequest {
        signer_id: alice_id.clone(),
        nonce: 0,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: context_id.clone(),
            kind: StellarContextRequestKind::UpdateApplication(updated_app.clone()),
        }),
    };

    let signed_request = create_signed_request(&alice_key, authorized_update, &env);
    client.mutate(&signed_request);

    // Verify application has been updated
    let app = client.application(&context_id);
    log!(&env, "Final application state: {:?}", app);
    assert_eq!(
        app.id, updated_app.id,
        "Application should have been updated"
    );
    assert_eq!(
        app.blob, updated_app.blob,
        "Application should have been updated"
    );

    log!(&env, "Application update test completed successfully");
}

#[test]
fn test_query_endpoints() {
    let env = Env::default();
    let contract_id = env.register(ContextContract, ());
    let client = ContextContractClient::new(&env, &contract_id);

    // Initialize contract
    let owner = Address::generate(&env);
    client.mock_all_auths().initialize(&owner);

    // Set up proxy code
    let wasm = include_bytes!("../../mock_proxy/res/calimero_mock_proxy_stellar.wasm");
    let proxy_wasm = Bytes::from_slice(&env, wasm);
    client.mock_all_auths().set_proxy_code(&proxy_wasm, &owner);

    // Generate keys
    let context_key =
        SigningKey::from_bytes(&env.as_contract(&contract_id, || env.prng().gen::<[u8; 32]>()));
    let alice_key =
        SigningKey::from_bytes(&env.as_contract(&contract_id, || env.prng().gen::<[u8; 32]>()));
    let bob_key =
        SigningKey::from_bytes(&env.as_contract(&contract_id, || env.prng().gen::<[u8; 32]>()));

    let context_id = BytesN::from_array(&env, &context_key.verifying_key().to_bytes());
    let alice_id = BytesN::from_array(&env, &alice_key.verifying_key().to_bytes());
    let bob_id = BytesN::from_array(&env, &bob_key.verifying_key().to_bytes());

    // Create initial application and context
    let initial_app = StellarApplication {
        id: BytesN::from_array(&env, &[1u8; 32]),
        blob: BytesN::from_array(&env, &[2u8; 32]),
        size: 100,
        source: "initial_app".into_val(&env),
        metadata: Bytes::from_array(&env, &[3u8; 32]),
    };

    // Create context with Alice as author
    let create_request = StellarRequest {
        signer_id: context_id.clone(),
        nonce: 0,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: context_id.clone(),
            kind: StellarContextRequestKind::Add(alice_id.clone(), initial_app.clone()),
        }),
    };

    let signed_request = create_signed_request(&context_key, create_request, &env);
    client.mutate(&signed_request);

    // Test initial nonces
    let alice_nonce = client.fetch_nonce(&context_id, &alice_id);
    assert_eq!(alice_nonce, Some(0), "Alice's initial nonce should be 0");
    let bob_nonce = client.fetch_nonce(&context_id, &bob_id);
    log!(&env, "Bob's nonce: {:?}", bob_nonce);
    assert!(bob_nonce.is_none(), "Bob should not have a nonce yet"); // Changed this line

    let app_revision = client.application_revision(&context_id);
    assert_eq!(app_revision, 0, "Initial application revision should be 0");

    // Create updated application
    let updated_app = StellarApplication {
        id: BytesN::from_array(&env, &[4u8; 32]),
        blob: BytesN::from_array(&env, &[5u8; 32]),
        size: 200,
        source: "updated_app".into_val(&env),
        metadata: Bytes::from_array(&env, &[6u8; 32]),
    };
    // Update application (should increment Alice's nonce)
    let update_app_request = StellarRequest {
        signer_id: alice_id.clone(),
        nonce: 0, // Using Alice's current nonce
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: context_id.clone(),
            kind: StellarContextRequestKind::UpdateApplication(updated_app.clone()),
        }),
    };

    let signed_request = create_signed_request(&alice_key, update_app_request, &env);
    client.mutate(&signed_request);

    // Verify Alice's nonce increased
    let alice_nonce_after_update = client.fetch_nonce(&context_id, &alice_id);
    assert_eq!(
        alice_nonce_after_update,
        Some(1),
        "Alice's nonce should be 1 after update"
    );

    // Try unauthorized application update (Bob)
    let unauthorized_update = StellarRequest {
        signer_id: bob_id.clone(),
        nonce: 0,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: context_id.clone(),
            kind: StellarContextRequestKind::UpdateApplication(updated_app.clone()),
        }),
    };

    let signed_request = create_signed_request(&bob_key, unauthorized_update, &env);
    let result = client.try_mutate(&signed_request);
    assert!(result.is_err(), "Unauthorized update should fail");

    // Verify revision didn't change after failed update
    let unchanged_revision = client.application_revision(&context_id);
    assert_eq!(
        unchanged_revision, 1,
        "Application revision should not change after failed update"
    );

    // Try using old nonce (should fail)
    let invalid_nonce_request = StellarRequest {
        signer_id: alice_id.clone(),
        nonce: 0, // Using old nonce
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: context_id.clone(),
            kind: StellarContextRequestKind::AddMembers(vec![&env, bob_id.clone()]),
        }),
    };

    let signed_request = create_signed_request(&alice_key, invalid_nonce_request, &env);
    let result = client.try_mutate(&signed_request);
    assert!(result.is_err(), "Request with old nonce should fail");

    assert!(
        !client.has_member(&context_id, &bob_id),
        "Bob should not be a member"
    );

    let members_rev = client.members_revision(&context_id);
    assert_eq!(members_rev, 0, "Members revision should be 0");

    // Add Bob as member
    let add_member_request = StellarRequest {
        signer_id: alice_id.clone(),
        nonce: 1, // Using Alice's current nonce
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: context_id.clone(),
            kind: StellarContextRequestKind::AddMembers(vec![&env, bob_id.clone()]),
        }),
    };

    let signed_request = create_signed_request(&alice_key, add_member_request, &env);
    client.mutate(&signed_request);

    // Verify Bob now has a nonce
    let bob_nonce_after_add = client.fetch_nonce(&context_id, &bob_id);
    assert_eq!(
        bob_nonce_after_add,
        Some(0),
        "Bob's initial nonce should be 0 after being added"
    );

    // Verify Alice's nonce increased again
    let alice_final_nonce = client.fetch_nonce(&context_id, &alice_id);
    assert_eq!(
        alice_final_nonce,
        Some(2),
        "Alice's nonce should be 2 after adding Bob"
    );

    // Try request with future nonce (should fail)
    let future_nonce_request = StellarRequest {
        signer_id: bob_id.clone(),
        nonce: 5, // Future nonce
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: context_id.clone(),
            kind: StellarContextRequestKind::AddMembers(vec![
                &env,
                BytesN::from_array(&env, &[0u8; 32]),
            ]),
        }),
    };

    let signed_request = create_signed_request(&bob_key, future_nonce_request, &env);
    let result = client.try_mutate(&signed_request);
    assert!(result.is_err(), "Request with future nonce should fail");

    // Test fetch_nonce
    let alice_nonce = client.fetch_nonce(&context_id, &alice_id);
    assert!(alice_nonce.is_some(), "Alice should have a nonce");
    let bob_nonce = client.fetch_nonce(&context_id, &bob_id);
    assert_eq!(bob_nonce, Some(0), "Bob should not have a nonce");

    // Test initial application_revision
    let app_revision = client.application_revision(&context_id);
    assert_eq!(app_revision, 1, "Application revision should be 1");

    // Test proxy_contract
    let proxy_address = client.proxy_contract(&context_id);
    assert!(
        proxy_address.to_string().len() > 0,
        "Proxy address should be set"
    );

    // Test has_member
    assert!(
        client.has_member(&context_id, &alice_id),
        "Alice should be a member"
    );

    // Test members_revision
    let members_rev = client.members_revision(&context_id);
    assert_eq!(members_rev, 1, "Members revision should be 1");

    log!(&env, "Query endpoints test completed successfully");
}
