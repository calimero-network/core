import CryptoKit
import Foundation

// DCAP quote verification on the device: a quote's signature chain to
// Intel's root, its QE report, its TCB level against the collateral, and its
// attributes. Port of `verify` from `@phala/dcap-qvl` 0.5.3 (`verify.js`,
// `tcb_info.js`, `qe_identity.js`, `intel.js`), the library mero-js passes as
// `dcapVerify`: here it is built in, since Swift has no such package.

/// What DCAP verification returns for a quote whose signature chain
/// verified. (== dcap-qvl `VerifiedReport`, mero-js `DcapVerifiedReport`.)
public struct DcapVerifiedReport: Sendable, Equatable {
    /// The merged TCB status, e.g. `UpToDate`.
    public var status: String
    public var advisoryIds: [String]
    public var report: DcapReport

    public init(status: String, advisoryIds: [String] = [], report: DcapReport) {
        self.status = status
        self.advisoryIds = advisoryIds
        self.report = report
    }
}

/// Verify a quote's signature chain against `collateral` as of `nowSecs`, and
/// appraise its TCB level; throw when it does not verify. ``Dcap/verify(_:collateral:nowSecs:)``
/// has exactly this shape. (== mero-js `DcapVerify`.)
public typealias DcapVerify =
    @Sendable (_ quote: Data, _ collateral: DcapCollateral, _ nowSecs: Int64) throws -> DcapVerifiedReport

/// DCAP quote verification against Intel's SGX root CA.
public enum Dcap {
    static let attestationKeyTypeEcdsaP256: UInt16 = 2
    static let pckCertChain: UInt16 = 5
    static let sgxExtension = "1.2.840.113741.1.13.1"
    static let tcbOid = "1.2.840.113741.1.13.1.2"
    static let fmspcOid = "1.2.840.113741.1.13.1.4"
    static let pceSvnOid = "1.2.840.113741.1.13.1.2.17"
    static let cpuSvnOid = "1.2.840.113741.1.13.1.2.18"

    /// Verify `quote` (SGX v3, TDX v4/v5) against `collateral` at `nowSecs`
    /// (Unix seconds), trusting Intel's SGX root CA, the way dcap-qvl's
    /// `verify` does: the collateral's CRLs, TCB info and QE identity, each
    /// signed by Intel and current; the PCK chain; the QE report and its
    /// binding to the attestation key; the quote's own signature; the
    /// platform's TCB level; and the TD's attributes (no debug TD).
    public static func verify(_ quote: Data, collateral: DcapCollateral, nowSecs: Int64) throws -> DcapVerifiedReport {
        try verify(quote, collateral: collateral, nowSecs: nowSecs, rootCa: DcapPki.intelRootCa)
    }

