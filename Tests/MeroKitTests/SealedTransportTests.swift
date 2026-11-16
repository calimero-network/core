import CryptoKit
import Foundation
import MeroKitTestSupport
import XCTest

@testable import MeroKit

/// Port of mero-js `sealed/sealed.test.ts`: core's published wire vectors
/// (`crates/server/src/sealed/tests.rs`), and a stand-in node that answers the
/// Noise handshake and seals its answers frame by frame, as core does.
final class SealedTransportTests: XCTestCase {
    private static let transportVector = "0faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20"
    private static let handshakeRequestVector =
        "020faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f207b0d47d93427f831116078"
        + "1c7c733fd89f88970aef490d8aa0ee19a4cb8a1b1461fba335b4f3e9be8a34a61900e5b4be"
    private static let handshakeResponseVector =
        "02ff2ee45601ec1b67310c7790404585ae697331eee1c1f8cf2419731c1fff3e6b7d6339b7296bdc4778248f"
        + "57126a10ee5b7fb39b0cb104dcbfcaaaf44d0e6017379638a4"
    private static let sealedRequestVector =
        "025555555555555555555555555555555500000000000000019fa4301264a48bdf6811cfb1d7c3dad713ede5"
        + "ea1820608358ff2779cee1605efc3d97879f24d6cf302a7877beda552ca6714ef9d5496f978fdcdde522435c"
        + "f663e4be5f60170e8eab74161a913608fe268e5352fcd3090d62e2a12c92b06b51885d43c4d83dcfe544"
    private static let sealedResponseVector =
        "0000004f4873ec5a589a1e3585360440100bc00d20522fe35bac2221dcfe80341ce3069481d4794b801776f4"
        + "a76188b81887eb83cfccd2981229af48a3820df924261b3ff12d4b29340ca07c392ed4e0ee4bed0000001cfa"
        + "20a7633655179808da19e2fcbc81bc219262fc4f5406582834f549000000111426897864d7a92eec9261fe29"
        + "93415592"
    private static let transportBindingVector = "30274595433e8afc5d4035e30a2b599d93caf0d470867527f13dc6af92fa12a8"

    private let base = URL(string: SealedStandInNode.baseURL)!
    private var transportKey: Data { try! Hex.decode(Self.transportVector, label: "t", bytes: 32) }
    private let vectorHead = SealedRequestHead(
        method: "POST", path: "/jsonrpc", headers: [("content-type", "application/json")])

    override func tearDown() {
        MockURLProtocol.reset()
        SealedURLProtocol.unregisterAll()
        super.tearDown()
    }

    /// The vector session: the handshake with core's fixed client ephemeral key.
    private func vectorSession() throws -> (NoiseInitiator, SealedSession) {
        let noise = try NoiseInitiator(
            remoteStatic: transportKey, prologue: SealedTransport.prologue,
            fixedEphemeral: Data(repeating: 0x33, count: 32))
        let reply = try Hex.decodeUnsized(Self.handshakeResponseVector, label: "r")
        return (noise, try SealedTransport.sessionFrom(noise, reply: reply))
    }

    /// Open `bytes` as a sealed response under the vector session's request 1.
    private func open(_ bytes: [UInt8]) async throws -> SealedResponse {
        let (_, session) = try vectorSession()
        let sealed = try SealedTransport.sealRequest(session, head: vectorHead, body: Data())
        let opener = SealedFrameOpener(key: session.responseKey, header: sealed.header, requestId: sealed.requestId)
        return try await SealedTransport.streamed { emitHead, continuation in
            let frames = SealedFrameReader(SealedTestBytes(bytes))
            try await SealedTransport.openResponse(frames, opener: opener, emitHead, continuation)
        }
    }

    // MARK: - Wire format

    func testOpensTheHandshakeExactlyAsTheNodeExpects() throws {
        let (noise, _) = try vectorSession()
        XCTAssertEqual(Hex.encode(Data([2]) + transportKey + noise.message1), Self.handshakeRequestVector)
    }

