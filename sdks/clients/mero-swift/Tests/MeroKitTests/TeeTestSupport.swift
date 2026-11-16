import Foundation
import XCTest

@testable import MeroKit

/// The TEE fixtures mero-js tests with, copied verbatim into `Fixtures/tee/`:
/// real mero-tee releases (each `published-mrtds.json`, its cosign bundle and
/// detached `.sig`), a mero-kms policy, and dcap-qvl's sample TDX quote with
/// the Intel-signed collateral for it.
enum TeeFixture {
    static func data(_ name: String) throws -> Data {
        guard let root = Bundle.module.resourceURL else { throw CocoaError(.fileNoSuchFile) }
        return try Data(contentsOf: root.appendingPathComponent("Fixtures/tee/\(name)"))
    }

    static func text(_ name: String) throws -> String {
        String(decoding: try data(name), as: UTF8.self)
    }

    /// `published-mrtds.json`, its bundle and its `.sig` for a release.
    static func release(_ version: String) throws -> TeeReleaseFixture {
        let base = "mero-tee-v\(version).published-mrtds.json"
        return TeeReleaseFixture(
            body: try text(base), bundle: try text("\(base).bundle.json"), signature: try text("\(base).sig"))
    }

    static func signed(_ version: String) throws -> SignedNodeRelease {
        let base = "mero-tee-v\(version).published-mrtds.json"
        return SignedNodeRelease(
            version: version, publishedMrtds: try text(base), bundle: try text("\(base).bundle.json"))
    }

    static var quote: Data { get throws { try data("tdx_quote.bin") } }

    static var collateral: DcapCollateral {
        get throws { try JSONDecoder().decode(DcapCollateral.self, from: try data("tdx_quote_collateral.json")) }
    }

    /// A moment inside the sample collateral's validity window (2025-06-27), in milliseconds.
    static let at: Int64 = 1_751_000_000_000

    // The sample quote's registers and report data, as dcap-qvl 0.5.3 reads them.
    static let mrtd =
        "91eb2b44d141d4ece09f0c75c2c53d247a3c68edd7fafe8a3520c942a604a407de03ae6dc5f87f27428b2538873118b7"
    static let rtmr0 =
        "44c0197b39157fdd7a4dcc44767f9d6b0bb3977c7a8e347b8492f827fe9d9e5c48aca29b220b80b6a540cf994b9bc9c0"
    static let rtmr1 =
        "0084452c01668329d4bc06acdf58a7205c26743304509973949e5619bf81a6a7aea8c323c173019b3093d54e579e9378"
    static let rtmr2 =
        "d833feef2cd945148aa38ead2c53e9b7f138190aaaebfc551dccd829fc207aa3ba80b70870d7330733642e01d48c3132"
    static let rtmr3 = String(repeating: "0", count: 96)
    static let nonce = "9a9d48e7f6799642d3d1b34e1e5e1742d4bb02dd6ddd551862c1211d35c304f9"
    static let suffix = "eca3efdbb481601c163cf52493d6e44aed55d51ec39b7e518fadb92c2b523f20"

    static var image: TrustedMeasurements {
        TrustedMeasurements(mrtd: mrtd, rtmr0: rtmr0, rtmr1: rtmr1, rtmr2: rtmr2, rtmr3: rtmr3)
    }

    /// The sample quote's arguments, with its own collateral.
    static func args(suffix: String = suffix, collateral: DcapCollateral? = nil) throws -> TransportQuoteArgs {
        TransportQuoteArgs(
            quoteB64: try quote.base64EncodedString(), nonce: nonce, reportDataSuffix: suffix,
            collateral: collateral)
    }
}

/// One release's files.
struct TeeReleaseFixture {
    let body: String
    let bundle: String
    let signature: String
}

/// The message of whatever `body` throws, or `nil` when it does not.
func teeThrownMessage(_ body: () throws -> Void) -> String? {
    do {
        try body()
        return nil
    } catch {
        return (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
    }
}

/// As ``teeThrownMessage(_:)``, for an async `body`.
func teeAsyncThrownMessage(_ body: () async throws -> Void) async -> String? {
    do {
        try await body()
        return nil
    } catch {
        return (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
    }
}

/// Assert `message` (a thrown error's) contains `expected`.
func assertTeeRefusal(
    _ message: String?, _ expected: String, file: StaticString = #filePath, line: UInt = #line
) {
    guard let message else { return XCTFail("expected a refusal containing \(expected)", file: file, line: line) }
    XCTAssertTrue(message.contains(expected), "\(message) does not contain \(expected)", file: file, line: line)
}