    static func verify(
        _ rawQuote: Data, collateral: DcapCollateral, nowSecs: Int64, rootCa: [UInt8]
    ) throws -> DcapVerifiedReport {
        let raw = [UInt8](rawQuote)
        let quote: DcapQuote
        do {
            quote = try DcapQuote.parse(raw)
        } catch {
            throw TeeTrustError("Failed to decode quote")
        }
        try checkHeader(quote.header)
        let tdx = quote.header.teeType == DcapQuote.teeTypeTdx

        let tcbInfo = try TcbInfo.parse(collateral.tcbInfo)
        if nowSecs * 1000 < tcbInfo.issueDate { throw TeeTrustError("TCBInfo issue date is in the future") }
        if nowSecs * 1000 > tcbInfo.nextUpdate { throw TeeTrustError("TCBInfo expired") }
        let crls = try DcapPki.validateCollateralCrls(
            root: rootCa, pckCrlIssuerChain: collateral.pckCrlIssuerChain, rootCaCrl: [UInt8](collateral.rootCaCrl),
            pckCrl: [UInt8](collateral.pckCrl), nowSecs: nowSecs)
        try verifySigned(
            collateral.tcbInfo, signature: collateral.tcbInfoSignature, chain: collateral.tcbInfoIssuerChain,
            what: "TCB Info", field: "tcb_info", nowSecs: nowSecs, crls: crls, rootCa: rootCa)

        let qeIdentity = try QeIdentity.parse(collateral.qeIdentity)
        if nowSecs * 1000 < qeIdentity.issueDate { throw TeeTrustError("QE Identity issue date is in the future") }
        if nowSecs * 1000 > qeIdentity.nextUpdate { throw TeeTrustError("QE Identity expired") }
        try verifySigned(
            collateral.qeIdentity, signature: collateral.qeIdentitySignature,
            chain: collateral.qeIdentityIssuerChain, what: "QE Identity", field: "qe_identity", nowSecs: nowSecs,
            crls: crls, rootCa: rootCa)
        guard qeIdentity.id == (tdx ? "TD_QE" : "QE"), (tdx ? [2, 3] : [2]).contains(qeIdentity.version) else {
            throw TeeTrustError("Unsupported QE Identity id/version for the quote TEE type")
        }

        let pckCerts = try pckChain(quote, collateral)
        try DcapPki.verifyChain(
            leaf: pckCerts[0], intermediates: Array(pckCerts.dropFirst()), nowSecs: nowSecs, crls: crls,
            trustAnchor: rootCa)
        let qeStatus = try verifyQeReport(quote.authData, pck: pckCerts[0], identity: qeIdentity)

        // The quote's own signature, by the attestation key.
        let auth = quote.authData
        let signed = SHA256.hash(data: Data(raw.prefix(quote.signedLength)))
        guard let attestationKey = try? P256.Signing.PublicKey(rawRepresentation: Data(auth.ecdsaAttestationKey)),
            let quoteSignature = try? P256.Signing.ECDSASignature(rawRepresentation: Data(auth.ecdsaSignature)),
            attestationKey.isValidSignature(quoteSignature, for: signed)
        else { throw TeeTrustError("ISV enclave report signature is invalid") }

        let platformStatus = try platformTcb(quote, pck: pckCerts[0], tcbInfo: tcbInfo)
        let status = platformStatus.merged(with: qeStatus)
        guard status.isValid else { throw TeeTrustError("TCB status is invalid: \(status.status)") }

        try validateAttributes(quote.report)
        return DcapVerifiedReport(status: status.status, advisoryIds: status.advisoryIds, report: quote.report)
    }

    private static func checkHeader(_ header: DcapQuote.Header) throws {
        guard [3, 4, 5].contains(header.version) else { throw TeeTrustError("Unsupported DCAP quote version") }
        let tdx = header.teeType == DcapQuote.teeTypeTdx
        guard header.teeType == DcapQuote.teeTypeSgx || tdx else { throw TeeTrustError("Unsupported DCAP TEE type") }
        if !tdx, header.version != 3 { throw TeeTrustError("SGX TEE quote must have version 3") }
        if tdx, header.version != 4, header.version != 5 {
            throw TeeTrustError("TDX TEE quote must have version 4 or 5")
        }
        guard header.attestationKeyType == attestationKeyTypeEcdsaP256 else {
            throw TeeTrustError("Unsupported DCAP attestation key type")
        }
    }

    /// A collateral document Intel signed: its chain to the root, then its signature.
    private static func verifySigned(
        _ text: String, signature: Data, chain: String, what: String, field: String, nowSecs: Int64,
        crls: [X509Crl], rootCa: [UInt8]
    ) throws {
        let certs = X509.pemCertificates(chain)
        guard certs.count >= 2 else { throw TeeTrustError("Certificate chain is too short for \(what)") }
        try DcapPki.verifyChain(
            leaf: certs[0], intermediates: Array(certs.dropFirst()), nowSecs: nowSecs, crls: crls, trustAnchor: rootCa)
        try verifySignature(
            certs[0], data: Array(text.utf8), signature: [UInt8](signature),
            refusal: "Signature is invalid for \(field)")
    }

