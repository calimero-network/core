import CryptoKit
import Foundation

/// What a package's served page hands a native app after it signed the person in.
public struct ServedPageSession: Codable, Sendable, Equatable {
    public var v: Int
    public var accountId: String
    public var sessionToken: String
    public var expiresAt: Double
    public var refreshToken: String
    /// The node the page signed in on, and the rest of its cluster.
    public var nodeUrl: String
    public var nodes: [String]

    public init(
        v: Int = 1, accountId: String, sessionToken: String, expiresAt: Double, refreshToken: String, nodeUrl: String,
        nodes: [String] = []
    ) {
        self.v = v
        self.accountId = accountId
        self.sessionToken = sessionToken
        self.expiresAt = expiresAt
        self.refreshToken = refreshToken
        self.nodeUrl = nodeUrl
        self.nodes = nodes
    }

    public var session: CalimeroSession {
        CalimeroSession(
            accountId: accountId, sessionToken: sessionToken, expiresAt: expiresAt, refreshToken: refreshToken)
    }
}

/// Why a hand-off was refused.
public struct ServedPageSignInError: Error, LocalizedError, Sendable, Equatable {
    public let message: String
    public init(_ message: String) { self.message = message }
    public var errorDescription: String? { message }
}

/// One native sign-in through a package's served page.
/// (== apps `@calimero-apps/passkey/native`.)
///
/// A native app can't use a `calimero.network` passkey safely itself: iOS
/// reports the assertion from the apex origin, which matches no package, so the
/// node would open an unbound session. Instead the app opens its package's
/// served page in the system sign-in sheet (``url(origin:version:)``). The
/// person signs in there as on the web, which binds the session to the package,
/// and the page sends the session back to the app's callback, sealed to this
/// hand-off's key (``open(_:)``).
///
/// Each hand-off makes a fresh P-256 key and `state`, and opens one answer only.
/// The box is `base64url(epk ‖ iv ‖ ciphertext)`: ECDH P-256 with `epk`,
/// HKDF-SHA256 (empty salt, info ``info``, 32 bytes), AES-256-GCM with the
/// UTF-8 `state` as additional data.
public final class ServedPageHandoff: @unchecked Sendable {
    public static let info = "calimero.native-sign-in.v1"
    /// The fragment that marks a native sign-in on the page.
    public static let fragment = "native-sign-in"
    /// An approval's HKDF info and fragment (``ServedPageApproval``).
    public static let approveInfo = "calimero.native-approve.v1"
    public static let approveFragment = "native-approve"

    /// The served page as a consent page's `returnTo`: it passes the answer on
    /// to the app (`callback?request_id=…&result=…`).
    public static func consentReturnTo(origin: URL, version: String) -> String {
        let base =
            origin.absoluteString.hasSuffix("/") ? String(origin.absoluteString.dropLast()) : origin.absoluteString
        return "\(base)/\(version)/?native-consent=1"
    }

    private let privateKey: P256.KeyAgreement.PrivateKey
    public let state: String
    private let lock = NSLock()
    private var used = false

    public convenience init() {
        var bytes = [UInt8](repeating: 0, count: 32)
        for i in bytes.indices { bytes[i] = UInt8.random(in: 0...255) }
        self.init(privateKey: P256.KeyAgreement.PrivateKey(), state: b64url(Data(bytes)))
    }

    public init(privateKey: P256.KeyAgreement.PrivateKey, state: String) {
        self.privateKey = privateKey
        self.state = state
    }

    /// The public key the page seals to: base64url of the uncompressed point (65 bytes).
    public var publicKey: String { b64url(privateKey.publicKey.x963Representation) }

    /// The page to open: `{origin}/{version}/#native-sign-in&state=…&key=…`.
    /// The parameters ride in the fragment, so no server sees them.
    public func url(origin: URL, version: String) -> URL {
        url(origin: origin, version: version, fragment: Self.fragment)
    }

    /// `{origin}/{version}/#{fragment}&state=…&key=…`, then `extra` (base64url or
    /// rp-id values, so nothing in them needs escaping).
    public func url(origin: URL, version: String, fragment: String, extra: [(String, String)] = []) -> URL {
        let base =
            origin.absoluteString.hasSuffix("/") ? String(origin.absoluteString.dropLast()) : origin.absoluteString
        let more = extra.map { "&\($0.0)=\($0.1)" }.joined()
        return URL(string: "\(base)/\(version)/#\(fragment)&state=\(state)&key=\(publicKey)\(more)")!
    }

    /// The session in the page's answer (`callback#state=…&box=…`). Throws on
    /// another `state`, a box this key can't open, or a second answer.
    public func open(_ callback: URL) throws -> ServedPageSession {
        let plain = try openBox(callback, info: Self.info)
        guard let session = try? JSONDecoder().decode(ServedPageSession.self, from: plain) else {
            throw ServedPageSignInError("the sign-in answer could not be opened")
        }
        guard session.v == 1 else { throw ServedPageSignInError("unknown sign-in answer version \(session.v)") }
        return session
    }

    /// The assertion in an approval page's answer. Throws ``PasskeyCancelled``
    /// when the person declined there (`#state=…&cancelled=1`).
    public func openApproval(_ callback: URL) throws -> AssertionJSON {
        struct Answer: Decodable {
            let v: Int
            let assertion: AssertionJSON
        }
        if value("cancelled", in: callback) == "1", value("state", in: callback) == state {
            throw PasskeyCancelled("the approval was declined")
        }
        let plain = try openBox(callback, info: Self.approveInfo)
        guard let answer = try? JSONDecoder().decode(Answer.self, from: plain), answer.v == 1 else {
            throw ServedPageSignInError("the approval answer could not be opened")
        }
        return answer.assertion
    }

    private func value(_ name: String, in callback: URL) -> String? {
        var parts = URLComponents()
        parts.percentEncodedQuery = callback.fragment ?? ""
        return parts.queryItems?.first { $0.name == name }?.value
    }

    /// The plaintext of `callback#state=…&box=…`, opened once, under HKDF `info`.
    private func openBox(_ callback: URL, info: String) throws -> Data {
        guard let state = value("state", in: callback), let box = value("box", in: callback) else {
            throw ServedPageSignInError("the page sent nothing back")
        }
        guard state == self.state else { throw ServedPageSignInError("the answer is not for this request") }
        lock.lock()
        let first = !used
        used = true
        lock.unlock()
        guard first else { throw ServedPageSignInError("this request was already answered") }
        let bytes = try fromB64url(box)
        guard bytes.count > 65 + 12 + 16 else { throw ServedPageSignInError("the answer is malformed") }
        do {
            let epk = try P256.KeyAgreement.PublicKey(x963Representation: bytes.prefix(65))
            let shared = try privateKey.sharedSecretFromKeyAgreement(with: epk)
            let key = shared.hkdfDerivedSymmetricKey(
                using: SHA256.self, salt: Data(), sharedInfo: Data(info.utf8), outputByteCount: 32)
            let sealed = try AES.GCM.SealedBox(combined: bytes.dropFirst(65))
            return try AES.GCM.open(sealed, using: key, authenticating: Data(state.utf8))
        } catch {
            throw ServedPageSignInError("the answer could not be opened")
        }
    }
}
