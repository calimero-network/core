import Foundation

// The consent kinds that make something: `createOrg`, `createInvite` and
// `acceptInvite` (calimero-network/core#5014), `renameOrg` and `deleteOrg`
// (calimero-network/core#5018). What an approval made is in
// `/consent/{id}/result`'s `outcome`, typed here as ``ConsentOutcome``. The
// other kinds core#5018 added (`leaveOrg`, `removeMember`, `setMemberRole`,
// `setMemberTeams`, `removeMemberTeams`, `revokeInvite`, `setCapabilities`,
// `setAllApps`) make nothing and answer no outcome.
//
// The request and result bodies go through ``CalimeroClient/post(_:_:)`` and
// ``CalimeroClient/get(_:)`` (sealed like every other route), so the outcome
// is typed by kind here; ``ConsentRequest/generated()`` gives the same body
// as the generated `MeroAPI` type for `client.consent.pageCreate(body:)`.
//
// An invite token is a bearer credential: nothing here writes it to a
// description, a debug description or a mirror (`print`, `dump`, string
// interpolation), and no error carries it.

/// What an approved consent request made. (== mero-js `ConsentOutcome`.)
public enum ConsentOutcome: Codable, Sendable, Equatable {
    /// `createOrg`: the new organization.
    case orgCreated(orgId: String)
    /// `createInvite`: the invite's token, a bearer credential. Never log it.
    case inviteCreated(token: String)
    /// `acceptInvite`: the organization joined.
    case inviteAccepted(org: String)
    /// `renameOrg`: the organization and its new name.
    case orgRenamed(orgId: String, name: String)
    /// `deleteOrg`: the apps deleted with the organization.
    case orgDeleted(apps: [String])

    enum CodingKeys: String, CodingKey { case orgId, token, org, name, apps }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        if let apps = try c.decodeIfPresent([String].self, forKey: .apps) {
            self = .orgDeleted(apps: apps)
        } else if let id = try c.decodeIfPresent(String.self, forKey: .orgId) {
            if let name = try c.decodeIfPresent(String.self, forKey: .name) {
                self = .orgRenamed(orgId: id, name: name)
            } else {
                self = .orgCreated(orgId: id)
            }
        } else if let token = try c.decodeIfPresent(String.self, forKey: .token) {
            self = .inviteCreated(token: token)
        } else if let org = try c.decodeIfPresent(String.self, forKey: .org) {
            self = .inviteAccepted(org: org)
        } else {
            throw DecodingError.dataCorrupted(
                .init(codingPath: decoder.codingPath, debugDescription: "an outcome has orgId, token, org or apps"))
        }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .orgCreated(let id): try c.encode(id, forKey: .orgId)
        case .inviteCreated(let token): try c.encode(token, forKey: .token)
        case .inviteAccepted(let org): try c.encode(org, forKey: .org)
        case .orgRenamed(let id, let name):
            try c.encode(id, forKey: .orgId)
            try c.encode(name, forKey: .name)
        case .orgDeleted(let apps): try c.encode(apps, forKey: .apps)
        }
    }

    /// The `kind` that answers this outcome.
    public var kind: String {
        switch self {
        case .orgCreated: return "createOrg"
        case .inviteCreated: return "createInvite"
        case .inviteAccepted: return "acceptInvite"
        case .orgRenamed: return "renameOrg"
        case .orgDeleted: return "deleteOrg"
        }
    }

    /// The kinds whose approval answers an outcome.
    static let kinds: Set<String> = ["createOrg", "createInvite", "acceptInvite", "renameOrg", "deleteOrg"]
}

extension ConsentOutcome: CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    /// The outcome, with an invite token redacted.
    public var description: String {
        switch self {
        case .orgCreated(let orgId): return "orgCreated(\(orgId))"
        case .inviteCreated: return "inviteCreated(token: <redacted>)"
        case .inviteAccepted(let org): return "inviteAccepted(\(org))"
        case .orgRenamed(let orgId, let name): return "orgRenamed(\(orgId), \(name))"
        case .orgDeleted(let apps): return "orgDeleted(apps: \(apps.count))"
        }
    }

    public var debugDescription: String { "ConsentOutcome.\(description)" }

    public var customMirror: Mirror { Mirror(self, children: ["description": description]) }
}

extension ConsentResult {
    /// Whether the person approved.
    public var isApproved: Bool { status == .approved }

