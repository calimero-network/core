import CryptoKit
import Foundation

#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

// Sealed transport: requests encrypted end to end to a TEE node's attested
// key. Port of mero-js `src/sealed/sealed.ts`; the node's side, and the wire
// format implemented here byte for byte, are core's
// `crates/server/src/sealed.rs`:
//
//   handshake   Noise_NK_25519_AESGCM_SHA256, prologue "calimero/sealed-http/v2"
//     request   POST /sealed/handshake      0x02 ‖ transport_pk ‖ Noise message 1
//               (nodes before it served only /sealed/v2/handshake; see legacyHandshakePath)
//     response  0x02 ‖ Noise message 2 (payload = session_id(16) ‖ lifetime secs, u32 BE)
//     keys      (req key, resp key) = Split()
//   exchange    POST /sealed/v2
//     header    0x02 ‖ session_id(16) ‖ request_id (u64 BE; from 1, each used once)
//     request   header ‖ AES-256-GCM(req key, nonce = request_id ‖ 0u32, aad = header, inner)
//     response  frames, each u32 BE length ‖ AES-256-GCM(resp key,
//               nonce = request_id ‖ frame index (u32 BE, from 0), aad = header, kind ‖ data)
//               kind 0 = head (JSON), 1 = body bytes, 2 = end; no end frame = cut short
//     inner     u32 BE head length ‖ head (JSON) ‖ body
//
// A node speaks plain HTTP, and any TLS in front of it ends outside the
// enclave, at whatever proxy or relay the operator runs; so everything
// crossing that hop (the bearer token, JSON-RPC arguments, results) is
// readable there. Sealing encrypts it so that only the attested TD can read
// it, and only the TD can produce a response the client accepts. Responses
// stream back in sealed frames, so server-sent events are sealed too.
// WebSockets cannot be, and the node refuses one inside the envelope (`501`).

/// The node no longer holds the transport key this client attested (it
/// restarted). (== mero-js `StaleTransportKeyError`.)
public struct StaleTransportKeyError: Error, LocalizedError, Sendable, Equatable {
    public init() {}
    public var errorDescription: String? {
        "The node no longer holds the attested transport key; attest again for the current one"
    }
}

/// The node refused the envelope itself, before anything inside it ran.
/// (== mero-js `SealedTransportError`.)
public struct SealedTransportError: Error, LocalizedError, Sendable, Equatable {
    public let status: Int
    /// The node's `error.code` (`unknown_session`, `busy`, `malformed`, …).
    public let code: String?
    public let message: String
    /// How long the node asked to wait before retrying (`Retry-After`, at most 5 s).
    public let retryAfter: TimeInterval?

    public init(status: Int, code: String?, message: String, retryAfter: TimeInterval? = nil) {
        self.status = status
        self.code = code
        self.message = message
        self.retryAfter = retryAfter
    }

    public var errorDescription: String? { message }
}

/// A response that came back through a ``SealedTransport``: its head, and a
/// body that opens each sealed frame as it arrives. A frame that does not
/// open, arrives out of order, or a stream that stops before its end frame
/// fails the body rather than passing anything unauthenticated on.
public struct SealedResponse: Sendable {
    public let status: Int
    /// Header names lowercased.
    public let headers: [String: String]
    public let body: AsyncThrowingStream<Data, Error>

    /// The whole body.
    public func data() async throws -> Data {
        var out = Data()
        for try await chunk in body { out.append(chunk) }
        return out
    }
}

/// Where a ``SealedTransport`` gets the node's transport key.
public enum TransportKeySource: Sendable {
    /// The attested key itself. A restarted node then surfaces as ``StaleTransportKeyError``.
    case key(Data)
    /// A function that attests and returns it — typically
    /// ``fetchAttestedTransportKey(_:verifier:applicationId:applicationHash:)``.
    /// A restarted node is handled: the key is attested again, through it,
    /// and the request retried once.
    case attest(@Sendable () async throws -> Data)
}

