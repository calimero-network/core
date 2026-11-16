import Foundation

#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

// A client for mero-agent-gate, the gate in a private agent TD (mero-tee,
// `mero-agent-gate/`): attest an agent, claim it with your key, give it its
// secrets sealed to the TD, and release it. Port of mero-js
// `src/agent-gate/gate.ts`; the wire shapes are mero-agent-gate's
// `protocol.rs` (`POST /attest`, `/provision`, `/release`). It runs where the
// owner is, so whoever carries the requests sees only quotes and ciphertext.
//
// One agent TD hosts several owners' agents, each named by an agent id. For
// an agent, the gate's quote binds in its report data:
//
//   nonce ‖ SHA-256("mero-agent-gate/attest/v3" ‖ provisioning_pub ‖ len(id) ‖ id ‖ signing_pub ‖ owner_pub)
//
// (signing and owner keys 32 zero bytes while unclaimed). `attestAgent`
// checks that and the quote itself before returning anything; every function
// that sends refuses an agent whose quote names another owner.

/// The gate's domains and limits.
public enum AgentGate {
    /// The report-data binding's domain. (== mero-js `AGENT_ATTEST_DOMAIN`.)
    public static let attestDomain = "mero-agent-gate/attest/v3"
    /// The HPKE `info` prefix. (== mero-js `AGENT_PROVISION_INFO_DOMAIN`.)
    public static let provisionInfoDomain = "mero-agent-gate/provision/v2"
    static let maxSecrets = 64
    static let maxSecretLength = 64 * 1024
}

/// A gate's refusal, with its `error` code (`not_the_owner`, `no_free_slot`,
/// …) when it gave one. (== mero-js `AgentGateError`.)
public struct AgentGateError: Error, LocalizedError, Sendable, Equatable {
    public let message: String
    public let status: Int?
    public let code: String?

    public init(_ message: String, status: Int? = nil, code: String? = nil) {
        self.message = message
        self.status = status
        self.code = code
    }

    public var errorDescription: String? { message }
}

/// An agent's keys, once its quote passed ``attestAgent(_:)``'s checks.
/// (== mero-js `VerifiedAgent`.)
public struct VerifiedAgent: Sendable, Equatable {
    public var agentId: String
    /// Base64; secrets are sealed to it. Valid until the gate restarts.
    public var provisioningPublicKey: String
    /// Base64 Ed25519 key the agent signs with; `nil` while unclaimed.
    public var signingPublicKey: String?
    /// Base64 X25519 key of the owner that claimed it; `nil` while unclaimed.
    public var ownerPublicKey: String?

    public init(
        agentId: String, provisioningPublicKey: String, signingPublicKey: String? = nil, ownerPublicKey: String? = nil
    ) {
        self.agentId = agentId
        self.provisioningPublicKey = provisioningPublicKey
        self.signingPublicKey = signingPublicKey
        self.ownerPublicKey = ownerPublicKey
    }
}

/// An owner's X25519 key. (== mero-js `OwnerKey`.)
public struct OwnerKey: Sendable, Equatable {
    /// 32 raw bytes: the only key that can provision or release the agent. Keep it.
    public let secret: Data
    /// Base64 of the public half: what the gate's quote names as owner.
    public let publicKey: String
}

/// What ``provisionAgent(_:)`` returns: the agent, claimed, and what was written.
/// (== mero-js `ProvisionedAgent`.)
public struct ProvisionedAgent: Sendable, Equatable {
    public let agentId: String
    public let provisioningPublicKey: String
    public let signingPublicKey: String
    public let ownerPublicKey: String
    /// The secret names written, sorted.
    public let written: [String]
    /// Whether this claimed the agent for your key.
    public let claimed: Bool
}

/// Refuse an agent id that is not `[a-z0-9][a-z0-9-]{0,62}`. (== mero-js `validateAgentId`.)
public func validateAgentId(_ agentId: String) throws {
    let bytes = Array(agentId.utf8)
    let first = bytes.first.map { (0x61...0x7a).contains($0) || (0x30...0x39).contains($0) } ?? false
    let rest = bytes.allSatisfy { (0x61...0x7a).contains($0) || (0x30...0x39).contains($0) || $0 == 0x2d }
    guard first, rest, bytes.count <= 63 else {
        throw AccountError.invalidInput(
            "Agent id \(CanonicalJSON.quote(agentId)) must match [a-z0-9][a-z0-9-]{0,62}")
    }
}

/// The agent id, length first, so no id is a prefix of another's encoding.
private func lengthPrefixed(_ agentId: String) -> Data {
    let id = Data(agentId.utf8)
    return Data([UInt8(min(id.count, 255))]) + id
}

