import Foundation
import MeroAPI
import MeroKit
import XCTest

/// End-to-end against a real development cluster (`merod local`, mock
/// attestation). Skipped unless `MERO_E2E_NODE_URL` names one of its nodes,
/// e.g. `http://127.0.0.1:2501`. With `MERO_E2E_BUNDLE` (a dev `.wasm` that
/// exports `set`/`get`, like core's kv-store) it also creates an app and
/// calls it.
final class RealNodeE2ETests: XCTestCase {
    private var nodeURL: String {
        get throws {
            guard let url = ProcessInfo.processInfo.environment["MERO_E2E_NODE_URL"], !url.isEmpty else {
                throw XCTSkip("MERO_E2E_NODE_URL is not set")
            }
            return url
        }
    }

    func testSignsUpCreatesAnOrgAndCallsAnApp() async throws {
        let client = CalimeroClient(CalimeroClientOptions(baseUrl: try nodeURL, policy: .development))
        let session = try await client.signUp(SoftPasskey(), name: "swift-e2e")
        XCTAssertFalse(session.accountId.isEmpty)
        // Against a real node, through the generated types as an app would.
        let org = try await client.orgs.create(name: "swift-e2e").orgId
        let orgs = try await client.auth.me().orgs
        XCTAssertTrue(orgs.contains { $0.orgId == org })
        try await client.refresh()

        guard let path = ProcessInfo.processInfo.environment["MERO_E2E_BUNDLE"] else { return }
        let bundle = try await client.packages.bundlePut(body: try Data(contentsOf: URL(fileURLWithPath: path))).hash
        let app = try await client.apps.create(name: "kv", bundle: bundle, org: org).appId
        _ = try await client.call(app, "set", ["key": "a", "value": "1"])
        let read = try await client.view(app, "get", ["key": "a"])
        XCTAssertEqual(read.result, "1")
    }
}
