import Foundation

// An app instance's contract, as an app on a phone reaches it. In Calimero
// 1.0 a read is a view (`POST /apps/{app}/views`) and a write an intent
// (`POST /apps/{app}/intents`), sealed to an attested node. Every app makes
// the same two calls; only the method names differ, and those belong to the
// app (or to the client mero-devtools-js generates for it, over
// ``ContextContractCalling``). A "context" here is an app instance: its id is
// the instance's app id (the node also takes its context id,
// `SHA-256(app_id)` hex).

/// The two ways an app reaches the contract in a context.
/// (== Kotlin `com.calimero.mero.app.ContextContract`.)
///
/// ``CloudContextContract`` is the real one; ``SignedOutContextContract``
/// stands in while signed out. ``bound(to:)`` fixes the context, for
/// a generated client.
public protocol ContextContract: Sendable {
    /// Run a view (`#[app::view]`) as the account: `POST /apps/{app}/views`.
    func query(contextId: String, method: String, args: JSONValue) async throws -> JSONValue?
    /// Run a mutation as the account: `POST /apps/{app}/intents`.
    func execute(contextId: String, method: String, args: JSONValue) async throws -> JSONValue?

    /// ``query(contextId:method:args:)`` with JSON bytes in and out, every
    /// number kept as written: what ``BoundContextContract`` calls. `argsJSON`
    /// is canonical JSON; the result is the return value (`null` for none).
    func query(contextId: String, method: String, argsJSON: Data) async throws -> Data
    /// ``execute(contextId:method:args:)`` with JSON bytes in and out, as
    /// ``query(contextId:method:argsJSON:)``.
    func execute(contextId: String, method: String, argsJSON: Data) async throws -> Data
}

extension ContextContract {
    /// This contract with `contextId` fixed: what a generated client calls.
    public func bound(to contextId: String) -> BoundContextContract {
        BoundContextContract(contract: self, contextId: contextId)
    }

    /// Through ``query(contextId:method:args:)``, so through ``JSONValue``:
    /// an integer beyond 2^53 loses digits. ``CloudContextContract`` keeps
    /// them; a contract of your own should implement this to keep them too.
    public func query(contextId: String, method: String, argsJSON: Data) async throws -> Data {
        let result = try await query(contextId: contextId, method: method, args: try Self.jsonValue(argsJSON))
        return CanonicalJSON.data(result ?? .null)
    }

    /// Through ``execute(contextId:method:args:)``, as ``query(contextId:method:argsJSON:)``.
    public func execute(contextId: String, method: String, argsJSON: Data) async throws -> Data {
        let result = try await execute(contextId: contextId, method: method, args: try Self.jsonValue(argsJSON))
        return CanonicalJSON.data(result ?? .null)
    }

    static func jsonValue(_ data: Data) throws -> JSONValue {
        do {
            return try JSONDecoder().decode(JSONValue.self, from: data)
        } catch {
            throw AccountError.invalidInput("argsJSON is not JSON: \(error.localizedDescription)")
        }
    }
}

/// ``ContextContract`` over a ``CalimeroClient``. (== Kotlin `CloudContextContract`.)
///
/// - `query` is ``CalimeroClient/view(_:_:_:stale:minIndex:)`` (strong);
///   the method's return value is the view's `result`.
/// - `execute` is ``CalimeroClient/call(_:_:_:idempotencyKey:)``: an intent,
///   retried on a 503 with one idempotency key; the method's return value is
///   the call's `result`. An app's refusal throws ``AppError``.
///
/// The byte-level calls keep every number exactly as written, both ways.
public struct CloudContextContract: ContextContract {
    public let client: CalimeroClient

    public init(client: CalimeroClient) {
        self.client = client
    }

    public func query(contextId: String, method: String, args: JSONValue) async throws -> JSONValue? {
        let result = try await client.view(contextId, method, args).result
        return result == .null ? nil : result
    }

    public func execute(contextId: String, method: String, args: JSONValue) async throws -> JSONValue? {
        let result = try await client.call(contextId, method, args).result
        return result == .null ? nil : result
    }

