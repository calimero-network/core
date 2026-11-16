import Foundation

/// JSON read and written with every integer kept exactly as spelled.
///
/// ``JSONValue`` holds a number as a `Double`, so an integer beyond 2^53 (a
/// `u64` nanosecond timestamp, `u64::MAX`) loses digits on the way through it.
/// The relay's byte-level calls (``RelayClient/queryJSON(contextId:method:argsJSON:)``,
/// ``RelayClient/executeJSON(contextId:method:argsJSON:)``) go through this
/// instead: arguments and results stay text from end to end.
///
/// ``canonical(_:)`` spells a document as ``CanonicalJSON`` spells a
/// ``JSONValue`` (no whitespace, object keys sorted, the same string escapes),
/// so a warrant's `intentHash` over it is the hash the relay re-derives; an
/// integer literal is written as given, and any other number as
/// ``CanonicalJSON`` writes its `Double`.
enum LiteralJSON {
    indirect enum Node: Equatable {
        case null
        case bool(Bool)
        /// The literal as it appeared.
        case number(String)
        case string(String)
        case array([Node])
        case object([String: Node])

        subscript(key: String) -> Node? {
            if case .object(let fields) = self { return fields[key] }
            return nil
        }
    }

    struct ParseError: Error, CustomStringConvertible {
        let description: String
    }

    /// `data` in canonical spelling. Throws ``ParseError`` when it is not one JSON value.
    static func canonical(_ data: Data) throws -> Data {
        Data(write(try parse(data)).utf8)
    }

    static func parse(_ data: Data) throws -> Node {
        var parser = Parser(bytes: Array(data))
        parser.skipSpace()
        let node = try parser.value(depth: 0)
        parser.skipSpace()
        guard parser.index == parser.bytes.count else { throw parser.fail("trailing characters") }
        return node
    }

    static func write(_ node: Node) -> String {
        var out = ""
        write(node, into: &out)
        return out
    }

