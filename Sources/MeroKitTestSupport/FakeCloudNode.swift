import CryptoKit
import Foundation
import MeroKit

#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

/// One request a ``FakeCloudNode`` opened from its sealed envelope.
public struct FakeCloudRequest: Sendable {
    public let method: String
    /// The path and query, e.g. `/apps/a1/intents`.
    public let path: String
    /// Header names lowercased.
    public let headers: [String: String]
    public let body: Data

    /// The body as JSON, or `nil`.
    public var json: JSONValue? { try? JSONDecoder().decode(JSONValue.self, from: body) }
    /// The bearer token, without `Bearer `.
    public var bearer: String? {
        headers["authorization"].map { $0.hasPrefix("Bearer ") ? String($0.dropFirst(7)) : $0 }
    }
}

/// What a ``FakeCloudNode`` answers: each chunk its own sealed frame.
public struct FakeCloudAnswer: Sendable {
    public var status: Int
    public var headers: [(String, String)]
    public var chunks: [Data]
    /// Send no end frame and keep the response open (a live event stream).
    public var holdOpen: Bool

    public init(status: Int, headers: [(String, String)] = [], chunks: [Data], holdOpen: Bool = false) {
        self.status = status
        self.headers = headers
        self.chunks = chunks
        self.holdOpen = holdOpen
    }

    /// `200` with a JSON body.
    public static func json(_ body: JSONValue, status: Int = 200) -> FakeCloudAnswer {
        FakeCloudAnswer(
            status: status, headers: [("content-type", "application/json")],
            chunks: [Data(String(decoding: try! JSONEncoder().encode(body), as: UTF8.self).utf8)])
    }

    /// The node's refusal: `{ error, message, details }`.
    public static func error(
        _ status: Int, _ code: String, message: String = "", details: JSONValue? = nil
    )
        -> FakeCloudAnswer
    {
        var body: [String: JSONValue] = [
            "status": .number(Double(status)), "error": .string(code), "message": .string(message),
        ]
        if let details { body["details"] = details }
        return .json(.object(body), status: status)
    }

    /// A server-sent event stream of `events`, held open when `holdOpen`.
    public static func events(_ events: [FakeCloudEvent], holdOpen: Bool = false) -> FakeCloudAnswer {
        let chunks = events.map { event in
            let data = String(decoding: try! JSONEncoder().encode(event.data), as: UTF8.self)
            return Data("id: \(event.id)\nevent: \(event.kind)\ndata: \(data)\n\n".utf8)
        }
        return FakeCloudAnswer(
            status: 200, headers: [("content-type", "text/event-stream")], chunks: chunks, holdOpen: holdOpen)
    }
}

/// One server-sent event a ``FakeCloudNode`` streams.
public struct FakeCloudEvent: Sendable {
    public let id: String
    public let kind: String
    public let data: JSONValue

    public init(id: String, kind: String, data: JSONValue) {
        self.id = id
        self.kind = kind
        self.data = data
    }
}

/// A stand-in Calimero 1.0 node for tests: it answers `GET /attestation` with
/// a mock quote (trust it with `CloudPolicy(allowMock: true)`), runs the
/// Noise NK handshake of `calimero/sealed-http/v2` with its transport key, and
/// opens each sealed request and seals its answer frame by frame, as core's
/// `calimero-sealed` does. What a request answers is up to `handler`.
///
/// ```swift
/// let node = FakeCloudNode { request in
///     request.path == "/auth/me" ? .json(["accountId": "acc"]) : .error(404, "NOT_FOUND")
/// }
/// node.install()   // MockURLProtocol's handler; use MockURLProtocol.makeSession()
/// ```
public final class FakeCloudNode: @unchecked Sendable {
    public let baseURL: String
    private let lock = NSLock()
    private var handler: @Sendable (FakeCloudRequest) -> FakeCloudAnswer
    private var secret = Curve25519.KeyAgreement.PrivateKey()
    private var sessions: [Data: (request: SymmetricKey, response: SymmetricKey)] = [:]
    private var _requests: [FakeCloudRequest] = []
    private var _handshakes = 0
    private var _attests = 0