/// Requests sealed to one TEE node's attested transport key.
/// (== mero-js `createSealedFetch`.)
///
/// Send through it directly (``send(_:)``, ``data(for:)``), or hand its
/// ``urlSession(configuration:)`` to anything that takes a `URLSession` —
/// ``Mero``, ``RelayClient``, ``URLSessionHttpClient``, the SSE clients —
/// and every request they make is sealed. A request for a URL outside the
/// base URL is refused rather than sent in the clear: falling back to
/// plaintext would silently undo the point of sealing.
///
/// One session is opened for many requests, concurrent ones included, and
/// renewed a minute before the node would drop it. A session the node has
/// forgotten (`unknown_session`) is reopened and the request sent again — the
/// node refused it before running anything, so that is safe.
public actor SealedTransport {
    /// Where a session is opened, relative to the node's base URL. (== mero-js `HANDSHAKE_PATH`.)
    public static let handshakePath = "/sealed/handshake"
    /// Where nodes released before ``handshakePath`` existed open a session (newer
    /// nodes still serve it as a deprecated alias). The handshake is tried here only
    /// when the node answers 404, 405 or 403 `sealed_required` at ``handshakePath``.
    /// (== mero-js `LEGACY_HANDSHAKE_PATH`.)
    public static let legacyHandshakePath = "/sealed/v2/handshake"
    /// Where sealed requests are posted. (== mero-js `SEALED_PATH`.)
    public static let sealedPath = "/sealed/v2"
    /// Content type of every sealed body. (== mero-js `SEALED_CONTENT_TYPE`.)
    public static let contentType = "application/vnd.calimero.sealed"

    static let version: UInt8 = 2
    static let prologue = Data("calimero/sealed-http/v2".utf8)
    static let sessionIdLength = 16
    static let headerLength = 1 + sessionIdLength + 8
    static let frameHead: UInt8 = 0
    static let frameData: UInt8 = 1
    static let frameEnd: UInt8 = 2
    /// The largest frame the node sends: a kind byte, at most 64 KiB of data, a tag.
    static let maxFrameLength = 1 + 64 * 1024 + 16
    /// Far more than a handshake reply (69 bytes) ever is.
    static let maxHandshakeReply = 1024
    /// Open a new session this long before the node would drop the current one.
    static let renewBefore: TimeInterval = 60
    static let nullBodyStatuses: Set<Int> = [101, 204, 205, 304]

    /// The node's base URL.
    public nonisolated let baseURL: URL
    nonisolated let origin: String
    nonisolated let basePath: String
    nonisolated let nodeURL: String
    let carrier: URLSession
    private let attest: (@Sendable () async throws -> Data)?
    private var transportKey: Task<Data, Error>
    private var sessionTask: Task<SealedSession, Error>?
    private var sessionGeneration: Int?
    private var generations = 0
    /// Go plain for a route the TD answers `403 sealed_route_unguarded`.
    private let unguardedFallback: Bool
    private var unguarded = false
    private let sleep: @Sendable (TimeInterval) async -> Void

    /// Seal to `baseURL` with a transport key from `key`; envelopes travel
    /// over `session`. Attesting (`.attest`) starts now; a failure surfaces
    /// on the request that needed the key.
    public init(baseURL: URL, key: TransportKeySource, session: URLSession = .shared) throws {
        try self.init(baseURL: baseURL, key: key, session: session, unguardedFallback: false)
    }

    init(
        baseURL: URL, key: TransportKeySource, session: URLSession, unguardedFallback: Bool,
        sleep: @escaping @Sendable (TimeInterval) async -> Void = { seconds in
            try? await Task.sleep(nanoseconds: UInt64(max(0, seconds) * 1_000_000_000))
        }
    ) throws {
        guard let split = SealedURLParts(baseURL) else {
            throw TeeTrustError("\(baseURL.absoluteString) is not an http(s) URL to seal to")
        }
        var basePath = split.path
        while basePath.hasSuffix("/") { basePath.removeLast() }
        self.baseURL = baseURL
        self.origin = split.origin
        self.basePath = basePath
        self.nodeURL = split.origin + basePath
        self.carrier = session
        self.unguardedFallback = unguardedFallback
        self.sleep = sleep
        switch key {
        case .key(let bytes):
            attest = nil
            transportKey = Task { bytes }
        case .attest(let attest):
            self.attest = attest
            transportKey = Task { try await attest() }
        }
    }

    /// Whether `url` is inside this node's base URL, and so may be sealed to it.
    public nonisolated func covers(_ url: URL) -> Bool {
        guard let split = SealedURLParts(url) else { return false }
        return split.origin == origin && split.path.hasPrefix(basePath + "/")
    }

    // MARK: - Sending

    /// Seal `request` to the node and open its answer as it streams.
    public func send(_ request: URLRequest) async throws -> SealedResponse {
        guard let url = request.url, covers(url), let split = SealedURLParts(url) else {
            let named = request.url?.absoluteString ?? "a request with no URL"
            throw TeeTrustError("Refusing to send \(named) outside the sealed node at \(baseURL.absoluteString)")
        }
        if unguardedFallback, unguarded, Self.needsSession(request, path: split.path) {
            return try await Self.plain(carrier, request)
        }
        let head = SealedRequestHead(
            method: request.httpMethod ?? "GET",
            path: String(split.path.dropFirst(basePath.count)) + split.query,
            headers: Self.headers(of: request))
        let response = try await sealedSend(head, body: Self.body(of: request))
        guard unguardedFallback, response.status == 403 else { return response }
        // A refusal is small: read it to see whether the TD cannot guard this route sealed.
        let refusal = try await response.data()
        let code = (try? MeroJSON.decode(JSONValue.self, from: refusal))?["error"]?["code"]?.stringValue
        guard code == "sealed_route_unguarded" else {
            return SealedResponse(status: response.status, headers: response.headers, body: Self.single(refusal))
        }
        unguarded = true
        return try await Self.plain(carrier, request)
    }

    /// Seal `request` and return its whole answer, as `URLSession.data(for:)` would.
    public func data(for request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        let response = try await send(request)
        let body = try await response.data()
        guard let url = request.url,
            let http = HTTPURLResponse(
                url: url, statusCode: response.status, httpVersion: "HTTP/1.1", headerFields: response.headers)
        else { throw TeeTrustError("The sealed response could not be read") }
        return (body, http)
    }

    /// A `URLSession` whose every request is sealed through this transport
    /// (``SealedURLProtocol``). `configuration`'s other settings are kept.
    public nonisolated func urlSession(configuration: URLSessionConfiguration = .ephemeral) -> URLSession {
        SealedURLProtocol.register(self)
        let config = (configuration.copy() as? URLSessionConfiguration) ?? configuration
        config.protocolClasses = [SealedURLProtocol.self] + (config.protocolClasses ?? [])
        return URLSession(configuration: config)
    }

    private func sealedSend(_ head: SealedRequestHead, body: Data) async throws -> SealedResponse {
        let (open, generation) = try await currentSession()
        do {
            return try await Self.exchange(carrier, nodeURL: nodeURL, session: open, head: head, body: body)
        } catch let error as SealedTransportError where error.code == "unknown_session" {
            // The node refused the envelope before running anything in it,
            // so the request can be sent again safely under a new session.
            let renewed = try await awaitSession(renew(stale: generation))
            return try await Self.exchange(carrier, nodeURL: nodeURL, session: renewed.0, head: head, body: body)
        }
    }

    // MARK: - Sessions

    /// The session to use now, and its generation, to name it if it turns out stale.
    private func currentSession() async throws -> (SealedSession, Int) {
        let current: (Task<SealedSession, Error>, Int)
        if let task = sessionTask, let generation = sessionGeneration {
            current = (task, generation)
        } else {
            current = renew(stale: nil)
        }
        let open = try await awaitSession(current)
        if Date() < open.0.renewAt { return open }
        return try await awaitSession(renew(stale: open.1))
    }

    /// Replace the session only if it is still the one found stale, so
    /// concurrent requests that all find it stale open one new session between them.
    private func renew(stale: Int?) -> (Task<SealedSession, Error>, Int) {
        if let task = sessionTask, let generation = sessionGeneration, generation != stale {
            return (task, generation)
        }
        generations += 1
        let generation = generations
        let task = Task { try await self.openSession() }
        sessionTask = task
        sessionGeneration = generation
        return (task, generation)
    }

    private func awaitSession(_ pending: (Task<SealedSession, Error>, Int)) async throws -> (SealedSession, Int) {
        do {
            return (try await pending.0.value, pending.1)
        } catch {
            if sessionGeneration == pending.1 {
                sessionTask = nil
                sessionGeneration = nil
            }
            throw error
        }
    }

    private func openSession() async throws -> SealedSession {
        do {
            return try await Self.handshake(carrier, nodeURL: nodeURL, key: try await checkedKey(), sleep: sleep)
        } catch is StaleTransportKeyError {
            guard let attest else { throw StaleTransportKeyError() }
            transportKey = Task { try await attest() }
            return try await Self.handshake(carrier, nodeURL: nodeURL, key: try await checkedKey(), sleep: sleep)
        }
    }

    private func checkedKey() async throws -> Data {
        let key = try await transportKey.value
        guard key.count == 32 else { throw TeeTrustError("transportPublicKey must be 32 bytes") }
        return key
    }

    // MARK: - The wire

    /// The node limits how fast sessions open. A refused handshake opened
    /// nothing, so trying once more after the pause it asks for is safe.
    static func handshake(
        _ carrier: URLSession, nodeURL: String, key: Data, sleep: @Sendable (TimeInterval) async -> Void
    ) async throws -> SealedSession {
        do {
            return try await handshakeOnce(carrier, nodeURL: nodeURL, key: key)
        } catch let error as SealedTransportError where error.code == "busy" {
            await sleep(error.retryAfter ?? 1)
            return try await handshakeOnce(carrier, nodeURL: nodeURL, key: key)
        }
    }

    static func handshakeOnce(_ carrier: URLSession, nodeURL: String, key: Data) async throws -> SealedSession {
        let noise = try NoiseInitiator(remoteStatic: key, prologue: prologue)
        let (data, http) = try await postHandshake(
            carrier, nodeURL: nodeURL, body: Data([version]) + key + noise.message1)
        guard http?.value(forHTTPHeaderField: "Content-Type") == contentType else {
            let retryAfter = http?.value(forHTTPHeaderField: "Retry-After")
            throw refusal(status: http?.statusCode ?? 0, body: data, retryAfter: retryAfter)
        }
        guard data.count <= maxHandshakeReply else {
            throw TeeTrustError("The handshake reply is malformed: it is too long")
        }
        return try sessionFrom(noise, reply: data)
    }

    /// Post handshake message 1 to ``handshakePath``; a node from before the path
    /// gets the same message once at ``legacyHandshakePath``. Such a node routes
    /// nothing there (404, 405) or, sealing everything as Cloud nodes do, refuses it
    /// as an unsealed request (403 `sealed_required`). That node never read the
    /// first message, so sending it again opens nothing twice. Any other answer is
    /// returned as it is.
    /// (== mero-js `postHandshake`.)
    static func postHandshake(
        _ carrier: URLSession, nodeURL: String, body: Data
    ) async throws -> (Data, HTTPURLResponse?) {
        func post(_ path: String) async throws -> (Data, HTTPURLResponse?) {
            let (data, response) = try await carrier.data(for: try Self.envelopeRequest(nodeURL + path, body: body))
            return (data, response as? HTTPURLResponse)
        }
        let first = try await post(handshakePath)
        guard predatesHandshakePath(status: first.1?.statusCode, body: first.0) else { return first }
        return try await post(legacyHandshakePath)
    }

    /// Whether a handshake answer says the node predates ``handshakePath`` (see `postHandshake`).
    static func predatesHandshakePath(status: Int?, body: Data) -> Bool {
        switch status {
        case 404, 405: return true
        case 403:
            let json = try? MeroJSON.decode(JSONValue.self, from: body)
            return json?["error"]?["code"]?.stringValue == "sealed_required"
        default: return false
        }
    }

    /// Finish the handshake with the node's reply. (== mero-js `sessionFrom`, for the published vectors.)
    static func sessionFrom(_ noise: NoiseInitiator, reply: Data, now: Date = Date()) throws -> SealedSession {
        let reply = Data(reply)
        guard reply.first == version else { throw TeeTrustError("The handshake reply is malformed") }
        let finished: (payload: Data, split: (Data, Data))
        do {
            finished = try noise.finish(Data(reply.dropFirst()))
        } catch {
            throw TeeTrustError("The handshake reply did not open: it did not come from the attested node")
        }
        let payload = finished.payload
        guard payload.count == sessionIdLength + 4 else { throw TeeTrustError("The handshake reply is malformed") }
        let lifetime = BigEndian.readU32(payload, at: sessionIdLength)
        return SealedSession(
            requestKey: SymmetricKey(data: finished.split.0), responseKey: SymmetricKey(data: finished.split.1),
            sessionId: Data(payload.prefix(sessionIdLength)),
            renewAt: now.addingTimeInterval(TimeInterval(lifetime) - renewBefore))
    }

    /// Seal one request under the session's next request id. (== mero-js `sealRequest`.)
    static func sealRequest(
        _ session: SealedSession, head: SealedRequestHead, body: Data
    ) throws -> SealedFrameEnvelope {
        let requestId = session.nextRequestId()
        let header = session.header(requestId)
        let sealed = try AES.GCM.seal(
            head.inner(body), using: session.requestKey, nonce: try nonce(requestId, 0), authenticating: header)
        return SealedFrameEnvelope(
            requestId: requestId, header: header, envelope: header + sealed.ciphertext + sealed.tag)
    }

    static func exchange(
        _ carrier: URLSession, nodeURL: String, session: SealedSession, head: SealedRequestHead, body: Data
    ) async throws -> SealedResponse {
        let sealed = try sealRequest(session, head: head, body: body)
        let request = try envelopeRequest(nodeURL + sealedPath, body: sealed.envelope)
        let opener = SealedFrameOpener(key: session.responseKey, header: sealed.header, requestId: sealed.requestId)
        return try await streamed { emitHead, continuation in
            let (bytes, response) = try await carrier.bytes(for: request)
            let http = response as? HTTPURLResponse
            guard http?.value(forHTTPHeaderField: "Content-Type") == SealedTransport.contentType else {
                let data = try await SealedTransport.collect(bytes, limit: 64 * 1024)
                let retryAfter = http?.value(forHTTPHeaderField: "Retry-After")
                throw SealedTransport.refusal(status: http?.statusCode ?? 0, body: data, retryAfter: retryAfter)
            }
            let frames = SealedFrameReader(bytes.makeAsyncIterator())
            try await SealedTransport.openResponse(frames, opener: opener, emitHead, continuation)
        }
    }

    /// Open a sealed response: its head first, then each body frame as it
    /// arrives, until the end frame. (== mero-js `openResponse`.)
    static func openResponse<Iterator: AsyncIteratorProtocol>(
        _ frames: SealedFrameReader<Iterator>, opener: SealedFrameOpener,
        _ emitHead: @Sendable (Int, [String: String]) -> Void,
        _ continuation: AsyncThrowingStream<Data, Error>.Continuation
    ) async throws where Iterator.Element == UInt8 {
        var frames = frames
        var opener = opener
        guard let first = try await frames.next() else { throw TeeTrustError("The sealed response is malformed") }
        let head = try opener.open(first)
        guard head.kind == frameHead, let json = try? MeroJSON.decode(JSONValue.self, from: head.data),
            let status = json["status"]?.intValue
        else { throw TeeTrustError("The sealed response is malformed") }
        var headers: [String: String] = [:]
        for pair in json["headers"]?.arrayValue ?? [] {
            guard let entry = pair.arrayValue, entry.count == 2, let name = entry[0].stringValue,
                let value = entry[1].stringValue
            else { continue }
            let key = name.lowercased()
            headers[key] = headers[key].map { "\($0), \(value)" } ?? value
        }
        emitHead(status, headers)
        if nullBodyStatuses.contains(status) {
            continuation.finish()
            return
        }
        while true {
            guard let sealed = try await frames.next() else {
                throw TeeTrustError("The sealed response was cut short")
            }
            let frame = try opener.open(sealed)
            switch frame.kind {
            case frameData: continuation.yield(frame.data)
            case frameEnd:
                continuation.finish()
                return
            default: throw TeeTrustError("The sealed response is malformed")
            }
        }
    }

    /// `request_id ‖ index`, as the node numbers them.
    static func nonce(_ requestId: UInt64, _ index: UInt32) throws -> AES.GCM.Nonce {
        try AES.GCM.Nonce(data: BigEndian.u64(requestId) + BigEndian.u32(index))
    }

    static func envelopeRequest(_ url: String, body: Data) throws -> URLRequest {
        guard let target = URL(string: url) else { throw TeeTrustError("bad sealed URL \(url)") }
        var request = URLRequest(url: target)
        request.httpMethod = "POST"
        request.setValue(contentType, forHTTPHeaderField: "Content-Type")
        request.httpBody = body
        return request
    }

    /// The node's JSON refusal, as the error it names. (== mero-js `envelopeRefusal`.)
    static func refusal(status: Int, body: Data, retryAfter: String?) -> Error {
        var code: String?
        var message = "Sealed request refused: HTTP \(status)"
        // Not the node's JSON refusal — a proxy's page, most likely — leaves
        // the status as all that can honestly be reported.
        if let json = try? MeroJSON.decode(JSONValue.self, from: body) {
            code = json["error"]?["code"]?.stringValue
            if let text = json["error"]?["message"]?.stringValue { message = "Sealed request refused: \(text)" }
        }
        if code == "stale_transport_key" { return StaleTransportKeyError() }
        let seconds = retryAfter.flatMap { Double($0.trimmingCharacters(in: .whitespaces)) }
        let wait = seconds.flatMap { $0.isFinite && $0 > 0 ? min($0, 5) : nil }
        return SealedTransportError(status: status, code: code, message: message, retryAfter: wait)
    }

    // MARK: - Plain requests and streams

    /// Send `request` as it is, streaming its answer (the unguarded-route fallback).
    static func plain(_ carrier: URLSession, _ request: URLRequest) async throws -> SealedResponse {
        try await streamed { emitHead, continuation in
            let (bytes, response) = try await carrier.bytes(for: request)
            guard let http = response as? HTTPURLResponse else { throw TeeTrustError("Non-HTTP response") }
            var headers: [String: String] = [:]
            for (key, value) in http.allHeaderFields {
                if let k = key as? String, let v = value as? String { headers[k.lowercased()] = v }
            }
            emitHead(http.statusCode, headers)
            // Hand lines on as they come (an event stream), and long runs in pieces.
            var chunk = Data()
            for try await byte in bytes {
                chunk.append(byte)
                if byte == 0x0a || chunk.count >= 16 * 1024 {
                    continuation.yield(chunk)
                    chunk = Data()
                }
            }
            if !chunk.isEmpty { continuation.yield(chunk) }
            continuation.finish()
        }
    }

    /// Run `work` in its own task: it emits the head once, then feeds the
    /// body. Returns when the head is known; a failure before it is thrown
    /// here, one after it ends the body.
    static func streamed(
        _ work:
            @escaping @Sendable (
                _ emitHead: @escaping @Sendable (Int, [String: String]) -> Void,
                _ body: AsyncThrowingStream<Data, Error>.Continuation
            ) async throws -> Void
    ) async throws -> SealedResponse {
        var captured: AsyncThrowingStream<Data, Error>.Continuation?
        let body = AsyncThrowingStream<Data, Error> { captured = $0 }
        guard let continuation = captured else { throw TeeTrustError("The sealed response could not be read") }
        let gate = SealedHeadGate()
        let head = try await withTaskCancellationHandler(
            operation: {
                try await withCheckedThrowingContinuation { (waiting: CheckedContinuation<SealedHead, Error>) in
                    gate.wait(waiting)
                    let task = Task {
                        do {
                            let emitHead: @Sendable (Int, [String: String]) -> Void = { status, headers in
                                gate.resume(.success(SealedHead(status: status, headers: headers)))
                            }
                            try await work(emitHead, continuation)
                            gate.resume(.failure(TeeTrustError("The sealed response is malformed")))
                        } catch {
                            gate.resume(.failure(error))
                            continuation.finish(throwing: error)
                        }
                    }
                    gate.track(task)
                    continuation.onTermination = { _ in task.cancel() }
                }
            },
            onCancel: { gate.cancel() })
        return SealedResponse(status: head.status, headers: head.headers, body: body)
    }

    static func single(_ data: Data) -> AsyncThrowingStream<Data, Error> {
        AsyncThrowingStream { continuation in
            if !data.isEmpty { continuation.yield(data) }
            continuation.finish()
        }
    }

    static func collect<S: AsyncSequence>(_ bytes: S, limit: Int) async throws -> Data where S.Element == UInt8 {
        var out = Data()
        for try await byte in bytes {
            out.append(byte)
            if out.count >= limit { break }
        }
        return out
    }

    // MARK: - Requests

    /// The request's headers, names lowercased and sorted, as `fetch` lists them.
    static func headers(of request: URLRequest) -> [(String, String)] {
        (request.allHTTPHeaderFields ?? [:]).map { ($0.key.lowercased(), $0.value) }.sorted { $0.0 < $1.0 }
    }

    /// The request's body, read from its stream when `URLProtocol` handed it over as one.
    static func body(of request: URLRequest) -> Data {
        if let body = request.httpBody { return body }
        guard let stream = request.httpBodyStream else { return Data() }
        stream.open()
        defer { stream.close() }
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 16 * 1024)
        while true {
            let read = stream.read(&buffer, maxLength: buffer.count)
            guard read > 0 else { break }
            data.append(buffer, count: read)
        }
        return data
    }

    /// Whether `request` needs a session the relay's proxy checks: a token, or a login.
    static func needsSession(_ request: URLRequest, path: String) -> Bool {
        request.value(forHTTPHeaderField: "Authorization") != nil || path.hasPrefix("/auth/")
    }
}

