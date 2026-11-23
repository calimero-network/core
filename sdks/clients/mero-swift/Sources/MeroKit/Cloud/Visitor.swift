import Foundation

// An anonymous visitor through a public link (core's `/apps/{app}/public-links`):
// the credential each request carries to call an instance's `#[app::public]`
// methods, made of the link's token and the visitor's own Ed25519 key, signed
// fresh per request. The node names the visitor to the app as
// `Caller::Visitor { link, key }`. Mirrors core's `calimero_client::visitor` and
// mero-js's `client/visitor.ts`; `VisitorTests` checks the vector they check.

private let publicLinkDomain = "calimero.public-link.v1\0"
private let visitorDomain = "calimero.visitor.v1\0"

/// A public link token's key (hex): how the node and the app name the link.
public func publicLinkKey(_ token: String) throws -> String {
    Hex.encode(try Ed25519.publicKey(seed: sha256(Data((publicLinkDomain + token).utf8))))
}

/// A new visitor seed: 32 random bytes. Keep it (in the keychain, say) to stay
/// the same visitor.
public func newVisitorSeed() -> Data {
    randomBytes(32)
}

/// A visitor of one public link: its token and the visitor's key.
/// ``CalimeroClient/useVisitor(link:seed:)`` makes one and signs every request with it.
public struct Visitor: Sendable, CustomStringConvertible {
    private let token: String
    private let seed: Data
    /// The link's key (hex).
    public let link: String
    /// The visitor's public key (64 lowercase hex): how the app names it.
    public let publicKey: String

    /// The visitor of the link whose token is `token`, with the key whose seed
    /// is `seed` (32 bytes).
    public init(token: String, seed: Data) throws {
        guard seed.count == 32 else {
            throw AccountError.invalidInput("a visitor seed is 32 bytes, got \(seed.count)")
        }
        guard !token.isEmpty else { throw AccountError.invalidInput("a public link token is not empty") }
        self.token = token
        self.seed = Data(seed)
        self.link = try publicLinkKey(token)
        self.publicKey = Hex.encode(try Ed25519.publicKey(seed: self.seed))
    }

    /// The bytes the visitor signs for a request at `at`:
    /// `"calimero.visitor.v1\0" || link || 0 || key || 0 || at`.
    public func message(at: Int64) -> Data {
        Data("\(visitorDomain)\(link)\0\(publicKey)\0\(at)".utf8)
    }

    /// The bearer credential for a request at `at` (Unix seconds; default
    /// now): `vis1.<token>.<key>.<at>.<signature>`, valid within 5 minutes of
    /// the node's clock.
    public func bearer(at: Int64 = Int64(Date().timeIntervalSince1970)) throws -> String {
        let signature = try Ed25519.sign(seed: seed, message: message(at: at))
        return "vis1.\(token).\(publicKey).\(at).\(Hex.encode(signature))"
    }

    public var description: String { "Visitor(publicKey: \(publicKey))" }
}
