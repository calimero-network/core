use calimero_context_config::stellar::stellar_types::{
    StellarApplication, StellarCapability, StellarContextRequest, StellarContextRequestKind,
    StellarRequest, StellarRequestKind, StellarSignedRequest, StellarSignedRequestPayload,
};
use ed25519_dalek::{Signer, SigningKey};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{log, vec, Address, Bytes, BytesN, Env, IntoVal};

use crate::{ContextContract, ContextContractClient};

fn create_signed_request(
    signer_key: &SigningKey,
    request: StellarRequest,
    env: &Env,
) -> StellarSignedRequest {
    StellarSignedRequest::new(env, StellarSignedRequestPayload::Context(request), |data| {
        Ok(signer_key.sign(data))
    })
    .unwrap()
}

// Helper struct to manage test context
struct TestContext<'a> {
    env: Env,
    client: ContextContractClient<'a>,
    context_key: SigningKey,
    context_id: BytesN<32>,
}

impl<'a> TestContext<'a> {
    fn setup() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        let owner = Address::generate(&env);
        let contract_id = env.register(
            ContextContract,
            (
                &owner,
                Address::from_str(
                    &env,
                    "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
                ),
            ),
        );
        let client = ContextContractClient::new(&env, &contract_id);

        // Set up proxy code
        let wasm = include_bytes!("../../../context-proxy/res/calimero_context_proxy_stellar.wasm");
        let proxy_wasm = Bytes::from_slice(&env, wasm);
        client.mock_all_auths().set_proxy_code(&proxy_wasm, &owner);

        // Generate context key
        let context_key =
            SigningKey::from_bytes(&env.as_contract(&contract_id, || env.prng().gen::<[u8; 32]>()));
        let context_id = BytesN::from_array(&env, &context_key.verifying_key().to_bytes());

        Self {
            env,
            client,
            context_key,
            context_id,
        }
    }

    fn generate_key(&self) -> (SigningKey, BytesN<32>) {
        let contract_id = self.client.address.clone(); // Get the contract's address
        let key = SigningKey::from_bytes(
            &self
                .env
                .as_contract(&contract_id, || self.env.prng().gen::<[u8; 32]>()),
        );
        let id = BytesN::from_array(&self.env, &key.verifying_key().to_bytes());
        (key, id)
    }

    fn create_application(&self, id: u8) -> StellarApplication {
        StellarApplication {
            id: BytesN::from_array(&self.env, &[id; 32]),
            blob: BytesN::from_array(&self.env, &[id + 1; 32]),
            size: 100,
            source: "test_app".into_val(&self.env),
            metadata: Bytes::from_array(&self.env, &[id + 2; 32]),
        }
    }

    fn create_context(&self, author_id: BytesN<32>, app: StellarApplication) {
        let request = StellarRequest {
            signer_id: self.context_id.clone(),
            nonce: 0,
            kind: StellarRequestKind::Context(StellarContextRequest {
                context_id: self.context_id.clone(),
                kind: StellarContextRequestKind::Add(author_id, app),
            }),
        };

        let signed_request = create_signed_request(&self.context_key, request, &self.env);
        self.client.mutate(&signed_request);
    }
}

#[test]
fn test_add_context() {
    let ctx = TestContext::setup();
    log!(
        &ctx.env,
        "Context contract address: {:?}",
        ctx.client.address
    );
    let (_author_key, author_id) = ctx.generate_key();
    let app = ctx.create_application(1);

    ctx.create_context(author_id.clone(), app.clone());

    let stored_app = ctx.client.application(&ctx.context_id);
    assert_eq!(stored_app.id, app.id, "Application not stored correctly");
}