    public init(
        baseURL: String = "https://node.test", handler: @escaping @Sendable (FakeCloudRequest) -> FakeCloudAnswer
    ) {
        var base = baseURL
        while base.hasSuffix("/") { base.removeLast() }
        self.baseURL = base
        self.handler = handler
    }

    /// Every request opened so far, in order.
    public var requests: [FakeCloudRequest] { locked { _requests } }
    public var handshakes: Int { locked { _handshakes } }
    public var attests: Int { locked { _attests } }

    public func setHandler(_ handler: @escaping @Sendable (FakeCloudRequest) -> FakeCloudAnswer) {
        locked { self.handler = handler }
    }

    /// A new transport key, every session forgotten: what a restart looks like.
    public func restart() {
        locked {
            secret = Curve25519.KeyAgreement.PrivateKey()
            sessions.removeAll()
        }
    }

    /// Route every ``MockURLProtocol`` request to this node.
    public func install() {
        Self.install([self])
    }

    /// Route ``MockURLProtocol`` requests to whichever of `nodes` their URL names.
    /// Any other request goes to `otherwise` (a node directory, say), or is a 404.
    public static func install(
        _ nodes: [FakeCloudNode], otherwise: (@Sendable (URLRequest) -> MockURLProtocol.Stub)? = nil
    ) {
        MockURLProtocol.setHandler { request in
            let url = request.url?.absoluteString ?? ""
            guard let node = nodes.first(where: { url.hasPrefix($0.baseURL + "/") }) else {
                return otherwise?(request) ?? .init(status: 404, headers: [:], body: Data())
            }
            return node.handle(request)
        }
    }

    /// Answer one plain HTTP request as the node.
    public func handle(_ request: URLRequest) -> MockURLProtocol.Stub {
        let path = String((request.url?.absoluteString ?? "").dropFirst(baseURL.count))
        let body = Self.body(request)
        do {
            if path.hasPrefix("/attestation") { return attest(request) }
            // A current node serves the handshake at both paths (v2 is the deprecated alias).
            if path == "/sealed/handshake" || path == "/sealed/v2/handshake" { return try handshake(body) }
            if path == "/sealed/v2" { return try exchange(body) }
        } catch {
            return Self.refusal(400, "malformed")
        }
        return .init(status: 404, headers: [:], body: Data())
    }

