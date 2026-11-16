import Foundation

// Which agent images to trust: a mero-kms release's signed agent allowlist,
// `kms-agent-attestation-policy.<profile>.json`, and a quote verifier built
// from it. Port of mero-js `src/agent-gate/policy.ts`.
//
// The KMS release workflow measures the release's agent image and signs its
// allowlist with cosign keyless, beside the rest of the release. The
// allowlist can therefore come from anywhere (a release asset, a mirror, a
// file in the app): `verifySignedAgentPolicy` checks the signature here,
// against the Sigstore trust root embedded in this SDK, and refuses anything else.

extension TeeTrust {
    /// The workflow that signs every mero-kms release, agent allowlists
    /// included. (== mero-js `KMS_RELEASE_SIGNER`.)
    public static let kmsReleaseSigner = SignerIdentity(
        identity: "https://github.com/calimero-network/mero-tee/.github/workflows/"
            + "release-kms.yaml@refs/heads/master",
        issuer: "https://token.actions.githubusercontent.com")
}

/// An agent allowlist, as the KMS release publishes it. Every list must be
/// non-empty. (== mero-js `AgentPolicy`.)
public struct AgentPolicy: Codable, Sendable, Equatable {
    public var schemaVersion: Int?
    public var role: String?
    public var profile: String?
    /// The mero-kms release it belongs to.
    public var version: String?
    public var allowedTcbStatuses: [String]
    public var allowedMrtd: [String]
    public var allowedRtmr0: [String]
    public var allowedRtmr1: [String]
    public var allowedRtmr2: [String]
    public var allowedRtmr3: [String]

    public init(
        schemaVersion: Int? = nil, role: String? = nil, profile: String? = nil, version: String? = nil,
        allowedTcbStatuses: [String], allowedMrtd: [String], allowedRtmr0: [String], allowedRtmr1: [String],
        allowedRtmr2: [String], allowedRtmr3: [String]
    ) {
        self.schemaVersion = schemaVersion
        self.role = role
        self.profile = profile
        self.version = version
        self.allowedTcbStatuses = allowedTcbStatuses
        self.allowedMrtd = allowedMrtd
        self.allowedRtmr0 = allowedRtmr0
        self.allowedRtmr1 = allowedRtmr1
        self.allowedRtmr2 = allowedRtmr2
        self.allowedRtmr3 = allowedRtmr3
    }

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case role, profile, version
        case allowedTcbStatuses = "allowed_tcb_statuses"
        case allowedMrtd = "allowed_mrtd"
        case allowedRtmr0 = "allowed_rtmr0"
        case allowedRtmr1 = "allowed_rtmr1"
        case allowedRtmr2 = "allowed_rtmr2"
        case allowedRtmr3 = "allowed_rtmr3"
    }

    /// Each list by its wire name, in mero-js's order.
    var lists: [(String, [String])] {
        [
            ("allowed_tcb_statuses", allowedTcbStatuses), ("allowed_mrtd", allowedMrtd),
            ("allowed_rtmr0", allowedRtmr0), ("allowed_rtmr1", allowedRtmr1), ("allowed_rtmr2", allowedRtmr2),
            ("allowed_rtmr3", allowedRtmr3),
        ]
    }

    /// As the gate's own verifier: an empty list accepts nothing.
    func checkLists() throws {
        for (name, list) in lists where list.isEmpty {
            throw TeeTrustError("The agent policy's \(name) is empty, so no agent would be accepted")
        }
    }
}

