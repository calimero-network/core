import Foundation
import MeroKitTestSupport
import XCTest

@testable import MeroKit

/// Port of mero-js `sealed/release.test.ts` (`trustSignedRelease`, fetching
/// a release, the release a node names, `createSignedReleaseVerifier`,
/// `createSignedReleaseSealedFetch`).
final class TeeNodeReleaseTests: XCTestCase {
    private var rig: CloudRig!
    private let releasedImage = "merotee-ubuntu-questing-25-10-locked-read-only-2-3-87"

    override func setUp() {
        super.setUp()
        rig = CloudRig()
        rig.install()
    }

    override func tearDown() {
        MockURLProtocol.reset()
        SealedURLProtocol.unregisterAll()
        rig = nil
        super.tearDown()
    }

    private func json(_ value: JSONValue) -> String { CanonicalJSON.string(value) }

    private func signedJSON(_ release: SignedNodeRelease) -> JSONValue {
        [
            "version": .string(release.version), "publishedMrtds": .string(release.publishedMrtds),
            "bundle": .string(release.bundle),
        ]
    }

    // MARK: - trustSignedRelease

    func testTrustsOneProfileOfASignedReleaseAtOrAboveTheMinimum() throws {
        let trusted = try trustSignedRelease(
            TrustSignedReleaseOptions(
                release: try TeeFixture.signed("2.3.87"), profile: "locked-read-only", minReleaseVersion: "2.3.86"))
        let published = try MeroJSON.decode(
            PublishedMrtds.self, from: Data(try TeeFixture.release("2.3.87").body.utf8))
        let image = try XCTUnwrap(published.profiles["locked-read-only"])
        XCTAssertEqual(
            trusted.allowedMeasurements,
            [
                TrustedMeasurements(
                    mrtd: image.mrtd, rtmr0: image.rtmr0, rtmr1: image.rtmr1, rtmr2: image.rtmr2, rtmr3: image.rtmr3)
            ])
        XCTAssertTrue(trusted.allowedTcbStatuses.contains("UpToDate"))
        let release = try TeeFixture.signed("2.3.87")
        assertTeeRefusal(
            teeThrownMessage {
                _ = try trustSignedRelease(
                    TrustSignedReleaseOptions(release: release, profile: "debug", minReleaseVersion: "2.3.87"))
            }, "not a production profile")
        XCTAssertNoThrow(
            try trustSignedRelease(
                TrustSignedReleaseOptions(
                    release: release, profile: "debug", minReleaseVersion: "2.3.87", allowDebugProfile: true)))
    }

    func testRefusesAReleaseOlderThanTheMinimumComparedAsVersions() throws {
        let r86 = try TeeFixture.signed("2.3.86")
        let r87 = try TeeFixture.signed("2.3.87")
        let trust = { (release: SignedNodeRelease, minimum: String) in
            try trustSignedRelease(
                TrustSignedReleaseOptions(release: release, profile: "locked-read-only", minReleaseVersion: minimum))
        }
        assertTeeRefusal(
            teeThrownMessage { _ = try trust(r86, "2.3.87") },
            "Release 2.3.86 is older than the minimum trusted, 2.3.87")
        assertTeeRefusal(teeThrownMessage { _ = try trust(r87, "2.3.100") }, "older than the minimum")
        XCTAssertNoThrow(try trust(r87, "2.3.87-rc.1"))
        XCTAssertNoThrow(try trust(r87, "2.3.9"))
    }

    func testRequiresAMinimumThatIsAVersion() throws {
        let release = try TeeFixture.signed("2.3.87")
        for junk in ["", "latest", "2.3", "2.3.x", "v2.3.87", "2.3.87-", "../2.3.87"] {
            assertTeeRefusal(
                teeThrownMessage {
                    _ = try trustSignedRelease(
                        TrustSignedReleaseOptions(
                            release: release, profile: "locked-read-only", minReleaseVersion: junk))
                }, "minReleaseVersion is")
        }
    }

