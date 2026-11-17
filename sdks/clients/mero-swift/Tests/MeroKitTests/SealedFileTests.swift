import CryptoKit
import Foundation
import MeroKitTestSupport
import XCTest

@testable import MeroKit

/// The vectors core's Rust reference (crates/sealed: `SegmentWriter`,
/// `file_id`, `wrap_export`) produced, as mero-js checks them
/// (`sealed/file.test.ts`, `sealed/file-multi-segment.test.ts`); copied from
/// mero-js `src/sealed/fixtures/` as JSON.
struct SealedFileVector: Decodable {
    let app: String
    let dataKey: String
    let ephemeral: String
    let id: String
    let name: String
    let plaintextLen: Int
    let plaintextSha256: String
    let recipientPublic: String
    let recipientSecret: String
    let segment0: String
    let segments: [String]
    let size: Int
    let wrapped: String
}

struct SealedFileVectors: Decodable {
    struct FileIdVector: Decodable {
        let id: String
        let segments: [String]
    }

    let chunkBytes: Int
    let segmentBytes: Int
    let fileIds: [FileIdVector]
    let vectors: [SealedFileVector]

    static func load() throws -> SealedFileVectors {
        try JSONDecoder().decode(SealedFileVectors.self, from: try fixture("sealed-file/sealed-file-vectors.json"))
    }
}

struct MultiSegmentVector: Decodable {
    let name: String
    let len: Int
    let more: Bool
    let key: String
    let id: String
    let sha256: String
    let size: Int
    let segments: [String]
    let lastLen: Int
}

func fixture(_ name: String) throws -> Data {
    guard let root = Bundle.module.resourceURL else { throw CocoaError(.fileNoSuchFile) }
    return try Data(contentsOf: root.appendingPathComponent("Fixtures/\(name)"))
}

func hexBytes(_ value: String) -> Data {
    // swiftlint:disable:next force_try
    try! Hex.decodeUnsized(value, label: "vector")
}

/// `byte i = i % 251`, the plaintext rule of every vector.
func plaintext(_ length: Int) -> Data {
    let cycle = Data((0..<251).map { UInt8($0) })
    var out = Data(capacity: length + 251)
    while out.count < length { out.append(cycle) }
    return out.prefix(length)
}

final class SealedFileTests: XCTestCase {
    private var v: SealedFileVectors!

    override func setUpWithError() throws {
        v = try SealedFileVectors.load()
    }

    private var three: SealedFileVector { v.vectors[0] }
    private var full: SealedFileVector { v.vectors[1] }

    private func reader(_ vector: SealedFileVector, _ list: [Data]? = nil, segment: Data) -> SegmentReader {
        SegmentReader(dataKey: hexBytes(vector.dataKey), segments: list ?? [Data(SHA256.hash(data: segment))])
    }

    private func code(_ body: () throws -> Void) -> String {
        do {
            try body()
            return "passed"
        } catch let error as ExportError {
            return error.code.rawValue + ": " + error.message
        } catch {
            return "not an ExportError: \(error)"
        }
    }

    // MARK: - Constants and ids

    func testSegmentSizeAndChunkSizeMatchTheRustReference() {
        XCTAssertEqual(SealedFile.segmentBytes, v.segmentBytes)
        XCTAssertEqual(SealedFile.chunkBytes, v.chunkBytes)
    }

    func testDerivesFileIdsOverOneToThreeSegmentsOrderIncluded() {
        for entry in v.fileIds {
            XCTAssertEqual(Hex.encode(SealedFile.fileId(entry.segments.map(hexBytes))), entry.id)
        }
        for vector in v.vectors {
            XCTAssertEqual(Hex.encode(SealedFile.fileId(vector.segments.map(hexBytes))), vector.id, vector.name)
        }
    }

    // MARK: - unwrapExport

    private func open(_ vector: SealedFileVector, app: String? = nil, file: String? = nil) throws -> Data {
        try SealedFile.unwrapExport(
            recipient: try NoiseDH.keyPair(hexBytes(vector.recipientSecret)), ephemeral: hexBytes(vector.ephemeral),
            appId: app ?? vector.app, fileId: file ?? vector.id, wrapped: hexBytes(vector.wrapped))
    }

