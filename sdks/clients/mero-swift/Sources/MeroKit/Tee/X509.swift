import CryptoKit
import Foundation

// Just enough DER and X.509 to check a Sigstore signing certificate and an
// Intel PCK chain with CryptoKit: read a certificate's signed part, validity,
// key and extensions, read a CRL, and verify the ECDSA signatures (P-256 and
// P-384) both use. Port of mero-js `src/sealed/x509.ts`.
//
// Written here, rather than taken from a library, so the SDK keeps no
// dependencies. It parses strictly and throws on anything it does not expect;
// it is not a general X.509 implementation. Internal, as in mero-js.

/// Why a TEE trust check refused: a quote, a certificate, a signed release, a
/// policy. The message says what did not hold.
public struct TeeTrustError: Error, LocalizedError, Sendable, Equatable, CustomStringConvertible {
    public let message: String

    public init(_ message: String) { self.message = message }

    public var errorDescription: String? { message }
    public var description: String { message }
}

/// The elliptic curves a certificate key may be on.
enum EcCurve: String, Sendable {
    case p256 = "P-256"
    case p384 = "P-384"

    /// The byte size of one signature half (`r` or `s`).
    var size: Int { self == .p256 ? 32 : 48 }
}

/// The hashes an ECDSA signature is made over.
enum EcdsaHash: String, Sendable {
    case sha256 = "SHA-256"
    case sha384 = "SHA-384"
    case sha512 = "SHA-512"
}

enum DerTag {
    static let boolean: UInt8 = 0x01
    static let integer: UInt8 = 0x02
    static let bitString: UInt8 = 0x03
    static let octetString: UInt8 = 0x04
    static let oid: UInt8 = 0x06
    static let utf8String: UInt8 = 0x0c
    static let sequence: UInt8 = 0x30
    static let utcTime: UInt8 = 0x17
    static let generalizedTime: UInt8 = 0x18
    static let context0: UInt8 = 0xa0
    static let context3: UInt8 = 0xa3
}

/// One DER element: its tag, and where it and its contents lie in `bytes`.
struct DerElement: Sendable {
    let tag: UInt8
    let bytes: [UInt8]
    /// Offset of the element's first (tag) byte.
    let offset: Int
    /// Offset of its contents.
    let start: Int
    /// Offset just past it.
    let end: Int

    var contents: [UInt8] { Array(bytes[start..<end]) }
    /// The whole element, header included.
    var whole: [UInt8] { Array(bytes[offset..<end]) }
}

enum Der {
    /// One element at `offset`, which must end by `limit`. `strict` also
    /// refuses a length not written in its shortest form, as DER requires.
    static func read(_ bytes: [UInt8], at offset: Int, limit: Int, strict: Bool = false) throws -> DerElement {
        guard offset + 2 <= limit else { throw TeeTrustError("it is truncated") }
        let tag = bytes[offset]
        guard tag & 0x1f != 0x1f else { throw TeeTrustError("it uses a tag this parser does not read") }
        var length = Int(bytes[offset + 1])
        var start = offset + 2
        if length & 0x80 != 0 {
            let count = length & 0x7f
            guard count != 0, count <= 4, start + count <= limit else {
                throw TeeTrustError("it has a length this parser does not read")
            }
            length = 0
            for i in 0..<count { length = length * 256 + Int(bytes[start + i]) }
            if strict, length < 0x80 || bytes[start] == 0 {
                throw TeeTrustError("it has a length that is not DER")
            }
            start += count
        }
        let end = start + length
        guard end <= limit else { throw TeeTrustError("it is truncated") }
        return DerElement(tag: tag, bytes: bytes, offset: offset, start: start, end: end)
    }

    /// Parse one complete DER element, refusing trailing bytes.
    static func parse(_ bytes: [UInt8], strict: Bool = false) throws -> DerElement {
        let element = try read(bytes, at: 0, limit: bytes.count, strict: strict)
        guard element.end == bytes.count else { throw TeeTrustError("it has trailing bytes") }
        return element
    }

    static func children(_ element: DerElement, strict: Bool = false) throws -> [DerElement] {
        var out: [DerElement] = []
        var offset = element.start
        while offset < element.end {
            let child = try read(element.bytes, at: offset, limit: element.end, strict: strict)
            out.append(child)
            offset = child.end
        }
        return out
    }

    /// `element` if it has `tag`; otherwise throws naming `what`.
    @discardableResult
    static func expect(_ element: DerElement?, _ tag: UInt8, _ what: String) throws -> DerElement {
        guard let element, element.tag == tag else { throw TeeTrustError("\(what) is not where it should be") }
        return element
    }

