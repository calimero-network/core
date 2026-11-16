import Foundation
import MeroAPI
import MeroKit

#if canImport(Combine)
import Combine
#endif

#if canImport(UIKit) && os(iOS)
import UIKit
#elseif canImport(AppKit) && os(macOS)
import AppKit
#endif

/// How a ``MeroAppSession`` treats links. (== Kotlin `MeroAppSessionOptions`.)
public struct MeroAppSessionOptions: Sendable {
    /// Whether a link the system handed the app is one of this app's
    /// invitations. `nil` takes ``MeroAppSession/isInvitationLink(_:packageName:)``:
    /// a link carrying `invitation=…` whose path names the app's package.
    public var isInvitation: (@Sendable (String) -> Bool)?

    public init(isInvitation: (@Sendable (String) -> Bool)? = nil) {
        self.isInvitation = isInvitation
    }
}

/// Opens the node's consent page in the system browser
/// (`ASWebAuthenticationSession`), where the account holder approves a
/// release, an upgrade or outbound hosts with their passkey. Approvals happen
/// only there, never in the app: the app asks
/// (``CalimeroClient/requestConsent(_:)``), the page decides, and sends the
/// person back to the request's `returnTo` with `?request_id=…&result=…`,
/// which the app hands to ``MeroAppSession/handle(_:)`` (or reads with
/// `consentReturn`) and confirms with ``CalimeroClient/consentResult(_:)``.
/// (== Kotlin `ConsentLauncher`, which opens a Custom Tab.)
public enum ConsentLauncher {
    /// Open `url` in the system browser; returns the URL the page sent the
    /// person back to, or `nil` when they dismissed it.
    @MainActor
    public static func open(
        _ url: URL, callback: CloudCallback, opener: any WebAuthenticating = MeroClient.defaultOpener()
    ) async -> URL? {
        try? await opener.authenticate(url: url, callback: callback)
    }

    /// ``CalimeroClient/openConsentPage(_:options:)`` in the system browser.
    /// A `code` the node answers that is not the page's
    /// (``confirmationCode(_:)``) throws `CONSENT_CODE_MISMATCH`. `confirm`
    /// runs before the page opens, with the node's answer: show the person
    /// its `code` there; answer `false` and the page is not opened. Answers
    /// the request once the page sent the person back (or was dismissed).
    @MainActor
    public static func request(
        client: CalimeroClient, request: ConsentRequest, callback: CloudCallback,
        opener: any WebAuthenticating = MeroClient.defaultOpener(),
        confirm: (@MainActor (ConsentStarted) async -> Bool)? = nil
    ) async throws -> (started: ConsentStarted, returned: URL?) {
        let started = try await client.requestConsent(request)
        if let confirm, !(await confirm(started)) { return (started, nil) }
        guard let url = URL(string: started.url) else { throw AccountError.protocolViolation("bad consent URL") }
        return (started, await open(url, callback: callback, opener: opener))
    }

    /// Ask, open the page, and answer the node's decision on the request this
    /// app started, with its typed ``ConsentResult/outcome`` (the new
    /// organization, the invite's token, the organization joined).
    ///
    /// - With an app scheme, or an https callback (a universal link) where the
    ///   system takes one (iOS 17.4+ / macOS 14.4+), the page opens in
    ///   `opener` (`ASWebAuthenticationSession`). When it sends the person
    ///   back, ``CalimeroClient/confirmConsentReturn(_:returned:)`` reads the
    ///   sealed result and throws `CONSENT_REQUEST_MISMATCH` for a return that
    ///   names another request, or `CONSENT_NOT_DECIDED` when this app's
    ///   request is still pending (a different request approved, or a forged
    ///   page). Dismissed, it answers the result as it stands.
    /// - With an https callback on an older system, the page opens in the
    ///   browser (`openInBrowser`) and the result is polled
    ///   (``CalimeroClient/waitForConsent(_:interval:expiresAt:)``) until the
    ///   request expires. The universal link reaches the app's `onOpenURL`;
    ///   hand it to ``MeroAppSession/handle(_:)``.
    ///
    /// `confirm` runs before the page opens, with the node's answer (show the
    /// person its `code`); answer `false` and the page is not opened, and the
    /// result answered is `pending`.
    @MainActor
    public static func decide(
        client: CalimeroClient, request: ConsentRequest, callback: CloudCallback,
        opener: any WebAuthenticating = MeroClient.defaultOpener(),
        openInBrowser: @escaping @MainActor (URL) async -> Bool = ConsentLauncher.openInBrowser,
        httpsCallbackSupported: Bool = ConsentLauncher.supportsHTTPSCallback,
        confirm: (@MainActor (ConsentStarted) async -> Bool)? = nil
    ) async throws -> (started: ConsentStarted, result: ConsentResult) {
        if case .https = callback, !httpsCallbackSupported {
            let started = try await client.requestConsent(request)
            if let confirm, !(await confirm(started)) {
                return (started, try await client.consentResult(started.requestId))
            }
            guard let url = URL(string: started.url) else {
                throw AccountError.protocolViolation("bad consent URL")
            }
            guard await openInBrowser(url) else {
                throw AccountError.protocolViolation("could not open the consent page")
            }
            return (started, try await client.waitForConsent(started.requestId, expiresAt: started.expiresAt))
        }
        let (started, returned) = try await Self.request(
            client: client, request: request, callback: callback, opener: opener, confirm: confirm)
        guard let returned else { return (started, try await client.consentResult(started.requestId)) }
        return (started, try await client.confirmConsentReturn(started, returned: returned))
    }

