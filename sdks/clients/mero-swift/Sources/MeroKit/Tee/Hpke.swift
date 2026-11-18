import CryptoKit
import Foundation

// RFC 9180 HPKE, the one suite and mode mero-agent-gate takes: Auth mode,
// DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, ChaCha20-Poly1305, single-shot
// (one message per context, sequence number 0). Port of mero-js
// `src/agent-gate/hpke.ts`, on CryptoKit's X25519, HMAC and ChaChaPoly
// (CryptoKit's own HPKE needs iOS 17). Auth mode proves the sender: the gate
// opens a bundle only under the X25519 key it names, so nobody can seal in an
// owner's name.

/// An HPKE message: the encapsulated key and the ciphertext (tag appended).
/// (== mero-js `HpkeSealed`.)
public struct HpkeSealed: Sendable, Equatable {
    /// The sender's ephemeral X25519 public key.
    public var enc: Data
    public var ciphertext: Data

    public init(enc: Data, ciphertext: Data) {
        self.enc = enc
        self.ciphertext = ciphertext
    }
}

enum Hpke {
    static let kemId: UInt16 = 0x0020
    static let kdfId: UInt16 = 0x0001
    static let aeadId: UInt16 = 0x0003
    static let modeAuth: UInt8 = 0x02

    static var kemSuite: Data { Data("KEM".utf8) + BigEndian.u16(kemId) }
    static var hpkeSuite: Data {
        Data("HPKE".utf8) + BigEndian.u16(kemId) + BigEndian.u16(kdfId) + BigEndian.u16(aeadId)
    }

    /// HMAC-SHA-256; an empty salt is HashLen zeros (RFC 5869).
    static func extract(_ salt: Data, _ ikm: Data) -> Data {
        hmacSha256(salt.isEmpty ? Data(count: 32) : salt, ikm)
    }

    static func expand(_ prk: Data, _ info: Data, _ length: Int) -> Data {
        var out = Data()
        var previous = Data()
        var counter: UInt8 = 1
        while out.count < length {
            previous = hmacSha256(prk, previous + info + Data([counter]))
            out.append(previous)
            counter += 1
        }
        return Data(out.prefix(length))
    }

    static func labeledExtract(_ suite: Data, _ salt: Data, _ label: String, _ ikm: Data) -> Data {
        extract(salt, Data("HPKE-v1".utf8) + suite + Data(label.utf8) + ikm)
    }

    static func labeledExpand(_ suite: Data, _ prk: Data, _ label: String, _ info: Data, _ length: Int) -> Data {
        expand(prk, BigEndian.u16(UInt16(length)) + Data("HPKE-v1".utf8) + suite + Data(label.utf8) + info, length)
    }

    static func extractAndExpand(_ dh: Data, _ kemContext: Data) -> Data {
        let prk = labeledExtract(kemSuite, Data(), "eae_prk", dh)
        return labeledExpand(kemSuite, prk, "shared_secret", kemContext, 32)
    }

    /// The single-shot key and nonce, in Auth mode with no PSK.
    static func keySchedule(_ sharedSecret: Data, _ info: Data) -> (key: SymmetricKey, nonce: Data) {
        let pskIdHash = labeledExtract(hpkeSuite, Data(), "psk_id_hash", Data())
        let infoHash = labeledExtract(hpkeSuite, Data(), "info_hash", info)
        let context = Data([modeAuth]) + pskIdHash + infoHash
        let secret = labeledExtract(hpkeSuite, sharedSecret, "secret", Data())
        return (
            key: SymmetricKey(data: labeledExpand(hpkeSuite, secret, "key", context, 32)),
            nonce: labeledExpand(hpkeSuite, secret, "base_nonce", context, 12)
        )
    }
}

extension BigEndian {
    static func u16(_ value: UInt16) -> Data {
        withUnsafeBytes(of: value.bigEndian) { Data($0) }
    }
}

/// Seal `plaintext` to `recipientPublic` in Auth mode as the holder of
/// `senderSecret` (32 raw X25519 bytes). `ephemeralSecret` is for tests.
/// (== mero-js `hpkeSealAuth`.)
public func hpkeSealAuth(
    recipientPublic: Data, senderSecret: Data, info: Data, plaintext: Data, aad: Data = Data(),
    ephemeralSecret: Data? = nil
) throws -> HpkeSealed {
    guard recipientPublic.count == 32 else { throw TeeTrustError("The recipient key is not 32 bytes of X25519") }
    let ephemeral = try NoiseDH.keyPair(ephemeralSecret)
    let sender = try NoiseDH.keyPair(senderSecret)
    let enc = ephemeral.publicKey.rawRepresentation
    let dh = try NoiseDH.dh(ephemeral, recipientPublic) + NoiseDH.dh(sender, recipientPublic)
    let shared = Hpke.extractAndExpand(dh, enc + recipientPublic + sender.publicKey.rawRepresentation)
    let schedule = Hpke.keySchedule(shared, info)
    let box = try ChaChaPoly.seal(
        plaintext, using: schedule.key, nonce: try ChaChaPoly.Nonce(data: schedule.nonce), authenticating: aad)
    return HpkeSealed(enc: enc, ciphertext: box.ciphertext + box.tag)
}

/// Open what ``hpkeSealAuth(recipientPublic:senderSecret:info:plaintext:aad:ephemeralSecret:)``
/// (or the gate's own sealer) made, as the recipient. (== mero-js `hpkeOpenAuth`.)
public func hpkeOpenAuth(
    recipientSecret: Data, senderPublic: Data, info: Data, sealed: HpkeSealed, aad: Data = Data()
) throws -> Data {
    let recipient = try NoiseDH.keyPair(recipientSecret)
    let dh = try NoiseDH.dh(recipient, sealed.enc) + NoiseDH.dh(recipient, senderPublic)
    let shared = Hpke.extractAndExpand(dh, sealed.enc + recipient.publicKey.rawRepresentation + senderPublic)
    let schedule = Hpke.keySchedule(shared, info)
    let ciphertext = Data(sealed.ciphertext)
    guard ciphertext.count >= 16 else { throw TeeTrustError("The message did not authenticate") }
    do {
        let box = try ChaChaPoly.SealedBox(
            nonce: try ChaChaPoly.Nonce(data: schedule.nonce),
            ciphertext: Data(ciphertext.prefix(ciphertext.count - 16)), tag: Data(ciphertext.suffix(16)))
        return try ChaChaPoly.open(box, using: schedule.key, authenticating: aad)
    } catch {
        throw TeeTrustError("The message did not authenticate")
    }
}
