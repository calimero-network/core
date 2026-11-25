import XCTest

@testable import MeroKit

/// Which image profile a node's quote is judged against (mero-js
/// `trustedProfile`), and what each Calimero Cloud trusts. The quote is then
/// checked against that profile's signed image, as `trustSignedRelease`
/// already does (TeeNodeReleaseTests), so naming another profile only gets a
/// node refused.
final class CloudEnvironmentTests: XCTestCase {
    private func doc(_ profile: String) -> AttestationDoc {
        AttestationDoc(
            quote: "", transportKey: "", release: AttestationDoc.Release(version: "2.3.109", profile: profile))
    }

    private var staging: CloudReleasePolicy { CloudEnvironment.staging.policy().release! }

    func testProductionTrustsLockedReadOnlyOnlyWhateverTheNodeNames() throws {
        let production = try XCTUnwrap(CloudPolicy.production.release)
        XCTAssertNil(production.profiles)
        XCTAssertFalse(production.allowDebugProfile)
        XCTAssertFalse(CloudPolicy.production.allowMock)
        XCTAssertEqual(production.minReleaseVersion, defaultMinRelease)
        XCTAssertEqual(try trustedProfile(doc("debug"), production), TeeTrust.productionProfile)
        XCTAssertEqual(CloudEnvironment.production.directory, defaultDirectory)
    }

    func testStagingTrustsEachOfItsProfilesTheOneTheNodeNames() throws {
        XCTAssertEqual(staging.profiles, [TeeTrust.productionProfile, "debug", "debug-read-only"])
        XCTAssertTrue(staging.allowDebugProfile)
        for profile in staging.profiles ?? [] {
            XCTAssertEqual(try trustedProfile(doc(profile), staging), profile)
        }
        XCTAssertEqual(CloudPolicy.staging.release?.profiles, staging.profiles)
        let floor = CloudEnvironment.staging.policy(minReleaseVersion: "2.3.140").release?.minReleaseVersion
        XCTAssertEqual(floor, "2.3.140")
        XCTAssertFalse(CloudPolicy.staging.allowMock)
        XCTAssertEqual(
            CloudEnvironment.staging.directory, "https://manager.staging.cloud.calimero.network/api/directory")
    }

    func testANodeNamingAProfileOffTheListIsRefused() {
        XCTAssertThrowsError(try trustedProfile(doc("debug-ssh"), staging)) { error in
            XCTAssertTrue("\(error)".contains("not one of"))
        }
        var onlyDebug = staging
        onlyDebug.profiles = ["debug"]
        XCTAssertThrowsError(try trustedProfile(doc(TeeTrust.productionProfile), onlyDebug))
    }

    func testAListedDebugProfileNeedsTheOptInAndTheListIsNeverEmptyNorDoubled() {
        var silent = staging
        silent.allowDebugProfile = false
        XCTAssertThrowsError(try trustedProfile(doc("debug"), silent)) { error in
            XCTAssertTrue("\(error)".contains("which an operator can read"))
        }
        var empty = staging
        empty.profiles = []
        XCTAssertThrowsError(try trustedProfile(doc(TeeTrust.productionProfile), empty))
        var doubled = staging
        doubled.profile = "debug"
        XCTAssertThrowsError(try trustedProfile(doc("debug"), doubled))
    }
}
