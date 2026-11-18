import CryptoKit
import Foundation
import MeroKitTestSupport
import XCTest

@testable import MeroKit

/// ``CalimeroClient/exportFile(_:_:to:)`` against ``FakeCloudNode`` holding
/// one of core's Rust vectors: the export answer and its one segment through
/// the real sealed channel. Port of mero-js `client/export-file.test.ts`.
final class ExportFileTests: XCTestCase {
    private var v: SealedFileVectors!

    override func setUpWithError() throws {
        v = try SealedFileVectors.load()
    }

    override func tearDown() {
        MockURLProtocol.reset()
        super.tearDown()
    }

    private var three: SealedFileVector { v.vectors[0] }
    private var full: SealedFileVector { v.vectors[1] }
    private var empty: SealedFileVector { v.vectors[2] }

    private func client() -> CalimeroClient {
        CalimeroClient(
            CalimeroClientOptions(
                baseUrl: "https://node.test", policy: CloudPolicy(allowMock: true),
                session: MockURLProtocol.makeSession(), retry: CloudRetryOptions(retries: 0)),
            sleep: { _ in })
    }

    private static func frames(_ data: Data) -> [Data] {
        stride(from: 0, to: data.count, by: 32 * 1024).map { Data(data[$0..<min(data.count, $0 + 32 * 1024)]) }
    }

    /// A node holding `vector`: its export answer (changed by `change`) and its segment.
    @discardableResult
    private func node(
        _ vector: SealedFileVector, change: @escaping @Sendable (inout [String: JSONValue]) -> Void = { _ in },
        segment: Data? = nil, segmentAnswer: (@Sendable () -> FakeCloudAnswer)? = nil
    ) -> FakeCloudNode {
        var answer: [String: JSONValue] = [
            "app": .string(vector.app), "id": .string(vector.id), "size": .number(Double(vector.size)),
            "sha256": .string(vector.plaintextSha256), "segments": .array(vector.segments.map { .string($0) }),
            "ephemeral": .string(vector.ephemeral), "wrapped": .string(vector.wrapped),
        ]
        change(&answer)
        let body = segment ?? hexBytes(vector.segment0)
        let fixed = answer
        let node = FakeCloudNode { request in
            if request.path.hasSuffix("/export") { return .json(.object(fixed)) }
            if request.path.contains("/segments/") {
                return segmentAnswer?() ?? FakeCloudAnswer(status: 200, chunks: Self.frames(body))
            }
            return .error(404, "NOT_FOUND")
        }
        node.install()
        return node
    }

    private func run(
        _ vector: SealedFileVector, app: String? = nil, id: String? = nil
    ) async throws -> (
        ExportedFile, [Data]
    ) {
        let sink = Sink()
        let done = try await client().exportFile(
            app ?? vector.app, id ?? vector.id, to: { sink.append($0) },
            recipientSecret: hexBytes(vector.recipientSecret))
        return (done, sink.chunks)
    }

    private func refusal(_ vector: SealedFileVector, id: String? = nil) async -> String {
        do {
            _ = try await run(vector, id: id)
            return "passed"
        } catch let error as ExportError {
            return "\(error.code.rawValue): \(error.message)"
        } catch {
            return "not an ExportError: \(error)"
        }
    }

    func testDecryptsTheRustVectorsToExactlyTheirPlaintext() async throws {
        for vector in v.vectors {
            let node = node(vector)
            let (done, chunks) = try await run(vector)
            XCTAssertEqual(done, ExportedFile(size: vector.size, sha256: vector.plaintextSha256), vector.name)
            XCTAssertEqual(chunks.reduce(Data(), +), plaintext(vector.plaintextLen), vector.name)
            XCTAssertEqual(
                node.requests.map { "\($0.method) \($0.path)" },
                [
                    "POST /apps/\(vector.app)/files/\(vector.id)/export",
                    "GET /apps/\(vector.app)/files/\(vector.id)/segments/0",
                ]
            )
            XCTAssertEqual(node.requests[0].json, ["recipient": .string(vector.recipientPublic)])
            MockURLProtocol.reset()
        }
    }

    func testHandsTheSinkChunksOfAtMost64KiBInOrderAndNothingForAnEmptyFile() async throws {
        node(three)
        let sizes = try await run(three).1.map(\.count)
        XCTAssertEqual(sizes, [65536, 65536, 5])
        MockURLProtocol.reset()
        node(empty)
        let none = try await run(empty).1
        XCTAssertEqual(none, [])
    }

    func testAFreshExportKeyCannotOpenAWrapMadeForAnother() async throws {
        node(three)
        do {
            _ = try await client().exportFile(three.app, three.id)
            XCTFail("expected a refusal")
        } catch let error as ExportError {
            XCTAssertEqual(error.code, .tampered)
            XCTAssertTrue(error.message.contains("does not open"))
        }
    }

    func testFailsATamperedSegmentBeforeAnyOfItsBytesReachTheSink() async throws {
        var tampered = hexBytes(three.segment0)
        tampered[3] ^= 1
        node(three, segment: tampered)
        let sink = Sink()
        do {
            _ = try await client().exportFile(
                three.app, three.id, to: { sink.append($0) }, recipientSecret: hexBytes(three.recipientSecret))
            XCTFail("expected a refusal")
        } catch let error as ExportError {
            XCTAssertEqual(error.code, .tampered)
            XCTAssertTrue(error.message.contains("is not the one the file lists"))
        }
        XCTAssertEqual(sink.chunks, [])
    }