/// The 32 bytes after the nonce in a gate's quote for `agentId` (empty: the
/// TD alone); signing and owner keys are zeros while unclaimed.
/// Pinned to mero-agent-gate's `the_binding_is_pinned`. (== mero-js `agentKeyBinding`.)
public func agentKeyBinding(
    provisioningPublic: Data, agentId: String, signingPublic: Data? = nil, ownerPublic: Data? = nil
) -> Data {
    let zero = Data(count: 32)
    return sha256(
        Data(AgentGate.attestDomain.utf8) + provisioningPublic + lengthPrefixed(agentId) + (signingPublic ?? zero)
            + (ownerPublic ?? zero))
}

/// HPKE `info` for a message to `agentId` on the gate whose provisioning
/// key is `recipientPublic`. (== mero-js `provisionInfo`.)
public func provisionInfo(recipientPublic: Data, agentId: String) -> Data {
    Data(AgentGate.provisionInfoDomain.utf8) + recipientPublic + lengthPrefixed(agentId)
}

/// A fresh owner key, made here; it never needs to leave the owner.
/// (== mero-js `generateOwnerKey`.)
public func generateOwnerKey() throws -> OwnerKey {
    let secret = randomBytes(32)
    return OwnerKey(secret: secret, publicKey: try ownerPublicKey(secret))
}

/// The base64 public half of an owner's X25519 secret. (== mero-js `ownerPublicKey`.)
public func ownerPublicKey(_ secret: Data) throws -> String {
    guard secret.count == 32 else { throw AccountError.invalidInput("An owner key is 32 bytes") }
    return try NoiseDH.keyPair(secret).publicKey.rawRepresentation.base64EncodedString()
}

/// What ``attestAgent(_:)`` needs. (== mero-js `AttestAgentOptions`.)
public struct AttestAgentOptions: Sendable {
    /// The gate's base URL, or a relay of it: only verified quotes are trusted.
    public var gateURL: String
    /// The agent to attest; `nil` for the TD alone (no keys, nothing to send to).
    public var agentId: String?
    /// Checks the quote's signature, TCB, image and report data:
    /// ``agentQuoteVerifier(_:fetchCollateral:dcapVerify:now:)``.
    public var verifier: any TransportQuoteVerifier
    public var session: URLSession

    public init(
        gateURL: String, agentId: String? = nil, verifier: any TransportQuoteVerifier, session: URLSession = .shared
    ) {
        self.gateURL = gateURL
        self.agentId = agentId
        self.verifier = verifier
        self.session = session
    }
}

/// Ask the gate for a quote for `agentId` over a fresh nonce and verify it:
/// a genuine quote from a measured agent image, for this nonce, committing
/// to the agent id asked about and to the keys the gate names. Returns those
/// keys only once every check has passed. (== mero-js `attestAgent`.)
public func attestAgent(_ options: AttestAgentOptions) async throws -> VerifiedAgent {
    let agentId = options.agentId ?? ""
    if !agentId.isEmpty { try validateAgentId(agentId) }
    let nonce = randomBytes(32)
    var request: [String: JSONValue] = ["nonceB64": .string(nonce.base64EncodedString())]
    if !agentId.isEmpty { request["agentId"] = .string(agentId) }
    let body = try await AgentGateHTTP.post(options.session, options.gateURL, "/attest", .object(request))
    let answeredFor = body["agentId"]?.stringValue
    guard (answeredFor ?? "") == agentId else {
        let named = answeredFor.map(CanonicalJSON.quote) ?? "undefined"
        throw TeeTrustError("Asked about agent \(CanonicalJSON.quote(agentId)), the gate answered for \(named)")
    }
    let provisioning = try AgentGateHTTP.key(
        body["provisioningPublicKeyB64"]?.stringValue ?? "", "provisioningPublicKeyB64")
    let signing = try AgentGateHTTP.optionalKey(body["signingPublicKeyB64"]?.stringValue, "signingPublicKeyB64")
    let owner = try AgentGateHTTP.optionalKey(body["ownerPublicKeyB64"]?.stringValue, "ownerPublicKeyB64")
    guard (signing == nil) == (owner == nil) else {
        throw TeeTrustError("A claimed agent has both a signing key and an owner; the gate named one")
    }
    // The id asked about, not the one answered: a quote for another agent fails here.
    let binding = agentKeyBinding(
        provisioningPublic: provisioning, agentId: agentId, signingPublic: signing, ownerPublic: owner)
    guard let quote = body["quoteB64"]?.stringValue else { throw TeeTrustError("The gate returned no quote") }
    let accepted = try await options.verifier.verify(
        TransportQuoteArgs(quoteB64: quote, nonce: Hex.encode(nonce), reportDataSuffix: Hex.encode(binding)))
    guard accepted else { throw TeeTrustError("The gate's quote did not verify") }
    return VerifiedAgent(
        agentId: agentId, provisioningPublicKey: provisioning.base64EncodedString(),
        signingPublicKey: signing?.base64EncodedString(), ownerPublicKey: owner?.base64EncodedString())
}