#[test]
fn test_member_management() {
    let ctx = TestContext::setup();
    let (alice_key, alice_id) = ctx.generate_key();
    let (bob_key, bob_id) = ctx.generate_key();
    let (_charlie_key, charlie_id) = ctx.generate_key();

    // Create context with Alice as author
    let app = ctx.create_application(1);
    ctx.create_context(alice_id.clone(), app);

    // Add Bob as member (signed by Alice)
    let add_bob_request = StellarRequest {
        signer_id: alice_id.clone(),
        nonce: 0,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: ctx.context_id.clone(),
            kind: StellarContextRequestKind::AddMembers(vec![&ctx.env, bob_id.clone()]),
        }),
    };

    let signed_request = create_signed_request(&alice_key, add_bob_request, &ctx.env);
    ctx.client.mutate(&signed_request);

    // Verify members after authorized addition
    let members = ctx.client.members(&ctx.context_id, &0u32, &10u32);
    log!(&ctx.env, "Members after authorized addition: {:?}", members);
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
            context_id: ctx.context_id.clone(),
            kind: StellarContextRequestKind::AddMembers(vec![&ctx.env, charlie_id.clone()]),
        }),
    };

    let signed_request = create_signed_request(&bob_key, unauthorized_request, &ctx.env);

    // This should fail with Unauthorized error
    let result = ctx.client.try_mutate(&signed_request);
    assert!(result.is_err(), "Unauthorized request should fail");

    // Verify members haven't changed
    let members = ctx.client.members(&ctx.context_id, &0u32, &10u32);
    log!(&ctx.env, "Final members list: {:?}", members);
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
            context_id: ctx.context_id.clone(),
            kind: StellarContextRequestKind::RemoveMembers(vec![&ctx.env, alice_id.clone()]),
        }),
    };

    let signed_request = create_signed_request(&bob_key, unauthorized_remove, &ctx.env);

    // This should fail with Unauthorized error
    let result = ctx.client.try_mutate(&signed_request);
    assert!(result.is_err(), "Unauthorized removal should fail");

    // Verify members haven't changed after failed removal
    let members = ctx.client.members(&ctx.context_id, &0u32, &10u32);
    log!(
        &ctx.env,
        "Members after failed removal attempt: {:?}",
        members
    );
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
            context_id: ctx.context_id.clone(),
            kind: StellarContextRequestKind::RemoveMembers(vec![&ctx.env, bob_id.clone()]),
        }),
    };

    let signed_request = create_signed_request(&alice_key, authorized_remove, &ctx.env);
    ctx.client.mutate(&signed_request);

    // Verify final membership after successful removal
    let members = ctx.client.members(&ctx.context_id, &0u32, &10u32);
    log!(
        &ctx.env,
        "Final members after authorized removal: {:?}",
        members
    );
    assert_eq!(members.len(), 1, "Should have only Alice as member");
    assert!(
        members.contains(&alice_id),
        "Alice should still be a member"
    );
    assert!(!members.contains(&bob_id), "Bob should have been removed");

    log!(&ctx.env, "Member management test completed successfully");
}