    func testFailsSegmentsThatAreNotTheFile() async throws {
        let n = node(three, change: { $0["segments"] = [.string(String(repeating: "07", count: 32))] })
        let refused = await refusal(three)
        XCTAssertTrue(refused.hasPrefix("NOT_THE_FILE") && refused.contains("are not the file"), refused)
        XCTAssertEqual(n.requests.count, 1)
        MockURLProtocol.reset()

        node(three)
        let other = await refusal(three, id: full.id)
        XCTAssertTrue(other.hasPrefix("NOT_THE_FILE"), other)
        MockURLProtocol.reset()

        let emptyId = Hex.encode(SealedFile.fileId([]))
        node(
            three,
            change: {
                $0["segments"] = []; $0["id"] = .string(emptyId)
            })
        let none = await refusal(three, id: emptyId)
        XCTAssertTrue(none.hasPrefix("NOT_THE_FILE"), none)
        MockURLProtocol.reset()

        let another = Data(repeating: 9, count: 32)
        let anotherId = Hex.encode(SealedFile.fileId([another]))
        node(
            three,
            change: {
                $0["segments"] = [.string(Hex.encode(another))]; $0["id"] = .string(anotherId)
            })
        let consistent = await refusal(three)
        XCTAssertTrue(consistent.hasPrefix("NOT_THE_FILE"), consistent)
    }

    func testFailsAWrongFinalSizeWithholdingTheLastChunkAndAWrongHash() async throws {
        let wrongSize = Double(three.size + 1)
        node(three, change: { $0["size"] = .number(wrongSize) })
        let sink = Sink()
        do {
            _ = try await client().exportFile(
                three.app, three.id, to: { sink.append($0) }, recipientSecret: hexBytes(three.recipientSecret))
            XCTFail("expected a refusal")
        } catch let error as ExportError {
            XCTAssertTrue(error.message.contains("not the one that was sealed"))
        }
        XCTAssertEqual(sink.chunks.reduce(0) { $0 + $1.count }, 2 * 65536)
        MockURLProtocol.reset()
        node(three, change: { $0["sha256"] = .string(String(repeating: "00", count: 32)) })
        let refused = await refusal(three)
        XCTAssertTrue(refused.contains("not the one that was sealed"), refused)
    }

    func testFailsAWrapMadeForAnotherApp() async throws {
        let n = node(three, change: { $0["app"] = "another-app" })
        let refused = await refusal(three)
        XCTAssertTrue(refused.hasPrefix("TAMPERED") && refused.contains("does not open"), refused)
        XCTAssertEqual(n.requests.count, 1)
    }

    func testTurnsAMalformedAnswerIntoBadAnswer() async throws {
        let bad: [@Sendable (inout [String: JSONValue]) -> Void] = [
            { $0["segments"] = "nope" },
            { $0["segments"] = [7] },
            { $0["segments"] = ["zz"] },
            { $0["app"] = 5 },
            { $0["id"] = nil },
            { $0["size"] = "3" },
            { $0["size"] = 1.5 },
            { $0["size"] = -1 },
            { $0["size"] = .number(pow(2, 60)) },
            { $0["sha256"] = .null },
            { $0["sha256"] = "ab" },
            { $0["ephemeral"] = "ab" },
            { $0["ephemeral"] = "zz" },
            { $0["wrapped"] = "abc" },
            { $0["wrapped"] = nil },
        ]
        for (i, change) in bad.enumerated() {
            node(three, change: change)
            let refused = await refusal(three)
            XCTAssertTrue(refused.hasPrefix("BAD_ANSWER"), "case \(i): \(refused)")
            MockURLProtocol.reset()
        }
    }

    func testLeavesTheNodesOwnRefusalAsItsTypedError() async throws {
        FakeCloudNode { _ in .error(404, "NOT_FOUND", message: "no such file") }.install()
        do {
            _ = try await client().exportFile(three.app, three.id)
            XCTFail("expected a refusal")
        } catch let error as CalimeroError {
            XCTAssertEqual(error.code, "NOT_FOUND")
        }
    }

    func testRefusesASegmentDeclaredOrStreamedPastAWholeSegment() async throws {
        node(
            three,
            segmentAnswer: {
                FakeCloudAnswer(
                    status: 200, headers: [("content-length", String(SealedFile.segmentBytes + 1))], chunks: [Data()])
            })
        let declared = await refusal(three)
        XCTAssertTrue(declared.hasPrefix("TOO_LARGE"), declared)
        MockURLProtocol.reset()

        let piece = Data(count: 64 * 1024)
        let pieces = Array(repeating: piece, count: SealedFile.segmentBytes / piece.count + 2)
        node(three, segmentAnswer: { FakeCloudAnswer(status: 200, chunks: pieces) })
        let streamed = await refusal(three)
        XCTAssertTrue(streamed.hasPrefix("TOO_LARGE"), streamed)
    }

    func testACancelledExportIsAborted() async throws {
        node(three)
        let cloud = client()
        let secret = hexBytes(three.recipientSecret)
        let (app, id) = (three.app, three.id)
        let task = Task {
            withUnsafeCurrentTask { $0?.cancel() }
            return try await cloud.exportFile(app, id, to: { _ in }, recipientSecret: secret)
        }
        do {
            _ = try await task.value
            XCTFail("expected a refusal")
        } catch let error as ExportError {
            XCTAssertEqual(error.code, .aborted)
        }
    }
}

/// What a sink received.
final class Sink: @unchecked Sendable {
    private let lock = NSLock()
    private var received: [Data] = []

    func append(_ chunk: Data) {
        lock.lock()
        defer { lock.unlock() }
        received.append(chunk)
    }

    var chunks: [Data] {
        lock.lock()
        defer { lock.unlock() }
        return received
    }
}