    /// `createOrg`, approved: the new organization's id.
    public var orgId: String? {
        if case .orgCreated(let id) = outcome { return id }
        return nil
    }

    /// `createInvite`, approved: the invite's token. A bearer credential: never log it.
    public var inviteToken: String? {
        if case .inviteCreated(let token) = outcome { return token }
        return nil
    }

    /// `acceptInvite`, approved: the organization joined.
    public var joinedOrg: String? {
        if case .inviteAccepted(let org) = outcome { return org }
        return nil
    }

    /// `renameOrg`, approved: the organization's new name.
    public var newOrgName: String? {
        if case .orgRenamed(_, let name) = outcome { return name }
        return nil
    }

    /// `deleteOrg`, approved: the apps deleted with the organization.
    public var deletedApps: [String]? {
        if case .orgDeleted(let apps) = outcome { return apps }
        return nil
    }

    /// Whether this is an approved request that changed an organization the
    /// account sees or its place in one (made, joined, left, renamed, deleted,
    /// a member's role, teams, capabilities or apps, an invite revoked):
    /// screens listing organizations, members or teams should read again.
    public var changesOrganizations: Bool {
        isApproved && Self.organizationKinds.contains(kind)
    }

    static let organizationKinds: Set<String> = [
        "createOrg", "acceptInvite", "leaveOrg", "removeMember", "setMemberRole", "setMemberTeams",
        "removeMemberTeams", "revokeInvite", "renameOrg", "deleteOrg", "setCapabilities", "setAllApps",
    ]

    /// Throws ``AccountError/protocolViolation(_:)`` (which never quotes the
    /// answer) when the outcome does not fit the kind: an approved
    /// `createOrg`, `createInvite`, `acceptInvite`, `renameOrg` or `deleteOrg`
    /// without its outcome, or an outcome of another kind.
    func checkOutcome() throws {
        if let outcome, outcome.kind != kind {
            throw AccountError.protocolViolation("a \(kind) consent answered a \(outcome.kind) outcome")
        }
        if status == .approved, outcome == nil, ConsentOutcome.kinds.contains(kind) {
            throw AccountError.protocolViolation("an approved \(kind) consent has no outcome")
        }
    }
}

extension ConsentResult: CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    /// The result, with an invite token redacted.
    public var description: String {
        "ConsentResult(\(requestId), \(kind), \(status.rawValue)\(outcome.map { ", \($0)" } ?? ""))"
    }

    public var debugDescription: String { description }

    public var customMirror: Mirror {
        Mirror(
            self,
            children: [
                "requestId": requestId, "kind": kind, "status": status.rawValue, "decidedAt": decidedAt as Any,
                "outcome": outcome?.description as Any,
            ])
    }
}

extension ConsentRequest: CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    /// The request, with an invite token redacted.
    public var description: String {
        switch self {
        case .release(let scope, let version, let withdraw, _):
            return "release(\(scope), \(version)\(withdraw ? ", withdraw" : ""))"
        case .upgrade(let app, let toVersion, _): return "upgrade(\(app) → \(toVersion))"
        case .outbound(let scope, let package, let prefixes, _):
            return "outbound(\(scope), package: \(package ?? "-"), prefixes: \(prefixes?.count ?? 0))"
        case .createOrg(let name, _): return "createOrg(\(name))"
        case .createInvite(let org, let role, _, _, _, _, _): return "createInvite(\(org), \(role.rawValue))"
        case .acceptInvite: return "acceptInvite(token: <redacted>)"
        case .leaveOrg(let org, _): return "leaveOrg(\(org))"
        case .removeMember(let org, let account, _): return "removeMember(\(org), \(account))"
        case .setMemberRole(let org, let account, let role, _):
            return "setMemberRole(\(org), \(account), \(role.rawValue))"
        case .setMemberTeams(let org, let account, let teams, _):
            return "setMemberTeams(\(org), \(account), teams: \(teams.count))"
        case .removeMemberTeams(let org, let account, let teams, _):
            return "removeMemberTeams(\(org), \(account), teams: \(teams.count))"
        case .revokeInvite(let org, _, _): return "revokeInvite(\(org))"
        case .renameOrg(let org, let name, _): return "renameOrg(\(org) → \(name))"
        case .deleteOrg(let org, _): return "deleteOrg(\(org))"
        case .setCapabilities(let org, let account, let team, let capabilities, _):
            return
                "setCapabilities(\(org), account: \(account ?? "-"), team: \(team ?? "-"), \(capabilities.rawValue))"
        case .setAllApps(let org, let account, let allApps, _): return "setAllApps(\(org), \(account), \(allApps))"
        }
    }

    public var debugDescription: String { "ConsentRequest.\(description)" }

    public var customMirror: Mirror {
        Mirror(self, children: ["description": description, "returnTo": returnTo])
    }
}

