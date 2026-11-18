import CryptoKit
import Foundation

// Opening a sealed file, byte for byte as core's `calimero-sealed` crate
// (`file.rs`, `wrap.rs`) writes it: 64 KiB AES-256-GCM STREAM chunks under a
// data key, cut into segments, and that key wrapped to an X25519 key the
// requester made for one export. Port of mero-js `sealed/file.ts`.

/// Why an export was refused here. A node's own refusal stays the client's
/// typed error (``CalimeroError``). Messages carry no plaintext or keys.
/// (== mero-js `ExportError`.)
public struct ExportError: Error, LocalizedError, Sendable, Equatable {
    /// What failed. (== mero-js `ExportErrorCode`.)
    public enum Code: String, Sendable {
        /// The node's answer is malformed.
        case badAnswer = "BAD_ANSWER"
        /// The segments the node listed are not the file asked for.
        case notTheFile = "NOT_THE_FILE"
        /// A segment, a chunk, the wrapped key or the whole file did not check out.
        case tampered = "TAMPERED"
        /// A segment is longer than a whole segment.
        case tooLarge = "TOO_LARGE"
        /// The task was cancelled.
        case aborted = "ABORTED"
    }

    public let code: Code
    public let message: String

    public init(_ code: Code, _ message: String) {
        self.code = code
        self.message = message
    }

    public var errorDescription: String? { "\(code.rawValue): \(message)" }
}

/// The sealed-file format's sizes and the derivations over it.
/// (== mero-js `sealed/file.ts`.)
public enum SealedFile {
    /// Plaintext bytes in every chunk but the last.
    public static let chunkBytes = 64 * 1024
    static let tagBytes = 16
    static let sealedChunkBytes = chunkBytes + tagBytes
    /// Stored bytes of every segment but the last: 256 whole sealed chunks. (== mero-js `SEGMENT_BYTES`.)
    public static let segmentBytes = 256 * sealedChunkBytes
    static let nonceBytes = 12
    static let aadPrefix = Data("calimero.file.v1".utf8)
    static let segmentDomain = Data("calimero.file.segments.v1\0".utf8)
    static let exportWrapSalt = Data("calimero.export-wrap.v1".utf8)
    static let deriveLabel = Data("calimero.sealed.derive.v1\0".utf8)

    /// The file id: SHA-256 over the domain, the segment count (u32
    /// big-endian) and the ordered segment hashes. (== mero-js `fileId`.)
    public static func fileId(_ segments: [Data]) -> Data {
        var hasher = SHA256()
        hasher.update(data: segmentDomain)
        hasher.update(data: BigEndian.u32(UInt32(segments.count)))
        for segment in segments { hasher.update(data: segment) }
        return Data(hasher.finalize())
    }

    private static func prefixed(_ field: String) -> Data {
        let bytes = Data(field.utf8)
        return BigEndian.u32(UInt32(bytes.count)) + bytes
    }

    /// Open the data key `wrapped` (`nonce ‖ AES-256-GCM`) the node wrapped
    /// to `recipient` for file `fileId` (hex) of app `appId`.
    /// (== mero-js `unwrapExport`.)
    public static func unwrapExport(
        recipient: Curve25519.KeyAgreement.PrivateKey, ephemeral: Data, appId: String, fileId: String, wrapped: Data
    ) throws -> Data {
        guard wrapped.count >= nonceBytes else { throw ExportError(.tampered, "the wrapped key is truncated") }
        let shared: Data
        do {
            shared = try NoiseDH.dh(recipient, ephemeral)
        } catch {
            throw ExportError(.badAnswer, "the ephemeral key is not an X25519 key")
        }
        guard shared.contains(where: { $0 != 0 }) else {
            throw ExportError(.badAnswer, "the ephemeral key is of low order")
        }
        let info =
            deriveLabel + ephemeral + recipient.publicKey.rawRepresentation + prefixed(appId) + prefixed(fileId)
        let key = HKDF<SHA256>.deriveKey(
            inputKeyMaterial: SymmetricKey(data: shared), salt: exportWrapSalt, info: info, outputByteCount: 32)
        let body = Data(wrapped)
        let secret: Data
        do {
            guard body.count >= nonceBytes + tagBytes else { throw ExportError(.tampered, "too short") }
            let box = try AES.GCM.SealedBox(
                nonce: AES.GCM.Nonce(data: body.prefix(nonceBytes)),
                ciphertext: body.dropFirst(nonceBytes).dropLast(tagBytes), tag: body.suffix(tagBytes))
            secret = try AES.GCM.open(box, using: key)
        } catch {
            throw ExportError(.tampered, "the wrapped key does not open")
        }
        guard secret.count == 32 else { throw ExportError(.tampered, "the wrapped key has the wrong length") }
        return secret
    }
}

