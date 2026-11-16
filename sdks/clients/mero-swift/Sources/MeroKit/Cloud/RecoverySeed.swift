import CryptoKit
import Foundation

/// An organization's recovery key (core rc.9+, `docs/specs/org-recovery.md`):
/// an Ed25519 key whose 32-byte seed the owner keeps offline. The node stores
/// only the public half (``publicKey``); the seed signs, here and never on a
/// node, the statement that hands the organization to one account.
/// Byte-for-byte core's `calimero_identity::recovery`. (== mero-js
/// `newRecoverySeed`, `recoverySeedText`, `parseRecoverySeed`,
/// `recoveryPublicKey`, `signRecovery`.)
///
/// ```swift
/// let seed = RecoverySeed()
/// show(seed.text)  // 64 hex digits; keep it offline
/// let org = try await client.createOrg("Acme", recoveryKey: seed.publicKey)
/// // every passkey lost: sign up again, then
/// let times = try await fresh.startRecovery(org, seed: try RecoverySeed(text: written))
/// // 72 hours later
/// try await fresh.finishRecovery(org, passkey: newPasskey)
/// ```
public struct RecoverySeed: Sendable, Equatable {
    /// What a recovery signature covers before the organization, the account and the nonce.
    public static let domain = "calimero.org-recovery.v1\0"
    /// The longest nonce a recovery carries, in bytes (core's `MAX_RECOVERY_NONCE_BYTES`).
    public static let maxNonceBytes = 64

    /// The 32-byte seed.
    public let bytes: Data

    /// A new random seed.
    public init() {
        bytes = randomBytes(32)
    }

    /// A seed from its 32 bytes.
    public init(bytes: Data) throws {
        guard bytes.count == 32 else { throw AccountError.invalidInput("a recovery seed is 32 bytes") }
        self.bytes = Data(bytes)
    }

    /// A seed from its written form: 64 hex digits; `-`, whitespace and case are ignored.
    public init(text: String) throws {
        let digits = text.filter { $0 != "-" && !$0.isWhitespace }
        guard digits.count == 64, digits.allSatisfy(\.isHexDigit) else {
            throw AccountError.invalidInput("a recovery seed is 64 hex digits")
        }
        try self.init(bytes: try Hex.decode(digits.lowercased(), label: "a recovery seed", bytes: 32))
    }

    /// The seed as people write it down: 64 hex digits in eight groups of eight, joined by `-`.
    public var text: String {
        let hex = Array(Hex.encode(bytes))
        return stride(from: 0, to: hex.count, by: 8).map { String(hex[$0..<$0 + 8]) }.joined(separator: "-")
    }

    /// The seed's public key, base64url: what the organization stores
    /// (``CalimeroClient/createOrg(_:recoveryKey:)``, ``CalimeroClient/setRecoveryKey(_:publicKey:passkey:)``).
    public var publicKey: String {
        // 32 bytes always make a key (checked in `init`).
        b64url((try! Curve25519.Signing.PrivateKey(rawRepresentation: bytes)).publicKey.rawRepresentation)
    }

    /// The bytes a recovery signs: `"calimero.org-recovery.v1\0" || org || 0 || account || 0 || nonce`.
    public static func message(org: String, account: String, nonce: String) -> Data {
        Data("\(domain)\(org)\0\(account)\0\(nonce)".utf8)
    }

    /// The seed's signature (base64url) handing `org` to `account` under `nonce` (1 to 64 bytes).
    public func sign(org: String, account: String, nonce: String) throws -> String {
        let nonceBytes = nonce.utf8.count
        guard nonceBytes > 0, nonceBytes <= Self.maxNonceBytes else {
            throw AccountError.invalidInput("a recovery nonce is 1 to \(Self.maxNonceBytes) bytes")
        }
        let key = try Curve25519.Signing.PrivateKey(rawRepresentation: bytes)
        return b64url(try key.signature(for: Self.message(org: org, account: account, nonce: nonce)))
    }
}