    /// Whether `ASWebAuthenticationSession` takes an https callback here
    /// (`.https(host:path:)`, iOS 17.4+ / macOS 14.4+).
    public static var supportsHTTPSCallback: Bool {
        #if canImport(AuthenticationServices) && (os(iOS) || os(macOS))
        if #available(iOS 17.4, macOS 14.4, *) { return true }
        return false
        #else
        return false
        #endif
    }

    /// Open `url` in the system browser (the fallback for an https callback
    /// before iOS 17.4 / macOS 14.4). Answers whether it opened.
    @MainActor
    public static func openInBrowser(_ url: URL) async -> Bool {
        #if canImport(UIKit) && os(iOS)
        return await UIApplication.shared.open(url)
        #elseif canImport(AppKit) && os(macOS)
        return NSWorkspace.shared.open(url)
        #else
        return false
        #endif
    }
}

/// A Calimero app's signed-in account on a Calimero 1.0 cluster: passkey
/// sign-in (``MeroClient``), and the ``MeroAppBackend`` built over it
/// (``CalimeroAppBackend``). Keep one for the app's life (a `@StateObject` at
/// the root). (== Kotlin `com.calimero.mero.compose.MeroAppSession`.)
///
/// ```swift
/// @StateObject var session = MeroAppSession(
///     packageName: "com.example.board", callback: .scheme("board"),
///     config: MeroProviderConfig(nodeURL: "https://node.example"))
///
/// .task { await session.restore() }
/// .onOpenURL { url in Task { await session.handle(url) } }
/// ```
///
/// Screens read ``backend`` (and re-read when ``revision`` moves); view
/// models take it as `any MeroAppBackend`, so a test hands them
/// `MeroKitTestSupport`'s `InMemoryAppBackend`. A generated contract client
/// calls `session.contract.bound(to: appId)`.
@MainActor
public final class MeroAppSession: ObservableObject {
    public let client: MeroClient
    /// Where the consent page sends the person back to (its `returnTo`).
    public let callback: CloudCallback
    public let options: MeroAppSessionOptions

    /// The signed-in account's backend, or `nil` when signed out.
    @Published public private(set) var backend: (any MeroAppBackend)?
    /// Bumped whenever ``backend`` is rebuilt, so screens re-read.
    @Published public private(set) var revision = 0
    /// ``restore()`` is running.
    @Published public private(set) var isRestoring = false
    /// An invitation link that arrived (``handle(_:)``) and waits to be
    /// redeemed (``redeemPendingInvitation()``). Clear it with ``dismissPendingInvitation()``.
    @Published public private(set) var pendingInvitation: String?
    /// The decision on the last consent request, once the page sent the
    /// person back (``handle(_:)``) and the node confirmed it.
    @Published public private(set) var consentResult: ConsentResult?

    /// The consent request this session started last, which a consent
    /// return (``handle(_:)``) must answer.
    private var consentRequestId: String?
    private var lastKey = ""
    private var restoreTask: Task<Void, Never>?
    #if canImport(Combine)
    private var watching: AnyCancellable?
    #endif

    /// A session over `client`, whose ``MeroProviderConfig/packageName`` names the app's package.
    public init(client: MeroClient, callback: CloudCallback, options: MeroAppSessionOptions = MeroAppSessionOptions()) {
        self.client = client
        self.callback = callback
        self.options = options
        #if canImport(Combine)
        watching = client.objectWillChange.sink { [weak self] _ in
            Task { @MainActor in self?.rebuild() }
        }
        #endif
        rebuild()
    }

