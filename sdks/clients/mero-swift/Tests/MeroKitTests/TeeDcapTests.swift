import Foundation
import MeroKitTestSupport
import XCTest

@testable import MeroKit

/// DCAP verification on the device (port of dcap-qvl's `verify`, which
/// mero-js tests with), and mero-js `sealed/verify.test.ts`:
/// `createQuoteVerifier` and `trustedMeasurementsFromReleases` over
/// dcap-qvl's sample TDX quote and its Intel-signed collateral.
final class TeeDcapTests: XCTestCase {
    private var nowSecs: Int64 { TeeFixture.at / 1000 }

    private func real(_ options: QuoteVerifierOptions? = nil) throws -> QuoteVerifier {
        try createQuoteVerifier(
            options ?? QuoteVerifierOptions(allowedMeasurements: [TeeFixture.image], now: { TeeFixture.at }))
    }

    private func verifierRefusal(_ verifier: QuoteVerifier, _ args: TransportQuoteArgs) async -> String? {
        await teeAsyncThrownMessage { _ = try await verifier.verify(args) }
    }

    // MARK: - Dcap.verify

    func testVerifiesTheSampleQuoteAgainstItsCollateral() throws {
        let report = try Dcap.verify(try TeeFixture.quote, collateral: try TeeFixture.collateral, nowSecs: nowSecs)
        XCTAssertEqual(report.status, "UpToDate")
        XCTAssertEqual(report.advisoryIds, [])
        XCTAssertEqual(report.report.type, "td10")
        let td = try XCTUnwrap(report.report.td10)
        XCTAssertEqual(Hex.encode(td.mrTd), TeeFixture.mrtd)
        XCTAssertEqual(Hex.encode(td.rtMr0), TeeFixture.rtmr0)
        XCTAssertEqual(Hex.encode(td.rtMr1), TeeFixture.rtmr1)
        XCTAssertEqual(Hex.encode(td.rtMr2), TeeFixture.rtmr2)
        XCTAssertEqual(Hex.encode(td.rtMr3), TeeFixture.rtmr3)
        XCTAssertEqual(Hex.encode(td.reportData), TeeFixture.nonce + TeeFixture.suffix)
        XCTAssertEqual(Hex.encode(td.tdAttributes), "0000001000000000")
        XCTAssertEqual(Hex.encode(td.teeTcbSvn), "06010300000000000000000000000000")
    }

    func testRefusesATamperedQuoteExpiredCollateralAndAnUnparseableQuote() throws {
        var tampered = [UInt8](try TeeFixture.quote)
        tampered[200] ^= 0x01  // a byte of the signed TD report body
        let collateral = try TeeFixture.collateral
        assertTeeRefusal(
            teeThrownMessage { _ = try Dcap.verify(Data(tampered), collateral: collateral, nowSecs: self.nowSecs) },
            "ISV enclave report signature is invalid")
        let in2040 = IsoTime.utcMillis(year: 2040, month: 1, day: 1, hour: 0, minute: 0, second: 0) / 1000
        let quote = try TeeFixture.quote
        assertTeeRefusal(
            teeThrownMessage { _ = try Dcap.verify(quote, collateral: collateral, nowSecs: in2040) },
            "TCBInfo expired")
        assertTeeRefusal(
            teeThrownMessage { _ = try Dcap.verify(Data(count: 10), collateral: collateral, nowSecs: self.nowSecs) },
            "Failed to decode quote")
    }

    func testRefusesCollateralWhoseSignaturesDoNotVerify() throws {
        let quote = try TeeFixture.quote
        var collateral = try TeeFixture.collateral
        collateral.tcbInfoSignature[5] ^= 0x01
        assertTeeRefusal(
            teeThrownMessage { _ = try Dcap.verify(quote, collateral: collateral, nowSecs: self.nowSecs) },
            "Signature is invalid for tcb_info")
        collateral = try TeeFixture.collateral
        collateral.qeIdentity += " "
        assertTeeRefusal(
            teeThrownMessage { _ = try Dcap.verify(quote, collateral: collateral, nowSecs: self.nowSecs) },
            "Signature is invalid for qe_identity")
        collateral = try TeeFixture.collateral
        collateral.pckCrl[collateral.pckCrl.count - 3] ^= 0x01
        assertTeeRefusal(
            teeThrownMessage { _ = try Dcap.verify(quote, collateral: collateral, nowSecs: self.nowSecs) },
            "CRL signature is invalid")
    }

