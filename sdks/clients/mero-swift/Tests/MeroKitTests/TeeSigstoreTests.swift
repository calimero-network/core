import Foundation
import XCTest

@testable import MeroKit

/// Port of mero-js `sealed/release.test.ts` (`verifySignedNodeRelease`, the
/// certificate chain, the embedded trust root) and the `x509.ts` helpers it
/// leans on, over real mero-tee releases.
final class TeeSigstoreTests: XCTestCase {
    private let kmsIdentity =
        "https://github.com/calimero-network/mero-tee/.github/workflows/release-kms.yaml@refs/heads/master"

    /// The 2.3.87 bundle with `edit` applied to its parsed JSON.
    private func edited(_ edit: (inout [String: JSONValue]) -> Void) throws -> String {
        let bundle = try TeeFixture.release("2.3.87").bundle
        var object = try XCTUnwrap(try MeroJSON.decode(JSONValue.self, from: Data(bundle.utf8)).objectValue)
        edit(&object)
        return CanonicalJSON.string(.object(object))
    }

    /// Set `rekorBundle.Payload[field]`.
    private func setPayload(_ bundle: inout [String: JSONValue], _ field: String, _ value: JSONValue) {
        var rekor = bundle["rekorBundle"]?.objectValue ?? [:]
        var payload = rekor["Payload"]?.objectValue ?? [:]
        payload[field] = value
        rekor["Payload"] = .object(payload)
        bundle["rekorBundle"] = .object(rekor)
    }

    /// A base64 value with its decoded byte `index` flipped.
    private func flipBase64(_ value: String, _ index: Int) -> String {
        var bytes = [UInt8](Data(base64Encoded: value) ?? Data())
        bytes[index] ^= 0x01
        return Data(bytes).base64EncodedString()
    }

    private func leafDer(_ bundle: String) throws -> [UInt8] {
        let json = try MeroJSON.decode(JSONValue.self, from: Data(bundle.utf8))
        let pem = String(decoding: Data(base64Encoded: json["cert"]?.stringValue ?? "") ?? Data(), as: UTF8.self)
        return try X509.pemCertificate(pem, "cert")
    }

    private func toPem(_ der: [UInt8]) -> String {
        "-----BEGIN CERTIFICATE-----\n\(Data(der).base64EncodedString())\n-----END CERTIFICATE-----\n"
    }

    private func verify(
        _ body: String, _ bundle: String, expectedVersion: String? = nil, identity: String? = nil,
        issuer: String? = nil, signature: String? = nil
    ) -> String? {
        teeThrownMessage {
            _ = try verifySignedNodeRelease(
                body, bundle: bundle, expectedVersion: expectedVersion, identity: identity, issuer: issuer,
                signature: signature)
        }
    }

    // MARK: - verifySignedNodeRelease

    func testVerifiesRealReleasesFromTextOrBytesWithTheBundleAsTextOrJSON() throws {
        let r86 = try TeeFixture.release("2.3.86")
        let release86 = try verifySignedNodeRelease(r86.body, bundle: r86.bundle, signature: r86.signature)
        XCTAssertEqual(release86.role, "node")
        XCTAssertEqual(release86.tag, "2.3.86")

        let r87 = try TeeFixture.release("2.3.87")
        let bundle = try MeroJSON.decode(JSONValue.self, from: Data(r87.bundle.utf8))
        let release87 = try verifySignedNodeRelease(
            Data(r87.body.utf8), bundle: bundle, expectedVersion: "2.3.87", signature: r87.signature)
        XCTAssertTrue(Set(release87.profiles.keys).isSuperset(of: ["debug", "locked-read-only"]))
        let rtmr3 = try XCTUnwrap(release87.profiles["locked-read-only"]?.rtmr3)
        XCTAssertEqual(rtmr3.count, 96)
    }

