import Foundation

// The parts of an Intel DCAP quote (SGX v3, TDX v4 and v5) a verifier reads.
// Port of dcap-qvl's `quote.js` (itself converted from `quote.rs`), the
// library mero-js verifies quotes with.

/// An SGX enclave report: the body of an SGX quote, and the QE's report in
/// every quote.
public struct SgxEnclaveReport: Sendable, Equatable {
    public var cpuSvn: Data
    public var miscSelect: UInt32
    public var attributes: Data
    public var mrEnclave: Data
    public var mrSigner: Data
    public var isvProdId: UInt16
    public var isvSvn: UInt16
    public var reportData: Data

    static let byteLength = 384

    static func decode(_ reader: inout DcapReader) throws -> SgxEnclaveReport {
        let cpuSvn = try reader.bytes(16)
        let miscSelect = try reader.u32()
        _ = try reader.bytes(28)
        let attributes = try reader.bytes(16)
        let mrEnclave = try reader.bytes(32)
        _ = try reader.bytes(32)
        let mrSigner = try reader.bytes(32)
        _ = try reader.bytes(96)
        let isvProdId = try reader.u16()
        let isvSvn = try reader.u16()
        _ = try reader.bytes(60)
        let reportData = try reader.bytes(64)
        return SgxEnclaveReport(
            cpuSvn: cpuSvn, miscSelect: miscSelect, attributes: attributes, mrEnclave: mrEnclave, mrSigner: mrSigner,
            isvProdId: isvProdId, isvSvn: isvSvn, reportData: reportData)
    }
}

/// A TDX 1.0 TD report: what a TDX quote measures. (dcap-qvl `TDReport10`.)
public struct TdReport10: Sendable, Equatable {
    public var teeTcbSvn: Data
    public var mrSeam: Data
    public var mrSignerSeam: Data
    public var seamAttributes: Data
    public var tdAttributes: Data
    public var xfam: Data
    public var mrTd: Data
    public var mrConfigId: Data
    public var mrOwner: Data
    public var mrOwnerConfig: Data
    public var rtMr0: Data
    public var rtMr1: Data
    public var rtMr2: Data
    public var rtMr3: Data
    public var reportData: Data

    static let byteLength = 584

    static func decode(_ reader: inout DcapReader) throws -> TdReport10 {
        TdReport10(
            teeTcbSvn: try reader.bytes(16), mrSeam: try reader.bytes(48), mrSignerSeam: try reader.bytes(48),
            seamAttributes: try reader.bytes(8), tdAttributes: try reader.bytes(8), xfam: try reader.bytes(8),
            mrTd: try reader.bytes(48), mrConfigId: try reader.bytes(48), mrOwner: try reader.bytes(48),
            mrOwnerConfig: try reader.bytes(48), rtMr0: try reader.bytes(48), rtMr1: try reader.bytes(48),
            rtMr2: try reader.bytes(48), rtMr3: try reader.bytes(48), reportData: try reader.bytes(64))
    }
}

/// A TDX 1.5 TD report: a 1.0 report and two more fields. (dcap-qvl `TDReport15`.)
public struct TdReport15: Sendable, Equatable {
    public var base: TdReport10
    public var teeTcbSvn2: Data
    public var mrServiceTd: Data

    static let byteLength = 648

    static func decode(_ reader: inout DcapReader) throws -> TdReport15 {
        let base = try TdReport10.decode(&reader)
        return TdReport15(base: base, teeTcbSvn2: try reader.bytes(16), mrServiceTd: try reader.bytes(48))
    }
}

/// The body of a quote, by kind. (dcap-qvl `Report`: `sgx`, `td10`, `td15`.)
public enum DcapReport: Sendable, Equatable {
    case sgx(SgxEnclaveReport)
    case td10(TdReport10)
    case td15(TdReport15)

    /// dcap-qvl's name for the kind.
    public var type: String {
        switch self {
        case .sgx: return "sgx"
        case .td10: return "td10"
        case .td15: return "td15"
        }
    }

    /// The TD 1.0 report of a TDX quote (a 1.5 report's base).
    public var td10: TdReport10? {
        switch self {
        case .sgx: return nil
        case .td10(let report): return report
        case .td15(let report): return report.base
        }
    }
}

/// Little-endian reads over a byte buffer, refusing to read past its end.
struct DcapReader {
    let bytes: [UInt8]
    var offset = 0

    init(_ bytes: [UInt8]) { self.bytes = bytes }

    mutating func raw(_ count: Int) throws -> [UInt8] {
        guard count >= 0, offset + count <= bytes.count else {
            throw TeeTrustError("Not enough data to fill buffer")
        }
        defer { offset += count }
        return Array(bytes[offset..<offset + count])
    }

    mutating func bytes(_ count: Int) throws -> Data { Data(try raw(count)) }