    /// The PCK chain: from the collateral, or else from the quote.
    private static func pckChain(_ quote: DcapQuote, _ collateral: DcapCollateral) throws -> [[UInt8]] {
        let certs: [[UInt8]]
        if let chain = collateral.pckCertificateChain {
            certs = X509.pemCertificates(chain)
            guard !certs.isEmpty else { throw TeeTrustError("Failed to extract PCK certificates from collateral") }
        } else {
            let data = quote.authData.certificationData
            guard data.certType == pckCertChain else {
                throw TeeTrustError(
                    "Unsupported DCAP PCK cert format: \(data.certType). "
                        + "Use get_collateral() to fetch PCK certificate.")
            }
            certs = X509.pemCertificates(String(decoding: data.body, as: UTF8.self))
            guard !certs.isEmpty else { throw TeeTrustError("Failed to extract PCK certificates from quote") }
        }
        guard certs.count >= 2 else { throw TeeTrustError("Certificate chain is too short in quote") }
        return certs
    }

    /// The QE report: signed by the PCK key, binding the attestation key, and
    /// the QE Intel names. Returns the QE's TCB status.
    private static func verifyQeReport(
        _ auth: DcapQuote.AuthData, pck: [UInt8], identity: QeIdentity
    ) throws -> TcbStatus {
        try verifySignature(
            pck, data: auth.qeReport, signature: auth.qeReportSignature,
            refusal: "Signature is invalid for qe_report in quote")
        var reader = DcapReader(auth.qeReport)
        let report = try SgxEnclaveReport.decode(&reader)
        // 96 bytes: the key, then the QE auth data zero-padded or cut to fit, as dcap-qvl copies it.
        let padding = [UInt8](repeating: 0, count: max(0, 32 - auth.qeAuthData.count))
        let hashed = auth.ecdsaAttestationKey + Array(auth.qeAuthData.prefix(32)) + padding
        guard sha256(Data(hashed)) == report.reportData.prefix(32) else {
            throw TeeTrustError("QE report hash mismatch")
        }
        return try identity.verifyPolicy(report)
    }

    /// The platform's TCB level, from the PCK certificate's FMSPC and SVNs.
    private static func platformTcb(_ quote: DcapQuote, pck: [UInt8], tcbInfo: TcbInfo) throws -> TcbStatus {
        let intel = try intelExtension(try DcapPki.parseCertificateStrict(pck))
        let cpuSvn = try findExtension([tcbOid, cpuSvnOid], intel)
        guard cpuSvn.count == 16 else { throw TeeTrustError("CpuSvn length mismatch") }
        let pceSvn = try readPceSvn(intel)
        let fmspc = try findExtension([fmspcOid], intel)
        guard fmspc.count == 6 else { throw TeeTrustError("Fmspc length mismatch") }
        guard let expected = try? Hex.decodeUnsized(tcbInfo.fmspc, label: "fmspc"), [UInt8](expected) == fmspc else {
            throw TeeTrustError("Fmspc mismatch")
        }
        if quote.header.teeType == DcapQuote.teeTypeTdx {
            guard tcbInfo.version >= 3, tcbInfo.id == "TDX" else {
                throw TeeTrustError("TDX quote with non-TDX TCB info in the collateral")
            }
        } else {
            guard tcbInfo.version >= 2, tcbInfo.id == "SGX" else {
                throw TeeTrustError("SGX quote with non-SGX TCB info in the collateral")
            }
        }
        return try tcbInfo.match(quote: quote, cpuSvn: cpuSvn, pceSvn: pceSvn)
    }

    /// Verify a raw (`r ‖ s`, 64-byte) P-256 signature with a certificate's key, over SHA-256.
    private static func verifySignature(
        _ certificate: [UInt8], data: [UInt8], signature: [UInt8], refusal: String
    ) throws {
        guard signature.count == 64 else {
            throw TeeTrustError("Key length is invalid: expected 64, got \(signature.count)")
        }
        guard let parsed = try? X509.parseCertificate(certificate),
            X509.verifyRawEcdsa(spki: parsed.spki, curve: parsed.curve, hash: .sha256, signature: signature, data: data)
        else { throw TeeTrustError(refusal) }
    }

    // MARK: - The PCK certificate's Intel extension

    static func intelExtension(_ certificate: X509Certificate) throws -> [UInt8] {
        guard let value = certificate.extensions[sgxExtension] else { throw TeeTrustError("Intel extension not found") }
        return value
    }