/// A URL as `fetch` compares it: scheme, host and non-default port; the
/// percent-encoded path; `?query`, or "" for none.
struct SealedURLParts {
    let origin: String
    let path: String
    let query: String

    init?(_ url: URL) {
        guard let parts = URLComponents(url: url, resolvingAgainstBaseURL: true),
            let scheme = parts.scheme?.lowercased(), scheme == "http" || scheme == "https",
            let host = parts.host?.lowercased(), !host.isEmpty
        else { return nil }
        let defaultPort = scheme == "https" ? 443 : 80
        let port = parts.port.flatMap { $0 == defaultPort ? nil : ":\($0)" } ?? ""
        let hostPart = host.contains(":") && !host.hasPrefix("[") ? "[\(host)]" : host
        origin = "\(scheme)://\(hostPart)\(port)"
        path = parts.percentEncodedPath.isEmpty ? "/" : parts.percentEncodedPath
        query = parts.percentEncodedQuery.flatMap { $0.isEmpty ? nil : "?\($0)" } ?? ""
    }
}

/// One sealed request: its id, its header (the AAD of every frame), and the bytes posted.
struct SealedFrameEnvelope {
    let requestId: UInt64
    let header: Data
    let envelope: Data
}

/// An open sealed session: its keys, id and next request id.
final class SealedSession: @unchecked Sendable {
    let requestKey: SymmetricKey
    let responseKey: SymmetricKey
    let sessionId: Data
    let renewAt: Date
    private let lock = NSLock()
    private var next: UInt64 = 1