    /// An object identifier, dotted.
    static func oid(_ element: DerElement?) throws -> String {
        let value = try expect(element, DerTag.oid, "An object identifier").contents
        guard let last = value.last, last & 0x80 == 0 else {
            throw TeeTrustError("it has a malformed object identifier")
        }
        var arcs: [UInt64] = []
        var arc: UInt64 = 0
        for byte in value {
            guard arc < (1 << 56) else { throw TeeTrustError("it has a malformed object identifier") }
            arc = arc * 128 + UInt64(byte & 0x7f)
            if byte & 0x80 == 0 {
                arcs.append(arc)
                arc = 0
            }
        }
        let first = arcs.removeFirst()
        let head: [UInt64] = first < 80 ? [first / 40, first % 40] : [2, first - 80]
        return (head + arcs).map { String($0) }.joined(separator: ".")
    }

    /// A `UTCTime` or `GeneralizedTime`, in milliseconds since the epoch.
    static func time(_ element: DerElement) throws -> Int64 {
        let text = String(decoding: element.contents, as: UTF8.self)
        let digits = Array(text.utf8)
        let yearDigits: Int
        switch element.tag {
        case DerTag.utcTime: yearDigits = 2
        case DerTag.generalizedTime: yearDigits = 4
        default: yearDigits = 0
        }
        let refusal = TeeTrustError(
            "it holds the time \(CanonicalJSON.quote(text)), which is not one a certificate may hold")
        guard yearDigits > 0, digits.count == yearDigits + 11, digits.last == UInt8(ascii: "Z"),
            digits.dropLast().allSatisfy({ (0x30...0x39).contains($0) })
        else { throw refusal }
        func number(_ from: Int, _ count: Int) -> Int {
            digits[from..<from + count].reduce(0) { $0 * 10 + Int($1 - 0x30) }
        }
        var year = number(0, yearDigits)
        if yearDigits == 2 { year += year < 50 ? 2000 : 1900 }
        let at = yearDigits
        return IsoTime.utcMillis(
            year: year, month: number(at, 2), day: number(at + 2, 2), hour: number(at + 4, 2),
            minute: number(at + 6, 2), second: number(at + 8, 2))
    }

    /// The object identifiers in a DER `SEQUENCE OF OBJECT IDENTIFIER` (an extended key usage).
    static func objectIdentifiers(_ der: [UInt8]) throws -> [String] {
        try children(try expect(try parse(der), DerTag.sequence, "The list")).map { try oid($0) }
    }
}

/// UTC times as mero-js reads and prints them (`Date.UTC`, `toISOString`).
enum IsoTime {
    /// Milliseconds since the epoch of a proleptic-Gregorian UTC time.
    static func utcMillis(
        year: Int, month: Int, day: Int, hour: Int, minute: Int, second: Int, millis: Int = 0
    ) -> Int64 {
        // Days from civil (Howard Hinnant's algorithm).
        let y = month <= 2 ? year - 1 : year
        let era = (y >= 0 ? y : y - 399) / 400
        let yoe = y - era * 400
        let shifted = month > 2 ? month - 3 : month + 9
        let doy = (153 * shifted + 2) / 5 + day - 1
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy
        let days = Int64(era * 146_097 + doe - 719_468)
        let seconds = days * 86_400 + Int64(hour * 3600 + minute * 60 + second)
        return seconds * 1000 + Int64(millis)
    }

    /// `YYYY-MM-DDTHH:MM:SS[.fff]Z` in milliseconds, or `nil`.
    static func parse(_ text: String) -> Int64? {
        let b = Array(text.utf8)
        guard b.count >= 20, b.last == UInt8(ascii: "Z"), b[4] == UInt8(ascii: "-"), b[7] == UInt8(ascii: "-"),
            b[10] == UInt8(ascii: "T"), b[13] == UInt8(ascii: ":"), b[16] == UInt8(ascii: ":")
        else { return nil }
        func number(_ from: Int, _ count: Int) -> Int? {
            var value = 0
            for i in from..<from + count {
                guard (0x30...0x39).contains(b[i]) else { return nil }
                value = value * 10 + Int(b[i] - 0x30)
            }
            return value
        }
        guard let year = number(0, 4), let month = number(5, 2), let day = number(8, 2), let hour = number(11, 2),
            let minute = number(14, 2), let second = number(17, 2)
        else { return nil }
        var millis = 0
        if b.count > 20 {
            let places = b.count - 21
            guard b[19] == UInt8(ascii: "."), places >= 1, number(20, places) != nil else { return nil }
            // The first three fraction digits, padded: `.5` is 500 ms, `.999999` is 999.
            let digits = (Array(b[20..<20 + places].prefix(3)) + [UInt8](repeating: 0x30, count: 3)).prefix(3)
            millis = digits.reduce(0) { $0 * 10 + Int($1 - 0x30) }
        } else if b.count != 20 {
            return nil
        }
        return utcMillis(year: year, month: month, day: day, hour: hour, minute: minute, second: second, millis: millis)
    }

