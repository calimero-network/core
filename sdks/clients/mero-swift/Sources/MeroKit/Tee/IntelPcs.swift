import Foundation

#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

/// Intel's collateral for a quote, fetched from a PCCS: the cloud's mirror
/// (``TeeTrust/defaultCollateralMirror``) or Intel's Provisioning
/// Certification Service itself. A Calimero 1.0 node sends no collateral with
/// its quote (it could not forge Intel's signatures anyway), so the client
/// fetches it. Port of `getCollateral` / `getCollateralFromPcs` from
/// `@phala/dcap-qvl`, which mero-js uses by default. (== mero-js
/// `CloudReleasePolicy.fetchCollateral`'s default.)
public enum IntelPcs {
    /// Intel's PCS.
    public static let defaultBaseURL = "https://api.trustedservices.intel.com"
    /// Where Intel publishes its SGX root CA's CRL.
    public static let rootCaCrlURL = "https://certificates.trustedservices.intel.com/IntelSGXRootCA.der"

    /// The collateral for `quote` from the PCCS at `baseURL` (Intel's PCS by
    /// default): the PCK CRL for the CA that issued its PCK certificate, the
    /// TCB info for its platform's FMSPC, the TD QE identity and the root CA's
    /// CRL (from the PCCS's `rootcacrl` when it serves one, else Intel's),
    /// each with the issuer chain sent. Nothing here is trusted:
    /// ``Dcap/verify(_:collateral:nowSecs:)`` checks every signature up to
    /// Intel's root.
    public static func collateral(
        for quote: Data, baseURL: String = defaultBaseURL, session: URLSession = .shared
    ) async throws -> DcapCollateral {
        let platform = try platform(of: quote)
        let (fmspc, ca, tdx) = (platform.fmspc, platform.ca, platform.tdx)
        let base = normalized(baseURL)
        let kind = tdx ? "tdx" : "sgx"
        async let pck = get("\(base)/sgx/certification/v4/pckcrl?ca=\(ca)&encoding=der", session: session)
        async let tcb = get("\(base)/\(kind)/certification/v4/tcb?fmspc=\(fmspc)", session: session)
        async let qe = get("\(base)/\(kind)/certification/v4/qe/identity?update=standard", session: session)
        async let root = rootCrl(base, session: session)
        let (pckCrl, tcbInfo, qeIdentity, rootCrl) = try await (pck, tcb, qe, root)
        let tcbSignature = try signatureOf(tcbInfo.body)
        let qeSignature = try signatureOf(qeIdentity.body)
        return DcapCollateral(
            pckCrlIssuerChain: try chain(pckCrl, "SGX-PCK-CRL-Issuer-Chain"), rootCaCrl: rootCrl,
            pckCrl: pckCrl.body,
            tcbInfoIssuerChain: try chain(tcbInfo, "SGX-TCB-Info-Issuer-Chain", "TCB-Info-Issuer-Chain"),
            tcbInfo: try rawField(tcbInfo.body, "tcbInfo"), tcbInfoSignature: tcbSignature,
            qeIdentityIssuerChain: try chain(qeIdentity, "SGX-Enclave-Identity-Issuer-Chain"),
            qeIdentity: try rawField(qeIdentity.body, "enclaveIdentity"), qeIdentitySignature: qeSignature)
    }

    /// `baseURL` without a trailing slash or a `/{sgx,tdx}/certification/v4` suffix.
    static func normalized(_ baseURL: String) -> String {
        var base = baseURL
        while base.hasSuffix("/") { base.removeLast() }
        for suffix in ["/sgx/certification/v4", "/tdx/certification/v4"] where base.hasSuffix(suffix) {
            base.removeLast(suffix.count)
        }
        return base
    }

    /// The root CA's CRL: a PCCS serves it hex-encoded at `rootcacrl`; Intel's
    /// PCS does not, and a PCCS that fails to falls back to Intel's.
    static func rootCrl(_ base: String, session: URLSession) async throws -> Data {
        if !base.hasPrefix(defaultBaseURL),
            let fetched = try? await get("\(base)/sgx/certification/v4/rootcacrl", session: session),
            let crl = try? Hex.decodeUnsized(
                String(decoding: fetched.body, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines),
                label: "rootcacrl"),
            !crl.isEmpty
        {
            return crl
        }
        return try await get(rootCaCrlURL, session: session).body
    }

