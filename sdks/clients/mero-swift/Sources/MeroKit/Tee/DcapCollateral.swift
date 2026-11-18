import Foundation

/// The collateral a quote is verified against — TCB info, QE identity, CRLs
/// and their chains — in dcap-qvl's form: what `POST /admin-api/tee/attest`
/// returns with `includeCollateral`. Byte fields arrive as hex strings or byte
/// arrays; both decode. (== mero-js `DcapCollateral`.)
public struct DcapCollateral: Codable, Sendable, Equatable {
    public var pckCrlIssuerChain: String
    public var rootCaCrl: Data
    public var pckCrl: Data
    public var tcbInfoIssuerChain: String
    /// The TCB info JSON, exactly as Intel signed it.
    public var tcbInfo: String
    public var tcbInfoSignature: Data
    public var qeIdentityIssuerChain: String
    /// The QE identity JSON, exactly as Intel signed it.
    public var qeIdentity: String
    public var qeIdentitySignature: Data
    /// The PCK certificate chain, when the collateral carries it; otherwise
    /// it is read from the quote (certification data type 5).
    public var pckCertificateChain: String?

    public init(
        pckCrlIssuerChain: String, rootCaCrl: Data, pckCrl: Data, tcbInfoIssuerChain: String, tcbInfo: String,
        tcbInfoSignature: Data, qeIdentityIssuerChain: String, qeIdentity: String, qeIdentitySignature: Data,
        pckCertificateChain: String? = nil
    ) {
        self.pckCrlIssuerChain = pckCrlIssuerChain
        self.rootCaCrl = rootCaCrl
        self.pckCrl = pckCrl
        self.tcbInfoIssuerChain = tcbInfoIssuerChain
        self.tcbInfo = tcbInfo
        self.tcbInfoSignature = tcbInfoSignature
        self.qeIdentityIssuerChain = qeIdentityIssuerChain
        self.qeIdentity = qeIdentity
        self.qeIdentitySignature = qeIdentitySignature
        self.pckCertificateChain = pckCertificateChain
    }

    enum CodingKeys: String, CodingKey {
        case pckCrlIssuerChain = "pck_crl_issuer_chain"
        case rootCaCrl = "root_ca_crl"
        case pckCrl = "pck_crl"
        case tcbInfoIssuerChain = "tcb_info_issuer_chain"
        case tcbInfo = "tcb_info"
        case tcbInfoSignature = "tcb_info_signature"
        case qeIdentityIssuerChain = "qe_identity_issuer_chain"
        case qeIdentity = "qe_identity"
        case qeIdentitySignature = "qe_identity_signature"
        case pckCertificateChain = "pck_certificate_chain"
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        pckCrlIssuerChain = try c.decode(String.self, forKey: .pckCrlIssuerChain)
        rootCaCrl = try Self.bytes(c, .rootCaCrl)
        pckCrl = try Self.bytes(c, .pckCrl)
        tcbInfoIssuerChain = try c.decode(String.self, forKey: .tcbInfoIssuerChain)
        tcbInfo = try c.decode(String.self, forKey: .tcbInfo)
        tcbInfoSignature = try Self.bytes(c, .tcbInfoSignature)
        qeIdentityIssuerChain = try c.decode(String.self, forKey: .qeIdentityIssuerChain)
        qeIdentity = try c.decode(String.self, forKey: .qeIdentity)
        qeIdentitySignature = try Self.bytes(c, .qeIdentitySignature)
        pckCertificateChain = try c.decodeIfPresent(String.self, forKey: .pckCertificateChain)
    }

    /// Byte fields are written as hex, as a node writes them.
    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(pckCrlIssuerChain, forKey: .pckCrlIssuerChain)
        try c.encode(Hex.encode(rootCaCrl), forKey: .rootCaCrl)
        try c.encode(Hex.encode(pckCrl), forKey: .pckCrl)
        try c.encode(tcbInfoIssuerChain, forKey: .tcbInfoIssuerChain)
        try c.encode(tcbInfo, forKey: .tcbInfo)
        try c.encode(Hex.encode(tcbInfoSignature), forKey: .tcbInfoSignature)
        try c.encode(qeIdentityIssuerChain, forKey: .qeIdentityIssuerChain)
        try c.encode(qeIdentity, forKey: .qeIdentity)
        try c.encode(Hex.encode(qeIdentitySignature), forKey: .qeIdentitySignature)
        try c.encodeIfPresent(pckCertificateChain, forKey: .pckCertificateChain)
    }

    /// Collateral as a node returned it, inside a ``JSONValue`` (``RelayAttestation/collateral``).
    public init(json: JSONValue) throws {
        self = try JSONDecoder().decode(DcapCollateral.self, from: try JSONEncoder().encode(json))
    }

    private static func bytes(_ c: KeyedDecodingContainer<CodingKeys>, _ key: CodingKeys) throws -> Data {
        if let text = try? c.decode(String.self, forKey: key) {
            guard let data = try? Hex.decodeUnsized(text, label: key.rawValue) else {
                throw DecodingError.dataCorruptedError(
                    forKey: key, in: c, debugDescription: "\(key.rawValue) is not hex")
            }
            return data
        }
        return Data(try c.decode([UInt8].self, forKey: key))
    }
}
