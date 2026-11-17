import CryptoKit
import Foundation

// Sealed files: uploading one, and downloading one as core's
// `Client::export_file_to` does: the node answers only ciphertext and the
// file's data key wrapped to a key made here for this call, and nothing it
// says is believed until it checks out. Port of mero-js `client/files.ts`
// and `CalimeroClient.uploadFile`.

/// What an export wrote, as proven against the file's mark. (== mero-js `ExportedFile`.)
public struct ExportedFile: Sendable, Equatable {
    /// The plaintext's size in bytes.
    public var size: Int
    /// The plaintext's SHA-256, hex.
    public var sha256: String

    public init(size: Int, sha256: String) {
        self.size = size
        self.sha256 = sha256
    }
}

/// The node's export answer, checked for shape; whether it names the requested file is checked after.
struct ExportAnswer {
    let app: String
    let size: Int
    let named: Data
    let segments: [Data]
    let sha256: String
    let ephemeral: Data
    let wrapped: Data

    init(_ answer: JSONValue) throws {
        guard case .object(let a) = answer else { throw ExportError(.badAnswer, "the node answered a bad export") }
        guard case .string(let app)? = a["app"] else { throw ExportError(.badAnswer, "the node answered a bad app") }
        guard case .number(let size)? = a["size"], size >= 0, size <= 9_007_199_254_740_991, size == size.rounded()
        else { throw ExportError(.badAnswer, "the node answered a bad size") }
        guard case .array(let segments)? = a["segments"] else {
            throw ExportError(.badAnswer, "the node answered a bad segment list")
        }
        self.app = app
        self.size = Int(size)
        named = try Self.hex(a["id"], "file id", 32)
        self.segments = try segments.map { try Self.hex($0, "segment hash", 32) }
        sha256 = Hex.encode(try Self.hex(a["sha256"], "sha256", 32))
        ephemeral = try Self.hex(a["ephemeral"], "ephemeral key", 32)
        wrapped = try Self.hex(a["wrapped"], "wrapped key", nil)
    }

    private static func hex(_ value: JSONValue?, _ label: String, _ bytes: Int?) throws -> Data {
        let bad = ExportError(.badAnswer, "the node answered a bad \(label)")
        guard case .string(let text)? = value else { throw bad }
        do {
            if let bytes { return try Hex.decode(text, label: label, bytes: bytes) }
            return try Hex.decodeUnsized(text, label: label)
        } catch {
            throw bad
        }
    }
}

extension CalimeroClient {
    /// Plaintext bytes a sealed file upload sends in one request; a larger
    /// file is a session of parts. (== mero-js `FILE_PART_BYTES`.)
    public static let filePartBytes = 16 * 1024 * 1024

    /// Upload a sealed file for component app `app` (`POST /apps/{app}/files`):
    /// sealed under a key of its own that only the app's code and its
    /// exports read, replicated on the app's replicas. Its owner and its
    /// organization's members only, charged to the organization's file quota
    /// (``QuotaExceeded``). Answers its `id`, `blobId`, the plaintext's `size`
    /// and `sha256`, and the upload's log `index`.
    ///
    /// Up to ``filePartBytes`` it is one request, sent once (the node mints a
    /// new file for each, so one that may have reached it is never repeated);
    /// a larger file is a session of parts (`/files/uploads`), each part sent
    /// again from where the node says the session is after a part out of
    /// order. A session the node lost (`UPLOAD_UNKNOWN`) is begun again once;
    /// any other failure aborts it. (== mero-js `uploadFile`.)
    public func uploadFile(_ app: String, _ data: Data) async throws -> BlobInfo {
        if data.count <= Self.filePartBytes {
            return try decode(
                BlobInfo.self, try await answerBytes { try await self.apps.filesUpload(app: app, body: data) })
        }
        do {
            return try await uploadSession(app, data)
        } catch let error as CalimeroError where error.code == "UPLOAD_UNKNOWN" {
            return try await uploadSession(app, data)
        }
    }

    /// Begin a session, send its parts and complete it, aborting it if that fails.
    private func uploadSession(_ app: String, _ data: Data) async throws -> BlobInfo {
        let begin = try await apps.uploadBegin(app: app, size: Int64(data.count))
        let upload = begin.uploadId
        let partBytes = Int(begin.partBytes)
        guard !upload.isEmpty, partBytes > 0, Int(begin.parts) == (data.count + partBytes - 1) / partBytes else {
            throw AccountError.protocolViolation("the node began an upload it cannot take")
        }
        let parts = Int(begin.parts)
        do {
            var next = 0
            var jumps = 0
            while next < parts {
                let lower = data.startIndex + next * partBytes
                let body = Data(data[lower..<min(lower + partBytes, data.endIndex)])
                do {
                    next = Int(try await apps.uploadPart(app: app, upload: upload, k: String(next), body: body).next)
                } catch let error as CalimeroError where error.code == "PART_OUT_OF_ORDER" {
                    jumps += 1
                    guard jumps <= 3, case .number(let at)? = error.details["next"], at >= 0, Int(at) <= parts else {
                        throw error
                    }
                    next = Int(at)
                }
            }
            return try decode(
                BlobInfo.self, try await answerBytes { try await self.apps.uploadComplete(app: app, upload: upload) })
        } catch {
            if (error as? CalimeroError)?.code != "UPLOAD_UNKNOWN" {
                _ = try? await apps.uploadAbort(app: app, upload: upload)
            }
            throw error
        }
    }