    public func query(contextId: String, method: String, argsJSON: Data) async throws -> Data {
        try Self.result(try await client.viewJSON(contextId, method, argsJSON: argsJSON))
    }

    public func execute(contextId: String, method: String, argsJSON: Data) async throws -> Data {
        try Self.result(try await client.callJSON(contextId, method, argsJSON: argsJSON))
    }

    /// The `result` of an answer, byte for byte (`null` when absent).
    static func result(_ answer: Data) throws -> Data {
        let node: LiteralJSON.Node
        do {
            node = try LiteralJSON.parse(answer)
        } catch {
            throw AccountError.protocolViolation("the node's answer is not JSON")
        }
        return Data(LiteralJSON.write(node["result"] ?? .null).utf8)
    }
}

/// No one is signed in: there is no account to call the contract as.
/// (== Kotlin `SignedOutException`; 0.x: `AccountError.noRelay`.)
public struct SignedOutError: Error, LocalizedError, Equatable, Sendable {
    /// The method that was called.
    public let method: String

    public init(method: String) {
        self.method = method
    }

    public var errorDescription: String? { "\(method) needs a signed-in account" }
}

/// The contract while no one is signed in: every call throws
/// ``SignedOutError``, naming the method. (== Kotlin `SignedOutContextContract`.)
public struct SignedOutContextContract: ContextContract {
    public init() {}

    public func query(contextId: String, method: String, args: JSONValue) async throws -> JSONValue? {
        throw SignedOutError(method: method)
    }

    public func execute(contextId: String, method: String, args: JSONValue) async throws -> JSONValue? {
        throw SignedOutError(method: method)
    }

    public func query(contextId: String, method: String, argsJSON: Data) async throws -> Data {
        throw SignedOutError(method: method)
    }

    public func execute(contextId: String, method: String, argsJSON: Data) async throws -> Data {
        throw SignedOutError(method: method)
    }
}

/// One context's contract, in the shape a generated client calls: JSON in,
/// JSON out, as UTF-8 bytes. (== Kotlin `ContextContractCalling`, which takes
/// and returns strings.)
///
/// ``BoundContextContract`` is the SDK's; ``ContextContract/bound(to:)`` makes one.
public protocol ContextContractCalling: Sendable {
    /// Run a view: `argsJSON` is the argument object; the result is the
    /// method's return value as JSON (`null` when it returns nothing).
    func query(method: String, argsJSON: Data) async throws -> Data
    /// Run a mutation: as ``query(method:argsJSON:)``, as an intent.
    func execute(method: String, argsJSON: Data) async throws -> Data
}

/// A ``ContextContract`` with its context fixed. (== Kotlin `BoundContextContract`.)
///
/// JSON passes through as text, never as a ``JSONValue``: an integer beyond
/// 2^53 (a `u64` nanosecond timestamp, `u64::MAX`) reaches the contract and
/// comes back exactly as written. The arguments are checked and put in
/// canonical spelling (keys sorted, no whitespace) before the call; empty
/// `argsJSON` is `{}`, and anything that is not JSON is refused
/// (``AccountError/invalidInput(_:)``) before a request is made.
public struct BoundContextContract: ContextContractCalling {
    public let contract: any ContextContract
    public let contextId: String

    public init(contract: any ContextContract, contextId: String) {
        self.contract = contract
        self.contextId = contextId
    }

    public func query(method: String, argsJSON: Data) async throws -> Data {
        try await contract.query(contextId: contextId, method: method, argsJSON: try Self.args(argsJSON))
    }

    public func execute(method: String, argsJSON: Data) async throws -> Data {
        try await contract.execute(contextId: contextId, method: method, argsJSON: try Self.args(argsJSON))
    }

    /// `data` in canonical spelling, `{}` when empty.
    static func args(_ data: Data) throws -> Data {
        if data.allSatisfy({ $0 == 0x20 || $0 == 0x0A || $0 == 0x0D || $0 == 0x09 }) { return Data("{}".utf8) }
        do {
            return try LiteralJSON.canonical(data)
        } catch {
            throw AccountError.invalidInput("argsJSON is not JSON: \(error)")
        }
    }
}