    /// `toISOString` of `ms`: `2026-09-28T10:29:25.000Z`.
    static func string(_ ms: Int64) -> String {
        let date = Date(timeIntervalSince1970: TimeInterval(ms) / 1000)
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC") ?? calendar.timeZone
        let c = calendar.dateComponents([.year, .month, .day, .hour, .minute, .second], from: date)
        let millis = ((ms % 1000) + 1000) % 1000
        return String(
            format: "%04ld-%02ld-%02ldT%02ld:%02ld:%02ld.%03ldZ", c.year ?? 0, c.month ?? 0, c.day ?? 0, c.hour ?? 0,
            c.minute ?? 0, c.second ?? 0, Int(millis))
    }
}

/// A parsed X.509 certificate with an ECDSA key, signed with ECDSA.
/// (== mero-js `Certificate`.) Fields are `var` so a test can bend one.
struct X509Certificate: Sendable {
    var der: [UInt8]
    /// The signed part, exactly as the issuer signed it.
    var tbs: [UInt8]
    /// The hash of the ECDSA signature the issuer made over `tbs`.
    var signatureHash: EcdsaHash
    /// That signature, DER.
    var signature: [UInt8]
    /// The issuer and subject names, DER, to compare byte for byte.
    var issuer: [UInt8]
    var subject: [UInt8]
    /// Validity, in milliseconds since the epoch, both ends inclusive.
    var notBefore: Int64
    var notAfter: Int64
    /// The subject's key, DER SubjectPublicKeyInfo, and its curve.
    var spki: [UInt8]
    var curve: EcCurve
    /// The serial number's contents (a DER INTEGER's bytes).
    var serial: [UInt8]
    /// Each extension's value (the contents of its OCTET STRING), by object identifier.
    var extensions: [String: [UInt8]]
    /// The extensions marked critical.
    var critical: Set<String>
}

enum X509 {
    static let ecPublicKey = "1.2.840.10045.2.1"
    static let curves: [String: EcCurve] = ["1.2.840.10045.3.1.7": .p256, "1.3.132.0.34": .p384]
    /// The certificate signature algorithms accepted: ECDSA with SHA-256 or SHA-384.
    static let signatureHashes: [String: EcdsaHash] = ["1.2.840.10045.4.3.2": .sha256, "1.2.840.10045.4.3.3": .sha384]