    /// Write the plaintext of sealed file `id` (64 hex) of component app
    /// `app` to `sink`, in order, one segment at a time, in chunks of at most
    /// 64 KiB. Each segment is checked against its hash before it is opened,
    /// and the segment list, size and plaintext hash against the file's id and
    /// mark at the end. When this throws, discard what `sink` received: it is
    /// authentic but may be a prefix of the file. A failed check is an
    /// ``ExportError``; a refusal by the node is its typed error
    /// (``NotFound`` to someone the file is not shared with). Cancelling the
    /// task stops the download (``ExportError/Code/aborted``).
    /// (== mero-js `exportFileTo`.)
    @discardableResult
    public func exportFile(
        _ app: String, _ id: String, to sink: @Sendable (Data) async throws -> Void
    ) async throws -> ExportedFile {
        try await exportFile(app, id, to: sink, recipientSecret: nil)
    }

    /// The plaintext of sealed file `id` of component app `app`, in memory;
    /// use ``exportFile(_:_:to:)`` for a large one. (== mero-js `exportFile`.)
    public func exportFile(_ app: String, _ id: String) async throws -> Data {
        let out = ExportBuffer()
        try await exportFile(app, id, to: { out.append($0) })
        return out.data
    }

    /// ``exportFile(_:_:to:)`` with the export key pinned to
    /// `recipientSecret`: for the published vectors only.
    func exportFile(
        _ app: String, _ id: String, to sink: @Sendable (Data) async throws -> Void, recipientSecret: Data?
    ) async throws -> ExportedFile {
        do {
            return try await exportChecked(app, id, sink, recipientSecret)
        } catch let error as ExportError {
            throw error
        } catch {
            if Task.isCancelled || error is CancellationError { throw ExportError(.aborted, "the export was aborted") }
            throw error
        }
    }

    private func exportChecked(
        _ app: String, _ id: String, _ sink: @Sendable (Data) async throws -> Void, _ recipientSecret: Data?
    ) async throws -> ExportedFile {
        let aborted = ExportError(.aborted, "the export was aborted")
        if Task.isCancelled { throw aborted }
        let recipient = try NoiseDH.keyPair(recipientSecret)
        let path = "apps/\(Self.seg(app))/files/\(Self.seg(id))"
        let raw = try await json(
            "POST", "\(path)/export",
            body: CanonicalJSON.data(["recipient": .string(Hex.encode(recipient.publicKey.rawRepresentation))]))
        let parsed: JSONValue
        do {
            parsed = try MeroJSON.decode(JSONValue.self, from: raw)
        } catch {
            throw ExportError(.badAnswer, "the node answered a bad export")
        }
        let answer = try ExportAnswer(parsed)
        let named = Hex.encode(answer.named)
        guard !answer.segments.isEmpty, Hex.encode(SealedFile.fileId(answer.segments)) == named, named == id else {
            throw ExportError(.notTheFile, "the node answered segments that are not the file \(id)")
        }
        let dataKey = try SealedFile.unwrapExport(
            recipient: recipient, ephemeral: answer.ephemeral, appId: answer.app, fileId: id, wrapped: answer.wrapped)
        var reader = SegmentReader(dataKey: dataKey, segments: answer.segments)
        for k in answer.segments.indices {
            if Task.isCancelled { throw aborted }
            let response = try await exchange("GET", "\(path)/segments/\(k)", body: nil, headers: [:])
            for chunk in try reader.push(try await Self.readSegment(response)) { try await sink(chunk) }
        }
        let last = try reader.finish(expectingSize: answer.size, sha256: answer.sha256)
        if !last.isEmpty { try await sink(last) }
        return ExportedFile(size: answer.size, sha256: answer.sha256)
    }

    /// A segment's bytes, refusing one longer than a whole segment before
    /// buffering it, and stopping when the task is cancelled.
    static func readSegment(_ response: SealedResponse) async throws -> Data {
        let tooLarge = ExportError(.tooLarge, "a segment is longer than a whole segment")
        if let declared = response.headers["content-length"].flatMap(Int.init), declared > SealedFile.segmentBytes {
            throw tooLarge
        }
        var out = Data()
        for try await chunk in response.body {
            if Task.isCancelled { throw ExportError(.aborted, "the export was aborted") }
            if out.count + chunk.count > SealedFile.segmentBytes { throw tooLarge }
            out.append(chunk)
        }
        if Task.isCancelled { throw ExportError(.aborted, "the export was aborted") }
        return out
    }
}

/// The plaintext ``CalimeroClient/exportFile(_:_:)`` gathers.
private final class ExportBuffer: @unchecked Sendable {
    private let lock = NSLock()
    private var bytes = Data()

    func append(_ chunk: Data) {
        lock.lock()
        defer { lock.unlock() }
        bytes.append(chunk)
    }

    var data: Data {
        lock.lock()
        defer { lock.unlock() }
        return bytes
    }
}