    func testOpensTheRustWrapToTheDataKey() throws {
        for vector in v.vectors {
            XCTAssertEqual(Hex.encode(try open(vector)), vector.dataKey, vector.name)
            let recipient = try NoiseDH.keyPair(hexBytes(vector.recipientSecret))
            XCTAssertEqual(Hex.encode(recipient.publicKey.rawRepresentation), vector.recipientPublic)
        }
    }

    func testDoesNotOpenForAnotherAppFileIdKeyOrSplit() throws {
        XCTAssertTrue(code { _ = try open(three, app: "other") }.contains("does not open"))
        XCTAssertTrue(code { _ = try open(three, file: full.id) }.contains("does not open"))
        XCTAssertTrue(
            code { _ = try open(three, app: "\(three.app)\0\(three.id)", file: "") }.contains("does not open"))
        XCTAssertThrowsError(
            try SealedFile.unwrapExport(
                recipient: try NoiseDH.keyPair(), ephemeral: hexBytes(three.ephemeral), appId: three.app,
                fileId: three.id, wrapped: hexBytes(three.wrapped)))
    }

    func testRefusesATruncatedTamperedOrLowOrderWrap() throws {
        let k = try NoiseDH.keyPair(hexBytes(three.recipientSecret))
        let wrapped = hexBytes(three.wrapped)
        let unwrap = { (ephemeral: Data, wrapped: Data) in
            try SealedFile.unwrapExport(
                recipient: k, ephemeral: ephemeral, appId: self.three.app, fileId: self.three.id, wrapped: wrapped)
        }
        XCTAssertTrue(code { _ = try unwrap(hexBytes(three.ephemeral), wrapped.prefix(5)) }.contains("truncated"))
        var flipped = wrapped
        flipped[flipped.count - 1] ^= 1
        XCTAssertTrue(code { _ = try unwrap(hexBytes(three.ephemeral), flipped) }.contains("does not open"))
        XCTAssertThrowsError(try unwrap(Data(count: 32), wrapped))
    }

    // MARK: - SegmentReader

    func testOpensEachRustVectorToItsPlaintextSizeAndHash() throws {
        for vector in v.vectors {
            let segment = hexBytes(vector.segment0)
            var r = reader(vector, segment: segment)
            var out = Data()
            for chunk in try r.push(segment) { out += chunk }
            out += try r.finish(expectingSize: vector.size, sha256: vector.plaintextSha256)
            XCTAssertEqual(out, plaintext(vector.plaintextLen), vector.name)
        }
    }

    func testHoldsTheLastChunkBackEvenWhenItIsFull() throws {
        let segment = hexBytes(full.segment0)
        var r = reader(full, segment: segment)
        XCTAssertEqual(try r.push(segment), [])
    }

    func testRefusesASegmentThatIsNotTheOneListedBeforeOpeningAnyOfIt() {
        let segment = hexBytes(three.segment0)
        var r = reader(three, segment: segment)
        var tampered = segment
        tampered[tampered.count - 1] ^= 1
        XCTAssertTrue(code { _ = try r.push(tampered) }.contains("is not the one the file lists"))
    }

    func testRefusesAShortNonLastSegmentAndASegmentPastTheList() throws {
        let segment = hexBytes(three.segment0)
        let hash = Data(SHA256.hash(data: segment))
        var two = reader(three, [hash, hash], segment: segment)
        XCTAssertTrue(code { _ = try two.push(segment) }.contains("not a whole segment"))
        var single = reader(three, [hash], segment: segment)
        _ = try single.push(segment)
        XCTAssertTrue(code { _ = try single.push(segment) }.contains("no segment 1"))
    }

    func testRefusesAMissingSegment() {
        var r = reader(three, [Data(count: 32), Data(count: 32)], segment: Data())
        XCTAssertTrue(code { _ = try r.finish(expectingSize: 0, sha256: "") }.contains("missing segments"))
    }