    /// Parse a DER X.509 certificate with an ECDSA key, signed with ECDSA.
    /// Throws naming what is wrong. (== mero-js `parseCertificate`.)
    static func parseCertificate(_ der: [UInt8], strict: Bool = false) throws -> X509Certificate {
        do {
            let parsed = try Der.parse(der, strict: strict)
            let top = try Der.children(try Der.expect(parsed, DerTag.sequence, "The certificate"), strict: strict)
            let tbsElement = top.first
            let algorithm = top.count > 1 ? top[1] : nil
            let signature = top.count > 2 ? top[2] : nil
            guard top.count <= 3 else { throw TeeTrustError("it has fields after its signature") }
            let fields = try Der.children(
                try Der.expect(tbsElement, DerTag.sequence, "The signed part"), strict: strict)
            // `version` is [0] EXPLICIT; everything after it is positional.
            let first = fields.first?.tag == DerTag.context0 ? 1 : 0
            let positional = Array(fields.dropFirst(first))
            func field(_ index: Int) -> DerElement? { index < positional.count ? positional[index] : nil }
            let serial = try Der.expect(field(0), DerTag.integer, "The serial number")

            let algorithmId = try Der.oid(
                try Der.children(try Der.expect(algorithm, DerTag.sequence, "The signature algorithm")).first)
            let innerId = try Der.oid(
                try Der.children(try Der.expect(field(1), DerTag.sequence, "The signature algorithm")).first)
            guard innerId == algorithmId else { throw TeeTrustError("it names two different signature algorithms") }
            guard let signatureHash = signatureHashes[algorithmId] else {
                throw TeeTrustError("it is signed with \(algorithmId), not ECDSA with SHA-256 or SHA-384")
            }

            let validity = try Der.children(try Der.expect(field(3), DerTag.sequence, "The validity")).map(Der.time)
            guard validity.count >= 2 else { throw TeeTrustError("its validity is incomplete") }

            let spki = try Der.expect(field(5), DerTag.sequence, "The public key")
            let spkiParts = try Der.children(spki)
            try Der.expect(spkiParts.count > 1 ? spkiParts[1] : nil, DerTag.bitString, "The public key")
            let keyAlgorithm = try Der.children(
                try Der.expect(spkiParts.first, DerTag.sequence, "The key algorithm"))
            guard try Der.oid(keyAlgorithm.first) == ecPublicKey else {
                throw TeeTrustError("its key is not an elliptic-curve key")
            }
            guard let curve = curves[try Der.oid(keyAlgorithm.count > 1 ? keyAlgorithm[1] : nil)] else {
                throw TeeTrustError("its key is on a curve other than P-256 or P-384")
            }

            var extensions: [String: [UInt8]] = [:]
            var critical = Set<String>()
            if let extensionsField = positional.dropFirst(6).first(where: { $0.tag == DerTag.context3 }) {
                let list = try Der.children(extensionsField).first
                for extensionElement in try Der.children(try Der.expect(list, DerTag.sequence, "The extensions")) {
                    let parts = try Der.children(try Der.expect(extensionElement, DerTag.sequence, "An extension"))
                    var isCritical = false
                    if parts.count == 3 {
                        let flag = try Der.expect(parts[1], DerTag.boolean, "An extension's critical flag")
                        isCritical = flag.contents.first.map { $0 != 0 } ?? false
                    } else if parts.count != 2 {
                        throw TeeTrustError("it has a malformed extension")
                    }
                    let id = try Der.oid(parts[0])
                    guard extensions[id] == nil else { throw TeeTrustError("it has extension \(id) twice") }
                    let value = try Der.expect(parts[parts.count - 1], DerTag.octetString, "An extension value")
                    extensions[id] = value.contents
                    if isCritical { critical.insert(id) }
                }
            }

            let signatureBits = try Der.expect(signature, DerTag.bitString, "The signature").contents
            guard signatureBits.first == 0 else { throw TeeTrustError("its signature is not a whole number of bytes") }
            return X509Certificate(
                der: der, tbs: tbsElement?.whole ?? [], signatureHash: signatureHash,
                signature: Array(signatureBits.dropFirst()),
                issuer: try Der.expect(field(2), DerTag.sequence, "The issuer").whole,
                subject: try Der.expect(field(4), DerTag.sequence, "The subject").whole,
                notBefore: validity[0], notAfter: validity[1], spki: spki.whole, curve: curve,
                serial: serial.contents, extensions: extensions, critical: critical)
        } catch let error as TeeTrustError {
            throw TeeTrustError("The certificate is malformed: \(error.message)")
        }
    }

    /// An ECDSA signature as CryptoKit takes it (IEEE P1363: `r ‖ s`, each the
    /// curve's size), from the DER `SEQUENCE { r INTEGER, s INTEGER }` X.509
    /// and cosign use. (== mero-js `ecdsaSignatureToP1363`.)
    static func ecdsaSignatureToP1363(_ der: [UInt8], curve: EcCurve) throws -> [UInt8] {
        let size = curve.size
        let refusal = TeeTrustError("The signature is not an ECDSA signature")
        let integers = try Der.children(try Der.expect(try Der.parse(der), DerTag.sequence, "The signature"))
        guard integers.count == 2 else { throw refusal }
        var out = [UInt8](repeating: 0, count: size * 2)
        for (index, integer) in integers.enumerated() {
            var value = try Der.expect(integer, DerTag.integer, "The signature").contents[...]
            // r and s are positive: a set top bit with no leading zero is a negative number.
            guard let top = value.first, top & 0x80 == 0 else { throw refusal }
            while value.count > 1, value.first == 0 { value = value.dropFirst() }
            guard value.count <= size else { throw refusal }
            let at = index * size + size - value.count
            out.replaceSubrange(at..<at + value.count, with: value)
        }
        return out
    }

