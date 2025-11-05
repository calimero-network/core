//! Tests for SecureStream authentication

mod common;

use common::{create_test_context_id, create_test_identity, create_test_shared_key};
use calimero_protocols::SecureStream;

/// Test that SecureStream exists and has correct methods
#[test]
fn test_secure_stream_exists() {
    // SecureStream is the authentication module
    // It provides:
    // - authenticate_p2p() - full mutual authentication
    // - verify_identity() - verify peer's identity
    // - prove_identity() - prove our identity
    
    // Validate functions exist
    let _ = SecureStream::authenticate_p2p as usize;
    let _ = SecureStream::verify_identity as usize;
    let _ = SecureStream::prove_identity as usize;
}

#[test]
fn test_identity_creation_and_equality() {
    // Test identity creation
    let identity1 = create_test_identity();
    let identity2 = create_test_identity();
    
    // Different identities should not be equal
    assert_ne!(identity1, identity2);
    
    // Same identity should be equal to itself
    assert_eq!(identity1, identity1);
}

#[test]
fn test_shared_key_creation() {
    // Test shared key creation (used in authentication)
    let shared_key = create_test_shared_key();
    
    // Test encryption/decryption
    let message = b"test message";
    let nonce: calimero_crypto::Nonce = rand::Rng::gen(&mut rand::thread_rng());
    
    let encrypted = shared_key.encrypt(message.to_vec(), nonce);
    assert!(encrypted.is_some());
    
    if let Some(encrypted_data) = encrypted {
        let decrypted = shared_key.decrypt(encrypted_data, nonce);
        assert!(decrypted.is_some());
        
        if let Some(decrypted_data) = decrypted {
            assert_eq!(decrypted_data, message);
        }
    }
}

#[test]
fn test_nonce_uniqueness() {
    use rand::Rng;
    
    // Test that nonces are unique (critical for security)
    let mut nonces = std::collections::HashSet::new();
    
    for _ in 0..1000 {
        let nonce: calimero_crypto::Nonce = rand::thread_rng().gen();
        // Each nonce should be unique
        assert!(nonces.insert(nonce), "Duplicate nonce generated!");
    }
}

#[test]
fn test_challenge_response_properties() {
    use calimero_primitives::identity::PrivateKey;
    use rand::Rng;
    
    // Test the properties needed for challenge-response authentication
    
    // 1. Each identity has a unique private/public key pair
    let private1 = PrivateKey::random(&mut rand::thread_rng());
    let public1 = private1.public_key();
    
    let private2 = PrivateKey::random(&mut rand::thread_rng());
    let public2 = private2.public_key();
    
    assert_ne!(public1, public2);
    
    // 2. Messages can be signed and verified
    let message = b"challenge message";
    let signature = private1.sign(message).expect("signing should succeed");
    
    // Verify with correct public key
    assert!(public1.verify(message, &signature).is_ok());
    
    // Verify with wrong public key should fail
    assert!(public2.verify(message, &signature).is_err());
}

#[test]
fn test_encryption_prevents_tampering() {
    let shared_key = create_test_shared_key();
    let message = b"original message";
    let nonce: calimero_crypto::Nonce = rand::Rng::gen(&mut rand::thread_rng());
    
    // Encrypt message
    let encrypted = shared_key.encrypt(message.to_vec(), nonce).unwrap();
    
    // Tamper with encrypted data
    let mut tampered = encrypted.clone();
    tampered[0] ^= 0xFF;
    
    // Decryption should fail on tampered data
    let decrypted = shared_key.decrypt(tampered, nonce);
    assert!(decrypted.is_none(), "Tampered message should not decrypt!");
}

#[test]
fn test_wrong_nonce_fails() {
    let shared_key = create_test_shared_key();
    let message = b"test message";
    let nonce1: calimero_crypto::Nonce = rand::Rng::gen(&mut rand::thread_rng());
    let nonce2: calimero_crypto::Nonce = rand::Rng::gen(&mut rand::thread_rng());
    
    // Encrypt with nonce1
    let encrypted = shared_key.encrypt(message.to_vec(), nonce1).unwrap();
    
    // Decrypt with different nonce should fail
    let decrypted = shared_key.decrypt(encrypted, nonce2);
    assert!(decrypted.is_none(), "Wrong nonce should fail decryption!");
}

// TODO: Add integration tests for:
// - Full bidirectional authentication flow
// - Challenge-response protocol execution
// - Deadlock prevention (deterministic role assignment)
// - Concurrent authentication attempts
// - Timeout handling
//
// These require:
// - Mock Stream (for bidirectional message passing)
// - Mock ContextClient (for identity storage)
// - Ability to simulate network conditions

