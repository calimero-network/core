import Foundation
import XCTest

@testable import MeroKit

/// mero-js `runtime/attestation.test.ts`'s interval form: each refusal core's
/// `verify_interval` makes.
final class IntervalAttestationTests: XCTestCase {
    private let transportKey = Data(repeating: 5, count: 32)
    private let seed = Data(repeating: 7, count: 32)
    private let nonce = Data(repeating: 1, count: 32)
    private let quoteNonce = Data(repeating: 2, count: 32)
    private let nowMs: Int64 = 1_800_000_000_000

    private func doc(
        signedNonce: Data? = nil, boundKey: Data? = nil, age: Int64 = 0, badSignature: Bool = false
    ) throws -> (AttestationDoc, IntervalProof) {
        let key = try Ed25519.publicKey(seed: seed)
        let issuedAt = nowMs / 1000 - age
        let report = quoteNonce + intervalKeyBinding(transportKey, boundKey ?? key, issuedAt: issuedAt)
        let quote = Data(mockQuoteHeader.utf8) + report
        let message = intervalMessage(
            nonce: signedNonce ?? nonce, appHash: Data(count: 32), transportKey: transportKey)
        var signature = try Ed25519.sign(seed: seed, message: message)
        if badSignature { signature[40] ^= 1 }
        let interval = IntervalProof(
            key: Hex.encode(key), quoteNonce: Hex.encode(quoteNonce), issuedAt: issuedAt,
            signature: Hex.encode(signature))
        var d = AttestationDoc(quote: quote.base64EncodedString(), transportKey: Hex.encode(transportKey))
        d.mock = true
        d.interval = interval
        return (d, interval)
    }

    private func check(_ d: (AttestationDoc, IntervalProof)) async throws {
        let now = nowMs
        try await verifyInterval(
            d.0, d.1, nonce: nonce, appHash: Data(count: 32), transportKey: transportKey,
            policy: CloudPolicy(allowMock: true, now: { now }), session: .shared)
    }

    private func refused(_ d: (AttestationDoc, IntervalProof), _ why: String) async {
        do {
            try await check(d)
            XCTFail("verified, expected a refusal: \(why)")
        } catch {
            XCTAssertTrue("\(error)".contains(why), "\(error) names \(why)")
        }
    }

    func testASignedNonceUnderTheBoundKeyVerifies() async throws {
        try await check(try doc())
    }

    func testASignatureOverAnotherNonceIsRefused() async throws {
        await refused(try doc(signedNonce: Data(repeating: 9, count: 32)), "did not sign this nonce")
    }

    func testAStaleQuoteIsRefused() async throws {
        await refused(try doc(age: intervalQuoteMaxAgeSecs + 30), "old, more than")
    }

    func testAQuoteDatedAheadIsRefused() async throws {
        await refused(try doc(age: -600), "ahead of this clock")
    }

    func testAQuoteBindingAnotherKeyIsRefused() async throws {
        await refused(try doc(boundKey: Data(repeating: 3, count: 32)), "does not bind")
    }

    func testABadSignatureIsRefused() async throws {
        await refused(try doc(badSignature: true), "did not sign this nonce")
    }
}