    /// Verify a DER ECDSA `signature` over `data` with a DER SubjectPublicKeyInfo key.
    /// (== mero-js `verifyEcdsa`.)
    static func verifyEcdsa(
        spki: [UInt8], curve: EcCurve, hash: EcdsaHash, signature: [UInt8], data: [UInt8]
    ) -> Bool {
        guard let raw = try? ecdsaSignatureToP1363(signature, curve: curve) else { return false }
        return verifyRawEcdsa(spki: spki, curve: curve, hash: hash, signature: raw, data: data)
    }

    /// Verify a P1363 (`r ‖ s`) ECDSA signature with a DER SubjectPublicKeyInfo key.
    static func verifyRawEcdsa(
        spki: [UInt8], curve: EcCurve, hash: EcdsaHash, signature: [UInt8], data: [UInt8]
    ) -> Bool {
        let message = Data(data)
        switch curve {
        case .p256:
            guard let key = try? P256.Signing.PublicKey(derRepresentation: Data(spki)),
                let sig = try? P256.Signing.ECDSASignature(rawRepresentation: Data(signature))
            else { return false }
            switch hash {
            case .sha256: return key.isValidSignature(sig, for: SHA256.hash(data: message))
            case .sha384: return key.isValidSignature(sig, for: SHA384.hash(data: message))
            case .sha512: return key.isValidSignature(sig, for: SHA512.hash(data: message))
            }
        case .p384:
            guard let key = try? P384.Signing.PublicKey(derRepresentation: Data(spki)),
                let sig = try? P384.Signing.ECDSASignature(rawRepresentation: Data(signature))
            else { return false }
            switch hash {
            case .sha256: return key.isValidSignature(sig, for: SHA256.hash(data: message))
            case .sha384: return key.isValidSignature(sig, for: SHA384.hash(data: message))
            case .sha512: return key.isValidSignature(sig, for: SHA512.hash(data: message))
            }
        }
    }

    /// Decode base64 (standard alphabet), refusing anything else.
    /// (== mero-js `fromBase64`.)
    static func fromBase64(_ value: String, _ what: String) throws -> [UInt8] {
        let clean = String(value.unicodeScalars.filter { !CharacterSet.whitespacesAndNewlines.contains($0) })
        let body = clean.utf8.prefix { $0 != UInt8(ascii: "=") }
        let padding = clean.utf8.count - body.count
        let alphabet = { (c: UInt8) -> Bool in
            (0x41...0x5a).contains(c) || (0x61...0x7a).contains(c) || (0x30...0x39).contains(c)
                || c == UInt8(ascii: "+") || c == UInt8(ascii: "/")
        }
        guard padding <= 2, clean.utf8.suffix(padding).allSatisfy({ $0 == UInt8(ascii: "=") }),
            body.allSatisfy(alphabet), clean.utf8.count % 4 == 0, let data = Data(base64Encoded: clean)
        else { throw TeeTrustError("\(what) is not base64") }
        return [UInt8](data)
    }

    /// The DER of the one `CERTIFICATE` block a PEM holds. (== mero-js `pemCertificate`.)
    static func pemCertificate(_ pem: String, _ what: String) throws -> [UInt8] {
        let blocks = pemBlocks(pem)
        guard blocks.count == 1 else { throw TeeTrustError("\(what) is not exactly one PEM certificate") }
        let rest = pem.replacingOccurrences(of: blocks[0].whole, with: "")
        guard rest.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw TeeTrustError("\(what) is not exactly one PEM certificate")
        }
        return try fromBase64(blocks[0].body, what)
    }

    /// Every certificate in a PEM chain, DER; blocks that do not decode are
    /// skipped (dcap-qvl's `extractCerts`).
    static func pemCertificates(_ pem: String) -> [[UInt8]] {
        pemBlocks(pem).compactMap { block in
            let clean = String(block.body.unicodeScalars.filter { !CharacterSet.whitespacesAndNewlines.contains($0) })
            return Data(base64Encoded: clean).map { [UInt8]($0) }
        }
    }

    private static let begin = "-----BEGIN CERTIFICATE-----"
    private static let end = "-----END CERTIFICATE-----"

    /// Each `BEGIN … END CERTIFICATE` block with no `-` inside.
    private static func pemBlocks(_ pem: String) -> [(whole: String, body: String)] {
        var out: [(whole: String, body: String)] = []
        var rest = pem[...]
        while let open = rest.range(of: begin) {
            let after = rest[open.upperBound...]
            guard let close = after.range(of: end) else { break }
            let body = after[..<close.lowerBound]
            if body.contains("-") {
                rest = after
                continue
            }
            out.append((whole: String(rest[open.lowerBound..<close.upperBound]), body: String(body)))
            rest = after[close.upperBound...]
        }
        return out
    }
}
