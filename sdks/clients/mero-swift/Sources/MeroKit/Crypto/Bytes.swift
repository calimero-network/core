import Foundation

/// Hex helpers shared by everything in the account layer that signs.
///
/// Byte-identical with mero-js `crypto/internal.ts`: lowercase output, and
/// sized decoding that refuses a truncated paste at the edge rather than deep
/// inside a signature check.
public enum Hex {
    /// Lowercase hex of `bytes`.
    public static func encode<B: Sequence>(_ bytes: B) -> String where B.Element == UInt8 {
        var out = ""
        out.reserveCapacity(64)
        for byte in bytes {
            out.append(Self.digits[Int(byte >> 4)])
            out.append(Self.digits[Int(byte & 0x0F)])
        }
        return out
    }

    /// Decode exactly `bytes` bytes of hex, or throw naming `label`.
    public static func decode(_ value: String, label: String, bytes: Int) throws -> Data {
        let clean = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard clean.count == bytes * 2, let data = decodeUnchecked(clean) else {
            throw AccountError.invalidInput(
                "\(label) must be \(bytes * 2) hex characters, got \(clean.count)")
        }
        return data
    }

    /// Decode hex of any even length (for already-encoded variable structures).
    public static func decodeUnsized(_ value: String, label: String) throws -> Data {
        let clean = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard clean.count % 2 == 0, let data = decodeUnchecked(clean) else {
            throw AccountError.invalidInput("\(label) must be an even number of hex characters")
        }
        return data
    }

    /// `value` is 64 hex characters (any case).
    public static func is32(_ value: String) -> Bool {
        value.count == 64 && decodeUnchecked(value) != nil
    }

    private static let digits: [Character] = Array("0123456789abcdef")

    private static func nibble(_ c: UInt8) -> UInt8? {
        switch c {
        case 0x30...0x39: return c - 0x30
        case 0x61...0x66: return c - 0x61 + 10
        case 0x41...0x46: return c - 0x41 + 10
        default: return nil
        }
    }

    private static func decodeUnchecked(_ s: String) -> Data? {
        let utf8 = Array(s.utf8)
        guard utf8.count % 2 == 0 else { return nil }
        var out = Data(capacity: utf8.count / 2)
        var i = 0
        while i < utf8.count {
            guard let hi = nibble(utf8[i]), let lo = nibble(utf8[i + 1]) else { return nil }
            out.append(hi << 4 | lo)
            i += 2
        }
        return out
    }
}

/// A little-endian, borsh-style byte writer.
///
/// Borsh for fixed-width data is plain concatenation; the only framing is a
/// `u32` LE length in front of strings and vectors, and a `0`/`1` tag in front
/// of an `Option`. Every wire layout in the account layer is built from these.
public struct BorshWriter: Sendable {
    public private(set) var data = Data()

    public init() {}

    public mutating func u8(_ value: UInt8) { data.append(value) }

    public mutating func u32(_ value: UInt32) {
        withUnsafeBytes(of: value.littleEndian) { data.append(contentsOf: $0) }
    }

    public mutating func u64(_ value: UInt64) {
        withUnsafeBytes(of: value.littleEndian) { data.append(contentsOf: $0) }
    }

    /// Raw bytes, no length prefix (fixed-width fields).
    public mutating func raw(_ bytes: Data) { data.append(bytes) }

    /// A borsh `Vec<u8>` / `String`: `u32` length, then the bytes.
    public mutating func bytes(_ bytes: Data) {
        u32(UInt32(bytes.count))
        data.append(bytes)
    }

    /// A borsh `String` (UTF-8).
    public mutating func string(_ value: String) { bytes(Data(value.utf8)) }

    /// A borsh `Option<Vec<u8>>`-style field: `0`, or `1` + `u32` length + bytes.
    public mutating func optionBytes(_ value: Data?) {
        guard let value else { return u8(0) }
        u8(1)
        bytes(value)
    }
}

/// Little-endian encodings, as standalone values (for `domainHash` parts).
public enum LittleEndian {
    public static func u32(_ value: UInt32) -> Data {
        withUnsafeBytes(of: value.littleEndian) { Data($0) }
    }

    public static func u64(_ value: UInt64) -> Data {
        withUnsafeBytes(of: value.littleEndian) { Data($0) }
    }
}