    func testOrdersReleaseVersionsAsSemverDoes() {
        XCTAssertEqual(ReleaseVersion.compare("2.3.9", "2.3.10"), -1)
        XCTAssertEqual(ReleaseVersion.compare("2.3.87", "2.3.87-rc.1"), 1)
        XCTAssertEqual(ReleaseVersion.compare("2.3.87-rc.2", "2.3.87-rc.10"), -1)
        XCTAssertEqual(ReleaseVersion.compare("2.3.87-alpha", "2.3.87-1"), 1)
        XCTAssertEqual(ReleaseVersion.compare("2.3.87-rc", "2.3.87-rc.1"), -1)
        XCTAssertEqual(ReleaseVersion.compare("2.3.87+build.1", "2.3.87"), 0)
    }

    func testRefusesAReleaseThatClaimsAVersionOtherThanItsFiles() throws {
        var release = try TeeFixture.signed("2.3.87")
        release.version = "2.3.88"
        let wrong = release
        assertTeeRefusal(
            teeThrownMessage {
                _ = try trustSignedRelease(
                    TrustSignedReleaseOptions(
                        release: wrong, profile: "locked-read-only", minReleaseVersion: "2.3.86"))
            }, "The signed release is 2.3.87, not 2.3.88")
        release.version = "latest"
        let latest = release
        assertTeeRefusal(
            teeThrownMessage {
                _ = try trustSignedRelease(
                    TrustSignedReleaseOptions(
                        release: latest, profile: "locked-read-only", minReleaseVersion: "2.3.86"))
            }, "The release version is \"latest\"")
    }

    func testRefusesAReleaseWhoseFileIsNotTheOneSignedOrWhoseProfileIsMissing() throws {
        var release = try TeeFixture.signed("2.3.87")
        release.publishedMrtds = try TeeFixture.text("mero-tee-v2.3.87.tampered.published-mrtds.json")
        let tampered = release
        assertTeeRefusal(
            teeThrownMessage {
                _ = try trustSignedRelease(
                    TrustSignedReleaseOptions(
                        release: tampered, profile: "locked-read-only", minReleaseVersion: "2.3.86"))
            }, "The file's SHA-256 is not the one Rekor logged")
        let genuine = try TeeFixture.signed("2.3.87")
        assertTeeRefusal(
            teeThrownMessage {
                _ = try trustSignedRelease(
                    TrustSignedReleaseOptions(release: genuine, profile: "prod", minReleaseVersion: "2.3.86"))
            }, "has no profile \"prod\"")
    }

    // MARK: - Fetching a release

    func testUnwrapsDataFromAMirrorWhichIsOnlyFetched() async throws {
        let signed = try TeeFixture.signed("2.3.87")
        rig.on("GET", "/api/tee/node-releases/2.3.87", json: json(["data": signedJSON(signed)]))
        let url = try cloudNodeReleaseUrl("https://cloud.example/", version: "2.3.87")
        XCTAssertEqual(url, "https://cloud.example/api/tee/node-releases/2.3.87")
        let fetched = try await fetchNodeRelease(url, session: MockURLProtocol.makeSession())
        XCTAssertEqual(fetched, signed)
        let accept = rig.requests("GET", "/api/tee/node-releases/2.3.87").first?.headers["accept"]
        XCTAssertEqual(accept, "application/json")
        assertTeeRefusal(
            teeThrownMessage { _ = try cloudNodeReleaseUrl("https://cloud.example", version: "../admin") },
            "not a release version")
        XCTAssertEqual(TeeTrust.defaultReleaseMirror, "https://cloud.calimero.network")
    }