    func testSealsARequestUnderTheSessionExactlyAsTheNodeExpects() throws {
        let (_, session) = try vectorSession()
        let sealed = try SealedTransport.sealRequest(session, head: vectorHead, body: Data("{}".utf8))
        XCTAssertEqual(Hex.encode(sealed.envelope), Self.sealedRequestVector)
        XCTAssertEqual(
            vectorHead.json, #"{"method":"POST","path":"/jsonrpc","headers":[["content-type","application/json"]]}"#)
    }

    func testOpensTheFramesOfAResponseTheNodeSealed() async throws {
        let response = try await open([UInt8](try Hex.decodeUnsized(Self.sealedResponseVector, label: "r")))
        XCTAssertEqual(response.status, 200)
        XCTAssertEqual(response.headers["content-type"], "application/json")
        let body = try await response.data()
        XCTAssertEqual(String(decoding: body, as: UTF8.self), #"{"ok":true}"#)
    }

    func testRefusesAHandshakeReplyFromAnyoneButTheAttestedNode() throws {
        let noise = try NoiseInitiator(
            remoteStatic: transportKey, prologue: SealedTransport.prologue,
            fixedEphemeral: Data(repeating: 0x33, count: 32))
        var forged = [UInt8](try Hex.decodeUnsized(Self.handshakeResponseVector, label: "r"))
        forged[forged.count - 1] ^= 1
        assertTeeRefusal(
            teeThrownMessage { _ = try SealedTransport.sessionFrom(noise, reply: Data(forged)) },
            "did not come from the attested node")
    }

    func testRefusesAResponseFrameThatAnyoneButTheNodeSealed() async throws {
        var forged = [UInt8](try Hex.decodeUnsized(Self.sealedResponseVector, label: "r"))
        forged[forged.count - 1] ^= 1
        let response = try await open(forged)
        let refusal = await teeAsyncThrownMessage { _ = try await response.data() }
        assertTeeRefusal(refusal, "did not open")
    }

    func testNoticesAResponseCutShortBeforeItsEndFrame() async throws {
        let whole = [UInt8](try Hex.decodeUnsized(Self.sealedResponseVector, label: "r"))
        let endFrame = 4 + 1 + 16
        let response = try await open(Array(whole.dropLast(endFrame)))
        let refusal = await teeAsyncThrownMessage { _ = try await response.data() }
        assertTeeRefusal(refusal, "cut short")
    }

    func testRefusesAFrameLargerThanTheNodeEverSendsBeforeBufferingIt() async throws {
        let pulled = Counter()
        let (_, session) = try vectorSession()
        let sealed = try SealedTransport.sealRequest(session, head: vectorHead, body: Data())
        let opener = SealedFrameOpener(key: session.responseKey, header: sealed.header, requestId: sealed.requestId)
        let refusal = await teeAsyncThrownMessage {
            _ = try await SealedTransport.streamed { emitHead, continuation in
                let frames = SealedFrameReader(SealedTestEndless(pulled: pulled))
                try await SealedTransport.openResponse(frames, opener: opener, emitHead, continuation)
            }
        }
        assertTeeRefusal(refusal, "larger than the node sends")
        XCTAssertLessThanOrEqual(pulled.count, 4)
    }

    func testComputesTheTransportBindingTheNodePutsInItsQuote() {
        let binding = transportKeyBinding(Data(repeating: 0x11, count: 32), Data(repeating: 0x22, count: 32))
        XCTAssertEqual(Hex.encode(binding), Self.transportBindingVector)
    }

    // MARK: - A sealed transport to a stand-in node

    private func transport(
        _ node: SealedStandInNode, attest: (@Sendable () async throws -> Data)? = nil
    ) throws -> SealedTransport {
        let key: TransportKeySource = attest.map { .attest($0) } ?? .key(node.transportKey)
        return try SealedTransport(baseURL: base, key: key, session: MockURLProtocol.makeSession())
    }

    private func request(_ path: String, method: String = "GET", body: String? = nil) -> URLRequest {
        var request = URLRequest(url: URL(string: SealedStandInNode.baseURL + path)!)
        request.httpMethod = method
        request.httpBody = body.map { Data($0.utf8) }
        return request
    }

    func testDeliversTheWholeRequestToTheNodeAndReturnsItsAnswer() async throws {
        let node = SealedStandInNode { _ in
            SealedStandInAnswer(status: 201, headers: [("x-answer", "yes")], chunks: [#"{"result":"#, "42}"])
        }
        node.install()
        var call = request("/jsonrpc?x=1", method: "POST", body: #"{"method":"secret-call"}"#)
        call.setValue("Bearer secret-token", forHTTPHeaderField: "Authorization")
        call.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let (body, response) = try await transport(node).data(for: call)

        let seen = try XCTUnwrap(node.requests.first)
        XCTAssertEqual(seen.method, "POST")
        XCTAssertEqual(seen.path, "/jsonrpc?x=1")
        XCTAssertTrue(seen.headers.contains { $0.0 == "authorization" && $0.1 == "Bearer secret-token" })
        XCTAssertEqual(seen.body, #"{"method":"secret-call"}"#)
        XCTAssertEqual(response.statusCode, 201)
        XCTAssertEqual(response.value(forHTTPHeaderField: "x-answer"), "yes")
        XCTAssertEqual(String(decoding: body, as: UTF8.self), #"{"result":42}"#)

        let wire = node.wire
        for secret in ["secret-token", "secret-call", "jsonrpc"] {
            XCTAssertNil(wire.range(of: Data(secret.utf8)), "\(secret) crossed the wire in the clear")
        }
    }

    func testOpensOneSessionForManyRequestsConcurrentOnesIncluded() async throws {
        let node = SealedStandInNode { _ in .ok() }
        node.install()
        let sealed = try transport(node)
        let call = request("/admin-api/health")
        try await withThrowingTaskGroup(of: Void.self) { group in
            for _ in 0..<3 { group.addTask { _ = try await sealed.data(for: call) } }
            try await group.waitForAll()
        }
        _ = try await sealed.data(for: call)
        XCTAssertEqual(node.handshakes, 1)
        XCTAssertEqual(node.requests.count, 4)
    }

    func testOpensANewSessionWhenTheNodeDroppedTheOldOneAndRetries() async throws {
        let node = SealedStandInNode { _ in .ok(#""again""#) }
        node.install()
        let sealed = try transport(node)
        _ = try await sealed.data(for: request("/admin-api/health"))
        node.forgetSessions()
        let (body, _) = try await sealed.data(for: request("/admin-api/health"))
        XCTAssertEqual(String(decoding: body, as: UTF8.self), #""again""#)
        XCTAssertEqual(node.handshakes, 2)
    }

    func testAttestsAgainThroughTheCallerWhenTheNodeRestarted() async throws {
        let node = SealedStandInNode { _ in .ok() }
        node.install()
        let attests = Counter()
        let sealed = try transport(node) {
            attests.increment()
            return node.transportKey
        }
        _ = try await sealed.data(for: request("/admin-api/health"))
        node.restart()
        let (_, response) = try await sealed.data(for: request("/admin-api/health"))
        XCTAssertEqual(response.statusCode, 200)
        XCTAssertEqual(attests.count, 2)
    }

    func testTellsTheCallerToAttestAgainWhenGivenAFixedKeyAndTheNodeRestarted() async throws {
        let node = SealedStandInNode { _ in .ok() }
        node.install()
        let sealed = try transport(node)
        node.restart()
        do {
            _ = try await sealed.data(for: request("/admin-api/health"))
            XCTFail("expected a stale transport key")
        } catch {
            XCTAssertTrue(error is StaleTransportKeyError, "\(error)")
        }
    }

    func testRefusesAHandshakeReplyTooLongToBeOne() async throws {
        MockURLProtocol.setHandler { _ in
            .init(status: 200, headers: ["Content-Type": SealedTransport.contentType], body: Data(count: 4096))
        }
        let sealed = try SealedTransport(baseURL: base, key: .key(transportKey), session: MockURLProtocol.makeSession())
        let call = request("/admin-api/health")
        let refusal = await teeAsyncThrownMessage { _ = try await sealed.data(for: call) }
        assertTeeRefusal(refusal, "too long")
    }

    func testWaitsAndRetriesOnceWhenTheNodeIsTooBusyToOpenASession() async throws {
        let node = SealedStandInNode { _ in .ok() }
        let refused = Counter()
        MockURLProtocol.setHandler { request in
            if request.url?.path.hasSuffix(SealedTransport.handshakePath) == true, refused.count < 1 {
                refused.increment()
                return SealedStandInNode.refusal(503, "busy", headers: ["Retry-After": "0.01"])
            }
            return node.handle(request)
        }
        let (_, response) = try await transport(node).data(for: request("/admin-api/health"))
        XCTAssertEqual(response.statusCode, 200)
        XCTAssertEqual(node.handshakes, 1)
    }

    func testOpensTheSessionAtTheUnversionedHandshakePath() async throws {
        let node = SealedStandInNode { _ in .ok() }
        let paths = Box<[String]>([])
        MockURLProtocol.setHandler { request in
            paths.update { $0.append(request.url?.path ?? "") }
            return node.handle(request)
        }
        let (_, response) = try await transport(node).data(for: request("/admin-api/health"))
        XCTAssertEqual(response.statusCode, 200)
        XCTAssertEqual(SealedTransport.handshakePath, "/sealed/handshake")
        XCTAssertEqual(paths.get, ["/node/sealed/handshake", "/node" + SealedTransport.sealedPath])
    }

    func testOpensTheSessionAtTheV2PathWhenTheNodeDoesNotRouteTheNewOne() async throws {
        // A node released before /sealed/handshake routes only the v2 path.
        for status in [404, 405] {
            let node = SealedStandInNode { _ in .ok() }
            let paths = Box<[String]>([])
            MockURLProtocol.setHandler { request in
                let path = request.url?.path ?? ""
                paths.update { $0.append(path) }
                if path == "/node" + SealedTransport.handshakePath {
                    return .init(status: status, headers: ["Content-Type": "text/plain"], body: Data("Not Found".utf8))
                }
                return node.handle(request)
            }
            let (_, response) = try await transport(node).data(for: request("/admin-api/health"))
            XCTAssertEqual(response.statusCode, 200, "\(status)")
            let expected = [
                "/node/sealed/handshake", "/node/sealed/v2/handshake", "/node" + SealedTransport.sealedPath,
            ]
            XCTAssertEqual(paths.get, expected, "\(status)")
            XCTAssertEqual(node.handshakes, 1, "\(status)")
        }
    }

    func testOpensTheSessionAtTheV2PathWhenASealedOnlyNodeRefusesTheNewOne() async throws {
        // An older node that seals everything refuses an unknown path as unsealed.
        let node = SealedStandInNode { _ in .ok() }
        let paths = Box<[String]>([])
        MockURLProtocol.setHandler { request in
            let path = request.url?.path ?? ""
            paths.update { $0.append(path) }
            if path == "/node" + SealedTransport.handshakePath {
                return SealedStandInNode.refusal(403, "sealed_required")
            }
            return node.handle(request)
        }
        let (_, response) = try await transport(node).data(for: request("/admin-api/health"))
        XCTAssertEqual(response.statusCode, 200)
        let expected = ["/node/sealed/handshake", "/node/sealed/v2/handshake", "/node" + SealedTransport.sealedPath]
        XCTAssertEqual(paths.get, expected)
    }

    func testReportsA404FromTheV2HandshakePathTooWithoutTryingAgain() async throws {
        let calls = Counter()
        MockURLProtocol.setHandler { _ in
            calls.increment()
            return .init(status: 404, headers: ["Content-Type": "text/plain"], body: Data("Not Found".utf8))
        }
        let sealed = try SealedTransport(baseURL: base, key: .key(transportKey), session: MockURLProtocol.makeSession())
        do {
            _ = try await sealed.data(for: request("/admin-api/health"))
            XCTFail("expected a refusal")
        } catch let error as SealedTransportError {
            XCTAssertEqual(error.status, 404)
        }
        XCTAssertEqual(calls.count, 2)
    }

    func testDoesNotTryTheV2HandshakePathOnAnyOtherRefusal() async throws {
        let refusals = [(400, "malformed"), (403, "forbidden"), (409, "stale_transport_key"), (500, "internal")]
        for (status, code) in refusals {
            let paths = Box<[String]>([])
            MockURLProtocol.setHandler { request in
                paths.update { $0.append(request.url?.path ?? "") }
                return SealedStandInNode.refusal(status, code)
            }
            let sealed = try SealedTransport(
                baseURL: base, key: .key(transportKey), session: MockURLProtocol.makeSession())
            let answered = try? await sealed.data(for: request("/admin-api/health"))
            XCTAssertNil(answered, "expected a refusal for \(status)")
            XCTAssertEqual(paths.get, ["/node" + SealedTransport.handshakePath], "\(status)")
        }
    }

    func testPassesANodeRefusalOnAsASealedTransportError() async throws {
        MockURLProtocol.setHandler { _ in SealedStandInNode.refusal(400, "malformed") }
        let sealed = try SealedTransport(baseURL: base, key: .key(transportKey), session: MockURLProtocol.makeSession())
        do {
            _ = try await sealed.data(for: request("/admin-api/health"))
            XCTFail("expected a refusal")
        } catch let error as SealedTransportError {
            XCTAssertEqual(error.code, "malformed")
            XCTAssertEqual(error.status, 400)
        }
    }

    func testHandsOverAStreamedBodyFrameByFrameBeforeTheStreamEnds() async throws {
        let node = SealedStandInNode { _ in
            SealedStandInAnswer(
                status: 200, headers: [("content-type", "text/event-stream")], chunks: ["data: one\n\n"],
                holdOpen: true)
        }
        node.install()
        var call = request("/sse")
        call.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        let response = try await transport(node).send(call)
        XCTAssertEqual(response.headers["content-type"], "text/event-stream")
        var iterator = response.body.makeAsyncIterator()
        let first = try await iterator.next()
        XCTAssertEqual(first.map { String(decoding: $0, as: UTF8.self) }, "data: one\n\n")
    }

    func testNeverSendsARequestOutsideTheNodeInTheClear() async throws {
        let calls = Counter()
        MockURLProtocol.setHandler { _ in
            calls.increment()
            return .init(status: 500, headers: [:], body: Data())
        }
        let sealed = try SealedTransport(baseURL: base, key: .key(transportKey), session: MockURLProtocol.makeSession())
        for url in ["https://elsewhere.example/node/jsonrpc", "https://tee-node.example/other/jsonrpc"] {
            let call = URLRequest(url: URL(string: url)!)
            let refusal = await teeAsyncThrownMessage { _ = try await sealed.send(call) }
            assertTeeRefusal(refusal, "Refusing")
        }
        XCTAssertEqual(calls.count, 0)
    }

    func testSealsEveryRequestOfItsURLSessionAndFailsOneOutsideClosed() async throws {
        let node = SealedStandInNode { seen in .ok(#"{"path":"\#(seen.path)"}"#) }
        node.install()
        let session = try transport(node).urlSession()
        var call = request("/admin-api/contexts", method: "POST", body: #"{"name":"x"}"#)
        call.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let (data, response) = try await session.data(for: call)
        XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 200)
        XCTAssertEqual(String(decoding: data, as: UTF8.self), #"{"path":"/admin-api/contexts"}"#)
        XCTAssertEqual(node.requests.first?.body, #"{"name":"x"}"#)
        do {
            _ = try await session.data(from: URL(string: "https://elsewhere.example/x")!)
            XCTFail("a request outside the node must not be sent")
        } catch {
            XCTAssertEqual(node.requests.count, 1)
        }
    }

    func testFallsBackToPlainOnlyForRoutesTheTDCannotGuardSealed() async throws {
        let node = SealedStandInNode { seen in
            seen.headers.contains { $0.0 == "authorization" }
                ? SealedStandInAnswer(
                    status: 403, headers: [("content-type", "application/json")],
                    chunks: [#"{"error":{"code":"sealed_route_unguarded","message":"no forward_auth"}}"#])
                : .ok()
        }
        node.install()
        let sealed = try SealedTransport(
            baseURL: base, key: .key(node.transportKey), session: MockURLProtocol.makeSession(),
            unguardedFallback: true)
        var authed = request("/admin-api/contexts")
        authed.setValue("Bearer t", forHTTPHeaderField: "Authorization")
        let (body, response) = try await sealed.data(for: authed)
        XCTAssertEqual(response.statusCode, 200)
        XCTAssertEqual(String(decoding: body, as: UTF8.self), "plain")
        XCTAssertEqual(node.plain, 1)
        XCTAssertEqual(node.requests.count, 1)
        // Logged-in calls now go plain at once; the others stay sealed.
        _ = try await sealed.data(for: authed)
        XCTAssertEqual(node.plain, 2)
        XCTAssertEqual(node.requests.count, 1)
        _ = try await sealed.data(for: request("/admin-api/health"))
        XCTAssertEqual(node.requests.count, 2)
    }

    // MARK: - fetchAttestedTransportKey
}

struct SealedTestBytes: AsyncIteratorProtocol {
    private let bytes: [UInt8]
    private var index = 0

    init(_ bytes: [UInt8]) { self.bytes = bytes }

    mutating func next() async -> UInt8? {
        guard index < bytes.count else { return nil }
        defer { index += 1 }
        return bytes[index]
    }
}

/// A length prefix far beyond any frame, then zeros forever.
struct SealedTestEndless: AsyncIteratorProtocol {
    let pulled: Counter
    private var index = 0

    init(pulled: Counter) { self.pulled = pulled }

    mutating func next() async -> UInt8? {
        pulled.increment()
        defer { index += 1 }
        return index < 4 ? 0xff : 0
    }
}

/// Values a `@Sendable` closure saw, for assertions after the call.
final class SealedTestRecorder<Value: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var seen: [Value] = []

    func record(_ value: Value) {
        lock.lock()
        seen.append(value)
        lock.unlock()
    }

    var values: [Value] {
        lock.lock()
        defer { lock.unlock() }
        return seen
    }
}

/// One request the stand-in node opened.
struct SealedStandInRequest: Sendable {
    let method: String
    let path: String
    let headers: [(String, String)]
    let body: String
}

/// What the stand-in node answers: each chunk its own frame.
struct SealedStandInAnswer: Sendable {
    var status: Int
    var headers: [(String, String)]
    var chunks: [String]
    /// Send no end frame and keep the response open (a live stream).
    var holdOpen = false

    static func ok(_ body: String = "{}") -> SealedStandInAnswer {
        SealedStandInAnswer(status: 200, headers: [("content-type", "application/json")], chunks: [body])
    }
}

/// A stand-in node: answers the Noise handshake with its transport secret and
/// seals its answers frame by frame, as core does; with `attesting`, also the
/// one unsealed route a client needs first, `POST /admin-api/tee/attest`,
/// answered with a "quote" that is only its report data.
final class SealedStandInNode: @unchecked Sendable {
    static let baseURL = "https://tee-node.example/node"

    private struct SealedStandInKeys {
        let request: SymmetricKey
        let response: SymmetricKey
    }

    private let lock = NSLock()
    private let answer: @Sendable (SealedStandInRequest) -> SealedStandInAnswer
    private var secret = Curve25519.KeyAgreement.PrivateKey()
    private var sessions: [Data: SealedStandInKeys] = [:]
    private var _handshakes = 0
    private var _attests = 0
    private var _plain = 0
    private var _requests: [SealedStandInRequest] = []
    private var _wire = Data()

    init(_ answer: @escaping @Sendable (SealedStandInRequest) -> SealedStandInAnswer) {
        self.answer = answer
    }

    var transportKey: Data { locked { secret.publicKey.rawRepresentation } }
    var handshakes: Int { locked { _handshakes } }
    var attests: Int { locked { _attests } }
    /// Requests that reached the node outside the envelope.
    var plain: Int { locked { _plain } }
    var requests: [SealedStandInRequest] { locked { _requests } }
    /// Every body that crossed the wire.
    var wire: Data { locked { _wire } }

    func restart() {
        locked {
            secret = Curve25519.KeyAgreement.PrivateKey()
            sessions.removeAll()
        }
    }

    func forgetSessions() {
        locked { sessions.removeAll() }
    }

    func install(attesting: Bool = false) {
        MockURLProtocol.setHandler { [self] request in
            if attesting, request.url?.path == "/node/admin-api/tee/attest" { return attest(request) }
            return handle(request)
        }
    }

    func handle(_ request: URLRequest) -> MockURLProtocol.Stub {
        let body = FakeCloudNode.body(request)
        locked { _wire.append(body) }
        let path = request.url?.path ?? ""
        do {
            if path == "/node" + SealedTransport.handshakePath { return try handshake(body) }
            if path == "/node" + SealedTransport.legacyHandshakePath { return try handshake(body) }
            if path == "/node" + SealedTransport.sealedPath { return try exchange(body) }
        } catch {
            return Self.refusal(400, "malformed")
        }
        locked { _plain += 1 }
        return .init(status: 200, headers: ["Content-Type": "text/plain"], body: Data("plain".utf8))
    }

    static func refusal(_ status: Int, _ code: String, headers: [String: String] = [:]) -> MockURLProtocol.Stub {
        var all = headers
        all["Content-Type"] = "application/json"
        let body = #"{"error":{"code":"\#(code)","message":"\#(code)"}}"#
        return .init(status: status, headers: all, body: Data(body.utf8))
    }

    private func attest(_ request: URLRequest) -> MockURLProtocol.Stub {
        let nonce =
            (try? MeroJSON.decode(JSONValue.self, from: FakeCloudNode.body(request)))?["nonce"]?.stringValue ?? ""
        let key = transportKey
        let quote = (try? Hex.decode(nonce, label: "n", bytes: 32)) ?? Data()
        let reportData = quote + transportKeyBinding(Data(count: 32), key)
        locked { _attests += 1 }
        let body = CanonicalJSON.string([
            "data": [
                "quoteB64": .string(reportData.base64EncodedString()), "transportPublicKey": .string(Hex.encode(key)),
            ]
        ])
        return .init(status: 200, headers: ["Content-Type": "application/json"], body: Data(body.utf8))
    }

    private func handshake(_ body: Data) throws -> MockURLProtocol.Stub {
        let bytes = [UInt8](body)
        let secret = locked { self.secret }
        guard bytes.count > 33, Data(bytes[1..<33]) == secret.publicKey.rawRepresentation else {
            return Self.refusal(409, "stale_transport_key")
        }
        let message1 = Data(bytes[33...])
        var state = NoiseSymmetricState()
        state.mixHash(SealedTransport.prologue)
        state.mixHash(secret.publicKey.rawRepresentation)
        let remote = Data(message1.prefix(32))
        state.mixHash(remote)
        state.mixKey(try NoiseDH.dh(secret, remote))
        _ = try state.decryptAndHash(Data(message1.dropFirst(32)))
        let ephemeral = Curve25519.KeyAgreement.PrivateKey()
        state.mixHash(ephemeral.publicKey.rawRepresentation)
        state.mixKey(try NoiseDH.dh(ephemeral, remote))
        let sessionId = randomBytes(16)
        let sealed = try state.encryptAndHash(sessionId + Data([0, 0, 0x0e, 0x10]))
        let (requestKey, responseKey) = state.split()
        locked {
            _handshakes += 1
            sessions[sessionId] = SealedStandInKeys(
                request: SymmetricKey(data: requestKey), response: SymmetricKey(data: responseKey))
        }
        let reply = Data([2]) + ephemeral.publicKey.rawRepresentation + sealed
        return .init(status: 200, headers: ["Content-Type": SealedTransport.contentType], body: reply)
    }

    private func exchange(_ envelope: Data) throws -> MockURLProtocol.Stub {
        let bytes = [UInt8](envelope)
        let header = Data(bytes.prefix(25))
        guard let keys = locked({ sessions[Data(bytes[1..<17])] }) else { return Self.refusal(409, "unknown_session") }
        let requestId = Data(bytes[17..<25])
        let nonce = { (index: UInt32) in try AES.GCM.Nonce(data: requestId + BigEndian.u32(index)) }
        let sealed = Data(bytes[25...])
        let box = try AES.GCM.SealedBox(
            nonce: try nonce(0), ciphertext: Data(sealed.prefix(sealed.count - 16)), tag: Data(sealed.suffix(16)))
        let inner = try AES.GCM.open(box, using: keys.request, authenticating: header)
        let length = Int(BigEndian.readU32(inner))
        let head = try MeroJSON.decode(JSONValue.self, from: Data(inner.dropFirst(4).prefix(length)))
        let headers = (head["headers"]?.arrayValue ?? []).compactMap { pair -> (String, String)? in
            guard let entry = pair.arrayValue, entry.count == 2, let name = entry[0].stringValue,
                let value = entry[1].stringValue
            else { return nil }
            return (name, value)
        }
        let seen = SealedStandInRequest(
            method: head["method"]?.stringValue ?? "", path: head["path"]?.stringValue ?? "", headers: headers,
            body: String(decoding: inner.dropFirst(4 + length), as: UTF8.self))
        locked { _requests.append(seen) }
        let reply = answer(seen)

        var index: UInt32 = 0
        var out = Data()
        func frame(_ kind: UInt8, _ data: Data) throws {
            let box = try AES.GCM.seal(
                Data([kind]) + data, using: keys.response, nonce: try nonce(index), authenticating: header)
            index += 1
            let framed = box.ciphertext + box.tag
            out.append(BigEndian.u32(UInt32(framed.count)) + framed)
        }
        let replyHead = CanonicalJSON.string([
            "status": .number(Double(reply.status)),
            "headers": .array(reply.headers.map { .array([.string($0.0), .string($0.1)]) }),
        ])
        try frame(0, Data(replyHead.utf8))
        for chunk in reply.chunks { try frame(1, Data(chunk.utf8)) }
        if !reply.holdOpen { try frame(2, Data()) }
        return .init(
            status: 200, headers: ["Content-Type": SealedTransport.contentType], body: out, holdOpen: reply.holdOpen)
    }

    private func locked<T>(_ body: () throws -> T) rethrows -> T {
        lock.lock()
        defer { lock.unlock() }
        return try body()
    }
}