#[test]
fn test_capability_management() {
    let ctx = TestContext::setup();
    let (alice_key, alice_id) = ctx.generate_key();
    let (bob_key, bob_id) = ctx.generate_key();
    let (_ , charlie_id) = ctx.generate_key();

    // Create context with Alice as author
    let app = ctx.create_application(1);
    ctx.create_context(alice_id.clone(), app);

    // Add Bob as member
    let add_member_request = StellarRequest {
        signer_id: alice_id.clone(),
        nonce: 0,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: ctx.context_id.clone(),
            kind: StellarContextRequestKind::AddMembers(vec![&ctx.env, bob_id.clone()]),
        }),
    };

    let signed_request = create_signed_request(&alice_key, add_member_request, &ctx.env);
    ctx.client.mutate(&signed_request);

    // Grant ManageMembers capability to Bob
    let grant_request = StellarRequest {
        signer_id: alice_id.clone(),
        nonce: 1,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: ctx.context_id.clone(),
            kind: StellarContextRequestKind::Grant(vec![
                &ctx.env,
                (bob_id.clone(), StellarCapability::ManageMembers),
            ]),
        }),
    };

    let signed_request = create_signed_request(&alice_key, grant_request, &ctx.env);
    ctx.client.mutate(&signed_request);

    // Verify Bob's capabilities
    let bob_privileges = ctx
        .client
        .privileges(&ctx.context_id, &vec![&ctx.env, bob_id.clone()]);
    log!(&ctx.env, "Bob's privileges: {:?}", bob_privileges);

    // Bob should now be able to add members
    let add_member_request = StellarRequest {
        signer_id: bob_id.clone(),
        nonce: 0,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: ctx.context_id.clone(),
            kind: StellarContextRequestKind::AddMembers(vec![&ctx.env, charlie_id.clone()]),
        }),
    };

    let signed_request = create_signed_request(&bob_key, add_member_request, &ctx.env);
    // This should succeed now that Bob has the ManageMembers capability
    ctx.client.mutate(&signed_request);

    // Verify Charlie was added
    let members = ctx.client.members(&ctx.context_id, &0u32, &10u32);
    assert!(
        members.contains(&charlie_id),
        "Charlie should have been added by Bob"
    );

    // Now revoke Bob's ManageMembers capability
    let revoke_request = StellarRequest {
        signer_id: alice_id.clone(),
        nonce: 2,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: ctx.context_id.clone(),
            kind: StellarContextRequestKind::Revoke(vec![
                &ctx.env,
                (bob_id.clone(), StellarCapability::ManageMembers),
            ]),
        }),
    };

    let signed_request = create_signed_request(&alice_key, revoke_request, &ctx.env);
    ctx.client.mutate(&signed_request);

    // Verify Bob's capabilities are gone
    let bob_privileges = ctx
        .client
        .privileges(&ctx.context_id, &vec![&ctx.env, bob_id.clone()]);
    log!(
        &ctx.env,
        "Bob's privileges after revocation: {:?}",
        bob_privileges
    );
    assert!(
        bob_privileges.is_empty() || !bob_privileges.contains_key(bob_id.clone()),
        "Bob should have no capabilities after revocation"
    );

    // Try to add another member with Bob (should fail now)
    let (_david_key, david_id) = ctx.generate_key();

    let unauthorized_add = StellarRequest {
        signer_id: bob_id.clone(),
        nonce: 1,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: ctx.context_id.clone(),
            kind: StellarContextRequestKind::AddMembers(vec![&ctx.env, david_id.clone()]),
        }),
    };

    let signed_request = create_signed_request(&bob_key, unauthorized_add, &ctx.env);
    let result = ctx.client.try_mutate(&signed_request);
    assert!(
        result.is_err(),
        "Bob should not be able to add members after capability revocation"
    );

    // Verify David was not added
    let members = ctx.client.members(&ctx.context_id, &0u32, &10u32);
    assert!(
        !members.contains(&david_id),
        "David should not have been added"
    );

    log!(
        &ctx.env,
        "Capability management test completed successfully"
    );
}

#[test]
fn test_application_update() {
    let ctx = TestContext::setup();
    let (alice_key, alice_id) = ctx.generate_key();
    let (bob_key, bob_id) = ctx.generate_key();

    // Create initial application
    let initial_app = ctx.create_application(1);

    // Create context with Alice as author
    ctx.create_context(alice_id.clone(), initial_app.clone());

    // Verify initial application state
    let app = ctx.client.application(&ctx.context_id);
    log!(&ctx.env, "Initial application: {:?}", app);
    assert_eq!(app.id, initial_app.id, "Initial application ID mismatch");
    assert_eq!(
        app.blob, initial_app.blob,
        "Initial application blob mismatch"
    );

    // Create updated application
    let updated_app = ctx.create_application(2);

    // Try unauthorized update (Bob)
    let unauthorized_update = StellarRequest {
        signer_id: bob_id.clone(),
        nonce: 0,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: ctx.context_id.clone(),
            kind: StellarContextRequestKind::UpdateApplication(updated_app.clone()),
        }),
    };

    let signed_request = create_signed_request(&bob_key, unauthorized_update, &ctx.env);
    let result = ctx.client.try_mutate(&signed_request);
    assert!(result.is_err(), "Unauthorized update should fail");

    // Verify application hasn't changed
    let app = ctx.client.application(&ctx.context_id);
    log!(&ctx.env, "Application after failed update: {:?}", app);
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
            context_id: ctx.context_id.clone(),
            kind: StellarContextRequestKind::UpdateApplication(updated_app.clone()),
        }),
    };

    let signed_request = create_signed_request(&alice_key, authorized_update, &ctx.env);
    ctx.client.mutate(&signed_request);

    // Verify application has been updated
    let app = ctx.client.application(&ctx.context_id);
    log!(&ctx.env, "Final application state: {:?}", app);
    assert_eq!(
        app.id, updated_app.id,
        "Application should have been updated"
    );
    assert_eq!(
        app.blob, updated_app.blob,
        "Application should have been updated"
    );

    log!(&ctx.env, "Application update test completed successfully");
}