    /// The value at `path` in Intel's nested `SEQUENCE OF { OID, value }`:
    /// an OCTET STRING's contents, or the value's whole DER otherwise.
    static func findExtension(_ path: [String], _ raw: [UInt8]) throws -> [UInt8] {
        var current = raw
        for oid in path {
            let items = try Der.children(try Der.expect(try Der.parse(current), DerTag.sequence, "The SGX extension"))
            var found: [UInt8]?
            for item in items {
                let parts = try Der.children(try Der.expect(item, DerTag.sequence, "An SGX extension item"))
                guard parts.count == 2 else { throw TeeTrustError("An SGX extension item is malformed") }
                if try Der.oid(parts[0]) == oid {
                    found = parts[1].whole
                    break
                }
            }
            guard let found else { throw TeeTrustError("OID \(oid) is missing") }
            current = found
        }
        if let element = try? Der.parse(current), element.tag == DerTag.octetString { return element.contents }
        return current
    }

    static func readPceSvn(_ intel: [UInt8]) throws -> Int {
        var data = try findExtension([tcbOid, pceSvnOid], intel)
        if data.count >= 2, data[0] == DerTag.integer {
            let length = Int(data[1])
            data = Array(data.dropFirst(2).prefix(length))
        }
        switch data.count {
        case 1: return Int(data[0])
        case 2: return Int(data[0]) << 8 | Int(data[1])
        default: throw TeeTrustError("PceSvn length mismatch")
        }
    }

    // MARK: - Attributes

    static func validateAttributes(_ report: DcapReport) throws {
        switch report {
        case .sgx(let sgx):
            try validateSgx(sgx)
        case .td10(let td):
            try validateTd10(td)
        case .td15(let td):
            guard td.mrServiceTd.allSatisfy({ $0 == 0 }) else { throw TeeTrustError("Invalid mr service td") }
            try validateTd10(td.base)
        }
    }

    /// Bit 1 of the attributes is DEBUG.
    static func validateSgx(_ report: SgxEnclaveReport) throws {
        if let flags = report.attributes.first, flags & 0x02 != 0 { throw TeeTrustError("Debug mode is enabled") }
    }

    static func validateTd10(_ report: TdReport10) throws {
        let a = [UInt8](report.tdAttributes)
        guard a.count == 8 else { throw TeeTrustError("Reserved bits in TD attributes are set") }
        // TUD bit 0 is DEBUG; bits 7:1 are reserved and must be zero.
        if a[0] & ~0x01 != 0 { throw TeeTrustError("Reserved bits in TD attributes are set") }
        if a[0] & 0x01 != 0 { throw TeeTrustError("Debug mode is enabled") }
        let reservedLower = (UInt32(a[3] & 0x0f) << 16) | (UInt32(a[2]) << 8) | UInt32(a[1])
        let reservedBit29 = a[3] & 0x20 != 0
        let reservedOther = (UInt32(a[7] & 0x7f) << 24) | (UInt32(a[6]) << 16) | (UInt32(a[5]) << 8) | UInt32(a[4])
        if reservedLower != 0 || reservedBit29 || reservedOther != 0 {
            throw TeeTrustError("Reserved bits in TD attributes are set")
        }
        guard a[3] & 0x10 != 0 else { throw TeeTrustError("SEPT_VE_DISABLE is not enabled") }
    }
}

// MARK: - TCB status

/// A TCB status and its advisories. (dcap-qvl `TcbStatus`.)
struct TcbStatus {
    let status: String
    let advisoryIds: [String]

    /// Higher is worse; an unknown status is the worst.
    static func severity(_ status: String) -> Int {
        switch status {
        case "UpToDate": return 0
        case "SWHardeningNeeded": return 1
        case "ConfigurationNeeded": return 2
        case "ConfigurationAndSWHardeningNeeded": return 3
        case "OutOfDate": return 4
        case "OutOfDateConfigurationNeeded": return 5
        case "Revoked": return 6
        default: return 100
        }
    }

    var isValid: Bool { Self.severity(status) <= 5 }

