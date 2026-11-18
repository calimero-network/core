import CryptoKit
import Foundation
import MeroKitTestSupport
import XCTest

@testable import MeroKit

/// Pipeline receipts as core's `calimero-client` checks them, over core's own
/// vectors (`Fixtures/receipts/receipt-vectors.json`, a copy of core's
/// `crates/client/tests/fixtures/receipt-vectors.json` that
/// `scripts/refresh-receipt-vectors.sh` makes). Port of mero-js
/// `client/receipts.test.ts`.
final class ReceiptTests: XCTestCase {
    private var v: JSONValue = .null
    private let mock = CloudPolicy(allowMock: true)
    private let app = "app-1"
    private let run = "run-1"

    override func setUpWithError() throws {
        v = try MeroJSON.decode(JSONValue.self, from: try fixture("receipts/receipt-vectors.json"))
    }

    override func tearDown() {
        MockURLProtocol.reset()
        super.tearDown()
    }

    private var cluster: String { v["cluster"]?.stringValue ?? "" }
    private var governance: JSONValue { v["governance"] ?? .null }
    private var nonce: Data { hexBytes(v["nonce"]?.stringValue ?? "") }
    private var jobAnswer: JSONValue { v["jobReceipt"]?["answer"] ?? .null }
    private var goodReceipt: JSONValue { jobAnswer["receipt"] ?? .null }
    private var jobKey: String { v["jobKeyBindings"]?.arrayValue?.first?["jobKey"]?.stringValue ?? "" }
    private func keyOf(_ node: Int) -> Data { hexBytes(v["nodes"]?["\(node)"]?.stringValue ?? "") }

    // MARK: - Helpers

    static func mockQuote(_ nonce: Data, _ suffix: Data) -> String {
        (Data(mockQuoteHeader.utf8) + nonce + suffix).base64EncodedString()
    }

    /// One replica of the stand-in cluster.
    struct Replica {
        var node: UInt64
        var docNode: UInt64?
        var bindCluster: String?
        var bindNode: UInt64?
        var bindGovernance: JSONValue?
        var governance: JSONValue?
        var error: String?
        var unbound = false
    }

    /// A stand-in for `GET /apps/{app}/attestation`, quoting over the nonce it is asked with.
    private func attestation(
        _ replicas: [Replica], cluster entry: String? = nil, nonce asked: String
    ) throws
        -> JSONValue
    {
        let entry = entry ?? cluster
        var out: [JSONValue] = []
        for r in replicas {
            if let error = r.error {
                out.append(["node": .number(Double(r.node)), "error": .string(error)])
                continue
            }
            let key = keyOf(Int(r.node))
            let g = r.governance ?? governance
            let suffix =
                r.unbound
                ? sha256(Data("calimero.tee-attest.receipt-key.v2".utf8) + key + sha256(try governanceCanonical(g)))
                : try receiptKeyBinding(
                    key, governance: r.bindGovernance ?? g, cluster: r.bindCluster ?? entry, node: r.bindNode ?? r.node)
            out.append([
                "node": .number(Double(r.node)),
                "attestation": [
                    "quote": .string(Self.mockQuote(hexBytes(asked), suffix)),
                    "transportKey": .string(String(repeating: "00", count: 32)),
                    "appHash": .string(String(repeating: "00", count: 32)), "nonce": .string(asked),
                    "release": ["version": "2.3.109", "profile": "locked-read-only"],
                    "cluster": .string(r.bindCluster ?? entry), "node": .number(Double(r.docNode ?? r.node)),
                    "mock": true, "receiptKey": .string(Hex.encode(key)), "governance": g,
                ],
            ])
        }
        return ["appId": .string(app), "cluster": .string(entry), "replicas": .array(out)]
    }

    private func attest(
        _ replicas: [Replica], policy: CloudPolicy? = nil, app: String? = nil,
        patch: @escaping @Sendable (inout JSONValue) -> Void = { _ in }
    ) async throws -> AttestedCluster {
        let answer = { [self] (asked: String) throws -> JSONValue in
            var a = try attestation(replicas, nonce: asked)
            patch(&a)
            return a
        }
        let fixed = UnsafeSendable(answer)
        return try await attestAppCluster(app ?? self.app, policy: policy ?? mock, session: .shared) { asked in
            try fixed.value(asked)
        }
    }

    private func check(
        _ answer: JSONValue, app: String? = nil, run: String? = nil, policy: CloudPolicy? = nil,
        replicas: [Replica] = [Replica(node: 1), Replica(node: 2)]
    ) async throws -> PipelineReceipt {
        let attested = try await attest(replicas, policy: policy, app: app)
        return try await checkPipelineReceipt(
            attested, app: app ?? self.app, run: run ?? self.run, answer: answer, policy: policy ?? mock,
            session: .shared)
    }

    /// An answer for `receipt`, signed by node 1's key (seed 0x0b), as the node would.
    private func signed(_ receipt: JSONValue, node: JSONValue = 1) throws -> JSONValue {
        let sig = try Ed25519.sign(seed: Data(repeating: 11, count: 32), message: receiptMessage(receipt))
        return ["receipt": receipt, "node": node, "signature": .string(Hex.encode(sig))]
    }

    /// The error `body` throws, as its message, failing when it succeeds.
    private func refusal(
        _ body: () async throws -> Any, file: StaticString = #filePath, line: UInt = #line
    ) async -> String {
        do {
            _ = try await body()
            XCTFail("expected a refusal", file: file, line: line)
            return ""
        } catch let error as ReceiptError {
            return error.message
        } catch let error as TeeTrustError {
            return error.message
        } catch {
            XCTFail("not a receipt error: \(error)", file: file, line: line)
            return "\(error)"
        }
    }