/// Joins a sealed file's segments in order, each checked against the file's
/// list before it is opened; holds at most one segment and one chunk. What it
/// answers is authentic but may be a prefix of the file until
/// ``finish(expectingSize:sha256:)`` returns. (== mero-js `SegmentReader`.)
public struct SegmentReader: Sendable {
    private let key: SymmetricKey
    private let segments: [String]
    private var pending = Data()
    private var index: UInt64 = 0
    private var next = 0
    private var size = 0
    private var hash = SHA256()

    /// A reader for the file whose ordered segment hashes are `segments`, under `dataKey`.
    public init(dataKey: Data, segments: [Data]) {
        key = SymmetricKey(data: dataKey)
        self.segments = segments.map { Hex.encode($0) }
    }

    /// Take the next segment; answers the plaintext of every chunk it completes, in order.
    public mutating func push(_ segment: Data) throws -> [Data] {
        guard next < segments.count else { throw ExportError(.tampered, "the file has no segment \(next)") }
        if next + 1 < segments.count && segment.count != SealedFile.segmentBytes {
            throw ExportError(.tampered, "segment \(next) is not a whole segment")
        }
        guard Hex.encode(SHA256.hash(data: segment)) == segments[next] else {
            throw ExportError(.tampered, "segment \(next) is not the one the file lists")
        }
        next += 1
        // Bytes past a full chunk prove it was not the last, so one chunk is always held back.
        let data = pending + segment
        var at = data.startIndex
        var out: [Data] = []
        while data.endIndex - at > SealedFile.sealedChunkBytes {
            out.append(try open(data[at..<(at + SealedFile.sealedChunkBytes)], last: false))
            at += SealedFile.sealedChunkBytes
        }
        pending = Data(data[at...])
        return out
    }

    /// Open the last chunk and check the whole plaintext's size and SHA-256
    /// (hex): GCM does not commit to its key. Answers the last chunk's plaintext.
    public mutating func finish(expectingSize size: Int, sha256: String) throws -> Data {
        guard next == segments.count else { throw ExportError(.tampered, "the file is missing segments") }
        guard pending.count >= SealedFile.tagBytes else { throw ExportError(.tampered, "the file is truncated") }
        let last = try open(pending, last: true)
        guard self.size == size, Hex.encode(hash.finalize()) == sha256 else {
            throw ExportError(.tampered, "the file is not the one that was sealed")
        }
        return last
    }

    private mutating func open(_ chunk: Data, last: Bool) throws -> Data {
        let plain: Data
        do {
            let nonce = try AES.GCM.Nonce(data: Data(count: 4) + BigEndian.u64(index))
            let box = try AES.GCM.SealedBox(
                nonce: nonce, ciphertext: chunk.dropLast(SealedFile.tagBytes), tag: chunk.suffix(SealedFile.tagBytes))
            plain = try AES.GCM.open(box, using: key, authenticating: SealedFile.aadPrefix + Data([last ? 1 : 0]))
        } catch {
            throw ExportError(
                .tampered, "chunk \(index) does not open: the file is altered, truncated or under another key")
        }
        hash.update(data: plain)
        size += plain.count
        index += 1
        return plain
    }
}
