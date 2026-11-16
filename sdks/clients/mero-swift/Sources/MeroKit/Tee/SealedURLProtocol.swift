import Foundation

#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

/// The `URLProtocol` behind ``SealedTransport/urlSession(configuration:)``:
/// every request a session built on it makes is sealed by the
/// ``SealedTransport`` registered for its URL, and a request no transport
/// covers fails instead of going out in the clear. (mero-js passes its sealed
/// `fetch` wherever a `fetch` goes; this is the `URLSession` equivalent.)
///
/// Transports are registered by base URL, one per base URL: registering a
/// second for the same base URL replaces the first.
public final class SealedURLProtocol: URLProtocol, @unchecked Sendable {
    private static let lock = NSLock()
    nonisolated(unsafe) private static var transports: [String: SealedTransport] = [:]

    private var loadTask: Task<Void, Never>?

    /// Route requests under `transport`'s base URL through it.
    public static func register(_ transport: SealedTransport) {
        lock.lock()
        defer { lock.unlock() }
        transports[transport.nodeURL] = transport
    }

    /// Stop routing requests under `baseURL` (a session that still uses it then fails closed).
    public static func unregister(baseURL: URL) {
        guard let parts = SealedURLParts(baseURL) else { return }
        var path = parts.path
        while path.hasSuffix("/") { path.removeLast() }
        lock.lock()
        defer { lock.unlock() }
        transports[parts.origin + path] = nil
    }

    /// Forget every registered transport.
    public static func unregisterAll() {
        lock.lock()
        defer { lock.unlock() }
        transports.removeAll()
    }

    /// The transport whose base URL covers `url`, the most specific first.
    static func transport(for url: URL) -> SealedTransport? {
        lock.lock()
        let all = Array(transports.values)
        lock.unlock()
        return all.filter { $0.covers(url) }.max { $0.basePath.count < $1.basePath.count }
    }

    public override class func canInit(with request: URLRequest) -> Bool {
        let scheme = request.url?.scheme?.lowercased()
        return scheme == "http" || scheme == "https"
    }

    public override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    public override func startLoading() {
        let request = self.request
        let client = SealedProtocolClient(self)
        loadTask = Task {
            do {
                guard let url = request.url, let transport = SealedURLProtocol.transport(for: url) else {
                    let named = request.url?.absoluteString ?? "a request with no URL"
                    throw TeeTrustError("Refusing to send \(named): no sealed transport covers it")
                }
                let response = try await transport.send(request)
                guard
                    let http = HTTPURLResponse(
                        url: url, statusCode: response.status, httpVersion: "HTTP/1.1", headerFields: response.headers)
                else { throw TeeTrustError("The sealed response could not be read") }
                client.received(http)
                for try await chunk in response.body { client.loaded(chunk) }
                client.finished()
            } catch {
                client.failed(error)
            }
        }
    }

    public override func stopLoading() {
        loadTask?.cancel()
    }
}

/// The protocol's client, handed to the task that feeds it.
private struct SealedProtocolClient: @unchecked Sendable {
    let owner: URLProtocol

    init(_ owner: URLProtocol) { self.owner = owner }

    func received(_ response: HTTPURLResponse) {
        owner.client?.urlProtocol(owner, didReceive: response, cacheStoragePolicy: .notAllowed)
    }

    func loaded(_ data: Data) {
        owner.client?.urlProtocol(owner, didLoad: data)
    }

    func finished() {
        owner.client?.urlProtocolDidFinishLoading(owner)
    }

    func failed(_ error: Error) {
        owner.client?.urlProtocol(owner, didFailWithError: Self.carryingMessage(error))
    }

    /// `error` as an `NSError` that keeps its message. On Apple platforms
    /// `URLSession` rebuilds a protocol's error from its domain, code and
    /// `userInfo`, which drops the Swift value and with it a
    /// `LocalizedError`'s description ("MeroKit.TeeTrustError error 1"), so
    /// the message is written into `userInfo` and the original kept as the
    /// underlying error.
    static func carryingMessage(_ error: Error) -> Error {
        guard !(error is URLError), let message = (error as? LocalizedError)?.errorDescription else { return error }
        let bridged = error as NSError
        var info = bridged.userInfo
        info[NSLocalizedDescriptionKey] = message
        info[NSUnderlyingErrorKey] = bridged
        return NSError(domain: bridged.domain, code: bridged.code, userInfo: info)
    }
}
