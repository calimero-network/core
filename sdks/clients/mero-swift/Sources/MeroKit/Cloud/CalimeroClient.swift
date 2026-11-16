import Foundation
import MeroAPI
import OpenAPIRuntime

#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

/// What a ``CalimeroClient`` is built with. (== mero-js `CalimeroClientOptions`.)
public struct CalimeroClientOptions: Sendable {
    /// One node of the cluster: the one the client talks to first.
    public var baseUrl: String
    /// Other nodes of the same cluster, in the order to move to them when a
    /// node answers `421 WRONG_NODE` (its release may not serve this
    /// account), or stays unavailable to a call or a view through all its
    /// retries. Each is attested under the same ``policy`` before anything is
    /// sent to it. Without them a `421` reaches the caller as ``WrongNode``.
    public var nodes: [String]
    public var policy: CloudPolicy
    /// Carries the attestation and the sealed envelopes. (mero-js's `fetch`.)
    public var session: URLSession
    /// How calls and views retry a 503 or a failed connection.
    public var retry: CloudRetryOptions

    public init(
        baseUrl: String, nodes: [String] = [], policy: CloudPolicy, session: URLSession = .shared,
        retry: CloudRetryOptions = CloudRetryOptions()
    ) {
        self.baseUrl = baseUrl
        self.nodes = nodes
        self.policy = policy
        self.session = session
        self.retry = retry
    }
}

/// How ``CalimeroClient/connect(_:)`` reaches a Calimero Cloud cluster by
/// name. (== mero-js `CalimeroConnectOptions`.)
public struct CalimeroConnectOptions: Sendable {
    /// The cluster's name, e.g. `eu-shared-1`.
    public var cluster: String
    /// What every node must attest to before anything is sent to it.
    public var policy: CloudPolicy
    /// The node directory to ask; ``defaultDirectory`` by default.
    public var directory: String
    /// Try the nodes in this zone (e.g. `europe-west4-a`) first.
    public var zone: String?
    /// Carries the directory, the attestation and the sealed envelopes.
    public var session: URLSession
    /// How calls and views retry a 503 or a failed connection.
    public var retry: CloudRetryOptions
    /// Randomness for the weighted node order, in [0, 1).
    public var random: @Sendable () -> Double

    public init(
        cluster: String, policy: CloudPolicy, directory: String = defaultDirectory, zone: String? = nil,
        session: URLSession = .shared, retry: CloudRetryOptions = CloudRetryOptions(),
        random: @escaping @Sendable () -> Double = { Double.random(in: 0..<1) }
    ) {
        self.cluster = cluster
        self.policy = policy
        self.directory = directory
        self.zone = zone
        self.session = session
        self.retry = retry
        self.random = random
    }
}

/// Where a client made by ``CalimeroClient/connect(_:)`` re-reads its node list.
private struct DirectorySource {
    let directory: String
    let cluster: String
    let zone: String?
    let random: @Sendable () -> Double
    /// When the listing is due to be read again.
    var due: Date
    /// The re-read under way, if any.
    var reading: Task<Void, Never>?
}