    #if canImport(Security)
    /// A session with its own ``MeroClient``, its session kept in the
    /// Keychain under `keychainService`.
    ///
    /// - Parameters:
    ///   - packageName: the app's package (`com.example.board`).
    ///   - config: the cluster; its `packageName` is replaced by `packageName`.
    ///   - walletURL: ignored on 1.0 (there is no wallet); kept so apps that pass it build.
    public convenience init(
        packageName: String, callback: CloudCallback, config: MeroProviderConfig = MeroProviderConfig(),
        walletURL: URL? = nil, keychainService: String = "network.calimero.merokit",
        options: MeroAppSessionOptions = MeroAppSessionOptions()
    ) {
        var config = config
        config.packageName = packageName
        let client = MeroClient(config: config, store: KeychainCloudSessionStore(service: keychainService))
        self.init(client: client, callback: callback, options: options)
    }
    #endif

    // MARK: - State

    /// Signed in.
    public var isSignedIn: Bool { client.isAuthenticated }
    /// The account, or `nil`.
    public var account: String? { client.account }
    /// The app's package: what a new org is founded for and an app instance
    /// created from (1.0's word for the application).
    public var applicationId: String? { client.config.packageName }
    /// True once signed in: a node serves every account on 1.0.
    public var hasRelay: Bool { backend?.hasRelay ?? false }
    /// Whether ``MeroAppBackend/events(contextId:)`` streams.
    public var canSubscribe: Bool { backend?.canSubscribe ?? false }
    /// The contract in any app instance: ``SignedOutContextContract`` while signed out.
    public var contract: any ContextContract { backend?.contract ?? SignedOutContextContract() }

    // MARK: - Sign-in

    /// Reconnect the stored session, if there is one. A link that arrives
    /// meanwhile (a cold start) waits for this to finish.
    public func restore() async {
        if let restoreTask { return await restoreTask.value }
        isRestoring = true
        let task = Task { _ = await client.restore() }
        restoreTask = task
        await task.value
        restoreTask = nil
        isRestoring = false
        rebuild()
    }

    /// Sign in with a passkey (the device's; on a development cluster a soft
    /// one kept for the node). A failure lands in the client's `errorMessage`.
    public func signIn() async {
        await client.signIn()
        rebuild()
    }

    /// Create an account named `name` with a new passkey, and sign in.
    public func signUp(name: String) async {
        await client.signUp(name: name)
        rebuild()
    }

    /// Talk to `nodeURL` (a development cluster with `allowMock`) before signing in.
    public func connect(nodeURL: String, allowMock: Bool = false) {
        client.connect(nodeURL: nodeURL, allowMock: allowMock)
        rebuild()
    }

    /// Sign out. `forgetDevice` is ignored on 1.0 (the passkey stays on the
    /// device either way); kept so apps that pass it build.
    public func signOut(forgetDevice: Bool = false) async {
        await client.logout()
        rebuild()
    }

    /// A URL the system handed the app: the consent page sending the person
    /// back (`returnTo?request_id=…&result=…`, confirmed with the node into
    /// ``consentResult``), or one of this app's invitation links (held in
    /// ``pendingInvitation`` for the person to confirm). Returns whether it was either.
    @discardableResult
    public func handle(_ url: URL) async -> Bool {
        if let restoreTask { await restoreTask.value }
        if let back = consentReturn(url) {
            if let cloud = client.cloud {
                // The request this session started, from the sealed result: a
                // return naming another request, or one that left it pending,
                // is refused (nil).
                consentResult = try? await cloud.confirmConsentReturn(
                    requestId: consentRequestId ?? back.requestId, returned: url)
                if consentResult?.changesOrganizations == true { revision += 1 }
            }
            return true
        }
        let raw = url.absoluteString
        let recognised =
            options.isInvitation?(raw) ?? Self.isInvitationLink(raw, packageName: client.config.packageName)
        guard recognised else { return false }
        pendingInvitation = raw
        return true
    }

    /// Forget ``pendingInvitation``: redeemed, or declined.
    public func dismissPendingInvitation() {
        pendingInvitation = nil
    }

    /// ``pendingInvitation`` decoded, or `nil` (not one, another app's, or a 0.x payload).
    public var pendingOrgInvitation: OrgInvitation? {
        pendingInvitation.flatMap { OrgInvitation.parse($0, expectedSlug: client.config.packageName) }
    }