    init(requestKey: SymmetricKey, responseKey: SymmetricKey, sessionId: Data, renewAt: Date) {
        self.requestKey = requestKey
        self.responseKey = responseKey
        self.sessionId = sessionId
        self.renewAt = renewAt
    }

    /// From 1, each used once.
    func nextRequestId() -> UInt64 {
        lock.lock()
        defer { lock.unlock() }
        let id = next
        next += 1
        return id
    }

    /// `0x02 ‖ session_id ‖ request_id (u64 BE)`.
    func header(_ requestId: UInt64) -> Data {
        Data([SealedTransport.version]) + sessionId + BigEndian.u64(requestId)
    }
}

/// The head of an inner request. (== mero-js `RequestHead`.)
struct SealedRequestHead: Sendable {
    let method: String
    let path: String
    let headers: [(String, String)]

    /// `{"method":…,"path":…,"headers":[[name,value],…]}`, as `JSON.stringify` writes it.
    var json: String {
        let pairs = headers.map { "[\(CanonicalJSON.quote($0.0)),\(CanonicalJSON.quote($0.1))]" }
        return "{\"method\":\(CanonicalJSON.quote(method)),\"path\":\(CanonicalJSON.quote(path)),"
            + "\"headers\":[\(pairs.joined(separator: ","))]}"
    }