    func testRefusesAWrongSizeOrPlaintextHash() throws {
        let segment = hexBytes(three.segment0)
        func attempt(_ size: Int, _ sha: String) throws {
            var r = reader(three, segment: segment)
            _ = try r.push(segment)
            _ = try r.finish(expectingSize: size, sha256: sha)
        }
        try attempt(three.size, three.plaintextSha256)
        XCTAssertTrue(
            code { try attempt(three.size + 1, three.plaintextSha256) }.contains("not the one that was sealed"))
        XCTAssertTrue(
            code { try attempt(three.size, String(repeating: "00", count: 32)) }.contains("not the one that was sealed")
        )
    }

    func testRefusesAFileCutAfterAChunkADroppedChunkOrShorterThanATag() throws {
        let sealed = 65536 + 16
        let segment = hexBytes(three.segment0)
        for keep in [2 * sealed, sealed] {
            let cut = segment.prefix(keep)
            var r = reader(three, segment: cut)
            _ = try r.push(cut)
            XCTAssertTrue(code { _ = try r.finish(expectingSize: 0, sha256: "") }.contains("does not open"))
        }
        let dropped = segment.prefix(sealed) + segment.dropFirst(2 * sealed)
        var r = reader(three, segment: dropped)
        _ = try r.push(dropped)
        XCTAssertTrue(code { _ = try r.finish(expectingSize: 0, sha256: "") }.contains("does not open"))
        var empty = reader(three, segment: Data())
        _ = try empty.push(Data())
        XCTAssertTrue(code { _ = try empty.finish(expectingSize: 0, sha256: "") }.contains("truncated"))
    }

    // MARK: - Multi-segment files

    /// The sealed stream the Rust writer makes: whole chunks, and `more` seals a full final chunk as not last.
    private func seal(_ key: Data, _ len: Int, more: Bool) throws -> Data {
        let plain = plaintext(len)
        let chunk = SealedFile.chunkBytes
        let count = max(1, (len + chunk - 1) / chunk + (more && len % chunk == 0 ? 1 : 0))
        var out = Data()
        out.reserveCapacity(len + count * 16)
        for i in 0..<count {
            let nonce = try AES.GCM.Nonce(data: Data(count: 4) + BigEndian.u64(UInt64(i)))
            let piece = plain[min(len, i * chunk)..<min(len, (i + 1) * chunk)]
            let aad = Data("calimero.file.v1".utf8) + Data([i == count - 1 ? 1 : 0])
            let box = try AES.GCM.seal(piece, using: SymmetricKey(data: key), nonce: nonce, authenticating: aad)
            out += box.ciphertext + box.tag
        }
        return out
    }

    func testMultiSegmentFilesMatchTheRustReference() throws {
        let vectors = try JSONDecoder().decode(
            [MultiSegmentVector].self, from: try fixture("sealed-file/sealed-file-multi-segment-vectors.json"))
        XCTAssertEqual(vectors.count, 4)
        for vector in vectors {
            let key = hexBytes(vector.key)
            let all = try seal(key, vector.len, more: vector.more)
            let segments = stride(from: 0, to: all.count, by: SealedFile.segmentBytes).map {
                all[$0..<min(all.count, $0 + SealedFile.segmentBytes)]
            }
            XCTAssertEqual(segments.map { Hex.encode(SHA256.hash(data: $0)) }, vector.segments, vector.name)
            XCTAssertEqual(segments.last?.count, vector.lastLen, vector.name)
            XCTAssertEqual(Hex.encode(SealedFile.fileId(vector.segments.map(hexBytes))), vector.id, vector.name)

            var r = SegmentReader(dataKey: key, segments: vector.segments.map(hexBytes))
            var hasher = SHA256()
            var length = 0
            for segment in segments {
                for chunk in try r.push(Data(segment)) {
                    hasher.update(data: chunk)
                    length += chunk.count
                }
            }
            let last = try r.finish(expectingSize: vector.size, sha256: vector.sha256)
            hasher.update(data: last)
            length += last.count
            XCTAssertEqual(length, vector.len, vector.name)
            XCTAssertEqual(Hex.encode(hasher.finalize()), Hex.encode(SHA256.hash(data: plaintext(vector.len))))
        }
    }
}
