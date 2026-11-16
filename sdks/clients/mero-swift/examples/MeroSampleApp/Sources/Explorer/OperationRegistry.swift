import Foundation
import MeroAPI
import MeroKit

// The Calimero 1.0 client surface, one `SDKOperation` per `CalimeroClient`
// method worth trying from a phone. Request bodies are entered as JSON;
// results are pretty-printed. Every call goes through the attested, sealed
// channel the session opened.

private func field(_ inputs: [String: String], _ key: String) throws -> String {
    guard let value = inputs.opt(key) else { throw AccountError.invalidInput("\(key) is required") }
    return value
}

// MARK: Account

private let accountOps: [SDKOperation] = [
    SDKOperation("me", "Account", "auth.me", "Your account, passkeys and organizations") { c, _ in
        Fmt.json(try await c.auth.me())
    },
    SDKOperation("usage", "Account", "auth.usage", "What the account uses, with the cluster's quotas") { c, _ in
        Fmt.json(try await c.auth.usage())
    },
    SDKOperation("refresh", "Account", "refresh", "Swap the refresh token for a new session") { c, _ in
        let session = try await c.refresh()
        return "Refreshed: session for \(session.accountId), expires \(Date(timeIntervalSince1970: session.expiresAt))"
    },
    SDKOperation(
        "attestation", "Account", "publicApi.attestation", "The node's attestation document (display only)"
    ) { c, _ in
        let nonce = Hex.encode(randomBytes(32))
        return Fmt.json(try await c.publicApi.attestation(nonce: nonce))
    },
    SDKOperation("apikeys", "Account", "auth.apikeyList", "The account's API keys (never the keys)") { c, _ in
        Fmt.json(try await c.auth.apikeyList())
    },
    SDKOperation("aliases", "Account", "auth.aliasList", "This account's aliases, alias to app id") { c, _ in
        Fmt.json(try await c.auth.aliasList())
    },
]

// MARK: Apps

private let appOps: [SDKOperation] = [
    SDKOperation(
        "apps", "Apps", "apps.list", "App instances you reach",
        fields: [.line("package", "Package", "com.calimero.chat")]
    ) { c, i in
        let package = i.opt("package")
        return Fmt.json(try await c.apps.list(package: package))
    },
    SDKOperation("app", "Apps", "apps.get", "One app instance's record", fields: [.line("app", "App id")]) { c, i in
        let app = try field(i, "app")
        return Fmt.json(try await c.apps.get(app: app))
    },
    SDKOperation(
        "abi", "Apps", "apps.abi", "The ABI the app's bundle embeds", fields: [.line("app", "App id")]
    ) { c, i in
        let app = try field(i, "app")
        return Fmt.json(try await c.apps.abi(app: app))
    },
    SDKOperation(
        "view", "Apps", "view", "Run a read-only method (POST /apps/{app}/views)",
        fields: [.line("app", "App id"), .line("method", "Method", "get_messages"), .json("args", "Arguments")]
    ) { c, i in
        Fmt.json(try await c.view(try field(i, "app"), try field(i, "method"), try Fmt.value(i.v("args"))))
    },
    SDKOperation(
        "call", "Apps", "call", "Call a method (POST /apps/{app}/intents), retried with one idempotency key",
        fields: [.line("app", "App id"), .line("method", "Method"), .json("args", "Arguments")]
    ) { c, i in
        Fmt.json(try await c.call(try field(i, "app"), try field(i, "method"), try Fmt.value(i.v("args"))))
    },
    SDKOperation(
        "create", "Apps", "apps.create", "Create an app instance of a package release",
        fields: [
            .line("name", "Name"), .line("package", "Package", "com.calimero.chat"), .line("version", "Version"),
            .line("org", "Org id"),
        ]
    ) { c, i in
        let (name, package) = (try field(i, "name"), try field(i, "package"))
        let (version, org) = (i.opt("version"), i.opt("org"))
        let app = try await c.apps.create(name: name, package: package, version: version, org: org)
        return "Created app \(app.appId)"
    },
    SDKOperation("blobs", "Apps", "listBlobs", "An app's blobs", fields: [.line("app", "App id")]) { c, i in
        Fmt.json(try await c.listBlobs(try field(i, "app")))
    },
    SDKOperation(
        "presence", "Apps", "apps.presenceGet", "Who is present in an app", fields: [.line("app", "App id")]
    ) { c, i in
        let app = try field(i, "app")
        return Fmt.json(try await c.apps.presenceGet(app: app))
    },
    SDKOperation("packages", "Apps", "packages.list", "Installed packages and their versions") { c, _ in
        Fmt.json(try await c.packages.list())
    },
]