    func testReadsTheCollateralAsANodeReturnsItAndWritesItBack() throws {
        let collateral = try TeeFixture.collateral
        XCTAssertTrue(collateral.pckCrlIssuerChain.contains("BEGIN CERTIFICATE"))
        XCTAssertEqual(collateral.tcbInfoSignature.count, 64)
        let json = try MeroJSON.decode(JSONValue.self, from: try JSONEncoder().encode(collateral))
        XCTAssertEqual(try DcapCollateral(json: json), collateral)
        // Byte fields as arrays decode too.
        var object = try XCTUnwrap(json.objectValue)
        object["tcb_info_signature"] = .array(collateral.tcbInfoSignature.map { .number(Double($0)) })
        XCTAssertEqual(try DcapCollateral(json: .object(object)), collateral)
    }

    // MARK: - createQuoteVerifier on the real quote

    func testAcceptsAGenuineQuoteFromATrustedImageThatCommitsToTheBinding() async throws {
        let verifier = try real()
        XCTAssertTrue(verifier.includeCollateral)
        let accepted = try await verifier.verify(try TeeFixture.args(collateral: try TeeFixture.collateral))
        XCTAssertTrue(accepted)
    }

    func testRefusesAQuoteThatCommitsToAnotherBinding() async throws {
        let collateral = try TeeFixture.collateral
        let args = try TeeFixture.args(suffix: String(repeating: "00", count: 32), collateral: collateral)
        assertTeeRefusal(await verifierRefusal(try real(), args), "does not commit to this nonce and binding")
    }

    func testRefusesAnImageItDoesNotTrustAndNeverCombinesImages() async throws {
        let args = try TeeFixture.args(collateral: try TeeFixture.collateral)
        let other = String(repeating: "ab", count: 48)
        var untrusted = TeeFixture.image
        untrusted.rtmr3 = other
        let refusing = try real(QuoteVerifierOptions(allowedMeasurements: [untrusted], now: { TeeFixture.at }))
        assertTeeRefusal(await verifierRefusal(refusing, args), "are not an image this verifier trusts")

        // Each of the sample's registers is trusted, but each by a different image.
        var a = TeeFixture.image
        a.rtmr1 = other
        var b = TeeFixture.image
        b.rtmr2 = other
        var c = TeeFixture.image
        c.rtmr3 = other
        let split = try real(QuoteVerifierOptions(allowedMeasurements: [a, b, c], now: { TeeFixture.at }))
        assertTeeRefusal(await verifierRefusal(split, args), "are not an image this verifier trusts")
        let either = try real(
            QuoteVerifierOptions(allowedMeasurements: [a, TeeFixture.image], now: { TeeFixture.at }))
        let accepted = try await either.verify(args)
        XCTAssertTrue(accepted)
    }

    func testAcceptsMRTDAllowlistsOnlyTogetherWithTheImageRegisters() async throws {
        let args = try TeeFixture.args(collateral: try TeeFixture.collateral)
        let lists = QuoteVerifierOptions(
            allowedMrtd: [TeeFixture.mrtd], allowedRtmr1: [TeeFixture.rtmr1], allowedRtmr2: [TeeFixture.rtmr2],
            allowedRtmr3: [TeeFixture.rtmr3], now: { TeeFixture.at })
        let accepted = try await real(lists).verify(args)
        XCTAssertTrue(accepted)
        var wrong = lists
        wrong.allowedMrtd = [String(repeating: "ab", count: 48)]
        assertTeeRefusal(await verifierRefusal(try real(wrong), args), "MRTD")
    }

    func testChecksAnRTMROnlyWhenAskedTo() async throws {
        let args = try TeeFixture.args(collateral: try TeeFixture.collateral)
        let checked = try real(
            QuoteVerifierOptions(
                allowedMeasurements: [TeeFixture.image], allowedRtmr0: [TeeFixture.rtmr0], now: { TeeFixture.at }))
        let accepted = try await checked.verify(args)
        XCTAssertTrue(accepted)
        let refusing = try real(
            QuoteVerifierOptions(
                allowedMeasurements: [TeeFixture.image], allowedRtmr0: [String(repeating: "cd", count: 48)],
                now: { TeeFixture.at }))
        assertTeeRefusal(await verifierRefusal(refusing, args), "RTMR0")
    }

