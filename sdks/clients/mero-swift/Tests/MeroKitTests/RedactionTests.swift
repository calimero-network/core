import Foundation
import XCTest

@testable import MeroKit

/// Credentials never reach a log through `print`, interpolation or `dump`.
final class RedactionTests: XCTestCase {
    private func shown(_ value: Any) -> [String] {
        var dumped = ""
        dump(value, to: &dumped)
        return ["\(value)", String(describing: value), String(reflecting: value), dumped]
    }

    private func assertHides(_ value: Any, _ secrets: [String], file: StaticString = #filePath, line: UInt = #line) {
        for text in shown(value) {
            for secret in secrets {
                XCTAssertFalse(text.contains(secret), "\(text) shows \(secret)", file: file, line: line)
            }
        }
    }

    func testSessionsSeedsAndOwnerKeysHideTheirSecrets() throws {
        let session = CalimeroSession(
            accountId: "acc_1", sessionToken: "v2.SESSIONTOKEN", expiresAt: 1, refreshToken: "rt_REFRESHTOKEN")
        assertHides(session, ["SESSIONTOKEN", "REFRESHTOKEN"])
        XCTAssertTrue("\(session)".contains("acc_1"))
        assertHides(StoredCloudSession(nodeURL: "https://n.test", session: session), ["SESSIONTOKEN", "REFRESHTOKEN"])
        assertHides(
            ServedPageSession(
                accountId: "acc_1", sessionToken: "v2.SESSIONTOKEN", expiresAt: 1, refreshToken: "rt_REFRESHTOKEN",
                nodeUrl: "https://n.test"),
            ["SESSIONTOKEN", "REFRESHTOKEN"])
        let seed = try RecoverySeed(bytes: Data(repeating: 0xAB, count: 32))
        assertHides(seed, ["abababab", "171, 171", "32 bytes"])
        let owner = try generateOwnerKey()
        assertHides(owner, [owner.secret.base64EncodedString(), "32 bytes"])
    }
}
