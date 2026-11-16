import CryptoKit
import Foundation
import HTTPTypes
import MeroAPI
import OpenAPIRuntime

// The hand-written half of MeroKit's generated API. MeroAPI's `Client` (from
// core's OpenAPI document, by swift-openapi-generator) builds each request
// and reads each answer; what it cannot know is written here:
//
// - ``SealedClientTransport`` carries every request to the node the
//   ``CalimeroClient`` uses now, sealed to its attested key, with the
//   client's credential and its `401` refresh and `421` node moves.
// - ``StepUpMiddleware`` answers a `412 CONFIRM_REQUIRED` with a passkey
//   assertion and a `428 OPERATOR_SIGNATURE_REQUIRED` with the operator
//   key's signature, sending the request once more.
// - ``CalimeroClient/perform(passkey:operatorSigner:_:)`` runs one request
//   for the facade (Generated/Facade.swift) and throws MeroKit's errors.

/// Signs the statement of a `428 OPERATOR_SIGNATURE_REQUIRED` with the
/// cluster's operator key.
public protocol OperatorSigner: Sendable {
    /// The Ed25519 signature over `message` (the statement's exact bytes, the
    /// refusal's `details.message` base64-decoded), hex.
    func sign(_ message: Data) async throws -> String
}

/// An ``OperatorSigner`` holding the operator's Ed25519 key in memory.
public struct Ed25519OperatorKey: OperatorSigner {
    /// The 32-byte seed; the key is rebuilt from it to sign.
    private let seed: Data

    /// The key from its 32-byte seed (`raw`).
    public init(raw: Data) throws {
        _ = try Curve25519.Signing.PrivateKey(rawRepresentation: raw)
        seed = raw
    }

    /// The public half, hex.
    public var publicKeyHex: String {
        // The seed was checked in `init`.
        let key = try? Curve25519.Signing.PrivateKey(rawRepresentation: seed)
        return key.map { Hex.encode($0.publicKey.rawRepresentation) } ?? ""
    }

    public func sign(_ message: Data) async throws -> String {
        let key = try Curve25519.Signing.PrivateKey(rawRepresentation: seed)
        return Hex.encode(try key.signature(for: message))
    }
}

/// The step-up answers a request carries down the middleware chain.
enum StepUp {
    /// The passkey the facade call named (`passkey:`), over the client's.
    @TaskLocal static var passkey: (any PasskeyProvider)?
    /// The operator signer the facade call named, over the client's.
    @TaskLocal static var operatorSigner: (any OperatorSigner)?
    /// Whether the request may be sent again after a 503 or a lost
    /// connection (`x-calimero-retry-safe`), and fail over to another node.
    @TaskLocal static var retry = false
    /// The JSON body's fields the facade call sends as an explicit `null`
    /// (a `T??` argument given `.some(nil)`), which the generated encoder
    /// would leave out.
    @TaskLocal static var nulls: Set<String> = []
    /// The `Content-Type` of a `*/*` body (a blob upload), for which the
    /// generated client would send `*/*` itself.
    @TaskLocal static var contentType: String?

    /// `confirm` for a `412 CONFIRM_REQUIRED`: `passkey`'s assertion over
    /// its challenge. A dismissed prompt is ``ConfirmCancelled``.
    static func confirmation(_ required: ConfirmRequired, passkey: any PasskeyProvider) async throws -> JSONValue {
        let d = required.details
        guard let challengeId = d["challengeId"]?.stringValue, let challenge = d["challenge"]?.stringValue,
            let rpId = d["rpId"]?.stringValue
        else { throw required }
        let assertion: AssertionJSON
        do {
            assertion = try await passkey.get(
                challenge: try fromB64url(challenge), rpId: rpId,
                allowCredentials: d["allowCredentials"]?.arrayValue?.compactMap(\.stringValue))
        } catch let cancelled as PasskeyCancelled {
            throw ConfirmCancelled(cause: cancelled.message)
        }
        let signed = try MeroJSON.decode(JSONValue.self, from: try MeroJSON.encode(assertion))
        return ["challengeId": .string(challengeId), "assertion": signed]
    }