/// Verify an agent allowlist against its cosign bundle, as `cosign
/// verify-blob` would, and return it parsed. (== mero-js `verifySignedAgentPolicy`.)
/// Throws ``TeeTrustError`` unless it was signed by the KMS release workflow
/// (``TeeTrust/kmsReleaseSigner`` unless `identity` / `issuer` say otherwise),
/// it is an agent allowlist (`role: "agent"`) for `profile`
/// (``TeeTrust/productionProfile`` by default, the only one whose host cannot
/// reach in), every list in it is non-empty, and `expectedVersion`, when
/// given, is its `version`.
public func verifySignedAgentPolicy(
    _ policy: Data, bundle: String, profile: String = TeeTrust.productionProfile, expectedVersion: String? = nil,
    identity: String? = nil, issuer: String? = nil
) throws -> AgentPolicy {
    try Sigstore.verifyCosignBundle(
        policy, bundle: bundle,
        signer: SignerIdentity(
            identity: identity ?? TeeTrust.kmsReleaseSigner.identity,
            issuer: issuer ?? TeeTrust.kmsReleaseSigner.issuer))
    guard let json = try? MeroJSON.decode(JSONValue.self, from: policy) else {
        throw TeeTrustError("The signed agent policy is not JSON")
    }
    guard json["role"]?.stringValue == "agent" else {
        let role = json["role"].map(CanonicalJSON.string) ?? "undefined"
        throw TeeTrustError("The signed file is a \(role) policy, not an agent one")
    }
    guard json["profile"]?.stringValue == profile else {
        let named = json["profile"].map(CanonicalJSON.string) ?? "undefined"
        throw TeeTrustError("The signed agent policy is for the \(named) profile, not \(profile)")
    }
    if let expectedVersion, json["version"]?.stringValue != expectedVersion {
        let named = json["version"].map(CanonicalJSON.string) ?? "undefined"
        throw TeeTrustError("The signed agent policy is release \(named), not \(expectedVersion)")
    }
    let names = ["allowed_tcb_statuses", "allowed_mrtd", "allowed_rtmr0", "allowed_rtmr1", "allowed_rtmr2"]
    for name in names + ["allowed_rtmr3"] where (json[name]?.arrayValue ?? []).isEmpty {
        throw TeeTrustError("The agent policy's \(name) is empty, so no agent would be accepted")
    }
    guard let parsed = try? MeroJSON.decode(AgentPolicy.self, from: policy) else {
        throw TeeTrustError("The signed agent policy is not an agent allowlist")
    }
    return parsed
}

/// As the overload taking the policy's bytes, with the policy as text.
public func verifySignedAgentPolicy(
    _ policy: String, bundle: String, profile: String = TeeTrust.productionProfile, expectedVersion: String? = nil,
    identity: String? = nil, issuer: String? = nil
) throws -> AgentPolicy {
    try verifySignedAgentPolicy(
        Data(policy.utf8), bundle: bundle, profile: profile, expectedVersion: expectedVersion, identity: identity,
        issuer: issuer)
}

/// A verifier for ``attestAgent(_:)`` that accepts a quote only from an
/// agent image in `policy`: its signature and collateral verify, its TCB
/// status is one the policy allows (never `Revoked`), it is not a debug TD,
/// and each of MRTD and RTMR0-3 is in the policy's list. Mock quotes never
/// pass. The gate sends no collateral, so `fetchCollateral` gets it (from a
/// PCCS). (== mero-js `agentQuoteVerifier`.)
public func agentQuoteVerifier(
    _ policy: AgentPolicy, fetchCollateral: @escaping @Sendable (Data) async throws -> DcapCollateral,
    dcapVerify: @escaping DcapVerify = { try Dcap.verify($0, collateral: $1, nowSecs: $2) },
    now: @escaping @Sendable () -> Int64 = { Int64(Date().timeIntervalSince1970 * 1000) }
) throws -> QuoteVerifier {
    try policy.checkLists()
    let statuses = try policy.allowedTcbStatuses.map { value -> String in
        guard let status = TeeTrust.tcbStatus(value.trimmingCharacters(in: .whitespacesAndNewlines)) else {
            throw TeeTrustError("The agent policy allows an unknown TCB status \(CanonicalJSON.quote(value))")
        }
        return status
    }
    let register = { (value: String) -> String in
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.lowercased().hasPrefix("0x") ? String(trimmed.dropFirst(2)) : trimmed
    }
    return try createQuoteVerifier(
        QuoteVerifierOptions(
            dcapVerify: dcapVerify, allowedMrtd: policy.allowedMrtd.map(register),
            allowedRtmr0: policy.allowedRtmr0.map(register), allowedRtmr1: policy.allowedRtmr1.map(register),
            allowedRtmr2: policy.allowedRtmr2.map(register), allowedRtmr3: policy.allowedRtmr3.map(register),
            allowedTcbStatuses: statuses, fetchCollateral: fetchCollateral, now: now))
}