    mutating func u16() throws -> UInt16 {
        let b = try raw(2)
        return UInt16(b[0]) | UInt16(b[1]) << 8
    }

    mutating func u32() throws -> UInt32 {
        let b = try raw(4)
        return UInt32(b[0]) | UInt32(b[1]) << 8 | UInt32(b[2]) << 16 | UInt32(b[3]) << 24
    }
}

/// A parsed DCAP quote. (dcap-qvl `Quote`.)
struct DcapQuote {
    static let teeTypeSgx: UInt32 = 0x0000_0000
    static let teeTypeTdx: UInt32 = 0x0000_0081
    static let headerByteLength = 48
    static let bodyByteSize = 6

    struct Header {
        let version: UInt16
        let attestationKeyType: UInt16
        let teeType: UInt32
        let qeSvn: UInt16
        let pceSvn: UInt16
        let qeVendorId: Data
        let userData: Data
    }

    struct CertificationData {
        let certType: UInt16
        let body: [UInt8]

        static func decode(_ reader: inout DcapReader) throws -> CertificationData {
            let certType = try reader.u16()
            let size = try reader.u32()
            return CertificationData(certType: certType, body: try reader.raw(Int(size)))
        }
    }

    /// Auth data in v3 form (a v4 quote's is unwrapped into it).
    struct AuthData {
        let ecdsaSignature: [UInt8]
        let ecdsaAttestationKey: [UInt8]
        let qeReport: [UInt8]
        let qeReportSignature: [UInt8]
        let qeAuthData: [UInt8]
        let certificationData: CertificationData
    }

    let header: Header
    let report: DcapReport
    let authData: AuthData

    static func parse(_ bytes: [UInt8]) throws -> DcapQuote {
        var reader = DcapReader(bytes)
        let header = Header(
            version: try reader.u16(), attestationKeyType: try reader.u16(), teeType: try reader.u32(),
            qeSvn: try reader.u16(), pceSvn: try reader.u16(), qeVendorId: try reader.bytes(16),
            userData: try reader.bytes(20))

        let report: DcapReport
        switch header.version {
        case 3:
            guard header.teeType == teeTypeSgx else { throw TeeTrustError("Invalid tee type for version 3") }
            report = .sgx(try SgxEnclaveReport.decode(&reader))
        case 4:
            if header.teeType == teeTypeSgx {
                report = .sgx(try SgxEnclaveReport.decode(&reader))
            } else if header.teeType == teeTypeTdx {
                report = .td10(try TdReport10.decode(&reader))
            } else {
                throw TeeTrustError("Invalid TEE type")
            }
        case 5:
            let bodyType = try reader.u16()
            _ = try reader.u32()
            switch bodyType {
            case 1: report = .sgx(try SgxEnclaveReport.decode(&reader))
            case 2: report = .td10(try TdReport10.decode(&reader))
            case 3: report = .td15(try TdReport15.decode(&reader))
            default: throw TeeTrustError("Unsupported body type")
            }
        default:
            throw TeeTrustError("Unsupported quote version")
        }

        let authSize = try reader.u32()
        var auth = DcapReader(try reader.raw(Int(authSize)))
        // Quote v5 uses the v4 auth data format.
        let authVersion = header.version == 5 ? 4 : header.version
        let authData: AuthData
        let signature = try auth.raw(64)
        let key = try auth.raw(64)
        if authVersion == 3 {
            authData = try qeReportData(&auth, signature: signature, key: key)
        } else {
            let outer = try CertificationData.decode(&auth)
            var inner = DcapReader(outer.body)
            authData = try qeReportData(&inner, signature: signature, key: key)
        }
        return DcapQuote(header: header, report: report, authData: authData)
    }

    /// The QE report, its signature, the QE auth data and the certification
    /// data: the rest of v3 auth data, and the body of v4's.
    private static func qeReportData(
        _ reader: inout DcapReader, signature: [UInt8], key: [UInt8]
    ) throws -> AuthData {
        let qeReport = try reader.raw(SgxEnclaveReport.byteLength)
        let qeReportSignature = try reader.raw(64)
        let qeAuthDataSize = try reader.u16()
        let qeAuthData = try reader.raw(Int(qeAuthDataSize))
        let certificationData = try CertificationData.decode(&reader)
        return AuthData(
            ecdsaSignature: signature, ecdsaAttestationKey: key, qeReport: qeReport,
            qeReportSignature: qeReportSignature, qeAuthData: qeAuthData, certificationData: certificationData)
    }

    /// How many leading bytes of the quote its attestation key signed.
    var signedLength: Int {
        var length = Self.headerByteLength
        switch report {
        case .sgx: length += SgxEnclaveReport.byteLength
        case .td10: length += TdReport10.byteLength
        case .td15: length += TdReport15.byteLength
        }
        if header.version == 5 { length += Self.bodyByteSize }
        return length
    }
}