/// The client for Calimero 1.0, the confidential cloud: one app = one
/// replicated log on attested nodes. It verifies the node (``CloudPolicy``),
/// seals every request to it (``SealedTransport``), signs people in with
/// passkeys and machines with API keys. Port of mero-js `CalimeroClient`;
/// method names and semantics match it one for one.
///
/// ```swift
/// let cloud = CalimeroClient(CalimeroClientOptions(
///     baseUrl: "https://node.example", policy: CloudPolicy(release: CloudReleasePolicy(minReleaseVersion: "2.3.109"))))
/// try await cloud.signIn(PlatformPasskey())
/// let answer = try await cloud.call(appId, "set", ["key": "a", "value": "1"])
/// ```
///
/// - A `401` to a request that carried a session is answered by one refresh,
///   shared by every request that needs it at the same time (a refresh token
///   is used once), and the request is repeated once. A refused refresh
///   surfaces as ``LoginRequired``.
/// - ``connect(_:)`` reads a cluster's nodes from the node directory, and
///   reads them again after a failover or once the listing is due.
/// - `421 WRONG_NODE` moves the client to the next node it knows and repeats
///   the request once (not a sign-in or a refresh's ceremony: those are the
///   caller's, or ``refresh()``'s, to repeat). `421 NOT_LEADER` repeats it
///   once at `details.leaderUrl` when that is one of its nodes.
/// - Calls and views retry a `503` or a failed connection with the same
///   idempotency key (``CloudRetryOptions``); when a node's retries run out
///   they fail over to the next node, with the same key.
public actor CalimeroClient {
    /// Every node this client may use, `baseUrl` first (or as the directory last ordered them).
    private var nodeUrls: [String]
    private var at = 0
    /// The directory the node list came from, for a client made by ``connect(_:)``.
    private var source: DirectorySource?
    private var channels: [String: SealedTransport] = [:]
    private let policy: CloudPolicy
    private let urlSession: URLSession
    private let retry: CloudRetryOptions
    private var credential: String?
    private var current: CalimeroSession?
    private var refreshing: Task<CalimeroSession, Error>?
    private var refreshGeneration = 0
    private var sessionObserver: (@Sendable (CalimeroSession?) -> Void)?
    private var stepUpPasskey: (any PasskeyProvider)?
    private var operatorKey: (any OperatorSigner)?
    private let sleep: @Sendable (Double) async -> Void

    public init(_ options: CalimeroClientOptions) {
        self.init(options, sleep: { seconds in try? await Task.sleep(nanoseconds: UInt64(max(0, seconds) * 1e9)) })
    }

    init(_ options: CalimeroClientOptions, sleep: @escaping @Sendable (Double) async -> Void) {
        var seen = Set<String>()
        nodeUrls = ([options.baseUrl] + options.nodes).map(Self.trim).filter { seen.insert($0).inserted }
        policy = options.policy
        urlSession = options.session
        retry = options.retry
        self.sleep = sleep
    }

    /// A client for the Calimero Cloud cluster named `cluster`: its node list
    /// read from the public node directory, those in `zone` first, each group
    /// in weighted-random order. The directory only picks which nodes to try:
    /// each is attested under `policy` before anything is sent to it. Throws
    /// ``DirectoryError`` when the directory names no usable node.
    ///
    /// The list is read again in the background after a failover, and at the
    /// first request once the directory's `refreshAfterSeconds` has passed.
    /// (== mero-js `CalimeroClient.connect`.)
    public static func connect(_ options: CalimeroConnectOptions) async throws -> CalimeroClient {
        let listing = try await readDirectory(options.directory, cluster: options.cluster, session: options.session)
        let ordered = orderNodes(listing.nodes, zone: options.zone, random: options.random)
        let client = CalimeroClient(
            CalimeroClientOptions(
                baseUrl: ordered[0], nodes: Array(ordered.dropFirst()), policy: options.policy,
                session: options.session, retry: options.retry))
        await client.setSource(
            DirectorySource(
                directory: options.directory, cluster: options.cluster, zone: options.zone, random: options.random,
                due: Date().addingTimeInterval(listing.refreshAfterSeconds)))
        return client
    }

    private func setSource(_ source: DirectorySource) {
        self.source = source
    }

    /// `baseUrl` with no trailing slash.
    static func trim(_ url: String) -> String {
        var out = url
        while out.hasSuffix("/") { out.removeLast() }
        return out
    }

    /// The node requests go to now: `baseUrl`, until a `421 WRONG_NODE` or a failover moved the client on.
    public var nodeUrl: String { nodeUrls[at] }

    /// Every node this client may use, in the order it moves through them.
    public var nodes: [String] { nodeUrls }

    /// Call `observer` whenever the session changes: signed in, refreshed (a
    /// new refresh token, which replaces the stored one), or signed out
    /// (`nil`). A mobile app persists it from here. Not in mero-js, whose
    /// callers read ``session`` after each call.
    public func onSessionChange(_ observer: (@Sendable (CalimeroSession?) -> Void)?) {
        sessionObserver = observer
    }

    /// The sealed channel to `url`, attested under the client's policy before its first request.
    private func channel(_ url: String) throws -> SealedTransport {
        if let sealed = channels[url] { return sealed }
        guard let base = URL(string: url) else { throw TeeTrustError("bad node URL \(url)") }
        let policy = self.policy
        let session = urlSession
        let sealed = try SealedTransport(
            baseURL: base, key: .attest { try await fetchAttestedKey(url, policy: policy, session: session) },
            session: session)
        channels[url] = sealed
        return sealed
    }

    /// Move on from node `from` (the one that answered `421 WRONG_NODE`) to
    /// the next one. False when there is no other. A request that saw the
    /// same node refuse after another request moved on already does not move again.
    private func moveOn(from: String) -> Bool {
        guard nodeUrls.count > 1 else { return false }
        if nodeUrl == from { at = (at + 1) % nodeUrls.count }
        return true
    }

    /// Fail over from node `from`, where a request's retries ran out, to the
    /// next node it has not `tried`. False when it has tried them all. A
    /// request whose node another request moved away from already goes to
    /// that node, if untried.
    private func failOver(from: String, tried: Set<String>) -> Bool {
        if nodeUrl != from && !tried.contains(nodeUrl) { return true }
        let count = nodeUrls.count
        for step in 1..<max(count, 1) {
            let next = (at + step) % count
            if !tried.contains(nodeUrls[next]) {
                at = next
                reread()
                return true
            }
        }
        return false
    }

    /// Read the node list again from the directory, in the background, for a
    /// client made by ``connect(_:)``: the node in use stays first, then the
    /// directory's order; a node no longer listed is dropped. A failed read is
    /// ignored. One read at a time.
    private func reread() {
        guard let source, source.reading == nil else { return }
        let session = urlSession
        self.source?.reading = Task {
            let listing = try? await readDirectory(source.directory, cluster: source.cluster, session: session)
            self.reread(finished: listing)
        }
    }

    private func reread(finished listing: DirectoryListing?) {
        guard var source else { return }
        source.reading = nil
        if let listing {
            let current = nodeUrl
            let listed = orderNodes(listing.nodes, zone: source.zone, random: source.random)
            nodeUrls = [current] + listed.filter { $0 != current }
            at = 0
            source.due = Date().addingTimeInterval(listing.refreshAfterSeconds)
        }
        self.source = source
    }

    // MARK: - Credentials

    /// Authenticate every request with an API key (`cal_…`).
    public func useApiKey(_ key: String) {
        credential = key
        setCurrent(nil)
    }

    /// Authenticate every request with `session` (a stored one, say).
    public func useSession(_ session: CalimeroSession) {
        credential = session.sessionToken
        setCurrent(session)
    }

    /// The passkey that approves a `412 CONFIRM_REQUIRED` for a request sent
    /// through the facade (``orgs``, ``apps``, ...) or ``api`` without a
    /// `passkey:` of its own; `nil` (the default) lets the refusal through as
    /// ``ConfirmRequired``.
    public func useStepUp(_ passkey: (any PasskeyProvider)?) {
        stepUpPasskey = passkey
    }

    /// The operator key that signs a `428 OPERATOR_SIGNATURE_REQUIRED`
    /// statement for a request without an `operatorSigner:` of its own;
    /// `nil` (the default) lets the refusal through as ``OperatorSignatureRequired``.
    public func useOperatorSigner(_ signer: (any OperatorSigner)?) {
        operatorKey = signer
    }

    /// What answers a step-up when the request names nothing itself.
    var stepUpDefaults: (passkey: (any PasskeyProvider)?, signer: (any OperatorSigner)?) {
        (stepUpPasskey, operatorKey)
    }

    /// The session in use, or `nil` (signed out, or an API key).
    public var session: CalimeroSession? { current }

    private func setCurrent(_ session: CalimeroSession?) {
        current = session
        sessionObserver?(session)
    }

    // MARK: - Sending

    /// One request to the node in use, with the credential held now; the
    /// response whatever its status.
    private func request(
        _ method: String, _ path: String, body: Data?, contentType: String, headers: [String: String] = [:]
    ) async throws -> SealedResponse {
        if let source, Date() >= source.due { reread() }
        let url = nodeUrl
        guard let target = URL(string: "\(url)/\(path)") else { throw AccountError.invalidInput("bad path \(path)") }
        var request = URLRequest(url: target)
        request.httpMethod = method
        for (name, value) in headers { request.setValue(value, forHTTPHeaderField: name) }
        if let credential { request.setValue("Bearer \(credential)", forHTTPHeaderField: "authorization") }
        if let body {
            if headers["content-type"] == nil { request.setValue(contentType, forHTTPHeaderField: "content-type") }
            request.httpBody = body
        }
        do {
            return try await channel(url).send(request)
        } catch let error as SealedTransportError where error.status == 413 {
            // The node refuses an envelope past its sealed cap in the clear,
            // before opening it: to the caller that is the API's `TOO_LARGE`.
            throw errorFrom(
                status: 413,
                body: [
                    "error": "TOO_LARGE",
                    "message": .string(error.message.replacingOccurrences(of: "Sealed request refused: ", with: "")),
                ])
        }
    }

    /// The ceremonies that mint a session, which ``send`` never repeats by
    /// itself. A node answers them `421 WRONG_NODE` before it spends the
    /// challenge or the refresh token.
    private static func ceremony(_ path: String) -> Bool {
        path == "auth/refresh" || path.hasPrefix("auth/passkey/")
    }

    /// ``request``, answering a lost issuing node, a node on the wrong
    /// release or a follower: a successful response, or the refusal as a
    /// typed error. (== mero-js `send`.)
    private func send(
        _ method: String, _ path: String, body: Data? = nil, contentType: String = "application/json",
        headers: [String: String] = [:]
    ) async throws -> SealedResponse {
        var refreshed = false
        var moved = false
        var redirected = false
        while true {
            let token = credential
            let from = nodeUrl
            let response = try await request(method, path, body: body, contentType: contentType, headers: headers)
            if (200..<300).contains(response.status) { return response }
            let error = errorFrom(
                status: response.status, data: (try? await response.data()) ?? Data(),
                retryAfter: response.headers["retry-after"])
            if error is WrongNode, !moved {
                moved = true
                if moveOn(from: from), !Self.ceremony(path) { continue }
            } else if let notLeader = error as? NotLeader, !redirected,
                let leader = notLeader.leaderUrl.map(Self.trim), let index = nodeUrls.firstIndex(of: leader)
            {
                redirected = true
                at = index
                continue
            } else if error is LoginRequired, !refreshed, let token, current != nil, !Self.ceremony(path) {
                refreshed = true
                // A refresh another request made already answers this one.
                if credential == token { _ = try await refresh() }
                continue
            }
            throw error
        }
    }

    /// ``send`` for the generated client's transport (``SealedClientTransport``):
    /// `headers` (names lowercased) are the generated request's, the
    /// credential is the client's.
    func exchange(
        _ method: String, _ path: String, body: Data?, headers: [String: String]
    ) async throws
        -> SealedResponse
    {
        try await send(method, path, body: body, headers: headers)
    }

    /// A route the node serves in the clear (`/health`, `/attestation`),
    /// fetched from the node in use without the sealed channel: what it
    /// answers is for display, never trusted. A refusal throws.
    func plain(_ method: String, _ path: String) async throws -> SealedResponse {
        guard let url = URL(string: "\(nodeUrl)/\(path)") else { throw AccountError.invalidInput("bad path \(path)") }
        var request = URLRequest(url: url)
        request.httpMethod = method
        let (data, response) = try await urlSession.data(for: request)
        let http = response as? HTTPURLResponse
        let status = http?.statusCode ?? 0
        guard (200..<300).contains(status) else {
            throw errorFrom(status: status, data: data, retryAfter: http?.value(forHTTPHeaderField: "retry-after"))
        }
        var headers: [String: String] = [:]
        for (name, value) in http?.allHeaderFields ?? [:] {
            if let name = name as? String, let value = value as? String { headers[name.lowercased()] = value }
        }
        let body = AsyncThrowingStream<Data, Error> {
            $0.yield(data)
            $0.finish()
        }
        return SealedResponse(status: status, headers: headers, body: body)
    }

    func json(
        _ method: String, _ path: String, body: Data? = nil, contentType: String = "application/json"
    )
        async throws -> Data
    {
        let response = try await send(method, path, body: body, contentType: contentType)
        return try await response.data()
    }

    func decode<T: Decodable>(_ type: T.Type, _ data: Data) throws -> T {
        do {
            return try MeroJSON.decode(type, from: data.isEmpty ? Data("{}".utf8) : data)
        } catch {
            throw AccountError.protocolViolation("the node's answer did not decode as \(T.self): \(error)")
        }
    }

    func value(_ data: Data) throws -> JSONValue {
        try decode(JSONValue.self, data)
    }

    /// A generated answer as MeroKit's own type `T`, through its JSON.
    static func reading<T: Decodable>(_ type: T.Type, _ value: some Encodable) throws -> T {
        try MeroJSON.convert(value, to: type)
    }

    /// `value` as generated type `T`, through its JSON.
    static func converted<T: Decodable>(_ value: some Encodable) throws -> T {
        try MeroJSON.decode(T.self, from: try MeroJSON.encode(value))
    }

    /// `value` as generated type `T`.
    static func converted<T: Decodable>(json value: JSONValue) throws -> T {
        try MeroJSON.decode(T.self, from: CanonicalJSON.data(value))
    }

    /// ``json`` for a call or a view: while the node answers 503 or cannot be
    /// reached, the same request, with the same idempotency key, is sent again
    /// after a backoff (or the pause the node asked for, if longer), at most
    /// `retries` times.
    ///
    /// When a node's retries run out, the request fails over to the next node
    /// the client knows and its retries start again there, with the same key:
    /// the key is kept in the app's replicated log, so the cluster answers it
    /// once whichever node it reaches. Each node is tried once per request.
    /// The last failure surfaces as ``UnavailableError``.
    ///
    /// `once` sends the request exactly once, to the node in use (the
    /// confirmed attempt of ``callConfirmed(_:_:_:passkey:idempotencyKey:)``,
    /// whose challenge is single-use).
    private func retried(_ path: String, body: Data, once: Bool = false) async throws -> Data {
        try await retrying(once: once) { try await self.json("POST", path, body: body) }
    }

    /// ``exchange`` for an operation the document marks retry-safe (a view,
    /// or a call that carries an idempotency key): retried and failed over as
    /// ``retried(_:body:once:)`` retries, with the whole answer read inside
    /// each attempt.
    func retriedExchange(
        _ method: String, _ path: String, body: Data?, headers: [String: String]
    ) async throws
        -> SealedResponse
    {
        try await retrying(once: false) {
            let response = try await self.send(method, path, body: body, headers: headers)
            let data = try await response.data()
            let replay = AsyncThrowingStream<Data, Error> {
                $0.yield(data)
                $0.finish()
            }
            return SealedResponse(status: response.status, headers: response.headers, body: replay)
        }
    }

    /// `operation`, again while the node answers 503 or cannot be reached (see ``retried(_:body:once:)``).
    private func retrying<T>(once: Bool, _ operation: () async throws -> T) async throws -> T {
        let limit = once ? 0 : retry.retries
        var tried = Set<String>()
        while true {
            var unavailable: UnavailableError?
            var node = nodeUrl
            var attempt = 0
            while true {
                node = nodeUrl
                tried.insert(node)
                do {
                    return try await operation()
                } catch let error as UnavailableError {
                    unavailable = error
                } catch let error as URLError {
                    unavailable = UnavailableError(
                        code: "NETWORK", message: "the node could not be reached: \(error.localizedDescription)",
                        cause: error)
                }
                if attempt >= limit { break }
                await sleep(max(retry.delay(attempt), unavailable?.retryAfter ?? 0))
                attempt += 1
            }
            if once || !failOver(from: node, tried: tried), let unavailable { throw unavailable }
        }
    }

    /// `POST path` with a JSON body; the answer as JSON.
    @discardableResult
    public func post(_ path: String, _ body: JSONValue = [:]) async throws -> JSONValue {
        try value(try await json("POST", path, body: CanonicalJSON.data(body)))
    }

    /// `GET path`; the answer as JSON.
    public func get(_ path: String) async throws -> JSONValue {
        try value(try await json("GET", path))
    }

    /// `PUT path` with a JSON body; the answer as JSON.
    @discardableResult
    public func put(_ path: String, _ body: JSONValue = [:]) async throws -> JSONValue {
        try value(try await json("PUT", path, body: CanonicalJSON.data(body)))
    }

    /// `DELETE path`, with a JSON body when given; the answer as JSON.
    @discardableResult
    public func delete(_ path: String, _ body: JSONValue? = nil) async throws -> JSONValue {
        try value(try await json("DELETE", path, body: body.map { CanonicalJSON.data($0) }))
    }

    // MARK: - Identity

    /// Create an account with a new passkey, and sign in. The node then asks
    /// the new passkey to sign the account it creates (`412
    /// CONFIRM_REQUIRED`, core rc.9+), so a sign-up prompts twice: create,
    /// then sign.
    ///
    /// `invite` is an organization invite's token (the new account joins the
    /// organization at the invite's role) or a cluster sign-up invite's; a
    /// cluster whose policy is `invite` or `closed` (``signUpPolicy()``)
    /// needs one. It goes on both halves and on the retry. A sign-up the
    /// policy refuses, or an unknown, expired or used invite, is
    /// ``SignUpRefused``, from `begin`, before any passkey is made.
    ///
    /// `origin` names the web origin the ceremony runs on, for a frontend
    /// served from an origin a package's manager registered
    /// (``addPackageOrigin(_:origin:passkey:rpId:)``): the passkey is made for
    /// that origin's relying party and the session is bound to its package.
    /// An unregistered one is ``OriginNotRegistered``. A native app leaves it `nil`.
    @discardableResult
    public func signUp(
        _ passkey: any PasskeyProvider, name: String, invite: String? = nil, origin: String? = nil
    ) async throws -> CalimeroSession {
        let invite = invite.flatMap { $0.isEmpty ? nil : $0 }
        let begin = try await beginCeremony(origin: origin) {
            try JSONValue(encoding: try await self.auth.registerBegin(name: name, origin: origin, invite: invite))
        }
        guard let challengeId = begin["challengeId"]?.stringValue, let options = begin["options"] else {
            throw AccountError.protocolViolation("the registration did not begin")
        }
        let registration = try await passkey.create(options: options)
        let converted: Components.Schemas.Registration = try Self.converted(registration)
        // The node asks the new passkey to sign the account it creates: the
        // step-up middleware answers with it.
        let session = try Self.reading(
            CalimeroSession.self,
            try await auth.registerFinish(
                challengeId: challengeId, registration: converted, invite: invite, passkey: passkey))
        useSession(session)
        return session
    }

    /// POST a ceremony's `begin`, naming the `origin` sent in an
    /// ``OriginNotRegistered`` the node leaves it out of.
    private func beginCeremony(
        origin: String?, _ begin: @Sendable () async throws -> JSONValue
    ) async throws -> JSONValue {
        do {
            return try await begin()
        } catch let error as OriginNotRegistered where error.origin.isEmpty && origin != nil {
            var details = error.details
            details["origin"] = origin.map(JSONValue.string)
            throw OriginNotRegistered(error.message, details: details)
        }
    }

    /// Sign in with an existing passkey. `origin`: as for
    /// ``signUp(_:name:invite:origin:)``; the assertion is made for the
    /// relying-party id the node answers for it.
    @discardableResult
    public func signIn(_ passkey: any PasskeyProvider, origin: String? = nil) async throws -> CalimeroSession {
        let begin = try await beginCeremony(origin: origin) {
            try JSONValue(encoding: try await self.auth.loginBegin(origin: origin))
        }
        guard let challengeId = begin["challengeId"]?.stringValue,
            let challenge = begin["options"]?["challenge"]?.stringValue,
            let rpId = begin["options"]?["rpId"]?.stringValue ?? begin["options"]?["rp"]?["id"]?.stringValue
        else { throw AccountError.protocolViolation("the sign-in did not begin") }
        let assertion = try await passkey.get(challenge: try fromB64url(challenge), rpId: rpId, allowCredentials: nil)
        let converted: Components.Schemas.Assertion = try Self.converted(assertion)
        let session = try Self.reading(
            CalimeroSession.self, try await auth.loginFinish(challengeId: challengeId, assertion: converted))
        useSession(session)
        return session
    }

    /// Swap the refresh token for a new session, on whichever node the client
    /// uses. A refresh token is used once, so calls made while one refresh is
    /// under way share it rather than sending the same token again (which
    /// would revoke the session).
    @discardableResult
    public func refresh() async throws -> CalimeroSession {
        if let refreshing { return try await refreshing.value }
        refreshGeneration += 1
        let generation = refreshGeneration
        let task = Task { try await self.exchangeRefresh() }
        refreshing = task
        defer { if refreshGeneration == generation { refreshing = nil } }
        return try await task.value
    }

    private func exchangeRefresh() async throws -> CalimeroSession {
        guard let current else { throw LoginRequired("no session to refresh") }
        // A node that may not serve the account answers `421 WRONG_NODE`
        // before it spends the refresh token, so the same token goes to the
        // next node (`send` moved there already): once per node, at most.
        var tried = 1
        while true {
            do {
                let token = current.refreshToken
                let session = try Self.reading(CalimeroSession.self, try await auth.refresh(refreshToken: token))
                // A sign-in or another session taken while this one was refreshing wins.
                if self.current == current { useSession(session) }
                return session
            } catch let error as WrongNode {
                if tried >= nodeUrls.count { throw error }
                tried += 1
            }
        }
    }

    /// End this session.
    public func signOut() async throws {
        _ = try await auth.logout()
        credential = nil
        setCurrent(nil)
    }

    /// End every session of the signed-in account, on every device, this one too.
    public func signOutEverywhere() async throws {
        _ = try await auth.logoutAll()
        credential = nil
        setCurrent(nil)
    }

    /// Forget the credential without telling the node (it is unreachable, say).
    public func forget() {
        credential = nil
        setCurrent(nil)
    }

    /// Add another passkey (another device, `passkey`) to the signed-in
    /// account; answers its credential id. The node asks for the approval of
    /// a passkey the account already has (`412 CONFIRM_REQUIRED`, never the
    /// new one), which `approver` gives.
    public func addPasskey(_ passkey: any PasskeyProvider, approver: any PasskeyProvider) async throws -> String {
        let begin = try JSONValue(encoding: try await auth.passkeyAddBegin())
        guard let challengeId = begin["challengeId"]?.stringValue, let options = begin["options"] else {
            throw AccountError.protocolViolation("adding a passkey did not begin")
        }
        let created = try await passkey.create(options: options)
        let registration: Components.Schemas.Registration = try Self.converted(created)
        let finished = try await auth.passkeyAddFinish(
            challengeId: challengeId, registration: registration, passkey: approver)
        return finished.credentialId
    }

    // MARK: - Bundles and blobs

    /// Upload a sealed file for component app `app` (core rc.9+,
    /// `POST /apps/{app}/blobs?sealed=true`): sealed under a key of its own
    /// that only the app's code reads, replicated on the app's replicas. Its
    /// owner and its organization's members only (`403` to others), charged
    /// to the organization's file quota (``QuotaExceeded``); a non-component
    /// app answers `409 NOT_SUPPORTED`. Answers its id, the plaintext's
    /// `size` and `sha256`, and the upload's log `index`. No route serves,
    /// replaces or deletes a sealed file: the app's runs open it by id.
    public func uploadFile(_ app: String, _ data: Data) async throws -> BlobInfo {
        try decode(
            BlobInfo.self, try await answerBytes { try await self.apps.blobUpload(app: app, body: data, sealed: true) })
    }

    /// ``downloadBlob(_:_:)`` (or, with `grant`,
    /// ``downloadBlobGranted(_:_:grant:)``) with the content type the node
    /// serves it as: the uploader's, for a component app;
    /// `application/octet-stream` otherwise.
    public func blobContent(_ app: String, _ id: String, grant: String? = nil) async throws -> BlobContent {
        // The document says `application/octet-stream`, but a component
        // app's blob comes back as the uploader's type, which the generated
        // decoder refuses: the tap keeps the bytes and the node's type.
        let tap = AnswerTap()
        do {
            _ = try await AnswerTap.$current.withValue(tap) {
                try await self.apps.blobDownload(app: app, id: id, grant: grant)
            }
        } catch {
            guard tap.data != nil else { throw error }
        }
        return BlobContent(data: tap.data ?? Data(), contentType: tap.contentType ?? "application/octet-stream")
    }

    /// `app`'s blobs.
    public func listBlobs(_ app: String) async throws -> [BlobInfo] {
        try decode(BlobList.self, try await answerBytes { try await self.apps.blobList(app: app) }).blobs
    }

    /// One of `app`'s blobs' id and size; ``NotFound`` if the app has no such blob.
    public func blobInfo(_ app: String, _ id: String) async throws -> BlobInfo {
        try decode(BlobInfo.self, try await answerBytes { try await self.apps.blobInfo(app: app, id: id) })
    }

    // MARK: - Packages

    // MARK: - App instances

    /// POST `body` to `path`, answering a `412 CONFIRM_REQUIRED` with
    /// `passkey`'s assertion over the node's challenge (deleting an org,
    /// approving a release, adding a member, ...): the same body again, with
    /// `confirm`.
    @discardableResult
    public func postConfirmed(_ path: String, _ body: JSONValue, passkey: any PasskeyProvider) async throws -> JSONValue
    {
        try value(
            try await confirmed(body, passkey: passkey) { body in
                try await self.json("POST", path, body: CanonicalJSON.data(body))
            })
    }

    /// ``postConfirmed(_:_:passkey:)`` for a DELETE (deleting an organization,
    /// removing a member or a passkey): `body` goes as JSON, and the
    /// confirmed retry carries `confirm` beside it.
    @discardableResult
    public func deleteConfirmed(
        _ path: String, passkey: any PasskeyProvider, body: JSONValue = [:]
    ) async throws
        -> JSONValue
    {
        try value(
            try await confirmed(body, passkey: passkey) { body in
                try await self.json("DELETE", path, body: CanonicalJSON.data(body))
            })
    }

    private func confirmed(
        _ body: JSONValue, passkey: any PasskeyProvider, send: @Sendable (JSONValue) async throws -> Data
    ) async throws -> Data {
        do {
            return try await send(body)
        } catch let required as ConfirmRequired {
            var confirmedBody = body.objectValue ?? [:]
            confirmedBody["confirm"] = try await StepUp.confirmation(required, passkey: passkey)
            return try await send(.object(confirmedBody))
        }
    }

    /// Call a method. A retry with the same `idempotencyKey` is answered
    /// once. Without one, the client makes one, so its own retries of a 503 share it.
    public func call(
        _ app: String, _ method: String, _ args: JSONValue = [:], idempotencyKey: String? = nil
    )
        async throws -> CallResult
    {
        try decode(
            CallResult.self,
            try await callJSON(app, method, argsJSON: CanonicalJSON.data(args), idempotencyKey: idempotencyKey))
    }

    /// ``call(_:_:_:idempotencyKey:)`` with JSON bytes in and the whole answer
    /// out, every number kept as written (a `u64` beyond 2^53).
    public func callJSON(
        _ app: String, _ method: String, argsJSON: Data, idempotencyKey: String? = nil
    )
        async throws -> Data
    {
        let key = idempotencyKey ?? Self.newIdempotencyKey()
        return try await retried(
            "apps/\(Self.seg(app))/intents", body: try Self.body(method: method, argsJSON: argsJSON, key: key))
    }

    /// Call a method that needs the account holder's approval: the node
    /// returns a challenge over this exact call, which `passkey` signs. A
    /// declined prompt throws ``ConfirmCancelled``.
    public func callConfirmed(
        _ app: String, _ method: String, _ args: JSONValue, passkey: any PasskeyProvider, idempotencyKey: String? = nil
    ) async throws -> CallResult {
        let path = "apps/\(Self.seg(app))/intents"
        let key = idempotencyKey ?? Self.newIdempotencyKey()
        let body: JSONValue = ["method": .string(method), "args": args, "idempotencyKey": .string(key)]
        // The confirmed attempt goes out exactly once: its challenge is single-use.
        let data = try await confirmed(body, passkey: passkey) { body in
            try await self.retried(path, body: CanonicalJSON.data(body), once: body["confirm"] != nil)
        }
        return try decode(CallResult.self, data)
    }

    /// Run a read-only method: strong by default, or stale from any replica.
    public func view(
        _ app: String, _ method: String, _ args: JSONValue = [:], stale: Bool = false, minIndex: Int = 0
    )
        async throws -> ViewResult
    {
        try decode(
            ViewResult.self,
            try await viewJSON(app, method, argsJSON: CanonicalJSON.data(args), stale: stale, minIndex: minIndex))
    }

    /// ``view(_:_:_:stale:minIndex:)`` with JSON bytes in and the whole answer out.
    public func viewJSON(
        _ app: String, _ method: String, argsJSON: Data, stale: Bool = false, minIndex: Int = 0
    )
        async throws -> Data
    {
        let args = try LiteralJSON.write(LiteralJSON.parse(argsJSON.isEmpty ? Data("{}".utf8) : argsJSON))
        let body =
            "{\"args\":\(args),\"consistency\":\(stale ? "\"stale\"" : "\"strong\""),"
            + "\"method\":\(CanonicalJSON.quote(method)),\"minIndex\":\(minIndex)}"
        return try await retried("apps/\(Self.seg(app))/views", body: Data(body.utf8))
    }

    /// Committed events from index `from`, then live, until the stream is
    /// cancelled (stop iterating, or cancel the task).
    public nonisolated func events(_ app: String, from: Int = 0) -> AsyncThrowingStream<CloudEvent, Error> {
        stream { try await self.apps.events(app: app, from: Int64(from)) }
    }

    /// A server-sent event stream on `path`, sealed like every other answer.
    private nonisolated func stream(
        _ open: @escaping @Sendable () async throws -> OpenAPIRuntime.HTTPBody
    ) -> AsyncThrowingStream<CloudEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    var parser = SseParser()
                    for try await chunk in try await open() {
                        for event in parser.feed(Data(chunk)) { continuation.yield(event) }
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    // MARK: - Presence

    /// Presence as a stream: a `snapshot` event, then a `presence` event per change.
    public nonisolated func presenceStream(_ app: String) -> AsyncThrowingStream<CloudEvent, Error> {
        stream { try await self.apps.presenceStream(app: app) }
    }

    // MARK: - Aliases

    // MARK: - Organizations

    /// Set or rotate (`publicKey`, ``RecoverySeed/publicKey``) or remove
    /// (`nil`) the organization's recovery key (core rc.9+): its owner only,
    /// approved with `passkey`. A recovery under way is cancelled, and the
    /// old seed's signatures are refused from then on.
    public func setRecoveryKey(_ org: String, publicKey: String?, passkey: any PasskeyProvider) async throws {
        // `nil` removes the key: an explicit `publicKey: null`.
        let key: String?? = .some(publicKey)
        _ = try await orgs.recoverySet(org: org, publicKey: key, passkey: passkey)
    }

    /// Start handing `org` to the signed-in account with the recovery
    /// `seed`, which signs here and never leaves this process (core rc.9+).
    /// Finish with ``finishRecovery(_:passkey:)`` once `readyAt` has passed
    /// (72 hours); until then the owner may ``cancelRecovery(_:passkey:)``
    /// and every member sees it in ``OrgInfo/recovery``. A wrong seed, no
    /// organization or none set is `403 WRONG_RECOVERY_KEY`.
    public func startRecovery(_ org: String, seed: RecoverySeed) async throws -> RecoveryTimes {
        let account = try await auth.me().accountId
        let nonce = b64url(randomBytes(16))
        let signature = try seed.sign(org: org, account: account, nonce: nonce)
        return try Self.reading(
            RecoveryTimes.self, try await orgs.recoveryStart(org: org, nonce: nonce, signature: signature))
    }

    // MARK: - Approvals

    /// ``appsReport(_:page:)``, every page, with the apps of all of them in `apps`.
    public func appsReportAll(_ org: String?) async throws -> AppsReport {
        var report = try await reportPage(org, PageOptions())
        var next = report.next
        while let after = next {
            let page = try await reportPage(org, PageOptions(after: after))
            report.apps += page.apps
            next = page.next
        }
        report.next = nil
        return report
    }

    /// ``appsReport(_:page:)``, for the methods that are not deprecated.
    private func reportPage(_ org: String?, _ page: PageOptions) async throws -> AppsReport {
        _ = try page.query()
        if let org {
            return try Self.reading(
                AppsReport.self, try await orgs.apps(org: org, after: page.after, limit: page.limit.map { Int64($0) }))
        }
        return try Self.reading(
            AppsReport.self, try await auth.apps(after: page.after, limit: page.limit.map { Int64($0) }))
    }

    /// What waits for a decision in a scope (`org`, or the account's own apps with `nil`).
    public func pendingApprovals(org: String? = nil) async throws -> PendingApprovals {
        async let report = appsReportAll(org)
        async let rules = publicApi.releasesPublic()
        return pendingApprovalsFrom(try await report, rules: try Self.reading(ReleaseRules.self, try await rules))
    }

    /// ``requestConsent(_:)``, then hand the page's URL to `navigate`.
    /// (== mero-js `openConsentPage`, whose default navigates the window.)
    public func openConsentPage(
        _ request: ConsentRequest, navigate: @escaping @Sendable (URL) async throws -> Void
    ) async throws
        -> ConsentStarted
    {
        try await openConsentPage(request, options: OpenConsentOptions(navigate: navigate))
    }

    /// ``requestConsent(_:)``, then send the person to the consent page with
    /// `options.navigate`. `options.confirm` runs first with the node's
    /// answer so the app can show the person its `code`; when it answers
    /// `false` the page is not opened. When the node sends a `code` that is
    /// not ``confirmationCode(_:)`` of `requestId`, this throws a
    /// ``CalimeroError`` (`CONSENT_CODE_MISMATCH`) and does not navigate.
    /// (== mero-js `openConsentPage(request, options)`.)
    public func openConsentPage(_ request: ConsentRequest, options: OpenConsentOptions) async throws -> ConsentStarted {
        let started = try await requestConsent(request)
        if let confirm = options.confirm, !(await confirm(started)) { return started }
        guard let url = URL(string: started.url) else { throw AccountError.protocolViolation("bad consent URL") }
        try await options.navigate(url)
        return started
    }

    /// Throws `CONSENT_CODE_MISMATCH` when the node's `code` is not the one
    /// its consent page shows for `requestId`.
    public static func checkConsentCode(_ started: ConsentStarted) throws {
        guard let code = started.code else { return }
        let expected = confirmationCode(started.requestId)
        guard code == expected else {
            throw CalimeroError(
                status: 200, code: "CONSENT_CODE_MISMATCH", message: "the consent page code does not match the request",
                details: [
                    "requestId": .string(started.requestId), "code": .string(code), "expected": .string(expected),
                ])
        }
    }

    /// Poll ``consentResult(_:)`` until the request is decided, every
    /// `interval` seconds, until the task is cancelled or `expiresAt` (Unix
    /// seconds) passes, when it answers the last `pending` result.
    public func waitForConsent(
        _ requestId: String, interval: Double = 2, expiresAt: Double? = nil
    ) async throws
        -> ConsentResult
    {
        while true {
            let result = try await consentResult(requestId)
            if result.status != .pending || Task.isCancelled { return result }
            if let expiresAt, Date().timeIntervalSince1970 >= expiresAt { return result }
            await sleep(interval)
        }
    }

    // MARK: - Operator

    // MARK: - Members, teams and invites

    /// Rename, change who may join, set the app's metadata, or move the team:
    /// `parent` `.some(nil)` moves it to the top, `nil` leaves it.
    public func updateTeam(
        _ org: String, _ team: String, name: String? = nil, join: TeamJoin? = nil, parent: String?? = nil,
        metadata: JSONValue? = nil
    ) async throws {
        let join: Components.Schemas.TeamJoin? = try join.map { try Self.converted($0) }
        let metadata = try metadata.map { try OpenAPIValueContainer($0) }
        // `.some(nil)` moves it to the top: an explicit `parent: null`.
        _ = try await orgs.teamUpdate(
            org: org, team: team, name: name, join: join, parent: parent, metadata: metadata)
    }

    // MARK: - Package origins

    // MARK: - Deployers

    // MARK: - Deployments

    // MARK: - Helpers

    /// One URL path segment, so an id cannot reach another route.
    static func seg(_ value: String) -> String {
        value.addingPercentEncoding(withAllowedCharacters: .urlPathSegment) ?? value
    }

    static func queryValue(_ value: String) -> String {
        value.addingPercentEncoding(withAllowedCharacters: .urlQueryValue) ?? value
    }

    /// A key for a call the caller gave none, made once so every retry
    /// shares it: `node:c-` and 16 random bytes in base64url, as the Rust
    /// client mints it (the node keeps a `node:` key 10 minutes).
    static func newIdempotencyKey() -> String {
        "node:c-\(b64url(randomBytes(16)))"
    }

    /// `{"args":…,"idempotencyKey":…,"method":…}` with `argsJSON` as written.
    static func body(method: String, argsJSON: Data, key: String) throws -> Data {
        let args: String
        do {
            args = LiteralJSON.write(try LiteralJSON.parse(argsJSON.isEmpty ? Data("{}".utf8) : argsJSON))
        } catch {
            throw AccountError.invalidInput("argsJSON is not JSON: \(error)")
        }
        return Data(
            "{\"args\":\(args),\"idempotencyKey\":\(CanonicalJSON.quote(key)),\"method\":\(CanonicalJSON.quote(method))}"
                .utf8)
    }

    static func json<T: Encodable>(_ value: T) throws -> JSONValue {
        try MeroJSON.decode(JSONValue.self, from: try MeroJSON.encode(value))
    }
}