    /// The worse of the two, with both sets of advisories.
    func merged(with other: TcbStatus) -> TcbStatus {
        let worse = Self.severity(other.status) > Self.severity(status) ? other.status : status
        var ids = advisoryIds
        for id in other.advisoryIds where !ids.contains(id) { ids.append(id) }
        return TcbStatus(status: worse, advisoryIds: ids)
    }
}

/// The TCB info Intel signs for a platform (FMSPC). (dcap-qvl `TcbInfo`.)
struct TcbInfo {
    struct TcbLevel {
        let sgxComponents: [Int]
        let tdxComponents: [Int]
        let pceSvn: Int
        let status: String
        let advisoryIds: [String]
    }

    let id: String
    let version: Int
    let issueDate: Int64
    let nextUpdate: Int64
    let fmspc: String
    let levels: [TcbLevel]

    static func parse(_ text: String) throws -> TcbInfo {
        let refusal = TeeTrustError("Failed to decode TcbInfo")
        guard let json = try? MeroJSON.decode(JSONValue.self, from: Data(text.utf8)),
            let id = json["id"]?.stringValue, let version = json["version"]?.intValue,
            let issueDate = json["issueDate"]?.stringValue.flatMap(IsoTime.parse),
            let nextUpdate = json["nextUpdate"]?.stringValue.flatMap(IsoTime.parse),
            let fmspc = json["fmspc"]?.stringValue, let rawLevels = json["tcbLevels"]?.arrayValue
        else { throw refusal }
        let levels: [TcbLevel] = try rawLevels.map { level in
            guard let tcb = level["tcb"], let sgx = tcb["sgxtcbcomponents"]?.arrayValue,
                let pceSvn = tcb["pcesvn"]?.intValue, let status = level["tcbStatus"]?.stringValue
            else { throw refusal }
            return TcbLevel(
                sgxComponents: sgx.compactMap { $0["svn"]?.intValue },
                tdxComponents: (tcb["tdxtcbcomponents"]?.arrayValue ?? []).compactMap { $0["svn"]?.intValue },
                pceSvn: pceSvn, status: status,
                advisoryIds: (level["advisoryIDs"]?.arrayValue ?? []).compactMap(\.stringValue))
        }
        return TcbInfo(
            id: id, version: version, issueDate: issueDate, nextUpdate: nextUpdate, fmspc: fmspc, levels: levels)
    }

    /// The first level the platform meets. (dcap-qvl `matchPlatformTcb`.)
    func match(quote: DcapQuote, cpuSvn: [UInt8], pceSvn: Int) throws -> TcbStatus {
        for level in levels {
            if pceSvn < level.pceSvn { continue }
            guard !level.sgxComponents.isEmpty else { throw TeeTrustError("No SGX components in the TCB info") }
            if !Self.meets(cpuSvn.map { Int($0) }, level.sgxComponents) { continue }
            if quote.header.teeType == DcapQuote.teeTypeTdx {
                guard let td = quote.report.td10 else { throw TeeTrustError("Failed to get TD10 report") }
                guard !level.tdxComponents.isEmpty else { throw TeeTrustError("No TDX components in the TCB info") }
                if !Self.meets(td.teeTcbSvn.map { Int($0) }, level.tdxComponents) { continue }
            }
            return TcbStatus(status: level.status, advisoryIds: level.advisoryIds)
        }
        throw TeeTrustError("No matching TCB level found")
    }

    /// Every component at least the level's.
    static func meets(_ actual: [Int], _ required: [Int]) -> Bool {
        actual.count == required.count && zip(actual, required).allSatisfy { $0 >= $1 }
    }
}

/// The identity Intel signs for its quoting enclave. (dcap-qvl `QeIdentity`.)
struct QeIdentity {
    let id: String
    let version: Int
    let issueDate: Int64
    let nextUpdate: Int64
    let miscselect: [UInt8]
    let miscselectMask: [UInt8]
    let attributes: [UInt8]
    let attributesMask: [UInt8]
    let mrsigner: String
    let isvProdId: Int
    struct QeTcbLevel {
        let isvSvn: Int
        let status: String
        let advisoryIds: [String]
    }

    let levels: [QeTcbLevel]