    private func refuses(
        _ body: () async throws -> Any, _ needle: String, file: StaticString = #filePath, line: UInt = #line
    ) async {
        let message = await refusal(body, file: file, line: line)
        XCTAssertTrue(message.contains(needle), "\(message) does not contain \(needle)", file: file, line: line)
    }

    /// Refuses the receipt `r`, signed again by node 1.
    private func refusedAs(
        _ r: JSONValue, _ needle: String, run: String? = nil, policy: CloudPolicy? = nil,
        file: StaticString = #filePath, line: UInt = #line
    ) async throws {
        let answer = try signed(r)
        await refuses({ try await self.check(answer, run: run, policy: policy) }, needle, file: file, line: line)
    }

    private func accepts(_ r: JSONValue, file: StaticString = #filePath, line: UInt = #line) async throws {
        let receipt = try await check(try signed(r))
        XCTAssertEqual(receipt.run, run, file: file, line: line)
    }

    private static let load: [JSONPathItem] = ["steps", "load", "runs", 0]
    private static let merge: [JSONPathItem] = ["steps", "merge", "runs", 0]

    /// Re-signs the report of the run at `path` with the job key (seed 0x41) over the fields it now has.
    private func rereport(_ r: inout JSONValue, _ path: [JSONPathItem], reason: String?, seed: UInt8 = 0x41) throws {
        let p = r[path] ?? .null
        let job = p["job"]
        let outputs = (p["outputs"]?.arrayValue ?? []).enumerated().map { i, o in
            JobOutput(
                id: o["id"]?.stringValue ?? "", size: UInt64(o["size"]?.doubleValue ?? 0),
                sha256: o["sha256"]?.stringValue ?? "",
                segments: job?["segments"]?.arrayValue?[i].arrayValue?.compactMap { $0.stringValue } ?? [])
        }
        let log: JobOutput? = try job?["log"].flatMap {
            $0 == .null ? nil : try MeroJSON.convert($0, to: JobOutput.self)
        }
        let message = try reportMessage(
            ReportFields(
                assignment: p["assignmentDigest"]?.stringValue ?? "", app: r["app"]?.stringValue ?? "",
                run: p["run"]?.stringValue ?? "", attempt: UInt32(p["attempt"]?.doubleValue ?? 0), reason: reason,
                message: job?["message"]?.stringValue ?? "", resultSha256: job?["resultSha256"]?.stringValue ?? "",
                outputs: outputs,
                usage: ["cpuMicros", "gibMicros", "fuel"].map { p["usage"]?[$0]?.stringValue ?? "" }, log: log))
        r[path + ["reportSha256"]] = .string(Hex.encode(sha256(message)))
        r[path + ["signature"]] = .string(
            Hex.encode(try Ed25519.sign(seed: Data(repeating: seed, count: 32), message: message)))
    }

    // MARK: - Bindings

    func testReceiptKeyBindingMatchesEveryVectorCoreProduced() throws {
        for b in v["bindings"]?.arrayValue ?? [] {
            let g = b["governance"] ?? .null
            XCTAssertEqual(
                String(decoding: try governanceCanonical(g), as: UTF8.self), b["governanceCanonical"]?.stringValue)
            let binding = try receiptKeyBinding(
                hexBytes(b["receiptKey"]?.stringValue ?? ""), governance: g, cluster: b["cluster"]?.stringValue ?? "",
                node: UInt64(b["node"]?.doubleValue ?? 0))
            XCTAssertEqual(Hex.encode(binding), b["binding"]?.stringValue)
        }
        XCTAssertEqual(v["bindings"]?.arrayValue?.count, 12)
    }