/// A call's bundle: a hash, or a package release. (== mero-js `createApp`'s `bundle`.)
public enum AppSource: Sendable, Equatable {
    /// A bundle hash (a raw upload, development clusters only).
    case bundle(String)
    /// A package release: the latest when `version` is `nil`; `service` names
    /// one service of a multi-service bundle; `deployment` creates it from a
    /// deployment's offer (``pageDeployment(_:)``).
    case package(String, version: String? = nil, service: String? = nil, deployment: String? = nil)
}

private struct BlobList: Decodable { let blobs: [BlobInfo] }
struct PackageOriginList: Decodable { let origins: [PackageOrigin] }
struct PackageList: Decodable { let packages: [PackageInfo] }
struct AppList: Decodable { let apps: [AppInfo] }
struct DeploymentList: Decodable { let deployments: [DeploymentInfo] }
struct PresenceList: Decodable { let present: [PresenceEntry] }

/// Server-sent event frames from a byte stream: `{ id, kind, data }` each,
/// `data` parsed as JSON when it is. (== mero-js `sseFrames`.)
struct SseParser {
    private var buffer = Data()

    mutating func feed(_ chunk: Data) -> [CloudEvent] {
        buffer.append(chunk)
        var out: [CloudEvent] = []
        while let cut = Self.frameEnd(buffer) {
            let frame = String(decoding: buffer[buffer.startIndex..<cut.lowerBound], as: UTF8.self)
            buffer = Data(buffer[cut.upperBound...])
            var id = ""
            var kind = ""
            var data = ""
            for line in frame.split(separator: "\n", omittingEmptySubsequences: false) {
                let line = line.hasSuffix("\r") ? line.dropLast() : line[...]
                if line.hasPrefix("id:") {
                    id = line.dropFirst(3).trimmingCharacters(in: .whitespaces)
                } else if line.hasPrefix("event:") {
                    kind = line.dropFirst(6).trimmingCharacters(in: .whitespaces)
                } else if line.hasPrefix("data:") {
                    data += line.dropFirst(5).trimmingCharacters(in: .whitespaces)
                }
            }
            if kind.isEmpty && data.isEmpty { continue }
            let parsed = (try? MeroJSON.decode(JSONValue.self, from: Data(data.utf8))) ?? .string(data)
            out.append(CloudEvent(id: id, kind: kind, data: parsed))
        }
        return out
    }

    private static func frameEnd(_ data: Data) -> Range<Data.Index>? {
        let lf = data.range(of: Data("\n\n".utf8))
        let crlf = data.range(of: Data("\r\n\r\n".utf8))
        switch (lf, crlf) {
        case (let a?, let b?): return a.lowerBound <= b.lowerBound ? a : b
        case (let a?, nil): return a
        case (nil, let b?): return b
        default: return nil
        }
    }
}