    /// `u32 BE head length ‖ head ‖ body`.
    func inner(_ body: Data) -> Data {
        let head = Data(json.utf8)
        return BigEndian.u32(UInt32(head.count)) + head + body
    }
}

/// Splits a byte stream into length-prefixed frames, refusing one larger than
/// the node sends before it is buffered.
struct SealedFrameReader<Iterator: AsyncIteratorProtocol> where Iterator.Element == UInt8 {
    var iterator: Iterator

    init(_ iterator: Iterator) { self.iterator = iterator }

    /// The next frame, or `nil` when the stream ended first.
    mutating func next() async throws -> Data? {
        guard let length = try await read(4) else { return nil }
        let size = BigEndian.readU32(length)
        guard size <= SealedTransport.maxFrameLength else {
            throw TeeTrustError("The sealed response is malformed: a frame is larger than the node sends")
        }
        return try await read(Int(size))
    }

    private mutating func read(_ count: Int) async throws -> Data? {
        var out = Data(capacity: count)
        while out.count < count {
            guard let byte = try await iterator.next() else { return nil }
            out.append(byte)
        }
        return out
    }
}

/// Opens response frames in order: frame `i` under nonce `request_id ‖ i`.
struct SealedFrameOpener: Sendable {
    let key: SymmetricKey
    let header: Data
    let requestId: UInt64
    private var index: UInt32 = 0

