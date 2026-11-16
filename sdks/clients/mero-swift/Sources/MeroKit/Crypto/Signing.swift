import CryptoKit
import Foundation

/// core's `domain_hash`: SHA-256 over the length-prefixed domain, then each
/// length-prefixed part (every length a `u64` LE).
///
/// The lengths are what stop two different field splits from hashing alike.
/// Byte-identical with mero-js `crypto/internal.ts` `domainHash` and core's
/// `crates/primitives/src/identity.rs`.
public func domainHash(_ domain: String, _ parts: [Data]) -> Data {
    var hasher = SHA256()
    let domainBytes = Data(domain.utf8)
    hasher.update(data: LittleEndian.u64(UInt64(domainBytes.count)))
    hasher.update(data: domainBytes)
    for part in parts {
        hasher.update(data: LittleEndian.u64(UInt64(part.count)))
        hasher.update(data: part)
    }
    return Data(hasher.finalize())
}

/// Plain SHA-256.
public func sha256(_ data: Data) -> Data {
    Data(SHA256.hash(data: data))
}

/// Ed25519 over CryptoKit.
///
/// CryptoKit's Ed25519 signatures are **randomized** (hedged), unlike WebCrypto
/// and core, which are deterministic per RFC 8032. Both verify identically —
/// the verifier only checks the equation — so signatures are interchangeable,
/// but a test must verify a signature rather than compare it byte for byte.
public enum Ed25519 {
    /// The public half of a 32-byte seed.
    public static func publicKey(seed: Data) throws -> Data {
        try privateKey(seed).publicKey.rawRepresentation
    }

    /// Sign `message` with the 32-byte seed. Returns 64 bytes.
    public static func sign(seed: Data, message: Data) throws -> Data {
        let signature = try privateKey(seed).signature(for: message)
        guard signature.count == 64 else {
            throw AccountError.protocolViolation("signer returned \(signature.count) bytes, expected 64")
        }
        return signature
    }

    /// Check a signature against a raw 32-byte public key.
    public static func verify(publicKey: Data, signature: Data, message: Data) -> Bool {
        guard let key = try? Curve25519.Signing.PublicKey(rawRepresentation: publicKey) else { return false }
        return key.isValidSignature(signature, for: message)
    }

    private static func privateKey(_ seed: Data) throws -> Curve25519.Signing.PrivateKey {
        guard seed.count == 32 else {
            throw AccountError.invalidInput("deviceSecret must be 64 hex characters, got \(seed.count * 2)")
        }
        return try Curve25519.Signing.PrivateKey(rawRepresentation: seed)
    }
}

/// X25519 key-delivery ("KEM") keys over CryptoKit.
public enum X25519 {
    public static func publicKey(secret: Data) throws -> Data {
        guard secret.count == 32 else {
            throw AccountError.invalidInput("kemSecret must be 64 hex characters, got \(secret.count * 2)")
        }
        return try Curve25519.KeyAgreement.PrivateKey(rawRepresentation: secret).publicKey.rawRepresentation
    }
}

/// `count` cryptographically random bytes.
public func randomBytes(_ count: Int) -> Data {
    var bytes = [UInt8](repeating: 0, count: count)
    var generator = SystemRandomNumberGenerator()
    for i in 0..<count { bytes[i] = UInt8.random(in: .min ... .max, using: &generator) }
    return Data(bytes)
}