    func testRefusesATamperedQuoteAndCollateralOutsideItsWindow() async throws {
        var tampered = [UInt8](try TeeFixture.quote)
        tampered[200] ^= 0x01
        var args = try TeeFixture.args(collateral: try TeeFixture.collateral)
        args.quoteB64 = Data(tampered).base64EncodedString()
        assertTeeRefusal(await verifierRefusal(try real(), args), "did not verify")

        let in2040 = IsoTime.utcMillis(year: 2040, month: 1, day: 1, hour: 0, minute: 0, second: 0)
        let late = try real(QuoteVerifierOptions(allowedMeasurements: [TeeFixture.image], now: { in2040 }))
        let fresh = try TeeFixture.args(collateral: try TeeFixture.collateral)
        assertTeeRefusal(await verifierRefusal(late, fresh), "did not verify")
    }

    func testGetsCollateralItselfWhenTheNodeSendsNoneAndRefusesWithoutAWayTo() async throws {
        let collateral = try TeeFixture.collateral
        let quote = try TeeFixture.quote
        let asked = Counter()
        let fetching = try real(
            QuoteVerifierOptions(
                allowedMeasurements: [TeeFixture.image],
                fetchCollateral: { got in
                    XCTAssertEqual(got, quote)
                    asked.increment()
                    return collateral
                }, now: { TeeFixture.at }))
        let args = try TeeFixture.args()
        let accepted = try await fetching.verify(args)
        XCTAssertTrue(accepted)
        XCTAssertEqual(asked.count, 1)
        assertTeeRefusal(await verifierRefusal(try real(), args), "no collateral")
    }

    // MARK: - createQuoteVerifier policy

    private func sample() throws -> TdReport10 {
        let report = try Dcap.verify(try TeeFixture.quote, collateral: try TeeFixture.collateral, nowSecs: nowSecs)
        return try XCTUnwrap(report.report.td10)
    }

    private func fixed(_ status: String, _ report: DcapReport) -> DcapVerify {
        { _, _, _ in DcapVerifiedReport(status: status, report: report) }
    }

    func testAcceptsOnlyUpToDatePlatformsByDefaultAndNeverARevokedOne() async throws {
        let td = try sample()
        let args = try TeeFixture.args(collateral: try TeeFixture.collateral)
        let outOfDate = try createQuoteVerifier(
            QuoteVerifierOptions(dcapVerify: fixed("OutOfDate", .td10(td)), allowedMeasurements: [TeeFixture.image]))
        assertTeeRefusal(await verifierRefusal(outOfDate, args), "TCB status is OutOfDate")
        let allowed = try createQuoteVerifier(
            QuoteVerifierOptions(
                dcapVerify: fixed("OutOfDate", .td10(td)), allowedMeasurements: [TeeFixture.image],
                allowedTcbStatuses: ["UpToDate", "OutOfDate"]))
        let accepted = try await allowed.verify(args)
        XCTAssertTrue(accepted)
        let revoked = try createQuoteVerifier(
            QuoteVerifierOptions(
                dcapVerify: fixed("Revoked", .td10(td)), allowedMeasurements: [TeeFixture.image],
                allowedTcbStatuses: ["Revoked"]))
        assertTeeRefusal(await verifierRefusal(revoked, args), "Revoked")
    }

    // Whatever dcapVerify was given: a debug TD's memory and registers are
    // open to the host, so its measurements prove nothing about what runs in it.
    func testRefusesADebugTDOrOneWhoseAttributesItCannotReadWhateverDcapVerifyAllows() async throws {
        let args = try TeeFixture.args(collateral: try TeeFixture.collateral)
        let td = try sample()
        func verifier(_ attributes: Data, td15: Bool = false) throws -> QuoteVerifier {
            var report = td
            report.tdAttributes = attributes
            let wrapped: DcapReport =
                td15
                ? .td15(TdReport15(base: report, teeTcbSvn2: Data(count: 16), mrServiceTd: Data(count: 48)))
                : .td10(report)
            return try createQuoteVerifier(
                QuoteVerifierOptions(dcapVerify: fixed("UpToDate", wrapped), allowedMeasurements: [TeeFixture.image]))
        }
        var debug = Data(count: 8)
        debug[0] = 0x01
        assertTeeRefusal(await verifierRefusal(try verifier(debug), args), "debug TD")
        assertTeeRefusal(await verifierRefusal(try verifier(debug, td15: true), args), "debug TD")
        assertTeeRefusal(await verifierRefusal(try verifier(Data()), args), "debug TD")
        assertTeeRefusal(await verifierRefusal(try verifier(Data(count: 4)), args), "debug TD")
        let accepted = try await verifier(Data(count: 8)).verify(args)
        XCTAssertTrue(accepted)
    }