/// The owner key `ownerSecret` may act on `agent`: it is unclaimed, or
/// claimed by that key. Returns the key's public half. (== mero-js `checkOwner`.)
@discardableResult
public func checkOwner(_ agent: VerifiedAgent, ownerSecret: Data) throws -> String {
    let mine = try ownerPublicKey(ownerSecret)
    if let owner = agent.ownerPublicKey, owner != mine {
        throw AgentGateError(
            "This agent is claimed by another key (\(owner)); it is not yours. Send it nothing and do not "
                + "authorize its signing key",
            code: "not_yours")
    }
    return mine
}

/// What ``provisionAgent(_:)`` needs. (== mero-js `ProvisionAgentOptions`.)
public struct ProvisionAgentOptions: Sendable {
    public var gateURL: String
    /// What ``attestAgent(_:)`` returned for this agent, just now.
    public var agent: VerifiedAgent
    public var ownerSecret: Data
    /// Name to value; a name replaces what was there, others stay.
    public var secrets: [String: String]
    /// The verifier `agent` was attested with: the claim is checked by a fresh quote.
    public var verifier: any TransportQuoteVerifier
    public var session: URLSession

    public init(
        gateURL: String, agent: VerifiedAgent, ownerSecret: Data, secrets: [String: String],
        verifier: any TransportQuoteVerifier, session: URLSession = .shared
    ) {
        self.gateURL = gateURL
        self.agent = agent
        self.ownerSecret = ownerSecret
        self.secrets = secrets
        self.verifier = verifier
        self.session = session
    }
}

/// Seal `secrets` to the agent as its owner and send them. The first bundle
/// for an unclaimed agent claims it for `ownerSecret`'s key. The gate's
/// answer is not taken as proof: a fresh quote must name your key as owner,
/// and the agent's signing key must not have changed. (== mero-js `provisionAgent`.)
public func provisionAgent(_ options: ProvisionAgentOptions) async throws -> ProvisionedAgent {
    try validateSecrets(options.secrets)
    let message: JSONValue = ["secrets": .object(options.secrets.mapValues { .string($0) })]
    let answer = try await sendSealed(
        options.session, options.gateURL, "/provision", options.agent, options.ownerSecret, message)
    let after = try await attestAgent(
        AttestAgentOptions(
            gateURL: options.gateURL, agentId: options.agent.agentId, verifier: options.verifier,
            session: options.session))
    let mine = try ownerPublicKey(options.ownerSecret)
    guard after.ownerPublicKey == mine, let signing = after.signingPublicKey else {
        throw AgentGateError("The gate accepted the secrets, but its quote does not name your key as owner")
    }
    if let before = options.agent.signingPublicKey, before != signing {
        throw AgentGateError("The agent's signing key changed while provisioning")
    }
    return ProvisionedAgent(
        agentId: after.agentId, provisioningPublicKey: after.provisioningPublicKey, signingPublicKey: signing,
        ownerPublicKey: mine, written: (answer["written"]?.arrayValue ?? []).compactMap(\.stringValue),
        claimed: answer["claimed"]?.boolValue == true)
}

/// What ``releaseAgent(_:)`` needs. (== mero-js `ReleaseAgentOptions`.)
public struct ReleaseAgentOptions: Sendable {
    public var gateURL: String
    public var agent: VerifiedAgent
    public var ownerSecret: Data
    public var verifier: any TransportQuoteVerifier
    public var session: URLSession

    public init(
        gateURL: String, agent: VerifiedAgent, ownerSecret: Data, verifier: any TransportQuoteVerifier,
        session: URLSession = .shared
    ) {
        self.gateURL = gateURL
        self.agent = agent
        self.ownerSecret = ownerSecret
        self.verifier = verifier
        self.session = session
    }
}

/// Release the agent as its owner: the gate stops it and erases its claim,
/// keys, secrets and state. Checked by a fresh quote that names no owner.
/// (== mero-js `releaseAgent`.)
public func releaseAgent(_ options: ReleaseAgentOptions) async throws {
    guard options.agent.ownerPublicKey != nil else {
        throw AccountError.invalidInput("This agent is not claimed; there is nothing to release")
    }
    _ = try await sendSealed(
        options.session, options.gateURL, "/release", options.agent, options.ownerSecret, ["release": true])
    let after = try await attestAgent(
        AttestAgentOptions(
            gateURL: options.gateURL, agentId: options.agent.agentId, verifier: options.verifier,
            session: options.session))
    if after.ownerPublicKey != nil {
        throw AgentGateError("The gate says it released the agent, but its quote still names an owner")
    }
}

