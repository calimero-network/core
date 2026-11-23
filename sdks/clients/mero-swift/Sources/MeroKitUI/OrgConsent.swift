import Foundation
import MeroKit

/// Asks the person to approve an organization action on the node's consent
/// page and answers the node's decision (calimero-network/core#5014).
///
/// A session from ``MeroClient/signInWithServedPage(cluster:callback:appsDomain:opener:)``
/// is bound to the package, and the node refuses it organization routes
/// (`POST /orgs`, `POST /orgs/{org}/invites`, `POST /invites/{token}/redeem`).
/// ``CalimeroAppBackend`` given one of these founds, invites and joins through
/// it instead, with the same answers.
public protocol OrgConsentApprover: Sendable {
    /// The decision on `request`, built for the `returnTo` the page sends the person back to.
    func decide(_ request: @escaping @Sendable (_ returnTo: String) -> ConsentRequest) async throws -> ConsentResult
}

/// The person declined an organization action on the consent page, or left it
/// without deciding. Nothing was changed.
public struct OrgConsentNotApproved: Error, Equatable, Sendable, LocalizedError {
    /// The consent kind (`createOrg`, `createInvite`, `acceptInvite`).
    public let kind: String
    public let status: ConsentStatus

    public init(kind: String, status: ConsentStatus) {
        self.kind = kind
        self.status = status
    }

    public var errorDescription: String? {
        status == .declined ? "Declined. Nothing was changed." : "Not approved. Nothing was changed."
    }
}

/// ``OrgConsentApprover`` for a session bound to the package on Calimero
/// Cloud: the consent page opens in the system sheet and sends the person
/// back through the package's served page
/// (``MeroClient/servedConsentReturnTo(cluster:appsDomain:)``), which passes
/// the answer on to `callback`. The decision is read from the node and must be
/// the one on this request (``ConsentLauncher/decide(client:request:callback:opener:openInBrowser:httpsCallbackSupported:confirm:)``).
public struct ServedPageConsent: OrgConsentApprover {
    public let client: CalimeroClient
    public let origin: URL
    public let callback: CloudCallback
    public let opener: any WebAuthenticating
    public let session: URLSession
    public let confirm: (@MainActor @Sendable (ConsentStarted) async -> Bool)?

    public init(
        client: CalimeroClient, origin: URL, callback: CloudCallback,
        opener: any WebAuthenticating, session: URLSession = .shared,
        confirm: (@MainActor @Sendable (ConsentStarted) async -> Bool)? = nil
    ) {
        self.client = client
        self.origin = origin
        self.callback = callback
        self.opener = opener
        self.session = session
        self.confirm = confirm
    }

    public func decide(
        _ request: @escaping @Sendable (_ returnTo: String) -> ConsentRequest
    ) async throws
        -> ConsentResult
    {
        let version = try await AppHost.latestVersion(origin: origin, session: session)
        let returnTo = ServedPageHandoff.consentReturnTo(origin: origin, version: version)
        return try await ConsentLauncher.decide(
            client: client, request: request(returnTo), callback: callback, opener: opener, confirm: confirm
        ).result
    }
}

extension MeroClient {
    /// Where organization actions are approved: on Calimero Cloud the node's
    /// consent page, returning through the package's served page
    /// (``ServedPageConsent``); `nil` on a development node, where the
    /// backend calls the node directly.
    ///
    /// - Parameters:
    ///   - callback: the app's own scheme the served page passes the answer on to (`mero-chat://consent`).
    ///   - confirm: runs before the page opens with the node's answer; show
    ///     its `code`, which the page shows too. Answer `false` not to open it.
    public func servedPageConsent(
        cluster: String? = nil, callback: CloudCallback, appsDomain: String = AppHost.defaultDomain,
        opener: any WebAuthenticating = MeroClient.defaultOpener(),
        confirm: (@MainActor @Sendable (ConsentStarted) async -> Bool)? = nil
    ) -> (any OrgConsentApprover)? {
        guard !allowMock, let package = config.packageName, let cloud else { return nil }
        return ServedPageConsent(
            client: cloud, origin: AppHost.origin(package: package, cluster: cluster, domain: appsDomain),
            callback: callback, opener: opener, session: urlSession, confirm: confirm)
    }
}
