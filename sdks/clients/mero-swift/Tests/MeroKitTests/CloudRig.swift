import MeroKitTestSupport
import XCTest

@testable import MeroKit

/// A scripted plain-HTTP server (a release mirror, Intel PCS) behind ``MockURLProtocol``: answers by
/// `METHOD path`, records every request (with headers), and lets a test swap a
/// route's answer mid-flight.
final class CloudRig: @unchecked Sendable {
    struct Seen {
        let method: String
        let url: URL
        let headers: [String: String]
        let body: Data
        var json: JSONValue? { try? MeroJSON.decode(JSONValue.self, from: body) }
    }

    typealias Route = @Sendable (Seen) -> MockURLProtocol.Stub
    private let lock = NSLock()
    private var routes: [String: Route] = [:]
    private var _seen: [Seen] = []

    var seen: [Seen] {
        lock.lock(); defer { lock.unlock() }
        return _seen
    }

    func requests(_ method: String, _ path: String) -> [Seen] {
        seen.filter { $0.method == method && $0.url.path == path }
    }

    func on(_ method: String, _ path: String, _ route: @escaping Route) {
        lock.lock(); routes["\(method) \(path)"] = route; lock.unlock()
    }

    func on(_ method: String, _ path: String, status: Int = 200, json: String) {
        on(method, path) { _ in Self.stub(status, json) }
    }

    static func stub(_ status: Int, _ json: String) -> MockURLProtocol.Stub {
        .init(status: status, headers: ["Content-Type": "application/json"], body: Data(json.utf8))
    }

    func install() {
        MockURLProtocol.setHandler { [weak self] req in
            guard let self else { return .init(status: 500, headers: [:], body: Data()) }
            var headers: [String: String] = [:]
            for (k, v) in req.allHTTPHeaderFields ?? [:] { headers[k.lowercased()] = v }
            let seen = Seen(
                method: req.httpMethod ?? "GET", url: req.url!, headers: headers, body: FakeCloudNode.body(req))
            self.lock.lock()
            self._seen.append(seen)
            let route = self.routes["\(seen.method) \(seen.url.path)"]
            self.lock.unlock()
            return route?(seen) ?? Self.stub(404, #"{"error":"no route \#(seen.method) \#(seen.url.path)"}"#)
        }
    }
}
