import Foundation

#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

/// A `URLProtocol` that routes every request through a registered handler, so
/// tests can stub HTTP without a live node — Swift's clean way to fake `URLSession`
/// (and the route-mocking analog of Playwright's `page.route`).
public final class MockURLProtocol: URLProtocol {
    /// Handler result: status, headers, and body bytes for a request.
    public struct Stub: Sendable {
        public let status: Int
        public let headers: [String: String]
        public let body: Data
        /// Hold the response back this long before delivering it.
        ///
        /// The handler still runs when the request is *made*, so a stub decided
        /// against the state of the world at t=0 can be delivered after that
        /// state has moved on — which is the only way to reproduce a response
        /// that races a token rotation.
        public let delay: TimeInterval
        /// Deliver the body but never finish the response, until the request
        /// is cancelled — a long-lived stream (`/sse`) that stays open after
        /// its first frames.
        public let holdOpen: Bool
        /// Fail the request with this error instead of answering it: no HTTP
        /// response at all, the way a dropped connection, a DNS failure or
        /// airplane mode looks to `URLSession`. `status`, `headers` and `body`
        /// are ignored. Through ``MeroKit`` it surfaces as `MeroError.network`.
        public let error: URLError?

        public init(
            status: Int, headers: [String: String], body: Data, delay: TimeInterval = 0, holdOpen: Bool = false,
            error: URLError? = nil
        ) {
            self.status = status
            self.headers = headers
            self.body = body
            self.delay = delay
            self.holdOpen = holdOpen
            self.error = error
        }

        /// A request that never reaches a server: it fails with `code`
        /// (`.notConnectedToInternet` by default), after `delay`. Use this,
        /// not a made-up status like `0`, to test what an app does offline.
        public static func transportError(
            _ code: URLError.Code = .notConnectedToInternet, delay: TimeInterval = 0
        ) -> Stub {
            .init(status: 0, headers: [:], body: Data(), delay: delay, error: URLError(code))
        }
    }

    private static let lock = NSLock()
    nonisolated(unsafe) private static var _handler: (@Sendable (URLRequest) -> Stub)?

    private let lock = NSLock()
    private var _cancelled = false
    private var cancelled: Bool {
        lock.lock(); defer { lock.unlock() }
        return _cancelled
    }

    /// Register the single handler that serves all requests.
    public static func setHandler(_ handler: @escaping @Sendable (URLRequest) -> Stub) {
        lock.lock(); defer { lock.unlock() }
        _handler = handler
    }

    public static func reset() {
        lock.lock(); defer { lock.unlock() }
        _handler = nil
    }

    private static func handler() -> (@Sendable (URLRequest) -> Stub)? {
        lock.lock(); defer { lock.unlock() }
        return _handler
    }

    /// Build a `URLSession` wired to this protocol.
    public static func makeSession() -> URLSession {
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [MockURLProtocol.self]
        return URLSession(configuration: config)
    }

    public override class func canInit(with request: URLRequest) -> Bool { true }
    public override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    public override func startLoading() {
        guard let handler = MockURLProtocol.handler() else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }
        let stub = handler(request)
        guard stub.delay > 0 else {
            deliver(stub)
            return
        }
        DispatchQueue.global().asyncAfter(deadline: .now() + stub.delay) { [weak self] in
            guard let self, !self.cancelled else { return }
            self.deliver(stub)
        }
    }

    public override func stopLoading() {
        lock.lock(); _cancelled = true; lock.unlock()
    }

    private func deliver(_ stub: Stub) {
        if let error = stub.error {
            client?.urlProtocol(self, didFailWithError: error)
            return
        }
        let response = HTTPURLResponse(
            url: request.url!,
            statusCode: stub.status,
            httpVersion: "HTTP/1.1",
            headerFields: stub.headers
        )!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: stub.body)
        if stub.holdOpen { return }
        client?.urlProtocolDidFinishLoading(self)
    }
}

/// A mutable, thread-safe counter for tests.
public final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var value = 0

    public init() {}

    @discardableResult
    public func increment() -> Int {
        lock.lock(); defer { lock.unlock() }
        value += 1
        return value
    }

    public var count: Int {
        lock.lock(); defer { lock.unlock() }
        return value
    }
}