// MARK: Organizations

private let orgOps: [SDKOperation] = [
    SDKOperation(
        "org.create", "Organizations", "orgs.create", "Create an organization", fields: [.line("name", "Name")]
    ) { c, i in
        let name = try field(i, "name")
        return "Created org \(try await c.orgs.create(name: name).orgId)"
    },
    SDKOperation(
        "org.get", "Organizations", "orgs.get", "An organization and its teams", fields: [.line("org", "Org id")]
    ) { c, i in
        let org = try field(i, "org")
        return Fmt.json(try await c.orgs.get(org: org))
    },
    SDKOperation(
        "team.create", "Organizations", "orgs.teamCreate", "Create a team (open: everyone may join)",
        fields: [.line("org", "Org id"), .line("name", "Name"), .line("join", "everyone or invite", "everyone")]
    ) { c, i in
        let join = TeamJoin(rawValue: i.opt("join") ?? "everyone") ?? .everyone
        let (org, name) = (try field(i, "org"), try field(i, "name"))
        return "Created team \(try await c.orgs.teamCreate(org: org, name: name, join: join.generated).teamId)"
    },
    SDKOperation(
        "team.get", "Organizations", "orgs.teamGet", "A team, its members and apps",
        fields: [.line("org", "Org id"), .line("team", "Team id")]
    ) { c, i in
        let (org, team) = (try field(i, "org"), try field(i, "team"))
        return Fmt.json(try await c.orgs.teamGet(org: org, team: team))
    },
    SDKOperation(
        "invite", "Organizations", "orgs.inviteCreate", "Mint an invite token (approved with your passkey)",
        fields: [.line("org", "Org id")],
        stepUp: { c, i, passkey in
            let (org, approval) = (try field(i, "org"), try approver(passkey))
            let invite = try await c.orgs.inviteCreate(org: org, role: .member, passkey: approval)
            return "Invite token: \(invite.token)"
        }),
    SDKOperation(
        "redeem", "Organizations", "orgs.inviteRedeem", "Join an organization with an invite token",
        fields: [.line("token", "Token", "inv_…")]
    ) { c, i in
        let token = try field(i, "token")
        return Fmt.json(try await c.orgs.inviteRedeem(token: token))
    },
]

// MARK: Approvals

private let approvalOps: [SDKOperation] = [
    SDKOperation(
        "pending", "Approvals", "pendingApprovals", "Releases and upgrades waiting for a decision",
        fields: [.line("org", "Org id (empty: your own apps)")]
    ) { c, i in
        let pending = try await c.pendingApprovals(org: i.opt("org"))
        let releases = pending.releases.map { "release \($0.version) on \($0.nodes) node(s)" }
        let upgrades = pending.upgrades.map { "upgrade \($0.name ?? $0.app): \($0.from) to \($0.to)" }
        let lines = releases + upgrades
        return lines.isEmpty ? "Nothing waits for a decision." : lines.joined(separator: "\n")
    },
    SDKOperation(
        "rules", "Approvals", "publicApi.releasesPublic", "The cluster's governance and release deadlines"
    ) { c, _ in
        Fmt.json(try await c.publicApi.releasesPublic())
    },
    SDKOperation(
        "consent.result", "Approvals", "consent.pageResult", "The decision on a consent request",
        fields: [.line("request", "Request id")]
    ) { c, i in
        let request = try field(i, "request")
        return Fmt.json(try await c.consent.pageResult(request: request))
    },
]

let sdkOperations: [SDKOperation] = accountOps + appOps + orgOps + approvalOps

/// Categories in display order (as first seen in `sdkOperations`).
let sdkCategories: [String] = {
    var seen: [String] = []
    for op in sdkOperations where !seen.contains(op.category) { seen.append(op.category) }
    return seen
}()