    func testRefusesAFileWithOneByteChangedOrAnotherReleasesBundle() throws {
        let tampered = try TeeFixture.text("mero-tee-v2.3.87.tampered.published-mrtds.json")
        let r87 = try TeeFixture.release("2.3.87")
        let r86 = try TeeFixture.release("2.3.86")
        assertTeeRefusal(verify(tampered, r87.bundle), "The file's SHA-256 is not the one Rekor logged")
        assertTeeRefusal(verify(r87.body, r86.bundle), "The file's SHA-256 is not the one Rekor logged")
    }

    func testRefusesASignerOtherThanThePinnedOne() throws {
        let r87 = try TeeFixture.release("2.3.87")
        let node = TeeTrust.nodeReleaseSigner.identity
        assertTeeRefusal(
            verify(r87.body, r87.bundle, identity: kmsIdentity),
            "The release was signed by \"\(node)\", not by \(kmsIdentity)")
        // A genuine file genuinely signed by the KMS workflow is not a node release.
        let kms = try TeeFixture.text("mero-kms-v2.3.87.kms-attestation-policy.json")
        let kmsBundle = try TeeFixture.text("mero-kms-v2.3.87.kms-attestation-policy.json.bundle.json")
        assertTeeRefusal(
            verify(kms, kmsBundle), "The release was signed by \"\(kmsIdentity)\", not by \(node)")
    }

    func testRefusesAnOIDCIssuerOtherThanThePinnedOne() throws {
        let r87 = try TeeFixture.release("2.3.87")
        assertTeeRefusal(
            verify(r87.body, r87.bundle, issuer: "https://accounts.google.com"),
            "The signing certificate's OIDC issuer is \"\(TeeTrust.nodeReleaseSigner.issuer)\", "
                + "not https://accounts.google.com")
    }

    func testRefusesAnAlteredSignedEntryTimestampTimeOrIndex() throws {
        let body = try TeeFixture.release("2.3.87").body
        let altered = try edited { bundle in
            var rekor = bundle["rekorBundle"]?.objectValue ?? [:]
            rekor["SignedEntryTimestamp"] = .string(
                self.flipBase64(rekor["SignedEntryTimestamp"]?.stringValue ?? "", 20))
            bundle["rekorBundle"] = .object(rekor)
        }
        assertTeeRefusal(verify(body, altered), "Rekor's signed entry timestamp does not verify")
        for field in ["integratedTime", "logIndex"] {
            let moved = try edited { bundle in
                let current = bundle["rekorBundle"]?["Payload"]?[field]?.doubleValue ?? 0
                self.setPayload(&bundle, field, .number(current - 1))
            }
            assertTeeRefusal(verify(body, moved), "Rekor's signed entry timestamp does not verify")
        }
    }

    func testRefusesAnEntryFromALogTheTrustRootDoesNotKnow() throws {
        let body = try TeeFixture.release("2.3.87").body
        let unknown = try edited { self.setPayload(&$0, "logID", .string(String(repeating: "ab", count: 32))) }
        assertTeeRefusal(verify(body, unknown), "which the Sigstore trust root does not know")
    }

    func testRefusesAnotherReleasesCertificateOrAByteFlippedInThisOne() throws {
        let r87 = try TeeFixture.release("2.3.87")
        let r86 = try TeeFixture.release("2.3.86")
        let otherCert = try MeroJSON.decode(JSONValue.self, from: Data(r86.bundle.utf8))["cert"] ?? .null
        let swapped = try edited { $0["cert"] = otherCert }
        assertTeeRefusal(verify(r87.body, swapped), "The bundle's certificate is not the one Rekor logged")

        var der = try leafDer(r87.bundle)
        der[der.count - 10] ^= 0x01  // a byte of the issuer's signature
        let pem = toPem(der)
        let flipped = try edited { $0["cert"] = .string(Data(pem.utf8).base64EncodedString()) }
        assertTeeRefusal(verify(r87.body, flipped), "The bundle's certificate is not the one Rekor logged")
    }