    func testRefusesAQuoteThatIsNotTDXAndPassesTheTimeInSeconds() async throws {
        let args = try TeeFixture.args(collateral: try TeeFixture.collateral)
        let enclave = SgxEnclaveReport(
            cpuSvn: Data(count: 16), miscSelect: 0, attributes: Data(count: 16), mrEnclave: Data(count: 32),
            mrSigner: Data(count: 32), isvProdId: 0, isvSvn: 0, reportData: Data(count: 64))
        let sgx = try createQuoteVerifier(
            QuoteVerifierOptions(
                dcapVerify: fixed("UpToDate", .sgx(enclave)), allowedMeasurements: [TeeFixture.image]))
        assertTeeRefusal(await verifierRefusal(sgx, args), "not a TDX one")

        let td = try sample()
        let seen = TeeSecondsBox()
        let timed = try createQuoteVerifier(
            QuoteVerifierOptions(
                dcapVerify: { quote, collateral, now in
                    seen.record(now)
                    XCTAssertEqual(quote, try TeeFixture.quote)
                    XCTAssertEqual(collateral, try TeeFixture.collateral)
                    return DcapVerifiedReport(status: "UpToDate", report: .td10(td))
                }, allowedMeasurements: [TeeFixture.image], now: { 1_234_567_890 }))
        _ = try await timed.verify(args)
        XCTAssertEqual(seen.value, 1_234_567)
    }

    func testWillNotBeBuiltToAcceptNothingOrWithAMeasurementThatIsNotOne() {
        assertTeeRefusal(
            teeThrownMessage { _ = try createQuoteVerifier(QuoteVerifierOptions()) }, "No image is trusted")
        assertTeeRefusal(
            teeThrownMessage { _ = try createQuoteVerifier(QuoteVerifierOptions(allowedMeasurements: [])) },
            "allowedMeasurements is empty")
        var short = TeeFixture.image
        short.rtmr2 = "aabb"
        assertTeeRefusal(
            teeThrownMessage { _ = try createQuoteVerifier(QuoteVerifierOptions(allowedMeasurements: [short])) },
            "allowedMeasurements[0].rtmr2")
        assertTeeRefusal(
            teeThrownMessage {
                _ = try createQuoteVerifier(
                    QuoteVerifierOptions(allowedMeasurements: [TeeFixture.image], allowedRtmr2: ["zz"]))
            }, "allowedRtmr2")
    }

    func testWillNotTrustAnMRTDAloneWhichNamesTheFirmwareAndNotTheImage() {
        assertTeeRefusal(
            teeThrownMessage { _ = try createQuoteVerifier(QuoteVerifierOptions(allowedMrtd: [TeeFixture.mrtd])) },
            "does not name an image")
        assertTeeRefusal(
            teeThrownMessage {
                _ = try createQuoteVerifier(
                    QuoteVerifierOptions(
                        allowedMrtd: [TeeFixture.mrtd], allowedRtmr1: [TeeFixture.rtmr1],
                        allowedRtmr2: [TeeFixture.rtmr2]))
            }, "does not name an image")
    }

    // MARK: - trustedMeasurementsFromReleases

    private func release(
        _ tag: String, _ rtmr3: String, _ statuses: [String] = ["uptodate", "outofdate"]
    ) -> PublishedMrtds {
        let image = TeeFixture.image
        return PublishedMrtds(
            role: "node", tag: tag,
            profiles: [
                "locked-read-only": PublishedProfile(
                    mrtd: image.mrtd, rtmr0: image.rtmr0, rtmr1: image.rtmr1, rtmr2: image.rtmr2, rtmr3: rtmr3,
                    allowedTcbStatuses: statuses),
                "debug": PublishedProfile(
                    mrtd: image.mrtd, rtmr0: image.rtmr0, rtmr1: image.rtmr1, rtmr2: image.rtmr2,
                    rtmr3: String(repeating: "dd", count: 48), allowedTcbStatuses: statuses),
            ])
    }

    func testTrustsEachReleasesImageOfOneProfileByAllOfItsMeasurements() throws {
        let aa = String(repeating: "aa", count: 48)
        let options = try trustedMeasurementsFromReleases(
            [release("2.3.76", aa), release("2.3.78", TeeFixture.rtmr3)], profile: "locked-read-only")
        var first = TeeFixture.image
        first.rtmr3 = aa
        XCTAssertEqual(options.allowedMeasurements, [first, TeeFixture.image])
        XCTAssertEqual(options.allowedTcbStatuses, ["UpToDate", "OutOfDate"])
    }