extension CalimeroClient {
    /// Ask the node to ask the person: `POST /consent/requests` (sealed), every
    /// kind. Show the answer's `code`, then send the person to its `url`
    /// (``openConsentPage(_:options:)``, or MeroKitUI's `ConsentLauncher`).
    /// Throws `CONSENT_CODE_MISMATCH` when the node's `code` is not the page's
    /// (``confirmationCode(_:)``). (== mero-js `requestConsent`.)
    public func requestConsent(_ request: ConsentRequest) async throws -> ConsentStarted {
        try request.validate()
        let answer = try await post("consent/requests", request.json)
        let started: ConsentStarted
        do {
            started = try MeroJSON.decode(ConsentStarted.self, from: CanonicalJSON.data(answer))
        } catch {
            throw AccountError.protocolViolation("a consent request answers requestId, url and expiresAt")
        }
        try Self.checkConsentCode(started)
        return started
    }

    /// `GET /consent/{id}/result` (sealed, the account that asked), every
    /// kind, with the outcome typed (``ConsentResult/outcome``). Throws a
    /// protocol violation when the outcome does not fit the kind.
    /// (== mero-js `consentResult`.)
    public func consentResult(_ requestId: String) async throws -> ConsentResult {
        guard !requestId.isEmpty else { throw AccountError.invalidInput("a consent request id is needed") }
        let id = requestId.addingPercentEncoding(withAllowedCharacters: .urlPathSegment) ?? requestId
        let answer = try await get("consent/\(id)/result")
        let result: ConsentResult
        do {
            result = try MeroJSON.decode(ConsentResult.self, from: CanonicalJSON.data(answer))
        } catch {
            throw AccountError.protocolViolation("a consent result needs requestId, kind and status")
        }
        guard result.requestId == requestId else {
            throw AccountError.protocolViolation("the node answered another consent request's result")
        }
        try result.checkOutcome()
        return result
    }

    /// The page sent the person back to `returned` after ``requestConsent(_:)``
    /// answered `started`: the node's decision on the request this app
    /// started, read from the sealed `/consent/{id}/result` (never from the
    /// URL, whose query is a hint only).
    ///
    /// - Throws `CONSENT_REQUEST_MISMATCH` when `returned` names another request.
    /// - Throws `CONSENT_NOT_DECIDED` when this app's request is still
    ///   `pending`: the page that sent the person back did not decide it (a
    ///   different request approved, or a forged page).
    /// - Throws `CONSENT_REPLACED` when the node no longer knows this app's
    ///   request (`404`): an account has one undecided request per node, and
    ///   a newer one replaced it, possibly on a forged page. Ask again.
    public func confirmConsentReturn(_ started: ConsentStarted, returned: URL) async throws -> ConsentResult {
        try await confirmConsentReturn(requestId: started.requestId, returned: returned)
    }

    /// ``confirmConsentReturn(_:returned:)`` for the request `requestId`.
    public func confirmConsentReturn(requestId: String, returned: URL) async throws -> ConsentResult {
        if let back = consentReturn(returned), back.requestId != requestId {
            throw CalimeroError(
                status: 200, code: "CONSENT_REQUEST_MISMATCH",
                message: "the consent page sent back another request than the one this app started",
                details: ["requestId": .string(requestId), "returned": .string(back.requestId)])
        }
        let result: ConsentResult
        do {
            result = try await consentResult(requestId)
        } catch let error as CalimeroError where error.status == 404 {
            throw CalimeroError(
                status: 404, code: "CONSENT_REPLACED",
                message: "this app's consent request was replaced by a newer one before it was decided",
                details: ["requestId": .string(requestId)])
        }
        guard result.status != .pending else {
            throw CalimeroError(
                status: 200, code: "CONSENT_NOT_DECIDED",
                message: "the consent page sent the person back, but this app's request is still pending",
                details: ["requestId": .string(requestId)])
        }
        return result
    }
}