    func testRefusesASignatureThatIsNotTheLoggedOneOrTheDetachedOnes() throws {
        let r87 = try TeeFixture.release("2.3.87")
        let r86 = try TeeFixture.release("2.3.86")
        let otherSignature = try MeroJSON.decode(JSONValue.self, from: Data(r86.bundle.utf8))["base64Signature"]
        let other = try edited { $0["base64Signature"] = otherSignature ?? .null }
        assertTeeRefusal(verify(r87.body, other), "The bundle's signature is not the one Rekor logged")
        assertTeeRefusal(
            verify(r87.body, r87.bundle, signature: r86.signature),
            "The detached signature is not the one in the bundle")
    }

    func testRefusesAFileWhoseTagIsNotTheVersionExpected() throws {
        let r87 = try TeeFixture.release("2.3.87")
        assertTeeRefusal(
            verify(r87.body, r87.bundle, expectedVersion: "2.3.86"), "The signed release is 2.3.87, not 2.3.86")
    }

    func testRefusesASignedFileThatIsNotANodeRelease() throws {
        let kms = try TeeFixture.text("mero-kms-v2.3.87.kms-attestation-policy.json")
        let kmsBundle = try TeeFixture.text("mero-kms-v2.3.87.kms-attestation-policy.json.bundle.json")
        assertTeeRefusal(
            verify(kms, kmsBundle, identity: kmsIdentity), "The signed release is a \"kms\" release, not a node one")
        // Signed by the node workflow, with a role and profiles, but no measurements.
        let provenance = try TeeFixture.text("mero-tee-v2.3.87.release-provenance.json")
        let provenanceBundle = try TeeFixture.text("mero-tee-v2.3.87.release-provenance.json.bundle.json")
        assertTeeRefusal(verify(provenance, provenanceBundle), "not a release's published-mrtds.json")
    }

    func testRefusesWhatIsNotABundle() throws {
        let body = try TeeFixture.release("2.3.87").body
        assertTeeRefusal(verify(body, "not json"), "The bundle is not JSON")
        assertTeeRefusal(verify(body, "{}"), "is not a cosign bundle")
        let bigIndex = try edited { self.setPayload(&$0, "logIndex", .number(1_152_921_504_606_846_976)) }
        assertTeeRefusal(verify(body, bigIndex), "is not a cosign bundle")
    }

    // MARK: - The certificate chain

    private func leafAndLoggedTime() throws -> (X509Certificate, Int64) {
        let bundle = try TeeFixture.release("2.3.87").bundle
        let leaf = try X509.parseCertificate(try leafDer(bundle))
        let json = try MeroJSON.decode(JSONValue.self, from: Data(bundle.utf8))
        let integrated = try XCTUnwrap(json["rekorBundle"]?["Payload"]?["integratedTime"]?.doubleValue)
        return (leaf, Int64(integrated) * 1000)
    }

    func testChainsTheRealLeafToFulcioWhenRekorLoggedItAndNotTenMinutesLater() throws {
        let (leaf, logged) = try leafAndLoggedTime()
        XCTAssertNoThrow(try Sigstore.chainsToFulcio(leaf, at: logged))
        assertTeeRefusal(
            teeThrownMessage { try Sigstore.chainsToFulcio(leaf, at: leaf.notAfter + 1000) },
            "not at 2026-09-28T10:29:25.000Z when Rekor logged it")
    }

    func testRefusesALeafWhoseIssuerSignatureDoesNotVerify() throws {
        let (_, logged) = try leafAndLoggedTime()
        var der = try leafDer(try TeeFixture.release("2.3.87").bundle)
        der[der.count - 10] ^= 0x01
        let leaf = try X509.parseCertificate(der)
        assertTeeRefusal(
            teeThrownMessage { try Sigstore.chainsToFulcio(leaf, at: logged) },
            "was not issued by a Fulcio CA in the Sigstore trust root")
    }

    func testNamesTheCAWhenItsTrustRootWindowDoesNotCoverTheTime() throws {
        // The intermediate that issued the leaf, used before the trust root says it was.
        var (leaf, _) = try leafAndLoggedTime()
        leaf.notBefore = 0
        let january2022 = IsoTime.utcMillis(year: 2022, month: 1, day: 1, hour: 0, minute: 0, second: 0)
        assertTeeRefusal(
            teeThrownMessage { try Sigstore.chainsToFulcio(leaf, at: january2022) },
            "the trust root says it was not issuing certificates then")
    }