    /// `operatorProof` for a `428 OPERATOR_SIGNATURE_REQUIRED`: the
    /// operator's signature over the statement, with its nonce.
    static func operatorProof(
        _ required: OperatorSignatureRequired, signer: any OperatorSigner
    ) async throws
        -> JSONValue
    {
        guard let nonce = required.details["nonce"]?.stringValue,
            let message = required.details["message"]?.stringValue,
            let bytes = Data(base64Encoded: message)
        else { throw required }
        return ["nonce": .string(nonce), "signature": .string(try await signer.sign(bytes))]
    }

    /// The JSON object `body` (or `{}`) with `field` set to `value`; every
    /// other number is kept as written.
    static func adding(_ field: String, _ value: JSONValue, to body: Data?) throws -> Data {
        var fields: [String: LiteralJSON.Node] = [:]
        if let body, !body.isEmpty {
            guard case .object(let existing) = try LiteralJSON.parse(body) else {
                throw AccountError.invalidInput("a step-up needs a JSON object body")
            }
            fields = existing
        }
        fields[field] = try LiteralJSON.parse(CanonicalJSON.data(value))
        return Data(LiteralJSON.write(.object(fields)).utf8)
    }
}

/// The generated client's transport: each request goes to the node the
/// ``CalimeroClient`` uses now through its sealed channel (attested under the
/// client's policy first), with the client's credential, its one shared
/// refresh after a `401`, and its moves after a `421`. `/health` and
/// `/attestation` go in the clear, as the node serves them. A refusal throws
/// MeroKit's typed error (``errorFrom(status:data:retryAfter:)``), so the
/// generated client only ever sees successes. `baseURL` is ignored: the client
/// picks the node.
struct SealedClientTransport: ClientTransport {
    let client: CalimeroClient

    /// The operations a node answers in the clear.
    static let plaintext: Set<String> = [Operations.Health.id, Operations.Attestation.id]