    private static func write(_ node: Node, into out: inout String) {
        switch node {
        case .null: out += "null"
        case .bool(let b): out += b ? "true" : "false"
        case .number(let literal): out += number(literal)
        case .string(let s): out += CanonicalJSON.quote(s)
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
                out += CanonicalJSON.quote(key)
                out += ":"
                write(fields[key] ?? .null, into: &out)
            }
            out += "}"
        }
    }

    /// An integer literal as given (`-0` as `0`); anything else as ``CanonicalJSON`` writes it.
    private static func number(_ literal: String) -> String {
        let digits = literal.hasPrefix("-") ? literal.dropFirst() : Substring(literal)
        if !digits.isEmpty, digits.allSatisfy({ $0.isASCII && $0.isNumber }) {
            return literal == "-0" ? "0" : literal
        }
        return CanonicalJSON.string(.number(Double(literal) ?? .nan))
    }

    private struct Parser {
        let bytes: [UInt8]
        var index = 0

        static let maxDepth = 512

        func fail(_ what: String) -> ParseError {
            ParseError(description: "\(what) at byte \(index)")
        }

        mutating func skipSpace() {
            while index < bytes.count, [0x20, 0x09, 0x0A, 0x0D].contains(bytes[index]) { index += 1 }
        }

        mutating func value(depth: Int) throws -> Node {
            guard depth < Self.maxDepth else { throw fail("nesting too deep") }
            guard index < bytes.count else { throw fail("unexpected end") }
            switch bytes[index] {
            case UInt8(ascii: "{"): return try object(depth: depth)
            case UInt8(ascii: "["): return try array(depth: depth)
            case UInt8(ascii: "\""): return .string(try string())
            case UInt8(ascii: "t"): try word("true"); return .bool(true)
            case UInt8(ascii: "f"): try word("false"); return .bool(false)
            case UInt8(ascii: "n"): try word("null"); return .null
            default: return .number(try number())
            }
        }

        mutating func word(_ text: String) throws {
            let expected = Array(text.utf8)
            guard index + expected.count <= bytes.count, Array(bytes[index..<index + expected.count]) == expected
            else { throw fail("expected \(text)") }
            index += expected.count
        }

        mutating func object(depth: Int) throws -> Node {
            index += 1
            var fields: [String: Node] = [:]
            skipSpace()
            if index < bytes.count, bytes[index] == UInt8(ascii: "}") {
                index += 1
                return .object(fields)
            }
            while true {
                skipSpace()
                guard index < bytes.count, bytes[index] == UInt8(ascii: "\"") else { throw fail("expected a key") }
                let key = try string()
                skipSpace()
                guard index < bytes.count, bytes[index] == UInt8(ascii: ":") else { throw fail("expected :") }
                index += 1
                skipSpace()
                fields[key] = try value(depth: depth + 1)
                skipSpace()
                guard index < bytes.count else { throw fail("unexpected end") }
                if bytes[index] == UInt8(ascii: ",") {
                    index += 1
                    continue
                }
                guard bytes[index] == UInt8(ascii: "}") else { throw fail("expected , or }") }
                index += 1
                return .object(fields)
            }
        }

        mutating func array(depth: Int) throws -> Node {
            index += 1
            var items: [Node] = []
            skipSpace()
            if index < bytes.count, bytes[index] == UInt8(ascii: "]") {
                index += 1
                return .array(items)
            }
            while true {
                skipSpace()
                items.append(try value(depth: depth + 1))
                skipSpace()
                guard index < bytes.count else { throw fail("unexpected end") }
                if bytes[index] == UInt8(ascii: ",") {
                    index += 1
                    continue
                }
                guard bytes[index] == UInt8(ascii: "]") else { throw fail("expected , or ]") }
                index += 1
                return .array(items)
            }
        }

        mutating func number() throws -> String {
            let start = index
            func digit(_ i: Int) -> Bool { i < bytes.count && bytes[i] >= 0x30 && bytes[i] <= 0x39 }
            if index < bytes.count, bytes[index] == UInt8(ascii: "-") { index += 1 }
            guard digit(index) else { throw fail("expected a value") }
            if bytes[index] == UInt8(ascii: "0") {
                index += 1
            } else {
                while digit(index) { index += 1 }
            }
            if index < bytes.count, bytes[index] == UInt8(ascii: ".") {
                index += 1
                guard digit(index) else { throw fail("expected a digit") }
                while digit(index) { index += 1 }
            }
            if index < bytes.count, bytes[index] == UInt8(ascii: "e") || bytes[index] == UInt8(ascii: "E") {
                index += 1
                if index < bytes.count, bytes[index] == UInt8(ascii: "+") || bytes[index] == UInt8(ascii: "-") {
                    index += 1
                }
                guard digit(index) else { throw fail("expected a digit") }
                while digit(index) { index += 1 }
            }
            return String(decoding: bytes[start..<index], as: UTF8.self)
        }

        mutating func string() throws -> String {
            index += 1
            var scalars = String.UnicodeScalarView()
            var run = index
            func flush(_ end: Int) throws {
                guard run < end else { return }
                guard let text = String(bytes: bytes[run..<end], encoding: .utf8) else { throw fail("bad UTF-8") }
                scalars.append(contentsOf: text.unicodeScalars)
            }
            while true {
                guard index < bytes.count else { throw fail("unterminated string") }
                let byte = bytes[index]
                if byte == UInt8(ascii: "\"") {
                    try flush(index)
                    index += 1
                    return String(scalars)
                }
                if byte < 0x20 { throw fail("control character in string") }
                if byte != UInt8(ascii: "\\") {
                    index += 1
                    continue
                }
                try flush(index)
                index += 1
                guard index < bytes.count else { throw fail("unterminated escape") }
                let escape = bytes[index]
                index += 1
                switch escape {
                case UInt8(ascii: "\""): scalars.append("\"")
                case UInt8(ascii: "\\"): scalars.append("\\")
                case UInt8(ascii: "/"): scalars.append("/")
                case UInt8(ascii: "b"): scalars.append("\u{08}")
                case UInt8(ascii: "f"): scalars.append("\u{0C}")
                case UInt8(ascii: "n"): scalars.append("\n")
                case UInt8(ascii: "r"): scalars.append("\r")
                case UInt8(ascii: "t"): scalars.append("\t")
                case UInt8(ascii: "u"):
                    var code = try hex4()
                    if (0xD800..<0xDC00).contains(code) {
                        guard index + 1 < bytes.count, bytes[index] == UInt8(ascii: "\\"),
                            bytes[index + 1] == UInt8(ascii: "u")
                        else { throw fail("unpaired surrogate") }
                        index += 2
                        let low = try hex4()
                        guard (0xDC00..<0xE000).contains(low) else { throw fail("unpaired surrogate") }
                        code = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00)
                    }
                    guard let scalar = Unicode.Scalar(code) else { throw fail("bad \\u escape") }
                    scalars.append(scalar)
                default: throw fail("bad escape")
                }
                run = index
            }
        }

        mutating func hex4() throws -> UInt32 {
            guard index + 4 <= bytes.count,
                let code = UInt32(String(decoding: bytes[index..<index + 4], as: UTF8.self), radix: 16)
            else { throw fail("bad \\u escape") }
            index += 4
            return code
        }
    }
}
