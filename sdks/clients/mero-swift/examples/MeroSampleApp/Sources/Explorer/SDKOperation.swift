import Foundation
import MeroKit

/// One input field for an operation form.
struct OpField: Identifiable, Sendable {
    enum Kind: Sendable { case line, multiline }
    let id: String
    let label: String
    let placeholder: String
    let kind: Kind

    static func line(_ id: String, _ label: String, _ ph: String = "") -> OpField {
        OpField(id: id, label: label, placeholder: ph, kind: .line)
    }
    static func json(_ id: String = "body", _ label: String = "Request JSON", _ ph: String = "{}") -> OpField {
        OpField(id: id, label: label, placeholder: ph, kind: .multiline)
    }
}

/// A single invokable SDK method: metadata, input fields, and an async
/// runner over the signed-in `CalimeroClient` that returns a rendered
/// (pretty-printed) result.
struct SDKOperation: Identifiable, Sendable {
    let id: String
    let category: String
    let name: String
    let summary: String
    let fields: [OpField]
    /// Runs over the client, the form's inputs and the signed-in passkey,
    /// which approves step-ups (`412 CONFIRM_REQUIRED`).
    let run: @Sendable (CalimeroClient, [String: String], (any PasskeyProvider)?) async throws -> String

    init(
        _ id: String, _ category: String, _ name: String, _ summary: String, fields: [OpField] = [],
        run: @escaping @Sendable (CalimeroClient, [String: String]) async throws -> String
    ) {
        self.init(id, category, name, summary, fields: fields, stepUp: { c, i, _ in try await run(c, i) })
    }

    /// An operation the node approves with the account's passkey.
    init(
        _ id: String, _ category: String, _ name: String, _ summary: String, fields: [OpField] = [],
        stepUp run:
            @escaping @Sendable (CalimeroClient, [String: String], (any PasskeyProvider)?) async throws
            -> String
    ) {
        self.id = id
        self.category = category
        self.name = name
        self.summary = summary
        self.fields = fields
        self.run = run
    }
}

/// The passkey a step-up needs, or a refusal to run without one.
func approver(_ passkey: (any PasskeyProvider)?) throws -> any PasskeyProvider {
    guard let passkey else { throw AccountError.notSignedIn("this needs your passkey's approval") }
    return passkey
}

// MARK: - Rendering / decoding helpers

enum Fmt {
    /// Parse a JSON field into a `JSONValue` (empty → `{}`).
    static func value(_ s: String) throws -> JSONValue {
        try decode(s, JSONValue.self)
    }

    static func json<T: Encodable>(_ value: T) -> String {
        if let value = value as? JSONValue, value == .null { return "null" }
        let enc = JSONEncoder()
        enc.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        if let data = try? enc.encode(value), let str = String(data: data, encoding: .utf8) { return str }
        return String(describing: value)
    }

    /// Decode a user-entered JSON string into a request type (empty → `{}`).
    static func decode<T: Decodable>(_ s: String, _ type: T.Type) throws -> T {
        let text = s.trimmingCharacters(in: .whitespacesAndNewlines)
        return try JSONDecoder().decode(T.self, from: Data((text.isEmpty ? "{}" : text).utf8))
    }
}

extension Dictionary where Key == String, Value == String {
    /// Trimmed value for a field id (empty string if absent).
    func v(_ key: String) -> String { (self[key] ?? "").trimmingCharacters(in: .whitespacesAndNewlines) }
    /// Optional trimmed value (nil if empty).
    func opt(_ key: String) -> String? { let s = v(key); return s.isEmpty ? nil : s }
}