    func testRefusesWhatIsNotARelease() async throws {
        let url = "https://cloud.example/api/tee/node-releases/2.3.87"
        let session = MockURLProtocol.makeSession()
        rig.on("GET", "/api/tee/node-releases/2.3.87", json: #"{"data":{}}"#)
        let empty = await teeAsyncThrownMessage { _ = try await fetchNodeRelease(url, session: session) }
        assertTeeRefusal(empty, "did not answer with a signed node release")
        rig.on("GET", "/api/tee/node-releases/2.3.87", status: 404, json: "{}")
        let missing = await teeAsyncThrownMessage { _ = try await fetchNodeRelease(url, session: session) }
        assertTeeRefusal(missing, "HTTP 404")
    }

    // MARK: - The release an image names

    func testReadsItFromTheImageNameOfAReleasedImage() {
        XCTAssertEqual(try nodeReleaseVersionFromImage(releasedImage), "2.3.87")
        let debugImage = "merotee-ubuntu-questing-25-10-debug-read-only-2-3-100"
        XCTAssertEqual(try nodeReleaseVersionFromImage(debugImage), "2.3.100")
        for name in ["unknown", "ubuntu-2404-tdx-v20250115", "\(releasedImage)-d412-1", "\(releasedImage)x"] {
            assertTeeRefusal(
                teeThrownMessage { _ = try nodeReleaseVersionFromImage(name) },
                "not the image of a mero-tee node release")
        }
    }

    func testFetchesAReleaseFromGithubAsItsTwoFiles() async throws {
        let signed = try TeeFixture.signed("2.3.87")
        let base = "/calimero-network/mero-tee/releases/download/mero-tee-v2.3.87/published-mrtds.json"
        rig.on("GET", base) { _ in .init(status: 200, headers: [:], body: Data(signed.publishedMrtds.utf8)) }
        rig.on("GET", base + ".bundle.json") { _ in .init(status: 200, headers: [:], body: Data(signed.bundle.utf8)) }
        let fetched = try await fetchGithubNodeRelease("2.3.87", session: MockURLProtocol.makeSession())
        XCTAssertEqual(fetched, signed)
    }

    // MARK: - createSignedReleaseVerifier

    func testVerifiesTheQuoteAgainstTheSignedReleaseAndRefusesAnImageNotOfIt() async throws {
        let asked = Counter()
        let signed = try TeeFixture.signed("2.3.87")
        let verifier = try createSignedReleaseVerifier(
            SignedReleaseVerifierOptions(
                release: {
                    asked.increment()
                    return signed
                }, profile: "locked-read-only", minReleaseVersion: "2.3.86", now: { TeeFixture.at }))
        XCTAssertTrue(verifier.includeCollateral)
        let args = try TeeFixture.args(collateral: try TeeFixture.collateral)
        let refusal = await teeAsyncThrownMessage { _ = try await verifier.verify(args) }
        assertTeeRefusal(refusal, "are not an image this verifier trusts")
        XCTAssertEqual(asked.count, 1)
    }

    func testRefusesBeforeTheQuoteWhenTheReleaseDoesNotPass() async throws {
        let args = try TeeFixture.args(collateral: try TeeFixture.collateral)
        let r86 = try TeeFixture.signed("2.3.86")
        let old = try createSignedReleaseVerifier(
            SignedReleaseVerifierOptions(
                release: { r86 }, profile: "locked-read-only", minReleaseVersion: "2.3.87", now: { TeeFixture.at }))
        let oldRefusal = await teeAsyncThrownMessage { _ = try await old.verify(args) }
        assertTeeRefusal(oldRefusal, "older than the minimum")

        var tampered = try TeeFixture.signed("2.3.87")
        tampered.publishedMrtds = try TeeFixture.text("mero-tee-v2.3.87.tampered.published-mrtds.json")
        let altered = tampered
        let changed = try createSignedReleaseVerifier(
            SignedReleaseVerifierOptions(
                release: { altered }, profile: "locked-read-only", minReleaseVersion: "2.3.86", now: { TeeFixture.at }))
        let changedRefusal = await teeAsyncThrownMessage { _ = try await changed.verify(args) }
        assertTeeRefusal(changedRefusal, "SHA-256")

        let r87 = try TeeFixture.signed("2.3.87")
        assertTeeRefusal(
            teeThrownMessage {
                _ = try createSignedReleaseVerifier(
                    SignedReleaseVerifierOptions(
                        release: { r87 }, profile: "locked-read-only", minReleaseVersion: "latest"))
            }, "minReleaseVersion")
    }
}
