import CryptoKit
import Foundation

// The Noise NK handshake (`Noise_NK_25519_AESGCM_SHA256`) that opens a sealed
// session, written against CryptoKit from the Noise specification (revision
// 34). Port of mero-js `src/sealed/noise.ts`; the node runs the other side
// with `snow`, and core's published vectors check the two against each other.
//
// NK: the initiator knows the responder's static key in advance — the
// transport key the node's quote commits to — and sends nothing but an
// ephemeral key before both ephemerals are mixed in, so everything after the
// handshake has forward secrecy.

/// Noise's `SymmetricState`: the chaining key, the handshake hash and the
/// cipher key. (== mero-js `SymmetricState`.)
struct NoiseSymmetricState {
    static let protocolName = "Noise_NK_25519_AESGCM_SHA256"

    private var ck: Data
    private(set) var h: Data
    private var k: SymmetricKey?
    private var n: UInt64 = 0

    init() {
        // A name of 32 bytes or fewer is used as the hash, zero-padded.
        var name = Data(Self.protocolName.utf8)
        name.append(Data(count: 32 - name.count))
        ck = name
        h = name
    }

    mutating func mixHash(_ data: Data) {
        h = sha256(h + data)
    }

    mutating func mixKey(_ inputKeyMaterial: Data) {
        let (chainingKey, key) = Self.hkdf2(ck, inputKeyMaterial)
        ck = chainingKey
        k = SymmetricKey(data: key)
        n = 0
    }

    mutating func encryptAndHash(_ plaintext: Data) throws -> Data {
        guard let k else { throw TeeTrustError("Noise: no cipher key yet") }
        let box = try AES.GCM.seal(plaintext, using: k, nonce: try Self.nonce(n), authenticating: h)
        let ciphertext = box.ciphertext + box.tag
        n += 1
        mixHash(ciphertext)
        return ciphertext
    }

    mutating func decryptAndHash(_ ciphertext: Data) throws -> Data {
        guard let k else { throw TeeTrustError("Noise: no cipher key yet") }
        guard ciphertext.count >= 16 else { throw TeeTrustError("Noise: the ciphertext is too short") }
        let box = try AES.GCM.SealedBox(
            nonce: try Self.nonce(n), ciphertext: Data(ciphertext.prefix(ciphertext.count - 16)),
            tag: Data(ciphertext.suffix(16)))
        let plaintext = try AES.GCM.open(box, using: k, authenticating: h)
        n += 1
        mixHash(ciphertext)
        return plaintext
    }

    /// The initiator-to-responder key, then the responder-to-initiator key.
    func split() -> (Data, Data) {
        Self.hkdf2(ck, Data())
    }

    /// Noise's AESGCM nonce: 32 zero bits, then the counter as a big-endian u64.
    static func nonce(_ n: UInt64) throws -> AES.GCM.Nonce {
        try AES.GCM.Nonce(data: Data(count: 4) + BigEndian.u64(n))
    }

    /// Noise's `HKDF` with two outputs.
    static func hkdf2(_ chainingKey: Data, _ inputKeyMaterial: Data) -> (Data, Data) {
        let temp = hmacSha256(chainingKey, inputKeyMaterial)
        let first = hmacSha256(temp, Data([1]))
        let second = hmacSha256(temp, first + Data([2]))
        return (first, second)
    }
}

/// HMAC-SHA-256 of `data` under `key`.
func hmacSha256(_ key: Data, _ data: Data) -> Data {
    Data(HMAC<SHA256>.authenticationCode(for: data, using: SymmetricKey(data: key)))
}

/// X25519 over CryptoKit, as the sealed transport and HPKE use it.
enum NoiseDH {
    /// A key pair from a 32-byte secret, or a fresh one.
    static func keyPair(_ secret: Data? = nil) throws -> Curve25519.KeyAgreement.PrivateKey {
        guard let secret else { return Curve25519.KeyAgreement.PrivateKey() }
        guard secret.count == 32 else { throw TeeTrustError("An X25519 secret is 32 bytes") }
        return try Curve25519.KeyAgreement.PrivateKey(rawRepresentation: secret)
    }

    /// The 32-byte shared secret of `privateKey` and `publicKey`.
    static func dh(_ privateKey: Curve25519.KeyAgreement.PrivateKey, _ publicKey: Data) throws -> Data {
        let peer = try Curve25519.KeyAgreement.PublicKey(rawRepresentation: publicKey)
        let shared = try privateKey.sharedSecretFromKeyAgreement(with: peer)
        return shared.withUnsafeBytes { Data($0) }
    }
}

/// Big-endian encodings (the sealed wire format's integers).
enum BigEndian {
    static func u64(_ value: UInt64) -> Data {
        withUnsafeBytes(of: value.bigEndian) { Data($0) }
    }

    static func u32(_ value: UInt32) -> Data {
        withUnsafeBytes(of: value.bigEndian) { Data($0) }
    }

    static func readU32(_ bytes: Data, at offset: Int = 0) -> UInt32 {
        let b = [UInt8](bytes.dropFirst(offset).prefix(4))
        guard b.count == 4 else { return 0 }
        return UInt32(b[0]) << 24 | UInt32(b[1]) << 16 | UInt32(b[2]) << 8 | UInt32(b[3])
    }
}

/// The initiator's half of an NK handshake to `remoteStatic`. (== mero-js `initiate`.)
struct NoiseInitiator {
    static let dhLength = 32
    static let tagLength = 16

    private var state: NoiseSymmetricState
    private let ephemeral: Curve25519.KeyAgreement.PrivateKey
    /// `e`, then the (empty) payload sealed under `es`.
    let message1: Data

    /// `fixedEphemeral` pins the ephemeral secret, for the published test vectors only.
    init(remoteStatic: Data, prologue: Data, fixedEphemeral: Data? = nil) throws {
        var state = NoiseSymmetricState()
        state.mixHash(prologue)
        // <- s (known in advance)
        state.mixHash(remoteStatic)
        // -> e, es
        let ephemeral = try NoiseDH.keyPair(fixedEphemeral)
        let ephemeralPublic = ephemeral.publicKey.rawRepresentation
        state.mixHash(ephemeralPublic)
        state.mixKey(try NoiseDH.dh(ephemeral, remoteStatic))
        let sealed = try state.encryptAndHash(Data())
        message1 = ephemeralPublic + sealed
        self.state = state
        self.ephemeral = ephemeral
    }

    /// Read the responder's `e, ee` and its payload; return the payload and
    /// the transport keys (`Split()`).
    func finish(_ message2: Data) throws -> (payload: Data, split: (Data, Data)) {
        let message = Data(message2)
        guard message.count >= Self.dhLength + Self.tagLength else {
            throw TeeTrustError("Noise: message 2 is too short")
        }
        var state = self.state
        // <- e, ee
        let remoteEphemeral = Data(message.prefix(Self.dhLength))
        state.mixHash(remoteEphemeral)
        state.mixKey(try NoiseDH.dh(ephemeral, remoteEphemeral))
        let payload = try state.decryptAndHash(Data(message.dropFirst(Self.dhLength)))
        return (payload: payload, split: state.split())
    }
}