    static func refusal(_ status: Int, _ code: String) -> MockURLProtocol.Stub {
        .init(
            status: status, headers: ["Content-Type": "application/json"],
            body: Data(#"{"error":{"code":"\#(code)","message":"\#(code)"}}"#.utf8))
    }

    private func attest(_ request: URLRequest) -> MockURLProtocol.Stub {
        let items = request.url.flatMap { URLComponents(url: $0, resolvingAgainstBaseURL: false)?.queryItems } ?? []
        let nonce = items.first { $0.name == "nonce" }?.value ?? ""
        let appHash = items.first { $0.name == "appHash" }?.value
        let key = locked { secret.publicKey.rawRepresentation }
        let inner = appHash.flatMap { Self.hex($0) } ?? Data(count: 32)
        let binding = Data(SHA256.hash(data: Data("calimero.tee-attest.transport-key.v1".utf8) + inner + key))
        let quote = Data(mockQuoteHeader.utf8) + (Self.hex(nonce) ?? Data(count: 32)) + binding
        locked { _attests += 1 }
        let doc: JSONValue = [
            "quote": .string(quote.base64EncodedString()), "transportKey": .string(Hex.encode(key)),
            "appHash": .string(appHash ?? String(repeating: "0", count: 64)), "nonce": .string(nonce),
            "release": ["version": "0.0.0", "profile": "mock"], "cluster": "fake", "node": 1, "mock": true,
        ]
        return .init(
            status: 200, headers: ["Content-Type": "application/json"], body: try! JSONEncoder().encode(doc))
    }

    // MARK: - Noise NK, the responder's side

    private func handshake(_ body: Data) throws -> MockURLProtocol.Stub {
        let bytes = [UInt8](body)
        let secret = locked { self.secret }
        guard bytes.count > 33, bytes[0] == 2, Data(bytes[1..<33]) == secret.publicKey.rawRepresentation else {
            return Self.refusal(409, "stale_transport_key")
        }
        let message1 = Data(bytes[33...])
        var state = Symmetric()
        state.mixHash(Data("calimero/sealed-http/v2".utf8))
        state.mixHash(secret.publicKey.rawRepresentation)
        let remote = Data(message1.prefix(32))
        state.mixHash(remote)
        state.mixKey(try Self.dh(secret, remote))
        _ = try state.decryptAndHash(Data(message1.dropFirst(32)))
        let ephemeral = Curve25519.KeyAgreement.PrivateKey()
        state.mixHash(ephemeral.publicKey.rawRepresentation)
        state.mixKey(try Self.dh(ephemeral, remote))
        var sessionId = Data(count: 16)
        for i in 0..<16 { sessionId[i] = UInt8.random(in: 0...255) }
        let sealed = try state.encryptAndHash(sessionId + Data([0, 0, 0x0e, 0x10]))
        let (requestKey, responseKey) = state.split()
        locked {
            _handshakes += 1
            sessions[sessionId] = (SymmetricKey(data: requestKey), SymmetricKey(data: responseKey))
        }
        return .init(
            status: 200, headers: ["Content-Type": "application/vnd.calimero.sealed"],
            body: Data([2]) + ephemeral.publicKey.rawRepresentation + sealed)
    }

    private func exchange(_ envelope: Data) throws -> MockURLProtocol.Stub {
        let bytes = [UInt8](envelope)
        guard bytes.count > 25 else { return Self.refusal(400, "malformed") }
        let header = Data(bytes.prefix(25))
        guard let keys = locked({ sessions[Data(bytes[1..<17])] }) else { return Self.refusal(409, "unknown_session") }
        let requestId = Data(bytes[17..<25])
        func nonce(_ index: UInt32) throws -> AES.GCM.Nonce {
            try AES.GCM.Nonce(data: requestId + Self.u32(index))
        }
        let sealed = Data(bytes[25...])
        let box = try AES.GCM.SealedBox(
            nonce: try nonce(0), ciphertext: Data(sealed.prefix(sealed.count - 16)), tag: Data(sealed.suffix(16)))
        let inner = try AES.GCM.open(box, using: keys.request, authenticating: header)
        let length = Int(Self.readU32(inner))
        let head = try JSONDecoder().decode(JSONValue.self, from: Data(inner.dropFirst(4).prefix(length)))
        var headers: [String: String] = [:]
        for pair in head["headers"]?.arrayValue ?? [] {
            if let entry = pair.arrayValue, entry.count == 2, let name = entry[0].stringValue,
                let value = entry[1].stringValue
            {
                headers[name.lowercased()] = value
            }
        }
        let seen = FakeCloudRequest(
            method: head["method"]?.stringValue ?? "", path: head["path"]?.stringValue ?? "", headers: headers,
            body: Data(inner.dropFirst(4 + length)))
        let answer = locked { () -> (@Sendable (FakeCloudRequest) -> FakeCloudAnswer) in
            _requests.append(seen)
            return handler
        }(seen)

        var index: UInt32 = 0
        var out = Data()
        func frame(_ kind: UInt8, _ data: Data) throws {
            let box = try AES.GCM.seal(
                Data([kind]) + data, using: keys.response, nonce: try nonce(index), authenticating: header)
            index += 1
            let framed = box.ciphertext + box.tag
            out.append(Self.u32(UInt32(framed.count)) + framed)
        }
        let replyHead: JSONValue = [
            "status": .number(Double(answer.status)),
            "headers": .array(answer.headers.map { .array([.string($0.0), .string($0.1)]) }),
        ]
        try frame(0, try JSONEncoder().encode(replyHead))
        for chunk in answer.chunks { try frame(1, chunk) }
        if !answer.holdOpen { try frame(2, Data()) }
        return .init(
            status: 200, headers: ["Content-Type": "application/vnd.calimero.sealed"], body: out,
            holdOpen: answer.holdOpen)
    }

    /// Noise's `SymmetricState` for `Noise_NK_25519_AESGCM_SHA256`.
    private struct Symmetric {
        var ck: Data
        var h: Data
        var k: SymmetricKey?
        var n: UInt64 = 0

        init() {
            var name = Data("Noise_NK_25519_AESGCM_SHA256".utf8)
            name.append(Data(count: 32 - name.count))
            ck = name
            h = name
        }

        mutating func mixHash(_ data: Data) { h = Data(SHA256.hash(data: h + data)) }

        mutating func mixKey(_ input: Data) {
            let (ck, key) = Self.hkdf(ck, input)
            self.ck = ck
            k = SymmetricKey(data: key)
            n = 0
        }

        mutating func encryptAndHash(_ plain: Data) throws -> Data {
            let box = try AES.GCM.seal(plain, using: k!, nonce: try nonce(), authenticating: h)
            n += 1
            let out = box.ciphertext + box.tag
            mixHash(out)
            return out
        }

        mutating func decryptAndHash(_ cipher: Data) throws -> Data {
            let box = try AES.GCM.SealedBox(
                nonce: try nonce(), ciphertext: Data(cipher.prefix(cipher.count - 16)), tag: Data(cipher.suffix(16)))
            let plain = try AES.GCM.open(box, using: k!, authenticating: h)
            n += 1
            mixHash(cipher)
            return plain
        }

        func split() -> (Data, Data) { Self.hkdf(ck, Data()) }

        private func nonce() throws -> AES.GCM.Nonce {
            try AES.GCM.Nonce(data: Data(count: 4) + withUnsafeBytes(of: n.bigEndian) { Data($0) })
        }

        static func hkdf(_ chainingKey: Data, _ input: Data) -> (Data, Data) {
            let temp = Data(HMAC<SHA256>.authenticationCode(for: input, using: SymmetricKey(data: chainingKey)))
            let first = Data(HMAC<SHA256>.authenticationCode(for: Data([1]), using: SymmetricKey(data: temp)))
            let second = Data(HMAC<SHA256>.authenticationCode(for: first + Data([2]), using: SymmetricKey(data: temp)))
            return (first, second)
        }
    }

    private static func dh(_ secret: Curve25519.KeyAgreement.PrivateKey, _ peer: Data) throws -> Data {
        let shared = try secret.sharedSecretFromKeyAgreement(
            with: Curve25519.KeyAgreement.PublicKey(rawRepresentation: peer))
        return shared.withUnsafeBytes { Data($0) }
    }

    private static func u32(_ value: UInt32) -> Data { withUnsafeBytes(of: value.bigEndian) { Data($0) } }

    private static func readU32(_ data: Data) -> UInt32 {
        let b = [UInt8](data.prefix(4))
        return UInt32(b[0]) << 24 | UInt32(b[1]) << 16 | UInt32(b[2]) << 8 | UInt32(b[3])
    }

    private static func hex(_ value: String) -> Data? {
        try? Hex.decode(value, label: "hex", bytes: 32)
    }

    /// A request's body, read from its stream when `URLProtocol` handed it over as one.
    public static func body(_ request: URLRequest) -> Data {
        if let body = request.httpBody { return body }
        guard let stream = request.httpBodyStream else { return Data() }
        stream.open()
        defer { stream.close() }
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 4096)
        while true {
            let read = stream.read(&buffer, maxLength: buffer.count)
            guard read > 0 else { break }
            data.append(buffer, count: read)
        }
        return data
    }

    private func locked<T>(_ body: () throws -> T) rethrows -> T {
        lock.lock()
        defer { lock.unlock() }
        return try body()
    }
}