    func testBuildsAVerifierThatAcceptsTheReleaseImageAndNotAnotherProfileOfIt() async throws {
        let args = try TeeFixture.args(collateral: try TeeFixture.collateral)
        let locked = try trustedMeasurementsFromReleases(
            [release("2.3.78", TeeFixture.rtmr3)], profile: "locked-read-only")
        let trusting = try createQuoteVerifier(QuoteVerifierOptions(trusting: locked, now: { TeeFixture.at }))
        let accepted = try await trusting.verify(args)
        XCTAssertTrue(accepted)
        let debug = try trustedMeasurementsFromReleases(
            [release("2.3.78", TeeFixture.rtmr3)], profile: "debug", allowDebugProfile: true)
        let refusing = try createQuoteVerifier(QuoteVerifierOptions(trusting: debug, now: { TeeFixture.at }))
        assertTeeRefusal(await verifierRefusal(refusing, args), "are not an image this verifier trusts")
    }

    func testTrustsOnlyTheProductionProfileUnlessDebugProfilesAreAskedFor() throws {
        var debugRelease = release("2.3.78", TeeFixture.rtmr3)
        let image = TeeFixture.image
        debugRelease.profiles["debug-read-only"] = PublishedProfile(
            mrtd: image.mrtd, rtmr0: image.rtmr0, rtmr1: image.rtmr1, rtmr2: image.rtmr2,
            rtmr3: String(repeating: "ee", count: 48), allowedTcbStatuses: ["uptodate"])
        for profile in ["debug", "debug-read-only"] {
            assertTeeRefusal(
                teeThrownMessage { _ = try trustedMeasurementsFromReleases([debugRelease], profile: profile) },
                "\"\(profile)\" is not a production profile")
            let allowed = try trustedMeasurementsFromReleases(
                [debugRelease], profile: profile, allowDebugProfile: true)
            XCTAssertEqual(allowed.allowedMeasurements.count, 1)
        }
        let locked = try trustedMeasurementsFromReleases([debugRelease], profile: TeeTrust.productionProfile)
        XCTAssertEqual(locked.allowedMeasurements, [TeeFixture.image])
    }

    func testAcceptsOnlyTheTCBStatusesEveryReleaseAccepts() throws {
        let options = try trustedMeasurementsFromReleases(
            [release("2.3.76", TeeFixture.rtmr3, ["uptodate"]), release("2.3.78", TeeFixture.rtmr3)],
            profile: "locked-read-only")
        XCTAssertEqual(options.allowedTcbStatuses, ["UpToDate"])
    }

    func testRefusesWhatItCannotReadAsANodeRelease() {
        let profile = "locked-read-only"
        assertTeeRefusal(
            teeThrownMessage { _ = try trustedMeasurementsFromReleases([], profile: profile) }, "No release")
        let one = release("2.3.78", TeeFixture.rtmr3)
        assertTeeRefusal(
            teeThrownMessage { _ = try trustedMeasurementsFromReleases([one], profile: "prod") },
            "has no profile \"prod\"")
        var kms = release("2.3.78", TeeFixture.rtmr3)
        kms.role = "kms"
        assertTeeRefusal(
            teeThrownMessage { _ = try trustedMeasurementsFromReleases([kms], profile: profile) },
            "not of a node image")
        let unknown = release("2.3.78", TeeFixture.rtmr3, ["sometimes"])
        assertTeeRefusal(
            teeThrownMessage { _ = try trustedMeasurementsFromReleases([unknown], profile: profile) }, "does not know")
        let disjoint = [
            release("2.3.76", TeeFixture.rtmr3, ["uptodate"]), release("2.3.78", TeeFixture.rtmr3, ["outofdate"]),
        ]
        assertTeeRefusal(
            teeThrownMessage { _ = try trustedMeasurementsFromReleases(disjoint, profile: profile) },
            "no TCB status in common")
    }
}

/// The seconds a `dcapVerify` was handed, for an assertion after the call.
final class TeeSecondsBox: @unchecked Sendable {
    private let lock = NSLock()
    private var seconds: Int64?

    func record(_ value: Int64) {
        lock.lock()
        seconds = value
        lock.unlock()
    }

    var value: Int64? {
        lock.lock()
        defer { lock.unlock() }
        return seconds
    }
}