    /// A shareable invitation link to `namespaceId` (an org): an invite token
    /// minted on the node, as ``OrgInvitation/link(slug:host:)`` spells it.
    public func createInvitationLink(namespaceId: String, contextId: String? = nil) async throws -> String {
        guard let backend else { throw SignedOutError(method: "createInvitationLink") }
        let minted = try await backend.createInvitation(namespaceId: namespaceId)
        return try OrgInvitation(
            namespaceId: minted.namespaceId, token: minted.token, contextId: contextId ?? minted.contextId,
            name: minted.name
        ).link(slug: client.config.packageName ?? "")
    }

    /// Redeem ``pendingInvitation`` (`POST /invites/{token}/redeem`) and
    /// forget it. Answers the organization joined, or `nil` when no invitation waits.
    @discardableResult
    public func redeemPendingInvitation() async throws -> String? {
        guard let backend else { throw SignedOutError(method: "redeemPendingInvitation") }
        guard let invitation = pendingOrgInvitation else { return nil }
        let joined = try await backend.joinNamespace(namespaceId: invitation.namespaceId, invitation: invitation)
        dismissPendingInvitation()
        await didJoinNamespace()
        return joined.namespaceId
    }

    /// After redeeming an invitation: re-read what the account sees.
    public func didJoinNamespace() async {
        revision += 1
    }

    // MARK: - Approvals

    /// What waits for the user's decision in `org` (or their own apps).
    public func pendingApprovals(org: String? = nil) async throws -> PendingApprovals {
        guard let cloud = client.cloud else { throw AccountError.notSignedIn("sign in first") }
        return try await cloud.pendingApprovals(org: org)
    }

    /// Ask the account holder to approve `request` on the node's consent
    /// page, opened in the system browser (``ConsentLauncher``) with
    /// ``callback`` as its `returnTo`, through `ConsentLauncher.decide`.
    /// ``consentResult`` holds the node's decision on this request, with its
    /// typed `outcome`; an approval that changed an organization
    /// (``ConsentResult/changesOrganizations``) bumps ``revision``.
    /// Throws `CONSENT_REQUEST_MISMATCH` / `CONSENT_NOT_DECIDED` when the page
    /// that sent the person back did not decide this request. Answers the request.
    @discardableResult
    public func requestConsent(
        _ request: ConsentRequest, opener: any WebAuthenticating = MeroClient.defaultOpener(),
        confirm: (@MainActor (ConsentStarted) async -> Bool)? = nil
    ) async throws -> ConsentStarted {
        guard let cloud = client.cloud else { throw SignedOutError(method: "requestConsent") }
        consentResult = nil
        let (started, result) = try await ConsentLauncher.decide(
            client: cloud, request: request, callback: callback, opener: opener,
            confirm: { [weak self] started in
                self?.consentRequestId = started.requestId
                return await confirm?(started) ?? true
            })
        consentResult = result
        if result.changesOrganizations { revision += 1 }
        return started
    }

    /// Rebuild ``backend`` from the client's current session. A no-op when
    /// nothing about the session changed, so open screens are not reset.
    public func rebuild() {
        let key = [
            "\(client.sessionRevision)", client.account ?? "-", client.nodeURL ?? "-",
            client.cloud == nil ? "0" : "1",
        ].joined(separator: "|")
        guard key != lastKey else { return }
        lastKey = key
        if let cloud = client.cloud, let account = client.account {
            backend = CalimeroAppBackend(
                client: cloud, account: account, packageName: client.config.packageName ?? "",
                packageVersion: client.config.packageVersion, passkey: client.passkeyForNode())
        } else {
            backend = nil
        }
        revision += 1
    }

    // MARK: - Links

    /// Whether `raw` is an invitation to the app with `packageName`: it
    /// carries `invitation=…`, and its path names the package. Without a
    /// package any invitation link is. (== Kotlin `MeroAppSession.isInvitationLink`.)
    public nonisolated static func isInvitationLink(_ raw: String, packageName: String?) -> Bool {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let query = trimmed.firstIndex(of: "?") else { return false }
        let hasToken = trimmed[trimmed.index(after: query)...].split(separator: "&").contains { pair in
            let parts = pair.split(separator: "=", maxSplits: 1)
            return parts.count == 2 && parts[0] == Substring(InviteLink.invitationParam) && !parts[1].isEmpty
        }
        guard hasToken else { return false }
        guard let packageName, !packageName.isEmpty else { return true }
        let path = trimmed[..<query].split(separator: "/").map { $0.lowercased() }
        return path.contains(packageName.lowercased())
    }
}