#[test]
fn test_query_endpoints() {
    let ctx = TestContext::setup();
    let (alice_key, alice_id) = ctx.generate_key();
    let (bob_key, bob_id) = ctx.generate_key();

    // Create initial application and context
    let initial_app = ctx.create_application(1);
    ctx.create_context(alice_id.clone(), initial_app.clone());

    // Test initial nonces
    assert_eq!(
        ctx.client.fetch_nonce(&ctx.context_id, &alice_id),
        Some(0),
        "Alice's initial nonce should be 0"
    );
    assert!(
        ctx.client.fetch_nonce(&ctx.context_id, &bob_id).is_none(),
        "Bob should not have a nonce yet"
    );

    // Test initial revisions
    assert_eq!(
        ctx.client.application_revision(&ctx.context_id),
        0,
        "Initial application revision should be 0"
    );

    // Update application (should increment Alice's nonce)
    let updated_app = ctx.create_application(2);
    let update_request = StellarRequest {
        signer_id: alice_id.clone(),
        nonce: 0,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: ctx.context_id.clone(),
            kind: StellarContextRequestKind::UpdateApplication(updated_app.clone()),
        }),
    };
    let signed_request = create_signed_request(&alice_key, update_request, &ctx.env);
    ctx.client.mutate(&signed_request);

    // Verify nonce and revision updates
    assert_eq!(
        ctx.client.fetch_nonce(&ctx.context_id, &alice_id),
        Some(1),
        "Alice's nonce should be 1 after update"
    );
    assert_eq!(
        ctx.client.application_revision(&ctx.context_id),
        1,
        "Application revision should be 1"
    );

    // Test unauthorized update
    let unauthorized_update = StellarRequest {
        signer_id: bob_id.clone(),
        nonce: 0,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: ctx.context_id.clone(),
            kind: StellarContextRequestKind::UpdateApplication(updated_app.clone()),
        }),
    };
    let signed_request = create_signed_request(&bob_key, unauthorized_update, &ctx.env);
    assert!(
        ctx.client.try_mutate(&signed_request).is_err(),
        "Unauthorized update should fail"
    );

    // Test invalid nonce scenarios
    let old_nonce_request = StellarRequest {
        signer_id: alice_id.clone(),
        nonce: 0, // Using old nonce
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: ctx.context_id.clone(),
            kind: StellarContextRequestKind::AddMembers(vec![&ctx.env, bob_id.clone()]),
        }),
    };
    let signed_request = create_signed_request(&alice_key, old_nonce_request, &ctx.env);
    assert!(
        ctx.client.try_mutate(&signed_request).is_err(),
        "Request with old nonce should fail"
    );

    // Add Bob as member
    let add_member_request = StellarRequest {
        signer_id: alice_id.clone(),
        nonce: 1,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: ctx.context_id.clone(),
            kind: StellarContextRequestKind::AddMembers(vec![&ctx.env, bob_id.clone()]),
        }),
    };
    let signed_request = create_signed_request(&alice_key, add_member_request, &ctx.env);
    ctx.client.mutate(&signed_request);

    // Verify member status and nonces
    assert_eq!(
        ctx.client.fetch_nonce(&ctx.context_id, &bob_id),
        Some(0),
        "Bob's initial nonce should be 0 after being added"
    );
    assert_eq!(
        ctx.client.fetch_nonce(&ctx.context_id, &alice_id),
        Some(2),
        "Alice's nonce should be 2 after adding Bob"
    );
    assert!(
        ctx.client.has_member(&ctx.context_id, &bob_id),
        "Bob should be a member"
    );
    assert_eq!(
        ctx.client.members_revision(&ctx.context_id),
        1,
        "Members revision should be 1"
    );

    // Test future nonce
    let future_nonce_request = StellarRequest {
        signer_id: bob_id.clone(),
        nonce: 5,
        kind: StellarRequestKind::Context(StellarContextRequest {
            context_id: ctx.context_id.clone(),
            kind: StellarContextRequestKind::AddMembers(vec![
                &ctx.env,
                BytesN::from_array(&ctx.env, &[0u8; 32]),
            ]),
        }),
    };
    let signed_request = create_signed_request(&bob_key, future_nonce_request, &ctx.env);
    assert!(
        ctx.client.try_mutate(&signed_request).is_err(),
        "Request with future nonce should fail"
    );

    // Test proxy contract
    let proxy_address = ctx.client.proxy_contract(&ctx.context_id);
    assert!(
        !proxy_address.to_string().is_empty(),
        "Proxy address should be set"
    );

    log!(&ctx.env, "Query endpoints test completed successfully");
}