    /// The quote's platform: its PCK certificate's FMSPC (hex), the CA that
    /// issued it (`processor` or `platform`), and whether it is a TD quote.
    static func platform(of rawQuote: Data) throws -> Platform {
        let quote = try DcapQuote.parse([UInt8](rawQuote))
        let data = quote.authData.certificationData
        guard data.certType == Dcap.pckCertChain else {
            throw TeeTrustError("The quote carries no PCK certificate chain (type \(data.certType))")
        }
        let certs = X509.pemCertificates(String(decoding: data.body, as: UTF8.self))
        guard let leaf = certs.first else { throw TeeTrustError("The quote's PCK certificate chain is empty") }
        let pck = try DcapPki.parseCertificateStrict(leaf)
        let fmspc = try Dcap.findExtension([Dcap.fmspcOid], try Dcap.intelExtension(pck))
        // The leaf names its issuer, "Intel SGX PCK Platform CA" or "… Processor CA", in plain ASCII.
        let ca = Data(leaf).range(of: Data("PCK Platform CA".utf8)) != nil ? "platform" : "processor"
        return Platform(
            fmspc: Hex.encode(fmspc).uppercased(), ca: ca, tdx: quote.header.teeType == DcapQuote.teeTypeTdx)
    }

    struct Platform {
        let fmspc: String
        let ca: String
        let tdx: Bool
    }

    struct Fetched {
        let body: Data
        let headers: [String: String]
    }

    static func get(_ url: String, session: URLSession) async throws -> Fetched {
        guard let target = URL(string: url) else { throw TeeTrustError("bad PCS URL \(url)") }
        let (data, response) = try await session.data(from: target)
        let http = response as? HTTPURLResponse
        guard let http, (200..<300).contains(http.statusCode) else {
            throw TeeTrustError("Fetching \(url) failed: HTTP \(http?.statusCode ?? 0)")
        }
        var headers: [String: String] = [:]
        for (key, value) in http.allHeaderFields {
            if let k = key as? String, let v = value as? String { headers[k.lowercased()] = v }
        }
        return Fetched(body: data, headers: headers)
    }

    /// An issuer chain header, URL-decoded to PEM.
    static func chain(_ fetched: Fetched, _ names: String...) throws -> String {
        for name in names {
            if let value = fetched.headers[name.lowercased()] { return value.removingPercentEncoding ?? value }
        }
        throw TeeTrustError("Intel's answer carries no \(names[0]) header")
    }

    static func signatureOf(_ body: Data) throws -> Data {
        guard let json = try? MeroJSON.decode(JSONValue.self, from: body),
            let signature = json["signature"]?.stringValue
        else { throw TeeTrustError("Intel's answer carries no signature") }
        return try Hex.decodeUnsized(signature, label: "signature")
    }

    /// The text of top-level field `key` of a JSON object, byte for byte as
    /// sent: Intel signs the field's exact spelling.
    static func rawField(_ body: Data, _ key: String) throws -> String {
        let bytes = [UInt8](body)
        let needle = [UInt8]("\"\(key)\"".utf8)
        var depth = 0
        var index = 0
        var inString = false
        while index < bytes.count {
            let byte = bytes[index]
            if inString {
                if byte == 0x5C {
                    index += 2
                    continue
                }
                if byte == 0x22 { inString = false }
                index += 1
                continue
            }
            if byte == 0x22 {
                if depth == 1, bytes.count >= index + needle.count, Array(bytes[index..<index + needle.count]) == needle
                {
                    var start = index + needle.count
                    while start < bytes.count, bytes[start] == 0x20 || bytes[start] == 0x3A { start += 1 }
                    return String(decoding: bytes[start..<(try valueEnd(bytes, from: start))], as: UTF8.self)
                }
                inString = true
            } else if byte == 0x7B || byte == 0x5B {
                depth += 1
            } else if byte == 0x7D || byte == 0x5D {
                depth -= 1
            }
            index += 1
        }
        throw TeeTrustError("Intel's answer carries no \(key)")
    }

    private static func valueEnd(_ bytes: [UInt8], from start: Int) throws -> Int {
        var depth = 0
        var index = start
        var inString = false
        while index < bytes.count {
            let byte = bytes[index]
            if inString {
                if byte == 0x5C {
                    index += 2
                    continue
                }
                if byte == 0x22 {
                    inString = false
                    if depth == 0 { return index + 1 }
                }
            } else if byte == 0x22 {
                inString = true
            } else if byte == 0x7B || byte == 0x5B {
                depth += 1
            } else if byte == 0x7D || byte == 0x5D {
                depth -= 1
                if depth == 0 { return index + 1 }
            } else if depth == 0, byte == 0x2C {
                return index
            }
            index += 1
        }
        throw TeeTrustError("Intel's answer is malformed")
    }
}
