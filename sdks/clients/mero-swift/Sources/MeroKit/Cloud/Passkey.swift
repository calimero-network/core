import CryptoKit
import Foundation

// Passkeys: the platform's WebAuthn authenticator, or anything that answers
// like it. Port of mero-js `cloud-v2/passkey.ts`. Every binary field crosses
// the wire as base64url, matching `calimero-identity`'s `Registration` and
// `Assertion`.

/// A passkey registration, as `POST /auth/passkey/register/finish` takes it.
/// (== mero-js `RegistrationJSON`.)
public struct RegistrationJSON: Codable, Sendable, Equatable {
    public var attestationObject: String
    public var clientDataJson: String

    public init(attestationObject: String, clientDataJson: String) {
        self.attestationObject = attestationObject
        self.clientDataJson = clientDataJson
    }
}

/// A passkey assertion, as sign-in and confirmations take it. (== mero-js `AssertionJSON`.)
public struct AssertionJSON: Codable, Sendable, Equatable {
    public var credentialId: String
    public var authenticatorData: String
    public var clientDataJson: String
    public var signature: String

    public init(credentialId: String, authenticatorData: String, clientDataJson: String, signature: String) {
        self.credentialId = credentialId
        self.authenticatorData = authenticatorData
        self.clientDataJson = clientDataJson
        self.signature = signature
    }
}

/// Something that holds a passkey. (== mero-js `PasskeyProvider`.)
///
/// `MeroKitUI`'s `PlatformPasskey` is the device's (Face ID, Touch ID, a
/// synced passkey); ``SoftPasskey`` is a software one for development
/// clusters and tests. A prompt the person dismissed throws ``PasskeyCancelled``.
public protocol PasskeyProvider: Sendable {
    /// Answer `navigator.credentials.create` for the node's `options`
    /// (`challenge`, `rp`, `user`, `pubKeyCredParams`, … as the node sends them).
    func create(options: JSONValue) async throws -> RegistrationJSON
    /// Sign `challenge` (an assertion) for relying party `rpId`, optionally
    /// limited to `allowCredentials` (base64url credential ids).
    func get(challenge: Data, rpId: String, allowCredentials: [String]?) async throws -> AssertionJSON
}

/// The person dismissed a passkey prompt. A confirmation reads it as
/// declined (``ConfirmCancelled``). (== a WebAuthn `NotAllowedError`.)
public struct PasskeyCancelled: Error, LocalizedError, Sendable, Equatable {
    public let message: String

    public init(_ message: String = "the passkey prompt was cancelled") {
        self.message = message
    }

    public var errorDescription: String? { message }
}

/// Bytes as base64url without padding. (== mero-js `b64url`.)
public func b64url(_ bytes: Data) -> String {
    bytes.base64EncodedString()
        .replacingOccurrences(of: "+", with: "-")
        .replacingOccurrences(of: "/", with: "_")
        .replacingOccurrences(of: "=", with: "")
}

/// base64url (padded or not) as bytes. (== mero-js `fromB64url`.)
public func fromB64url(_ string: String) throws -> Data {
    var s = string.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
    s += String(repeating: "=", count: (4 - s.count % 4) % 4)
    guard let data = Data(base64Encoded: s) else { throw AccountError.invalidInput("not base64url: \(string)") }
    return data
}

/// A software passkey (ES256, attestation `none`): what a development cluster
/// (`merod local`, mock attestation) takes in place of the device's own.
/// Port of core's `calimero_identity::testing::SoftAuthenticator`, and the
/// same JSON as `meroctl login --dev-passkey` keeps (``json``), so one file
/// works in both. (== calimero-client-py `SoftPasskey`.)
///
/// Whoever holds its JSON holds the account: never use one against a real
/// cluster, where sign-in needs the device's passkey.
public final class SoftPasskey: PasskeyProvider, @unchecked Sendable {
    private let lock = NSLock()
    private let key: P256.Signing.PrivateKey
    private let credential: Data
    public let rpId: String
    public let origin: String
    private var counter: UInt32

    /// A new passkey for `rpId`, used from `origin` (one of the node's `--origins`).
    public init(rpId: String = "localhost", origin: String = "http://localhost:3000") {
        key = P256.Signing.PrivateKey()
        credential = randomBytes(16)
        self.rpId = rpId
        self.origin = origin
        counter = 0
    }

