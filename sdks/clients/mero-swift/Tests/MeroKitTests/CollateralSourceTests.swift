import Foundation
import MeroKitTestSupport
import XCTest

@testable import MeroKit

/// Intel's collateral from the cloud's mirror (mero-js #321).
final class CollateralSourceTests: XCTestCase {
    override func tearDown() {
        MockURLProtocol.reset()
        super.tearDown()
    }

    private func collateral(issued: String) -> DcapCollateral {
        DcapCollateral(
            pckCrlIssuerChain: "", rootCaCrl: Data(), pckCrl: Data(), tcbInfoIssuerChain: "",
            tcbInfo: #"{"id":"TDX","issueDate":"\#(issued)","nextUpdate":"2026-12-01T00:00:00Z"}"#,
            tcbInfoSignature: Data(), qeIdentityIssuerChain: "", qeIdentity: "{}", qeIdentitySignature: Data())
    }

    func testTheDefaultSourceIsTheCloudsMirror() {
        XCTAssertEqual(TeeTrust.defaultCollateralMirror, "https://cloud.calimero.network/api/tee/pcs")
        let policy = CloudReleasePolicy(minReleaseVersion: "2.3.109")
        XCTAssertEqual(policy.collateralSource, TeeTrust.defaultCollateralMirror)
        XCTAssertNil(policy.maxCollateralAgeDays)
        XCTAssertEqual(IntelPcs.normalized("https://pccs.test/tdx/certification/v4/"), "https://pccs.test")
    }

    func testFreshCollateralRefusesCollateralOlderThanTheLimit() async throws {
        let now: @Sendable () -> Int64 = { 1_793_577_600_000 }  // 2026-11-02T00:00:00Z
        let stale = collateral(issued: "2026-10-01T00:00:00Z")
        let fresh = collateral(issued: "2026-10-30T12:00:00Z")
        let unbounded = freshCollateral({ _ in stale }, maxCollateralAgeDays: nil, now: now)
        _ = try await unbounded(Data())
        let bounded = freshCollateral({ _ in stale }, maxCollateralAgeDays: 7, now: now)
        do {
            _ = try await bounded(Data())
            XCTFail("expected stale collateral to be refused")
        } catch let error as TeeTrustError {
            XCTAssertTrue(error.message.contains("32 days ago"), error.message)
        }
        let ok = try await freshCollateral({ _ in fresh }, maxCollateralAgeDays: 7, now: now)(Data())
        XCTAssertEqual(ok, fresh)
        let undated = DcapCollateral(
            pckCrlIssuerChain: "", rootCaCrl: Data(), pckCrl: Data(), tcbInfoIssuerChain: "", tcbInfo: "{}",
            tcbInfoSignature: Data(), qeIdentityIssuerChain: "", qeIdentity: "{}", qeIdentitySignature: Data())
        do {
            _ = try await freshCollateral({ _ in undated }, maxCollateralAgeDays: 7, now: now)(Data())
            XCTFail("expected undated collateral to be refused")
        } catch is TeeTrustError {}
    }

    func testAMirrorsRootCrlIsReadFromItsRootcacrlRouteElseIntels() async throws {
        let seen = Box<[String]>([])
        MockURLProtocol.setHandler { request in
            let url = request.url?.absoluteString ?? ""
            seen.update { $0.append(url) }
            if url == "https://pccs.test/sgx/certification/v4/rootcacrl" {
                return .init(status: 200, headers: [:], body: Data("0a0b0c".utf8))
            }
            return .init(status: 200, headers: [:], body: Data([9]))
        }
        let session = MockURLProtocol.makeSession()
        let mirrored = try await IntelPcs.rootCrl("https://pccs.test", session: session)
        XCTAssertEqual(mirrored, Data([0x0a, 0x0b, 0x0c]))
        let intel = try await IntelPcs.rootCrl(IntelPcs.defaultBaseURL, session: session)
        XCTAssertEqual(intel, Data([9]))
        XCTAssertEqual(seen.get.last, IntelPcs.rootCaCrlURL, "Intel's PCS serves no rootcacrl")
    }
}
