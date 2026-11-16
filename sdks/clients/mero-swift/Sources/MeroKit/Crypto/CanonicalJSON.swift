import Foundation

/// A compact, deterministic JSON serialization of ``JSONValue``.
///
/// A warrant commits to its arguments by hash (`intentHash` /
/// `creationInitHash`), so the bytes hashed and the bytes sent must be the same
/// bytes the relay re-derives. mero-js uses `JSON.stringify`; Swift dictionaries
/// have no insertion order, so this writes object keys **sorted** — what
/// `serde_json` writes for a `Value` — and sends the request body in the same
/// spelling (see ``RelayClient``), so either reading of the arguments agrees.
///
/// Spelling rules follow `JSON.stringify` / `serde_json`: no whitespace, `/`
/// not escaped, non-ASCII written as UTF-8, control characters as `\n`, `\t`,
/// … or `\u00XX`, integral numbers without a fraction.
public enum CanonicalJSON {
    public static func string(_ value: JSONValue) -> String {
        var out = ""
        write(value, into: &out)
        return out
    }

    public static func data(_ value: JSONValue) -> Data {
        Data(string(value).utf8)
    }

    private static func write(_ value: JSONValue, into out: inout String) {
        switch value {
        case .null: out += "null"
        case .bool(let b): out += b ? "true" : "false"
        case .number(let n): out += number(n)
        case .string(let s): writeString(s, into: &out)
        case .array(let items):
            out += "["
            for (i, item) in items.enumerated() {
                if i > 0 { out += "," }
                write(item, into: &out)
            }
            out += "]"
        case .object(let fields):
            out += "{"
            for (i, key) in fields.keys.sorted().enumerated() {
                if i > 0 { out += "," }
                writeString(key, into: &out)
                out += ":"
                write(fields[key] ?? .null, into: &out)
            }
            out += "}"
        }
    }

    private static func number(_ n: Double) -> String {
        guard n.isFinite else { return "null" }
        if n == n.rounded(), abs(n) < 1e15 {
            return String(Int64(n))
        }
        return "\(n)"
    }

    /// `s` as a JSON string literal, spelled as `JSON.stringify` spells it.
    static func quote(_ s: String) -> String {
        var out = ""
        writeString(s, into: &out)
        return out
    }

    private static func writeString(_ s: String, into out: inout String) {
        out += "\""
        for scalar in s.unicodeScalars {
            switch scalar {
            case "\"": out += "\\\""
            case "\\": out += "\\\\"
            case "\n": out += "\\n"
            case "\r": out += "\\r"
            case "\t": out += "\\t"
            case "\u{08}": out += "\\b"
            case "\u{0C}": out += "\\f"
            default:
                if scalar.value < 0x20 {
                    out += String(format: "\\u%04x", scalar.value)
                } else {
                    out.unicodeScalars.append(scalar)
                }
            }
        }
        out += "\""
    }
}