    /// Load a passkey saved with ``json`` (or by `meroctl login --dev-passkey`).
    public init(json: Data) throws {
        guard let value = try? MeroJSON.decode(JSONValue.self, from: json),
            let pkcs8 = value["pkcs8"]?.stringValue, let id = value["credentialId"]?.stringValue,
            let rpId = value["rpId"]?.stringValue, let origin = value["origin"]?.stringValue
        else { throw AccountError.invalidInput("not a passkey file") }
        do {
            key = try P256.Signing.PrivateKey(derRepresentation: try fromB64url(pkcs8))
        } catch {
            throw AccountError.invalidInput("the passkey file's key does not load")
        }
        credential = try fromB64url(id)
        self.rpId = rpId
        self.origin = origin
        counter = UInt32(max(0, value["counter"]?.intValue ?? 0))
    }

    /// The passkey, private key included, as JSON (`pkcs8`, `credentialId`, `rpId`, `origin`, `counter`).
    public var json: Data {
        let counter = locked { self.counter }
        let value: JSONValue = [
            "pkcs8": .string(b64url(key.derRepresentation)), "credentialId": .string(credentialId),
            "rpId": .string(rpId), "origin": .string(origin), "counter": .number(Double(counter)),
        ]
        return CanonicalJSON.data(value)
    }

    /// Its credential id, base64url.
    public var credentialId: String { b64url(credential) }

    public func create(options: JSONValue) async throws -> RegistrationJSON {
        guard let challenge = options["challenge"]?.stringValue else {
            throw AccountError.protocolViolation("the registration options carry no challenge")
        }
        let raw = key.publicKey.x963Representation  // 0x04 ‖ x ‖ y
        var cose = Cbor.map(5)
        cose += Cbor.int(1) + Cbor.int(2)  // kty: EC2
        cose += Cbor.int(3) + Cbor.int(-7)  // alg: ES256
        cose += Cbor.int(-1) + Cbor.int(1)  // crv: P-256
        cose += Cbor.int(-2) + Cbor.bytes(raw.subdata(in: 1..<33))
        cose += Cbor.int(-3) + Cbor.bytes(raw.subdata(in: 33..<65))
        var attested = Data(count: 16)  // AAGUID
        attested += BigEndian.u16(UInt16(credential.count)) + credential + cose
        let authData = authenticatorData(flags: 0x01 | 0x04 | 0x40, counter: locked { counter }) + attested
        var attestation = Cbor.map(3)
        attestation += Cbor.text("fmt") + Cbor.text("none")
        attestation += Cbor.text("attStmt") + Cbor.map(0)
        attestation += Cbor.text("authData") + Cbor.bytes(authData)
        return RegistrationJSON(
            attestationObject: b64url(attestation),
            clientDataJson: b64url(clientData("webauthn.create", challenge: try fromB64url(challenge))))
    }

    public func get(challenge: Data, rpId: String, allowCredentials: [String]?) async throws -> AssertionJSON {
        let counter = locked {
            self.counter += 1
            return self.counter
        }
        let client = clientData("webauthn.get", challenge: challenge)
        let authData = authenticatorData(flags: 0x01 | 0x04, counter: counter)
        let signature = try key.signature(for: authData + sha256(client))
        return AssertionJSON(
            credentialId: credentialId, authenticatorData: b64url(authData), clientDataJson: b64url(client),
            signature: b64url(signature.derRepresentation))
    }

    private func clientData(_ type: String, challenge: Data) -> Data {
        let value: JSONValue = [
            "type": .string(type), "challenge": .string(b64url(challenge)), "origin": .string(origin),
            "crossOrigin": false,
        ]
        return CanonicalJSON.data(value)
    }

    private func authenticatorData(flags: UInt8, counter: UInt32) -> Data {
        sha256(Data(rpId.utf8)) + Data([flags]) + BigEndian.u32(counter)
    }

    private func locked<T>(_ body: () -> T) -> T {
        lock.lock()
        defer { lock.unlock() }
        return body()
    }
}

/// The few CBOR items a passkey registration needs (RFC 8949, definite lengths).
enum Cbor {
    static func head(_ major: UInt8, _ value: UInt64) -> Data {
        let tag = major << 5
        switch value {
        case 0..<24: return Data([tag | UInt8(value)])
        case 24..<256: return Data([tag | 24, UInt8(value)])
        case 256..<65_536: return Data([tag | 25]) + BigEndian.u16(UInt16(value))
        default: return Data([tag | 26]) + BigEndian.u32(UInt32(value))
        }
    }

    static func int(_ value: Int) -> Data {
        value >= 0 ? head(0, UInt64(value)) : head(1, UInt64(-1 - value))
    }

    static func bytes(_ value: Data) -> Data { head(2, UInt64(value.count)) + value }
    static func text(_ value: String) -> Data { head(3, UInt64(value.utf8.count)) + Data(value.utf8) }
    static func map(_ count: Int) -> Data { head(5, UInt64(count)) }
}