    static func parse(_ text: String) throws -> QeIdentity {
        let refusal = TeeTrustError("Failed to decode QeIdentity")
        func bytes(_ key: String, _ json: JSONValue) -> [UInt8]? {
            json[key]?.stringValue.flatMap { try? Hex.decodeUnsized($0, label: key) }.map { [UInt8]($0) }
        }
        guard let json = try? MeroJSON.decode(JSONValue.self, from: Data(text.utf8)),
            let id = json["id"]?.stringValue, let version = json["version"]?.intValue,
            let issueDate = json["issueDate"]?.stringValue.flatMap(IsoTime.parse),
            let nextUpdate = json["nextUpdate"]?.stringValue.flatMap(IsoTime.parse),
            let miscselect = bytes("miscselect", json), miscselect.count >= 4,
            let miscselectMask = bytes("miscselectMask", json), miscselectMask.count >= 4,
            let attributes = bytes("attributes", json), attributes.count >= 16,
            let attributesMask = bytes("attributesMask", json), attributesMask.count >= 16,
            let mrsigner = json["mrsigner"]?.stringValue, let isvProdId = json["isvprodid"]?.intValue,
            let rawLevels = json["tcbLevels"]?.arrayValue
        else { throw refusal }
        let levels = try rawLevels.map { level -> QeTcbLevel in
            guard let isvSvn = level["tcb"]?["isvsvn"]?.intValue, let status = level["tcbStatus"]?.stringValue else {
                throw refusal
            }
            let advisories = (level["advisoryIDs"]?.arrayValue ?? []).compactMap(\.stringValue)
            return QeTcbLevel(isvSvn: isvSvn, status: status, advisoryIds: advisories)
        }
        return QeIdentity(
            id: id, version: version, issueDate: issueDate, nextUpdate: nextUpdate, miscselect: miscselect,
            miscselectMask: miscselectMask, attributes: attributes, attributesMask: attributesMask,
            mrsigner: mrsigner, isvProdId: isvProdId, levels: levels)
    }

    /// The QE report against this identity, then its TCB level. (dcap-qvl `verifyQeIdentityPolicy`.)
    func verifyPolicy(_ report: SgxEnclaveReport) throws -> TcbStatus {
        guard let expected = try? Hex.decodeUnsized(mrsigner, label: "mrsigner"), expected == report.mrSigner else {
            throw TeeTrustError(
                "QE MRSIGNER mismatch: expected \(mrsigner), got \(Hex.encode(report.mrSigner).uppercased())")
        }
        guard Int(report.isvProdId) == isvProdId else {
            throw TeeTrustError("QE ISVPRODID mismatch: expected \(isvProdId), got \(report.isvProdId)")
        }
        // The QE is always production-mode; allowing debug only ever concerns the workload.
        try Dcap.validateSgx(report)
        let little = { (b: [UInt8]) -> UInt32 in
            UInt32(b[0]) | UInt32(b[1]) << 8 | UInt32(b[2]) << 16 | UInt32(b[3]) << 24
        }
        let mask = little(miscselectMask)
        let gotMisc = report.miscSelect & mask
        let wantMisc = little(miscselect) & mask
        guard gotMisc == wantMisc else {
            throw TeeTrustError(
                "QE MISCSELECT mismatch: expected \(String(format: "%08X", wantMisc)) (masked), "
                    + "got \(String(format: "%08X", gotMisc)) (masked)")
        }
        let got = [UInt8](report.attributes)
        for i in 0..<16 {
            let want = attributes[i] & attributesMask[i]
            let have = (i < got.count ? got[i] : 0) & attributesMask[i]
            guard want == have else {
                throw TeeTrustError(
                    "QE ATTRIBUTES mismatch at byte \(i): expected \(String(format: "%02X", want)) (masked), "
                        + "got \(String(format: "%02X", have)) (masked)")
            }
        }
        for level in levels where Int(report.isvSvn) >= level.isvSvn {
            return TcbStatus(status: level.status, advisoryIds: level.advisoryIds)
        }
        guard let last = levels.last else { throw TeeTrustError("No TCB levels found in QE Identity") }
        throw TeeTrustError(
            "QE ISVSVN \(report.isvSvn) is below minimum required \(last.isvSvn) from QE Identity")
    }
}
