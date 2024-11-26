use candid::Principal;
use pocket_ic::PocketIc;
use ed25519_dalek::SigningKey;
use rand::Rng;
use std::time::{SystemTime, UNIX_EPOCH};

use context_contract::types::{
    ICApplication, ICApplicationId, ICBlobId, ICContextId, ICContextIdentity,
    ICPSigned, ICSignerId, Request, RequestKind, ContextRequest, ContextRequestKind
};

fn setup() -> (PocketIc, Principal) {
    let pic = PocketIc::new();
    let wasm = std::fs::read("res/context_contract.wasm").expect("failed to read wasm");
    let canister = pic.create_canister();
    pic.install_canister(
        canister,
        wasm,
        vec![],
        None, // No controller
    );
    (pic, canister)
}

fn create_signed_request(
    _signer_key: &SigningKey,
    request: Request,
) -> ICPSigned<Request> {
    // TODO: Implement actual signature creation
    ICPSigned {
        payload: request,
        signature: vec![], // For now empty, we'll implement proper signing later
    }
}

fn get_time_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_millis() as u64
}

#[test]
fn test_mutate_success_cases() {
    let (pic, canister) = setup();
    let mut rng = rand::thread_rng();

    // Test Case 1: Successful context addition
    {
        let context_sk = SigningKey::from_bytes(&rng.gen());
        let context_pk = context_sk.verifying_key();
        let context_id = ICContextId::new(context_pk.to_bytes());
        let author_id = ICContextIdentity::new(rng.gen());

        let request = Request {
            kind: RequestKind::Context(ContextRequest {
                context_id: context_id.clone(),
                kind: ContextRequestKind::Add {
                    author_id: author_id.clone(),
                    application: ICApplication {
                        id: ICApplicationId::new(rng.gen()),
                        blob: ICBlobId::new(rng.gen()),
                        size: 0,
                        source: String::new(),
                        metadata: vec![],
                    },
                },
            }),
            signer_id: ICSignerId::new(context_id.0),
            timestamp_ms: get_time_ms(pic.get_time()),
        };

        let signed_request = create_signed_request(&context_sk, request);
        let response = pic.update_call(
            canister,
            Principal::anonymous(),
            "mutate",
            candid::encode_one(signed_request).unwrap(),
        );

        assert!(response.is_ok(), "Context addition should succeed");
    }
}

#[test]
fn test_mutate_failure_cases() {
    let (pic, canister) = setup();
    let mut rng = rand::thread_rng();

    // Test Case 1: Request timestamp expired
    {
        let context_sk = SigningKey::from_bytes(&rng.gen());
        let context_pk = context_sk.verifying_key();
        let context_id = ICContextId::new(context_pk.to_bytes());
        let author_id = ICContextIdentity::new(rng.gen());

        // Set time to 6 minutes ago (exceeds 5 minute threshold)
        let current_time = get_time_ms(pic.get_time());
        let expired_time = current_time.saturating_sub(1000 * 60 * 6);

        let request = Request {
            kind: RequestKind::Context(ContextRequest {
                context_id: context_id.clone(),
                kind: ContextRequestKind::Add {
                    author_id: author_id.clone(),
                    application: ICApplication {
                        id: ICApplicationId::new(rng.gen()),
                        blob: ICBlobId::new(rng.gen()),
                        size: 0,
                        source: String::new(),
                        metadata: vec![],
                    },
                },
            }),
            signer_id: ICSignerId::new(context_id.0),
            timestamp_ms: expired_time,
        };

        let signed_request = create_signed_request(&context_sk, request);
        let response = pic.update_call(
            canister,
            Principal::anonymous(),
            "mutate",
            candid::encode_one(signed_request).unwrap(),
        );

        let err = response.unwrap_err();
        assert!(err.description.contains("request expired"), 
            "Expected expired request error, got: {}", err.description);
    }

    // Test Case 2: Wrong signer for context addition
    {
        let wrong_signer_sk = SigningKey::from_bytes(&rng.gen());
        let context_id = ICContextId::new(rng.gen());
        let author_id = ICContextIdentity::new(rng.gen());

        let request = Request {
            kind: RequestKind::Context(ContextRequest {
                context_id: context_id.clone(),
                kind: ContextRequestKind::Add {
                    author_id: author_id.clone(),
                    application: ICApplication {
                        id: ICApplicationId::new(rng.gen()),
                        blob: ICBlobId::new(rng.gen()),
                        size: 0,
                        source: String::new(),
                        metadata: vec![],
                    },
                },
            }),
            signer_id: ICSignerId::new(wrong_signer_sk.verifying_key().to_bytes()),
            timestamp_ms: get_time_ms(pic.get_time()),
        };

        let signed_request = create_signed_request(&wrong_signer_sk, request);
        let response = pic.update_call(
            canister,
            Principal::anonymous(),
            "mutate",
            candid::encode_one(signed_request).unwrap(),
        );

        let err = response.unwrap_err();
        assert!(err.description.contains("context addition must be signed by the context itself"), 
            "Expected wrong signer error, got: {}", err.description);
    }

    // Test Case 3: Duplicate context addition
    {
        let context_sk = SigningKey::from_bytes(&rng.gen());
        let context_pk = context_sk.verifying_key();
        let context_id = ICContextId::new(context_pk.to_bytes());
        let author_id = ICContextIdentity::new(rng.gen());
        let application = ICApplication {
            id: ICApplicationId::new(rng.gen()),
            blob: ICBlobId::new(rng.gen()),
            size: 0,
            source: String::new(),
            metadata: vec![],
        };

        let request = Request {
            kind: RequestKind::Context(ContextRequest {
                context_id: context_id.clone(),
                kind: ContextRequestKind::Add {
                    author_id: author_id.clone(),
                    application: application,
                },
            }),
            signer_id: ICSignerId::new(context_id.0),
            timestamp_ms: get_time_ms(pic.get_time()),
        };

        // First addition (should succeed)
        let signed_request = create_signed_request(&context_sk, request.clone());
        let _ = pic.update_call(
            canister,
            Principal::anonymous(),
            "mutate",
            candid::encode_one(signed_request).unwrap(),
        );

        // Second addition (should fail)
        let signed_request = create_signed_request(&context_sk, request);
        let response = pic.update_call(
            canister,
            Principal::anonymous(),
            "mutate",
            candid::encode_one(signed_request).unwrap(),
        );

        let err = response.unwrap_err();
        assert!(err.description.contains("context already exists"), 
            "Expected duplicate context error, got: {}", err.description);
    }
}