    init(key: SymmetricKey, header: Data, requestId: UInt64) {
        self.key = key
        self.header = header
        self.requestId = requestId
    }

    mutating func open(_ sealed: Data) throws -> (kind: UInt8, data: Data) {
        let refusal = TeeTrustError("A sealed response frame did not open: it was not sealed by the attested node")
        let frame = Data(sealed)
        guard frame.count >= 16 else { throw refusal }
        let plain: Data
        do {
            let box = try AES.GCM.SealedBox(
                nonce: try SealedTransport.nonce(requestId, index), ciphertext: Data(frame.prefix(frame.count - 16)),
                tag: Data(frame.suffix(16)))
            plain = try AES.GCM.open(box, using: key, authenticating: header)
        } catch {
            throw refusal
        }
        index += 1
        guard let kind = plain.first else { throw TeeTrustError("The sealed response is malformed") }
        return (kind: kind, data: Data(plain.dropFirst()))
    }
}

struct SealedHead: Sendable {
    let status: Int
    let headers: [String: String]
}

/// The one-shot hand-over of a response head from the task reading it.
final class SealedHeadGate: @unchecked Sendable {
    private let lock = NSLock()
    private var waiting: CheckedContinuation<SealedHead, Error>?
    private var task: Task<Void, Never>?
    private var cancelled = false

    func wait(_ continuation: CheckedContinuation<SealedHead, Error>) {
        lock.lock()
        waiting = continuation
        lock.unlock()
    }

    func track(_ task: Task<Void, Never>) {
        lock.lock()
        self.task = task
        let cancel = cancelled
        lock.unlock()
        if cancel { task.cancel() }
    }

    func resume(_ result: Result<SealedHead, Error>) {
        lock.lock()
        let continuation = waiting
        waiting = nil
        lock.unlock()
        continuation?.resume(with: result)
    }

    func cancel() {
        lock.lock()
        cancelled = true
        let task = self.task
        lock.unlock()
        task?.cancel()
        resume(.failure(CancellationError()))
    }
}

/// What the attest endpoint puts in report data bytes `32..<64` when asked
/// to bind its transport key: `SHA-256(DOMAIN ‖ inner ‖ key)`, `inner` the
/// expected application hash or 32 zero bytes. Mirrors core's
/// `attest_transport_binding`. (== mero-js `transportKeyBinding`.)
public func transportKeyBinding(_ inner: Data, _ transportKey: Data) -> Data {
    sha256(Data("calimero.tee-attest.transport-key.v1".utf8) + inner + transportKey)
}