private func validateSecrets(_ secrets: [String: String]) throws {
    guard !secrets.isEmpty else { throw AccountError.invalidInput("There are no secrets to provision") }
    guard secrets.count <= AgentGate.maxSecrets else {
        throw AccountError.invalidInput("At most \(AgentGate.maxSecrets) secrets at once")
    }
    for (name, value) in secrets.sorted(by: { $0.key < $1.key }) {
        let bytes = Array(name.utf8)
        let alphanumeric = { (c: UInt8) in
            (0x41...0x5a).contains(c) || (0x61...0x7a).contains(c) || (0x30...0x39).contains(c)
        }
        let valid =
            bytes.count <= 64 && bytes.first.map(alphanumeric) == true
            && bytes.allSatisfy { alphanumeric($0) || $0 == 0x5f || $0 == 0x2e || $0 == 0x2d }
        guard valid else {
            throw AccountError.invalidInput(
                "Secret name \(CanonicalJSON.quote(name)) must match [A-Za-z0-9][A-Za-z0-9_.-]{0,63}")
        }
        guard value.utf8.count <= AgentGate.maxSecretLength else {
            throw AccountError.invalidInput("Secret \(name) is over \(AgentGate.maxSecretLength) bytes")
        }
    }
}

private func sendSealed(
    _ session: URLSession, _ gateURL: String, _ path: String, _ agent: VerifiedAgent, _ ownerSecret: Data,
    _ message: JSONValue
) async throws -> JSONValue {
    guard !agent.agentId.isEmpty else {
        throw AccountError.invalidInput("This quote names no agent; attest with the agent id")
    }
    let mine = try checkOwner(agent, ownerSecret: ownerSecret)
    let recipient = try AgentGateHTTP.key(agent.provisioningPublicKey, "provisioningPublicKey")
    let sealed = try hpkeSealAuth(
        recipientPublic: recipient, senderSecret: ownerSecret,
        info: provisionInfo(recipientPublic: recipient, agentId: agent.agentId),
        plaintext: CanonicalJSON.data(message))
    return try await AgentGateHTTP.post(
        session, gateURL, path,
        [
            "agentId": .string(agent.agentId),
            "provisioningPublicKeyB64": .string(agent.provisioningPublicKey),
            "senderPublicKeyB64": .string(mine),
            "encappedKeyB64": .string(sealed.enc.base64EncodedString()),
            "ciphertextB64": .string(sealed.ciphertext.base64EncodedString()),
        ])
}

enum AgentGateHTTP {
    /// A 32-byte key in base64, or a refusal naming `label`.
    static func key(_ value: String, _ label: String) throws -> Data {
        guard let bytes = Data(base64Encoded: value.trimmingCharacters(in: .whitespacesAndNewlines)) else {
            throw TeeTrustError("\(label) is not base64")
        }
        guard bytes.count == 32 else { throw TeeTrustError("\(label) is not 32 bytes") }
        return bytes
    }

    /// An optional key: absent or empty is `nil`.
    static func optionalKey(_ value: String?, _ label: String) throws -> Data? {
        guard let value, !value.isEmpty else { return nil }
        return try key(value, label)
    }

    /// `POST` JSON to the gate; a refusal is an ``AgentGateError`` with its code.
    static func post(
        _ session: URLSession, _ gateURL: String, _ path: String, _ body: JSONValue
    ) async throws -> JSONValue {
        var base = gateURL
        while base.hasSuffix("/") { base.removeLast() }
        guard let url = URL(string: base + path) else { throw AgentGateError("bad gate URL \(gateURL)") }
        var request = URLRequest(url: url, timeoutInterval: 30)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = CanonicalJSON.data(body)
        let (data, response) = try await session.data(for: request)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        let text = String(decoding: data, as: UTF8.self)
        let parsed = data.isEmpty ? nil : try? MeroJSON.decode(JSONValue.self, from: data)
        guard (200..<300).contains(status) else {
            let code: String? = parsed?["error"]?.stringValue
            let details = parsed?["details"]?.stringValue ?? String(text.prefix(200))
            throw AgentGateError(
                "The gate answered \(status)\(code.map { " \($0)" } ?? ""): \(details)", status: status, code: code)
        }
        return parsed ?? .null
    }
}