    func testEmbedsATrustRootWhoseLogIdsAreTheirKeysAndWhoseChainsVerify() throws {
        for log in Sigstore.rekorLogs {
            let key = try X509.fromBase64(log.publicKey, "key")
            XCTAssertEqual(Hex.encode(sha256(Data(key))), log.logId)
        }
        for authority in Sigstore.fulcioAuthorities {
            let chain = try authority.certChain.map { try X509.parseCertificate(try X509.fromBase64($0, "cert")) }
            for (index, certificate) in chain.enumerated() {
                let parent = index + 1 < chain.count ? chain[index + 1] : certificate
                XCTAssertEqual(certificate.curve, .p384)
                XCTAssertEqual(certificate.signatureHash, .sha384)
                XCTAssertTrue(
                    X509.verifyEcdsa(
                        spki: parent.spki, curve: parent.curve, hash: certificate.signatureHash,
                        signature: certificate.signature, data: certificate.tbs))
            }
        }
    }

    // MARK: - DER and PEM

    func testConvertsDERSignaturesToP1363AndRefusesNegativeOrOversizedHalves() throws {
        // r = 0x00 0x80 (a positive 128 with its padding byte), s = 0x01.
        let der: [UInt8] = [0x30, 0x07, 0x02, 0x02, 0x00, 0x80, 0x02, 0x01, 0x01]
        let raw = try X509.ecdsaSignatureToP1363(der, curve: .p256)
        XCTAssertEqual(raw.count, 64)
        XCTAssertEqual(raw[31], 0x80)
        XCTAssertEqual(raw[63], 0x01)
        XCTAssertEqual(raw.filter { $0 != 0 }.count, 2)
        let negative: [UInt8] = [0x30, 0x06, 0x02, 0x01, 0x80, 0x02, 0x01, 0x01]
        XCTAssertThrowsError(try X509.ecdsaSignatureToP1363(negative, curve: .p256))
        let long: [UInt8] = [0x30, 0x26, 0x02, 0x21, 0x01] + [UInt8](repeating: 1, count: 32) + [0x02, 0x01, 0x01]
        XCTAssertThrowsError(try X509.ecdsaSignatureToP1363(long, curve: .p256))
        XCTAssertFalse(X509.verifyEcdsa(spki: [], curve: .p256, hash: .sha256, signature: negative, data: []))
    }

    func testReadsOnlyStrictBase64AndExactlyOnePEMCertificate() throws {
        XCTAssertEqual(try X509.fromBase64("AQID\n", "x"), [1, 2, 3])
        assertTeeRefusal(teeThrownMessage { _ = try X509.fromBase64("AQ-D", "The value") }, "The value is not base64")
        assertTeeRefusal(teeThrownMessage { _ = try X509.fromBase64("AQI", "The value") }, "is not base64")
        let der = try leafDer(try TeeFixture.release("2.3.87").bundle)
        let pem = toPem(der)
        XCTAssertEqual(try X509.pemCertificate(pem, "cert"), der)
        assertTeeRefusal(
            teeThrownMessage { _ = try X509.pemCertificate(pem + pem, "The chain") },
            "The chain is not exactly one PEM certificate")
        assertTeeRefusal(
            teeThrownMessage { _ = try X509.pemCertificate(pem + "trailing", "The file") },
            "is not exactly one PEM certificate")
        XCTAssertEqual(X509.pemCertificates(pem + pem).count, 2)
    }

    func testPrintsTimesAsToISOStringDoes() {
        XCTAssertEqual(IsoTime.string(1_790_591_365_000), "2026-09-28T10:29:25.000Z")
        XCTAssertEqual(IsoTime.parse("2022-12-31T23:59:59.999Z"), 1_672_531_199_999)
        XCTAssertEqual(IsoTime.parse("2021-01-12T11:53:27Z"), 1_610_452_407_000)
        XCTAssertNil(IsoTime.parse("2021-01-12 11:53:27"))
    }
}