    func send(
        _ request: HTTPRequest, body: HTTPBody?, baseURL: URL, operationID: String
    ) async throws -> (HTTPResponse, HTTPBody?) {
        var data: Data?
        if let body { data = try await Data(collecting: body, upTo: .max) }
        var headers: [String: String] = [:]
        for field in request.headerFields where field.name != .authorization && field.name != .contentLength {
            let name = field.name.canonicalName
            headers[name] = headers[name].map { "\($0), \(field.value)" } ?? field.value
        }
        if headers["content-type"] == "*/*" {
            headers["content-type"] = StepUp.contentType ?? "application/octet-stream"
        }
        let path = String((request.path ?? "/").drop { $0 == "/" })
        let response: SealedResponse
        if Self.plaintext.contains(operationID) {
            response = try await client.plain(request.method.rawValue, path)
        } else if StepUp.retry {
            response = try await client.retriedExchange(request.method.rawValue, path, body: data, headers: headers)
        } else {
            response = try await client.exchange(request.method.rawValue, path, body: data, headers: headers)
        }
        var fields = HTTPFields()
        for (name, value) in response.headers {
            if let name = HTTPField.Name(name) { fields.append(HTTPField(name: name, value: value)) }
        }
        let frames = response.body
        let chunks = AsyncThrowingStream<HTTPBody.ByteChunk, any Error> { continuation in
            let task = Task {
                do {
                    for try await frame in frames { continuation.yield(ArraySlice(frame)) }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
        return (
            HTTPResponse(status: .init(code: response.status), headerFields: fields),
            HTTPBody(chunks, length: .unknown)
        )
    }
}

/// Answers a step-up refusal and sends the request once more: a `412
/// CONFIRM_REQUIRED` with `confirm` (a passkey assertion over the node's
/// challenge), a `428 OPERATOR_SIGNATURE_REQUIRED` with `operatorProof`. The
/// passkey and the signer are the call's (``StepUp``), else the client's
/// (``CalimeroClient/useStepUp(_:)``, ``CalimeroClient/useOperatorSigner(_:)``);
/// without one the refusal goes to the caller.
struct StepUpMiddleware: ClientMiddleware {
    let client: CalimeroClient

    func intercept(
        _ request: HTTPRequest, body: HTTPBody?, baseURL: URL, operationID: String,
        next: @Sendable (HTTPRequest, HTTPBody?, URL) async throws -> (HTTPResponse, HTTPBody?)
    ) async throws -> (HTTPResponse, HTTPBody?) {
        // Kept, to send again with the answer.
        var bytes: Data?
        if let body { bytes = try await Data(collecting: body, upTo: .max) }
        for field in StepUp.nulls.sorted() { bytes = try StepUp.adding(field, .null, to: bytes) }
        do {
            return try await next(request, bytes.map { HTTPBody($0) }, baseURL)
        } catch let error as ClientError {
            let defaults = await client.stepUpDefaults
            let answered: Data
            if let required = error.underlyingError as? ConfirmRequired {
                guard let passkey = StepUp.passkey ?? defaults.passkey else { throw error }
                let confirm = try await StepUp.confirmation(required, passkey: passkey)
                answered = try StepUp.adding("confirm", confirm, to: bytes)
            } else if let required = error.underlyingError as? OperatorSignatureRequired {
                guard let signer = StepUp.operatorSigner ?? defaults.signer else { throw error }
                let proof = try await StepUp.operatorProof(required, signer: signer)
                answered = try StepUp.adding("operatorProof", proof, to: bytes)
            } else {
                throw error
            }
            var retry = request
            retry.headerFields[.contentType] = "application/json; charset=utf-8"
            // The answer goes once: its challenge (or nonce) is single-use.
            return try await StepUp.$retry.withValue(false) { try await next(retry, HTTPBody(answered), baseURL) }
        }
    }
}

/// Holds the bytes of the answer to the request a deprecated method makes,
/// for its own decoding: `uploadFile`, `listBlobs`, `blobInfo` (a sealed
/// file's `sha256` and `sealed`, which the document's blob shapes leave out)
/// and `blobContent` (the type a blob is served as).
final class AnswerTap: @unchecked Sendable {
    @TaskLocal static var current: AnswerTap?

    private let lock = NSLock()
    private var bytes: Data?
    private var type: String?

    var data: Data? {
        lock.lock()
        defer { lock.unlock() }
        return bytes
    }

    /// The answer's `Content-Type`, as the node sent it.
    var contentType: String? {
        lock.lock()
        defer { lock.unlock() }
        return type
    }

    func keep(_ data: Data, contentType: String?) {
        lock.lock()
        bytes = data
        type = contentType
        lock.unlock()
    }
}

/// Keeps a successful answer's bytes in the task's ``AnswerTap``, when it has one.
struct AnswerTapMiddleware: ClientMiddleware {
    func intercept(
        _ request: HTTPRequest, body: HTTPBody?, baseURL: URL, operationID: String,
        next: @Sendable (HTTPRequest, HTTPBody?, URL) async throws -> (HTTPResponse, HTTPBody?)
    ) async throws -> (HTTPResponse, HTTPBody?) {
        let (response, answer) = try await next(request, body, baseURL)
        guard let tap = AnswerTap.current else { return (response, answer) }
        var data = Data()
        if let answer { data = try await Data(collecting: answer, upTo: .max) }
        tap.keep(data, contentType: response.headerFields[.contentType])
        return (response, HTTPBody(data))
    }
}

extension CalimeroClient {
    /// The client generated from core's OpenAPI document (`import MeroAPI`
    /// for its types), over this client's sealed channel, credential and
    /// node list. Refusals throw `ClientError` around MeroKit's typed error
    /// (`underlyingError`); a `412` or `428` is answered with the client's
    /// ``useStepUp(_:)`` passkey or ``useOperatorSigner(_:)`` signer.
    /// The facade (``orgs``, ``apps``, ``auth``, ...) wraps it one method
    /// per operation and throws the typed error itself; prefer it.
    public nonisolated var api: Client {
        Client(
            serverURL: Self.placeholderServer,
            configuration: Configuration(jsonEncodingOptions: [.sortedKeys, .withoutEscapingSlashes]),
            transport: SealedClientTransport(client: self),
            middlewares: [AnswerTapMiddleware(), StepUpMiddleware(client: self)])
    }

    /// The generated client wants a server URL; the transport picks the node.
    private static let placeholderServer = URL(string: "https://node.invalid")!

    /// Run one request of the generated client with the call's step-up
    /// answers, throwing MeroKit's typed errors rather than `ClientError`.
    /// What the facade's methods call.
    nonisolated func perform<T: Sendable>(
        passkey: (any PasskeyProvider)? = nil, operatorSigner: (any OperatorSigner)? = nil, retry: Bool = false,
        nulls: Set<String> = [], contentType: String? = nil, _ body: @Sendable (Client) async throws -> T
    ) async throws -> T {
        let api = self.api
        do {
            return try await StepUp.$passkey.withValue(passkey ?? StepUp.passkey) {
                try await StepUp.$operatorSigner.withValue(operatorSigner ?? StepUp.operatorSigner) {
                    try await StepUp.$retry.withValue(retry) {
                        try await StepUp.$nulls.withValue(nulls) {
                            try await StepUp.$contentType.withValue(contentType) { try await body(api) }
                        }
                    }
                }
            }
        } catch {
            throw Self.unwrapped(error)
        }
    }

    /// The error under a `ClientError`: a refusal's typed error, a transport
    /// failure, or an answer that did not decode as
    /// ``AccountError/protocolViolation(_:)``.
    static func unwrapped(_ error: any Error) -> any Error {
        guard let error = error as? ClientError else { return error }
        if error.underlyingError is DecodingError {
            return AccountError.protocolViolation(
                "the node's answer to \(error.operationID) did not decode: \(error.underlyingError)")
        }
        return error.underlyingError
    }

    /// The bytes the node answered to the one request `operation` makes
    /// through the facade, whether or not the generated types accept them
    /// (see ``AnswerTap``).
    func answerBytes<T: Sendable>(_ operation: @Sendable () async throws -> T) async throws -> Data {
        let tap = AnswerTap()
        do {
            _ = try await AnswerTap.$current.withValue(tap) { try await operation() }
        } catch {
            // A refusal never reaches the tap: what it holds was a success
            // the generated decoder found stricter than this method's own.
            guard let data = tap.data else { throw error }
            return data
        }
        return tap.data ?? Data()
    }
}

extension OpenAPIValueContainer {
    /// `value` as the generated client's free-form JSON.
    public init(_ value: JSONValue) throws {
        self = try MeroJSON.decode(OpenAPIValueContainer.self, from: CanonicalJSON.data(value))
    }
}

extension AppAccess {
    /// This access as the generated API's `Access` (discriminated by `kind`).
    public var generated: Components.Schemas.Access {
        switch self {
        case .owner: return .owner(.init(kind: .owner))
        case .org: return .org(.init(kind: .org))
        case .public: return ._public(.init(kind: ._public))
        case .accounts(let accounts): return .accounts(.init(kind: .accounts, accounts: accounts))
        case .team(let team): return .team(.init(kind: .team, team: team))
        }
    }
}

extension Deployer {
    /// This deployer as the generated API's `Deployer`.
    public var generated: Components.Schemas.Deployer {
        switch self {
        case .account(let id): return .case1(.init(account: id))
        case .org(let id): return .case2(.init(org: id))
        }
    }
}

extension OrgRole {
    /// This role as the generated API's `OrgRole`.
    public var generated: Components.Schemas.OrgRole { Components.Schemas.OrgRole(rawValue: rawValue) ?? .member }
}

extension TeamRole {
    /// This role as the generated API's `TeamRole`.
    public var generated: Components.Schemas.TeamRole { Components.Schemas.TeamRole(rawValue: rawValue) ?? .member }
}

extension TeamJoin {
    /// This rule as the generated API's `TeamJoin`.
    public var generated: Components.Schemas.TeamJoin { Components.Schemas.TeamJoin(rawValue: rawValue) ?? .invite }
}

extension ConsentRequest {
    /// This request as the body of `client.consent.pageCreate(body:)`, every
    /// kind. ``CalimeroClient/requestConsent(_:)`` sends the same body and also
    /// checks the request and the node's confirmation code.
    public func generated() throws -> Operations.ConsentPageCreate.Input.Body.JsonPayload {
        try MeroJSON.decode(Operations.ConsentPageCreate.Input.Body.JsonPayload.self, from: CanonicalJSON.data(json))
    }
}

extension JSONValue {
    /// A generated value (a facade method's answer) as JSON, to read as one
    /// of MeroKit's types: `OrgInfo(json: try JSONValue(encoding: try await client.orgs.get(org: id)))`.
    public init(encoding value: some Encodable) throws {
        self = try MeroJSON.decode(JSONValue.self, from: try MeroJSON.encode(value))
    }
}

extension MeroJSON {
    /// A generated value (a facade method's answer) as one of MeroKit's
    /// `Decodable` types, through its JSON:
    /// `try MeroJSON.convert(try await client.consent.pageResult(request: id), to: ConsentResult.self)`.
    public static func convert<T: Decodable>(_ value: some Encodable, to type: T.Type) throws -> T {
        try decode(type, from: try encode(value))
    }
}