    func testGovernanceCanonicalDropsDefaultsAndKeepsFieldOrder() throws {
        let noisy: JSONValue = [
            "signUp": "open", "outbound": "owner", "deployers": "operator", "releases": "owner",
            "deployments": "open", "appUpgrades": "owner",
        ]
        XCTAssertEqual(
            String(decoding: try governanceCanonical(noisy), as: UTF8.self),
            #"{"releases":"owner","appUpgrades":"owner","outbound":"owner"}"#)
        var bad = noisy
        bad[["releases"]] = "nobody"
        XCTAssertThrowsError(try governanceCanonical(bad)) {
            XCTAssertTrue("\($0)".contains("releases"))
        }
    }

    // MARK: - verifyReceiptKey

    func testVerifyReceiptKeyAcceptsTheDocumentCoreAccepts() async throws {
        let got = try await verifyReceiptKey(
            v["mockDoc"] ?? .null, nonce: nonce, cluster: cluster, node: 1, policy: mock)
        XCTAssertEqual(Hex.encode(got.key), v["nodes"]?["1"]?.stringValue)
        XCTAssertEqual(got.governance, try MeroJSON.convert(governance, to: [String: String].self))
    }

    func testVerifyReceiptKeyRefusesWhatDoesNotBind() async throws {
        let doc = v["mockDoc"] ?? .null
        let (n, c, p) = (nonce, cluster, mock)
        await refuses(
            { try await verifyReceiptKey(self.v["unboundMockDoc"] ?? .null, nonce: n, cluster: c, node: 1, policy: p) },
            "does not bind")
        await refuses({ try await verifyReceiptKey(doc, nonce: n, cluster: c, node: 2, policy: p) }, "does not bind")
        await refuses(
            { try await verifyReceiptKey(doc, nonce: n, cluster: "other", node: 1, policy: p) }, "does not bind")
        await refuses(
            { try await verifyReceiptKey(doc, nonce: n, cluster: c, node: 1, policy: CloudPolicy()) },
            "mock quote refused")
        var noKey = doc
        noKey[["receiptKey"]] = nil
        await refuses({ try await verifyReceiptKey(noKey, nonce: n, cluster: c, node: 1, policy: p) }, "no receipt key")
        var noGovernance = doc
        noGovernance[["governance"]] = nil
        await refuses(
            { try await verifyReceiptKey(noGovernance, nonce: n, cluster: c, node: 1, policy: p) }, "no governance")
    }

    // MARK: - verifyAppCluster

    func testAttestsEachReplicaKeyAndTheSharedGovernance() async throws {
        let got = try await attest([Replica(node: 1), Replica(node: 2)])
        XCTAssertEqual(got.keys, [1: keyOf(1), 2: keyOf(2)])
        XCTAssertEqual(got.governance, try MeroJSON.convert(governance, to: [String: String].self))
        XCTAssertEqual(got.cluster, cluster)
    }

    func testRefusesReplicasThatDoNotCheckOut() async throws {
        await refuses({ try await self.attest([Replica(node: 1, bindNode: 2)]) }, "replica 1")
        await refuses({ try await self.attest([Replica(node: 1, bindCluster: "other")]) }, "replica 1")
        await refuses({ try await self.attest([Replica(node: 1, docNode: 2)]) }, "answered as node 2")
        var other = governance
        other[["releases"]] = "owner"
        await refuses(
            { try await self.attest([Replica(node: 1), Replica(node: 2, governance: other)]) },
            "replica 2 attests other governance")
        await refuses(
            { try await self.attest([Replica(node: 1), Replica(node: 2, error: "unreachable")]) },
            "replica 2 gave no attestation: unreachable")
        await refuses({ try await self.attest([Replica(node: 1, unbound: true)]) }, "replica 1")
        await refuses({ try await self.attest([Replica(node: 1)], policy: CloudPolicy()) }, "mock quote refused")
        await refuses({ try await self.attest([]) }, "no replica of app-1 attested")
    }

    func testReadsNodeIdsAndHexAsCoreDoes() async throws {
        await refuses(
            { try await self.attest([Replica(node: 1)]) { $0[["replicas", 0, "node"]] = "1" } }, "not a node id")
        await refuses(
            { try await self.attest([Replica(node: 1)]) { $0[["replicas", 0, "attestation", "node"]] = "1" } },
            "not a node id")
        await refuses(
            { try await self.attest([Replica(node: 1)]) { $0[["replicas", 0, "node"]] = .number(pow(2, 53)) } },
            "not a node id")
        await refuses(
            {
                try await self.attest([Replica(node: 1)]) {
                    let key = $0[["replicas", 0, "attestation", "receiptKey"]]?.stringValue ?? ""
                    $0[["replicas", 0, "attestation", "receiptKey"]] = .string(" \(key)")
                }
            }, "receipt key")
        let got = try await attest([Replica(node: 1)]) { $0[["replicas", 0, "attestation", "governance", "evil"]] = 1 }
        XCTAssertEqual(got.governance, try MeroJSON.convert(governance, to: [String: String].self))
    }

    func testRefusesMalformedAttestations() async throws {
        let p = mock
        await refuses(
            {
                try await attestAppCluster("a", policy: p, session: .shared) { _ in ["cluster": "c", "replicas": "x"] }
            },
            "malformed attestation")
        await refuses(
            {
                try await attestAppCluster("a", policy: p, session: .shared) { _ in
                    ["cluster": "c", "replicas": [["node": 1]]]
                }
            }, "neither an attestation nor an error")
        await refuses(
            {
                try await attestAppCluster("a", policy: p, session: .shared) { _ in ["cluster": "c", "replicas": [nil]]
                }
            },
            "not an object")
    }

    // MARK: - verifyPipelineReceipt

    func testAcceptsTheAnswerCoreSignedAndAnswersTheReceipt() async throws {
        let receipt = try await check(jobAnswer)
        XCTAssertEqual(receipt.version, 2)
        XCTAssertEqual(receipt.app, app)
        XCTAssertEqual(
            try MeroJSON.convert(receipt, to: JSONValue.self),
            try MeroJSON.decode(JSONValue.self, from: Data(ReceiptJCS.string(goodReceipt).utf8)))
    }

    func testRefusesTheVersion1ReceiptCoreAlsoSignsThoughItsSignatureHolds() async throws {
        let v1 = v["receiptAnswer"] ?? .null
        await refuses({ try await self.check(v1) }, "the receipt is version 1")
        XCTAssertEqual(try verifyReceiptSignature(v1, key: keyOf(1)).version, 1)
    }

    func testRefusesWhatTheAttestedNodeDidNotSign() async throws {
        var other = jobAnswer
        other[["node"]] = 2
        await refuses({ try await self.check(other) }, "not signed by that key")
        await refuses({ try await self.check(self.jobAnswer, replicas: [Replica(node: 2)]) }, "not an attested replica")
        var changed = jobAnswer
        changed[["receipt", "state"]] = "succeedeD"
        await refuses({ try await self.check(changed) }, "not signed by that key")
        var short = jobAnswer
        short[["signature"]] = "abcd"
        await refuses({ try await self.check(short) }, "signature")
        await refuses(
            { try await self.check(self.jobAnswer, app: "app-2", replicas: [Replica(node: 1)]) },
            "is for app app-1, not app-2")
        await refuses({ try await self.check(self.jobAnswer, run: "run-2") }, "is for run run-1, not run-2")
        await refuses(
            { try await self.check(self.jobAnswer, replicas: [Replica(node: 1, unbound: true)]) }, "replica 1")
    }

    func testAcceptsParamsHoldingFloatsAndIntegersPast2To53AsCoreCanonicalizesThem() throws {
        let floats = try MeroJSON.decode(JSONValue.self, from: Data((v["floatAnswerText"]?.stringValue ?? "").utf8))
        let f = try verifyReceiptSignature(floats, key: keyOf(1))
        XCTAssertEqual(f.params["a"], 0.1)
        XCTAssertEqual(f.params["b"], 1e21)
        XCTAssertEqual(f.params["d"], .number(5e-324))
        XCTAssertEqual(f.params["e"], 1.5)
        let big = try MeroJSON.decode(JSONValue.self, from: Data((v["bigNumberAnswerText"]?.stringValue ?? "").utf8))
        let b = try verifyReceiptSignature(big, key: keyOf(1))
        XCTAssertEqual(b.params["a"], .number(9_007_199_254_740_992))
        XCTAssertEqual(b.params["b"], .number(5e18))
        XCTAssertEqual(b.params["c"], .number(-9_223_372_036_854_775_808))
        XCTAssertEqual(b.params["d"], .number(1e19))
    }

    func testWritesNumbersAsECMAScriptDoes() {
        let cases: [(Double, String)] = [
            (0.1, "0.1"), (1e21, "1e+21"), (-0.0, "0"), (5e-324, "5e-324"), (1.5, "1.5"),
            (1.7976931348623157e308, "1.7976931348623157e+308"), (1e-7, "1e-7"), (123_456_789.125, "123456789.125"),
            (100, "100"), (1e22, "1e+22"), (9_007_199_254_740_992, "9007199254740992"), (5e18, "5000000000000000000"),
            (1e20, "100000000000000000000"), (0.000001, "0.000001"), (-12.5, "-12.5"), (123e-20, "1.23e-18"),
        ]
        for (n, text) in cases { XCTAssertEqual(ReceiptJCS.number(n), text, "\(n)") }
        XCTAssertEqual(ReceiptJCS.string(["b": 1, "a": [true, nil, "x\n"]]), #"{"a":[true,null,"x\n"],"b":1}"#)
    }

    // MARK: - Schema, node ids, hex

    func testKeepsToCoresSchemaEvenUnderAValidSignature() async throws {
        var nullCost = goodReceipt
        nullCost[["costSoFar"]] = .null
        try await accepts(nullCost)
        let mutations: [Edit] = [
            Edit("unknown field", { $0[["extra"]] = 1 }, "receipt.extra is not a known field"),
            Edit("missing field", { $0[["pipeline"]] = nil }, "receipt.pipeline is missing"),
            Edit("wrong type", { $0[["state"]] = 5 }, "receipt.state is not a string"),
            Edit(
                "negative size", { $0[["inputs", "data", 0, "size"]] = -1 },
                "inputs.data[0].size is not an unsigned integer"
            ),
            Edit("unknown nested", { $0[["steps", "load", "extra"]] = 1 }, "steps.load.extra is not a known field"),
            Edit("wrong nested type", { $0[["steps", "load", "runs", 0, "index"]] = "0" }, "steps.load.runs[0]"),
            Edit("neither run", { $0[["steps", "load", "runs", 1]] = ["index": 1] }, "steps.load.runs[1]"),
            Edit("params not an object", { $0[["params"]] = [] }, "receipt.params is not an object"),
            Edit("u64 of 2^64", { $0[["startTsMs"]] = .number(pow(2, 64)) }, "unsigned integer in range"),
        ]
        for item in mutations {
            let (name, mutate, needle) = (item.name, item.apply, item.needle)
            var r = goodReceipt
            mutate(&r)
            let message = await refusal({ try await self.check(try self.signed(r)) })
            XCTAssertTrue(message.contains(needle), "\(name): \(message)")
        }
    }

    func testRefusesAnAnsweringNodeThatIsNotANodeId() async throws {
        for node: JSONValue in ["1", true, [1], -1, .number(pow(2, 53)), 1.5, .null] {
            var answer = jobAnswer
            answer[["node"]] = node
            await refuses({ try await self.check(answer) }, "not a node id")
        }
        var missing = jobAnswer
        missing[["node"]] = nil
        await refuses({ try await self.check(missing) }, "not a node id")
    }

    func testReadsHexAsCoreDoes() async throws {
        var upper = jobAnswer
        upper[["signature"]] = .string((jobAnswer["signature"]?.stringValue ?? "").uppercased())
        let upperRun = try await check(upper).run
        XCTAssertEqual(upperRun, run)
        var padded = jobAnswer
        padded[["signature"]] = .string(" \(jobAnswer["signature"]?.stringValue ?? "")\n")
        await refuses({ try await self.check(padded) }, "signature")
    }

    func testMalformedInputIsAReceiptError() async throws {
        var extra = jobAnswer
        extra[["extra"]] = 1
        await refuses({ try verifyReceiptSignature(extra, key: self.keyOf(1)) }, "extra is not a known field")
        var badSignature = jobAnswer
        badSignature[["signature"]] = 5
        var noReceipt = jobAnswer
        noReceipt[["receipt"]] = nil
        var stringReceipt = jobAnswer
        stringReceipt[["receipt"]] = "x"
        for answer in [JSONValue.null, badSignature, noReceipt, stringReceipt] {
            _ = await refusal({ try await self.check(answer) })
        }
    }

    // MARK: - Job reports

    func testRebuildsEveryReportMessageAndJobKeyBindingCoreComputes() throws {
        let messages = v["reportMessages"]?.arrayValue ?? []
        XCTAssertEqual(messages.count, 5)
        for m in messages {
            let fields = try MeroJSON.convert(m, to: ReportFields.self)
            let message = try reportMessage(fields)
            XCTAssertEqual(Hex.encode(message), m["messageHex"]?.stringValue)
            XCTAssertEqual(Hex.encode(sha256(message)), m["sha256"]?.stringValue)
        }
        let bindings = v["jobKeyBindings"]?.arrayValue ?? []
        XCTAssertEqual(bindings.count, 7)
        for b in bindings {
            let identity = b["clusterIdentity"]?.stringValue.map(hexBytes)
            let got = jobKeyBinding(
                transportKey: hexBytes(b["transportKey"]?.stringValue ?? ""),
                jobKey: hexBytes(b["jobKey"]?.stringValue ?? ""),
                release: b["release"]?.stringValue ?? "", cluster: b["cluster"]?.stringValue ?? "",
                node: UInt64(b["node"]?.doubleValue ?? 0), clusterIdentity: identity)
            XCTAssertEqual(Hex.encode(got), b["binding"]?.stringValue)
        }
    }

    func testRefusesANumberOrAHashItCannotWrite() throws {
        let good = try MeroJSON.convert(v["reportMessages"]?.arrayValue?[0] ?? .null, to: ReportFields.self)
        for usage in [["18446744073709551616", "0", "0"], ["+1", "0", "0"], ["1", "0", ""]] {
            var f = good
            f.usage = usage
            XCTAssertThrowsError(try reportMessage(f)) { XCTAssertTrue("\($0)".contains("not a decimal integer")) }
        }
        var upper = good
        upper.assignment = upper.assignment.uppercased()
        XCTAssertThrowsError(try reportMessage(upper)) { XCTAssertTrue("\($0)".contains("lowercase hex")) }
    }

    func testAJobSignedReceiptNamesItsOneJobKeyAndNode() throws {
        let receipt = try verifyReceiptSignature(jobAnswer, key: keyOf(1))
        XCTAssertEqual(try verifyJobReports(receipt), [jobKey: 4])
    }

    func testVerifiesReportsWithoutALogFailuresWithAMessageAndCancels() async throws {
        var r = goodReceipt
        r[Self.load + ["job", "log"]] = .null
        try rereport(&r, Self.load, reason: nil)
        try await accepts(r)
        r[Self.load + ["outcome"]] = "failed:app-error"
        r[Self.load + ["outputs"]] = []
        r[Self.load + ["job", "message"]] = "invalid input: row 7"
        r[Self.load + ["job", "segments"]] = []
        try rereport(&r, Self.load, reason: "app-error")
        try await accepts(r)
        for outcome: JSONValue in ["failed:cancelled", "cancelled"] {
            var c = goodReceipt
            c[Self.load + ["outcome"]] = outcome
            c[Self.load + ["outputs"]] = []
            c[Self.load + ["job", "segments"]] = []
            c[Self.load + ["job", "log"]] = .null
            try rereport(&c, Self.load, reason: "cancelled")
            try await accepts(c)
            if outcome == "cancelled" {
                try rereport(&c, Self.load, reason: nil)
                try await refusedAs(c, "the report hash is not that of what the proof says")
            }
        }
    }

    func testRefusesAnythingTheJobDidNotSign() async throws {
        let edits: [(String, (inout JSONValue) -> Void)] = [
            ("signature", { $0[Self.load + ["signature"]] = .string(String(repeating: "06", count: 64)) }),
            ("job key", { $0[Self.load + ["jobKey"]] = .string(String(repeating: "08", count: 32)) }),
            ("assignment", { $0[Self.load + ["assignmentDigest"]] = .string(String(repeating: "45", count: 32)) }),
            ("attempt", { $0[Self.load + ["attempt"]] = 3 }),
            ("outcome", { $0[Self.load + ["outcome"]] = "failed:trap" }),
            ("output id", { $0[Self.load + ["outputs", 0, "id"]] = .string(String(repeating: "11", count: 32)) }),
            ("output size", { $0[Self.load + ["outputs", 0, "size"]] = 2049 }),
            (
                "output hash",
                { $0[Self.load + ["outputs", 1, "sha256"]] = .string(String(repeating: "99", count: 32)) }
            ),
            (
                "output order",
                { $0[Self.load + ["outputs"]] = .array(($0[Self.load + ["outputs"]]?.arrayValue ?? []).reversed()) }
            ),
            (
                "dropped output",
                {
                    $0[Self.load + ["outputs"]] = .array(($0[Self.load + ["outputs"]]?.arrayValue ?? []).dropLast())
                    $0[Self.load + ["job", "segments"]] = .array(
                        ($0[Self.load + ["job", "segments"]]?.arrayValue ?? []).dropLast())
                }
            ),
            (
                "segment count",
                {
                    $0[Self.load + ["job", "segments"]] = .array(
                        ($0[Self.load + ["job", "segments"]]?.arrayValue ?? []).dropLast())
                }
            ),
            (
                "extra segment list",
                {
                    $0[Self.load + ["job", "segments"]] = .array(
                        ($0[Self.load + ["job", "segments"]]?.arrayValue ?? []) + [[]])
                }
            ),
            ("segment", { $0[Self.load + ["job", "segments", 0, 0]] = .string(String(repeating: "77", count: 32)) }),
            ("result", { $0[Self.load + ["job", "resultSha256"]] = .string(String(repeating: "34", count: 32)) }),
            ("message", { $0[Self.load + ["job", "message"]] = "bang" }),
            ("log", { $0[Self.load + ["job", "log"]] = .null }),
            ("log id", { $0[Self.load + ["job", "log", "id"]] = .string(String(repeating: "12", count: 32)) }),
            ("usage", { $0[Self.load + ["usage", "fuel"]] = "3501" }),
            ("report hash", { $0[Self.load + ["reportSha256"]] = .string(String(repeating: "00", count: 32)) }),
        ]
        for (name, edit) in edits {
            var r = goodReceipt
            edit(&r)
            let message = await refusal({ try await self.check(try self.signed(r)) })
            XCTAssertTrue(
                message.contains("run-1.load.0") || message.contains("job evidence") || message.contains("segments"),
                "\(name): \(message)")
        }
        var noJob = goodReceipt
        noJob[Self.load + ["job"]] = nil
        try await refusedAs(noJob, "has no job evidence")
        for field in ["assignmentDigest", "jobKey"] {
            var upper = goodReceipt
            upper[Self.load + [JSONPathItem.key(field)]] = .string(
                (upper[Self.load + [JSONPathItem.key(field)]]?.stringValue ?? "").uppercased())
            try await refusedAs(upper, "lowercase hex")
        }
        var upperResult = goodReceipt
        upperResult[Self.load + ["job", "resultSha256"]] = .string(
            (upperResult[Self.load + ["job", "resultSha256"]]?.stringValue ?? "").uppercased())
        try await refusedAs(upperResult, "lowercase hex")
    }

    func testRefusesASmallOrderROrKeyOrAMalleatedS() throws {
        let points = [
            "01" + String(repeating: "00", count: 31), "ec" + String(repeating: "ff", count: 30) + "7f",
            String(repeating: "00", count: 32), String(repeating: "00", count: 31) + "80",
            "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac037a",
            "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac03fa",
            "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc05",
            "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc85",
        ]
        for point in points { XCTAssertTrue(Ed25519Strict.isSmallOrder(hexBytes(point)), point) }
        XCTAssertFalse(Ed25519Strict.isSmallOrder(keyOf(1)))
        // S + L, the same signature malleated: refused though the equation still holds.
        let message = Data("m".utf8)
        let seed = Data(repeating: 0x41, count: 32)
        let sig = try Ed25519.sign(seed: seed, message: message)
        let key = try Ed25519.publicKey(seed: seed)
        XCTAssertTrue(Ed25519Strict.verify(publicKey: key, message: message, signature: sig))
        let order: [UInt8] = [
            0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10,
        ]
        var s = Array(sig.suffix(32))
        var carry = 0
        for i in 0..<32 {
            let sum = Int(s[i]) + Int(order[i]) + carry
            s[i] = UInt8(sum & 0xff)
            carry = sum >> 8
        }
        XCTAssertFalse(Ed25519Strict.verify(publicKey: key, message: message, signature: sig.prefix(32) + Data(s)))
        XCTAssertFalse(
            Ed25519Strict.verify(
                publicKey: hexBytes(points[0]), message: message, signature: hexBytes(points[0]) + Data(count: 32)))
    }

    func testRefusesASignatureByASmallOrderKey() async throws {
        var r = goodReceipt
        let identity = "01" + String(repeating: "00", count: 31)
        r[Self.load + ["jobKey"]] = .string(identity)
        try rereport(&r, Self.load, reason: nil)
        r[Self.load + ["signature"]] = .string(identity + String(repeating: "00", count: 32))
        r[["jobAttestations", .key(identity)]] = r[["jobAttestations", .key(jobKey)]]
        r[["jobAttestations", .key(identity), "jobKey"]] = .string(identity)
        r[["jobAttestations", .key(jobKey)]] = nil
        try await refusedAs(r, "the job key did not sign")
    }

    func testBindsAProofToItsRunStepAndIndexListedOnceInOrder() async throws {
        await refuses(
            { try await self.check(try self.signed(self.goodReceipt), run: "run-2") }, "is for run run-1, not run-2")
        var other = goodReceipt
        other[["run"]] = "run-2"
        try await refusedAs(other, "run-1.load.0 is listed as run-2.load.0", run: "run-2")
        var step = goodReceipt
        step[["steps", "other"]] = step[["steps", "load"]]
        step[["steps", "load"]] = nil
        try await refusedAs(step, "run-1.load.0 is listed as run-1.other.0")
        var index = goodReceipt
        index[Self.load + ["index"]] = 1
        try await refusedAs(index, "lists run 1 where run 0 belongs")
        var gap = goodReceipt
        gap[["steps", "load", "runs", 1, "index"]] = 2
        try await refusedAs(gap, "lists run 2 where run 1 belongs")
        var twice = goodReceipt
        twice[["steps", "load", "runs", 1, "index"]] = 0
        try await refusedAs(twice, "lists run 0 where run 1 belongs")
        var notCached = goodReceipt
        notCached[["steps", "load", "runs", 1, "cached"]] = false
        try await refusedAs(notCached, "run 1 is listed as not cached")
        var v1 = goodReceipt
        v1[["version"]] = 1
        try await refusedAs(v1, "the receipt is version 1")
    }

    func testRefusesOneJobKeyNamedOnTwoNodes() async throws {
        var r = goodReceipt
        var second = r[Self.load] ?? .null
        second[["run"]] = "run-1.load.1"
        second[["index"]] = 1
        second[["node"]] = 5
        r[["steps", "load", "runs", 1]] = second
        try rereport(&r, ["steps", "load", "runs", 1], reason: nil)
        try await refusedAs(r, "job key \(jobKey) is named on two nodes")
        r[["steps", "load", "runs", 1, "node"]] = 4
        try await accepts(r)
    }

    func testARunTheLeaderEndedNamesNoReportOutputsOrUsage() async throws {
        for outcome in [
            "failed:lost", "failed:refused", "failed:timeout", "failed:quota", "failed:not-granted", "failed:too-large",
            "cancelled",
        ] {
            var r = goodReceipt
            r[Self.merge + ["outcome"]] = .string(outcome)
            try await accepts(r)
        }
        let loadJob = goodReceipt[Self.load + ["job"]] ?? .null
        let edits: [Edit] = [
            Edit(
                "outputs",
                {
                    $0[Self.merge + ["outputs"]] = [
                        [
                            "id": .string(String(repeating: "10", count: 32)), "size": 1,
                            "sha256": .string(String(repeating: "11", count: 32)),
                        ]
                    ]
                }, "by the leader"
            ),
            Edit("usage", { $0[Self.merge + ["usage", "fuel"]] = "5" }, "by the leader"),
            Edit("usage spelled 00", { $0[Self.merge + ["usage", "cpuMicros"]] = "00" }, "by the leader"),
            Edit(
                "a report hash", { $0[Self.merge + ["reportSha256"]] = .string(String(repeating: "05", count: 32)) },
                "by the leader"
            ),
            Edit(
                "a signature", { $0[Self.merge + ["signature"]] = .string(String(repeating: "06", count: 64)) },
                "by the leader"
            ),
            Edit(
                "job evidence",
                {
                    var job = loadJob
                    job[["segments"]] = []
                    $0[Self.merge + ["job"]] = job
                }, "by the leader"
            ),
            Edit("done without a report", { $0[Self.merge + ["outcome"]] = "done" }, "has no assignment digest"),
            Edit(
                "a job failure without its report", { $0[Self.merge + ["outcome"]] = "failed:fuel" },
                "has no assignment digest"
            ),
            Edit(
                "a reason no run ends with", { $0[Self.merge + ["outcome"]] = "failed:nonsense" },
                "is not one a run ends with"
            ),
            Edit("an unknown outcome", { $0[Self.merge + ["outcome"]] = "exploded" }, "is not one a run ends with"),
        ]
        for item in edits {
            let (name, edit, needle) = (item.name, item.apply, item.needle)
            var r = goodReceipt
            edit(&r)
            let message = await refusal({ try await self.check(try self.signed(r)) })
            XCTAssertTrue(message.contains(needle), "\(name): \(message)")
        }
    }

    func testEachJobKeyCarriesItsJobNodeAttestation() async throws {
        let k = JSONPathItem.key(jobKey)
        let edits: [Edit] = [
            Edit("missing", { $0[["jobAttestations"]] = nil }, "carries no attestation for it"),
            Edit(
                "another key", { $0[["jobAttestations", k, "jobKey"]] = .string(String(repeating: "08", count: 32)) },
                "attestation is another key's"
            ),
            Edit("no key", { $0[["jobAttestations", k, "jobKey"]] = nil }, "attestation is another key's"),
            Edit("another node", { $0[["jobAttestations", k, "node"]] = 5 }, "not node 4 of eu-shared-1"),
            Edit(
                "another cluster", { $0[["jobAttestations", k, "cluster"]] = "eu-other" }, "not node 4 of eu-shared-1"),
            Edit(
                "another identity",
                { $0[["jobAttestations", k, "clusterIdentity"]] = .string(String(repeating: "22", count: 32)) },
                "another cluster identity"
            ),
            Edit("no identity", { $0[["jobAttestations", k, "clusterIdentity"]] = nil }, "another cluster identity"),
            Edit(
                "receipt of another identity", { $0[["cluster"]] = .string(String(repeating: "22", count: 32)) },
                "another cluster identity"
            ),
            Edit("bad nonce", { $0[["jobAttestations", k, "nonce"]] = "zz" }, "nonce is not 32 bytes of hex"),
        ]
        for item in edits {
            let (name, edit, needle) = (item.name, item.apply, item.needle)
            var r = goodReceipt
            edit(&r)
            let message = await refusal({ try await self.check(try self.signed(r)) })
            XCTAssertTrue(message.hasPrefix("job key \(jobKey): ") && message.contains(needle), "\(name): \(message)")
        }
    }

    func testRefusesAJobQuoteThatBindsAnotherKeyNodeOrIdentity() async throws {
        let k = JSONPathItem.key(jobKey)
        func rebind(_ r: inout JSONValue, key: String? = nil, node: UInt64? = nil, identity: String? = nil) {
            let d = r[["jobAttestations", k]] ?? .null
            let suffix = jobKeyBinding(
                transportKey: hexBytes(d["transportKey"]?.stringValue ?? ""), jobKey: hexBytes(key ?? jobKey),
                release: d["release"]?["version"]?.stringValue ?? "", cluster: d["cluster"]?.stringValue ?? "",
                node: node ?? UInt64(d["node"]?.doubleValue ?? 0),
                clusterIdentity: hexBytes(identity ?? d["clusterIdentity"]?.stringValue ?? ""))
            r[["jobAttestations", k, "quote"]] = .string(
                Self.mockQuote(hexBytes(d["nonce"]?.stringValue ?? ""), suffix))
        }
        for over in 0..<3 {
            var r = goodReceipt
            switch over {
            case 0: rebind(&r, key: String(repeating: "08", count: 32))
            case 1: rebind(&r, node: 5)
            default: rebind(&r, identity: String(repeating: "22", count: 32))
            }
            let message = await refusal({ try await self.check(try self.signed(r)) })
            XCTAssertTrue(message.hasPrefix("job key ") && message.contains("does not bind"), message)
        }
        var ok = goodReceipt
        rebind(&ok)
        try await accepts(ok)
        await refuses(
            {
                let attested = try await self.attest([Replica(node: 1)])
                return try await checkPipelineReceipt(
                    attested, app: self.app, run: self.run, answer: self.jobAnswer, policy: CloudPolicy(),
                    session: .shared)
            }, "mock quote refused")
    }

    func testTheAttestationKeepsToCoresSchema() async throws {
        let k = JSONPathItem.key(jobKey)
        var noTransport = goodReceipt
        noTransport[["jobAttestations", k, "transportKey"]] = nil
        try await refusedAs(noTransport, "transportKey is missing")
        var extra = goodReceipt
        extra[["jobAttestations", k, "future"]] = 1
        try await accepts(extra)
        var bad = goodReceipt
        bad[["jobAttestations"]] = .null
        try await refusedAs(bad, "jobAttestations is not an object")
        for field in ["governance", "kms"] {
            var r = goodReceipt
            r[["jobAttestations", k, .key(field)]] = 5
            try await refusedAs(r, "\(field) is not an object")
        }
    }

    // MARK: - Through the client

    func testTheClientVerifiesAgainstTheAttestationRouteUnderItsOwnPolicy() async throws {
        let replicas = [Replica(node: 1), Replica(node: 2)]
        let make = UnsafeSendable { [self] (asked: String) in try attestation(replicas, nonce: asked) }
        let answer = jobAnswer
        let node = FakeCloudNode { request in
            if request.path.hasPrefix("/apps/app-1/attestation?nonce=") {
                let asked = String(request.path.split(separator: "=").last ?? "")
                return (try? make.value(asked)).map { .json($0) } ?? .error(500, "BAD")
            }
            if request.path == "/apps/app-1/pipeline-runs/run-1/receipt?version=2" { return .json(answer) }
            return .error(404, "NOT_FOUND")
        }
        node.install()
        func client(_ policy: CloudPolicy) -> CalimeroClient {
            CalimeroClient(
                CalimeroClientOptions(
                    baseUrl: "https://node.test", policy: policy, session: MockURLProtocol.makeSession(),
                    retry: CloudRetryOptions(retries: 0)), sleep: { _ in })
        }
        let cloud = client(mock)
        let attested = try await cloud.verifyAppCluster(app)
        XCTAssertEqual(Set(attested.keys.keys), [1, 2])
        let fetched = try await cloud.pipelineReceipt(app, run)
        XCTAssertEqual(fetched, answer)
        let verified = try await cloud.verifyPipelineReceipt(app, run, fetched)
        XCTAssertEqual(verified.run, run)
        // The channel itself refuses a mock node to a client that does not allow one.
        do {
            _ = try await client(CloudPolicy()).verifyAppCluster(app)
            XCTFail("expected a refusal")
        } catch {
            XCTAssertTrue("\(error)".contains("mock"), "\(error)")
        }
    }
}

/// One edit of a receipt, and what refusing it says.
struct Edit {
    let name: String
    let apply: (inout JSONValue) -> Void
    let needle: String

    init(_ name: String, _ apply: @escaping (inout JSONValue) -> Void, _ needle: String) {
        self.name = name
        self.apply = apply
        self.needle = needle
    }
}

/// A closure the tests know is only read.
struct UnsafeSendable<T>: @unchecked Sendable {
    let value: T
    init(_ value: T) { self.value = value }
}

/// One step of a path into a ``JSONValue``.
enum JSONPathItem: ExpressibleByStringLiteral, ExpressibleByIntegerLiteral {
    case key(String)
    case index(Int)

    init(stringLiteral value: String) { self = .key(value) }
    init(integerLiteral value: Int) { self = .index(value) }
}

extension JSONValue {
    /// The value at `path`; setting `nil` removes an object's field.
    subscript(path: [JSONPathItem]) -> JSONValue? {
        get {
            var at: JSONValue? = self
            for item in path {
                switch item {
                case .key(let key): at = at?[key]
                case .index(let i):
                    guard let items = at?.arrayValue, items.indices.contains(i) else { return nil }
                    at = items[i]
                }
            }
            return at
        }
        set {
            guard let first = path.first else {
                self = newValue ?? .null
                return
            }
            let rest = Array(path.dropFirst())
            switch (first, self) {
            case (.key(let key), .object(var fields)):
                if rest.isEmpty {
                    fields[key] = newValue
                } else {
                    var child = fields[key] ?? .null
                    child[rest] = newValue
                    fields[key] = child
                }
                self = .object(fields)
            case (.index(let i), .array(var items)) where items.indices.contains(i):
                if rest.isEmpty {
                    items[i] = newValue ?? .null
                } else {
                    items[i][rest] = newValue
                }
                self = .array(items)
            default: break
            }
        }
    }
}
