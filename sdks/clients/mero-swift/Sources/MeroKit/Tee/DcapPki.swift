import Foundation

// The certificate checks of DCAP verification: Intel's chains, their CRLs and
// revocation, as dcap-qvl's `utils.js` runs them (`verifyCertificateChain`,
// `validateCollateralCrls`, `verifyCrl`, `checkCertificateRevocation`), on
// this SDK's own DER reader.

/// A parsed X.509 CRL. (dcap-qvl `parseCrl`.)
struct X509Crl {
    let der: [UInt8]
    let tbs: [UInt8]
    let issuer: [UInt8]
    let thisUpdate: Int64
    let nextUpdate: Int64
    let revokedSerials: [[UInt8]]
    let signatureAlgorithm: String
    let signature: [UInt8]
}

enum DcapPki {
    /// Intel's SGX root CA (`TrustedRootCA.der`), as dcap-qvl embeds it.
    static let intelRootCaBase64 = [
        "MIICjzCCAjSgAwIBAgIUImUM1lqdNInzg7SVUr9QGzknBqwwCgYIKoZIzj0EAwIwaDEaMBgGA1UE",
        "AwwRSW50ZWwgU0dYIFJvb3QgQ0ExGjAYBgNVBAoMEUludGVsIENvcnBvcmF0aW9uMRQwEgYDVQQH",
        "DAtTYW50YSBDbGFyYTELMAkGA1UECAwCQ0ExCzAJBgNVBAYTAlVTMB4XDTE4MDUyMTEwNDUxMFoX",
        "DTQ5MTIzMTIzNTk1OVowaDEaMBgGA1UEAwwRSW50ZWwgU0dYIFJvb3QgQ0ExGjAYBgNVBAoMEUlu",
        "dGVsIENvcnBvcmF0aW9uMRQwEgYDVQQHDAtTYW50YSBDbGFyYTELMAkGA1UECAwCQ0ExCzAJBgNV",
        "BAYTAlVTMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEC6nEwMDIYZOj/iPWsCzaEKi71OiOSLRF",
        "hWGjbnBVJfVnkY4u3IjkDYYL0MxO4mqsyYjlBalTVYxFP2sJBK5zlKOBuzCBuDAfBgNVHSMEGDAW",
        "gBQiZQzWWp00ifODtJVSv1AbOScGrDBSBgNVHR8ESzBJMEegRaBDhkFodHRwczovL2NlcnRpZmlj",
        "YXRlcy50cnVzdGVkc2VydmljZXMuaW50ZWwuY29tL0ludGVsU0dYUm9vdENBLmRlcjAdBgNVHQ4E",
        "FgQUImUM1lqdNInzg7SVUr9QGzknBqwwDgYDVR0PAQH/BAQDAgEGMBIGA1UdEwEB/wQIMAYBAf8C",
        "AQEwCgYIKoZIzj0EAwIDSQAwRgIhAOW/5QkR+S9CiSDcNoowLuPRLsWGf/Yi7GSX94BgwTwgAiEA",
        "4J0lrHoMs+Xo5o/sX6O9QWxHRAvZUGOdRQ7cvqRXaqI=",
    ].joined()

    static var intelRootCa: [UInt8] { [UInt8](Data(base64Encoded: intelRootCaBase64) ?? Data()) }

    static let crlSignatureHashes: [String: EcdsaHash] = [
        "1.2.840.10045.4.3.2": .sha256, "1.2.840.10045.4.3.3": .sha384, "1.2.840.10045.4.3.4": .sha512,
    ]
    static let basicConstraintsOid = "2.5.29.19"
    static let keyUsageOid = "2.5.29.15"
    static let keyCertSign = 32
    static let crlSign = 64

    // MARK: - Certificates

    static func parseCertificateStrict(_ der: [UInt8]) throws -> X509Certificate {
        guard !der.isEmpty else { throw TeeTrustError("Invalid certificate: empty input") }
        do {
            return try X509.parseCertificate(der, strict: true)
        } catch let error as TeeTrustError {
            throw TeeTrustError("Failed to parse certificate: \(error.message)")
        }
    }

    /// `cA` and `pathLenConstraint`, or `nil` when the certificate has no basic constraints.
    static func basicConstraints(_ certificate: X509Certificate) throws -> (isCA: Bool, pathLength: Int?)? {
        guard let value = certificate.extensions[basicConstraintsOid] else { return nil }
        do {
            let parts = try Der.children(try Der.expect(try Der.parse(value), DerTag.sequence, "Basic constraints"))
            var isCA = false
            var pathLength: Int?
            var rest = parts[...]
            if let first = rest.first, first.tag == DerTag.boolean {
                isCA = first.contents.first.map { $0 != 0 } ?? false
                rest = rest.dropFirst()
            }
            if let next = rest.first, next.tag == DerTag.integer {
                pathLength = next.contents.reduce(0) { $0 * 256 + Int($1) }
                rest = rest.dropFirst()
            }
            guard rest.isEmpty else { throw TeeTrustError("unexpected fields") }
            return (isCA: isCA, pathLength: pathLength)
        } catch {
            throw TeeTrustError("Certificate has invalid basic constraints")
        }
    }

    static func isCA(_ certificate: X509Certificate) throws -> Bool {
        try basicConstraints(certificate)?.isCA == true
    }

    static func isSelfIssued(_ certificate: X509Certificate) -> Bool {
        certificate.subject == certificate.issuer
    }

    /// RFC 5280 only constrains use when keyUsage is present.
    static func requireKeyUsage(_ certificate: X509Certificate, _ flag: Int, _ description: String) throws {
        guard let value = certificate.extensions[keyUsageOid] else { return }
        let bits: [UInt8]
        do {
            bits = try Der.expect(try Der.parse(value), DerTag.bitString, "Key usage").contents
        } catch {
            throw TeeTrustError("Certificate has invalid key usage")
        }
        guard let unused = bits.first, unused < 8 else { throw TeeTrustError("Certificate has invalid key usage") }
        // Bit 0 (digitalSignature) is the top bit of the first byte, and flag 1.
        let count = (bits.count - 1) * 8 - Int(unused)
        var usage = 0
        for index in 0..<max(0, count) where (bits[1 + index / 8] >> (7 - index % 8)) & 1 == 1 {
            usage |= 1 << index
        }
        guard usage & flag != 0 else { throw TeeTrustError("Certificate is not allowed to \(description)") }
    }

    static func validateTime(_ certificate: X509Certificate, _ nowSecs: Int64) throws {
        let nowMs = nowSecs * 1000
        guard nowMs >= certificate.notBefore, nowMs <= certificate.notAfter else {
            throw TeeTrustError("Certificate is expired or not yet valid")
        }
    }

    /// A DER INTEGER's bytes with the sign recorded and padding stripped, so
    /// a negative serial never equals a positive one.
    static func serialBytes(_ serial: [UInt8]) -> [UInt8] {
        let sign: UInt8 = (serial.first ?? 0) & 0x80 != 0 ? 1 : 0
        var bytes = serial[...]
        while bytes.count > 1, bytes.first == 0 { bytes = bytes.dropFirst() }
        return [sign] + bytes
    }

    // MARK: - CRLs

    static func parseCrl(_ der: [UInt8]) throws -> X509Crl {
        guard !der.isEmpty else { throw TeeTrustError("Invalid CRL: empty input") }
        let parsed: (crl: X509Crl, version: Int?)
        let flags: (hasExtensions: Bool, critical: String?)
        let algorithmsMatch: Bool
        do {
            let top = try Der.children(
                try Der.expect(try Der.parse(der, strict: true), DerTag.sequence, "The CRL"), strict: true)
            guard top.count == 3 else { throw TeeTrustError("it is not a CRL") }
            let tbs = try Der.expect(top[0], DerTag.sequence, "The signed part")
            let outerAlgorithm = try Der.expect(top[1], DerTag.sequence, "The signature algorithm")
            let signatureBits = try Der.expect(top[2], DerTag.bitString, "The signature").contents
            guard signatureBits.first == 0 else { throw TeeTrustError("its signature is not whole bytes") }

            let fields = try Der.children(tbs, strict: true)
            var index = 0
            func next() -> DerElement? { index < fields.count ? fields[index] : nil }
            func isTime(_ element: DerElement?) -> Bool {
                element?.tag == DerTag.utcTime || element?.tag == DerTag.generalizedTime
            }
            var version: Int?
            if let field = next(), field.tag == DerTag.integer {
                version = field.contents.reduce(0) { $0 * 256 + Int($1) }
                index += 1
            }
            let innerAlgorithm = try Der.expect(next(), DerTag.sequence, "The signature algorithm")
            index += 1
            let issuer = try Der.expect(next(), DerTag.sequence, "The issuer").whole
            index += 1
            guard let thisUpdateField = next(), isTime(thisUpdateField) else {
                throw TeeTrustError("its thisUpdate is not a time")
            }
            let thisUpdate = try Der.time(thisUpdateField)
            index += 1
            var nextUpdate: Int64?
            if let field = next(), isTime(field) {
                nextUpdate = try Der.time(field)
                index += 1
            }
            var revoked = RevokedEntries()
            if let field = next(), field.tag == DerTag.sequence {
                revoked = try revokedEntries(field)
                index += 1
            }
            let serials = revoked.serials
            var hasExtensions = revoked.hasExtensions
            var critical = revoked.critical
            if let field = next(), field.tag == DerTag.context0 {
                guard let list = try Der.children(field).first else { throw TeeTrustError("empty extensions") }
                let found = try extensionList(list)
                hasExtensions = hasExtensions || !found.isEmpty
                if let id = found.first(where: { $0.critical })?.id {
                    critical = "Unsupported critical CRL extension: \(id)"
                }
                index += 1
            }
            guard index == fields.count else { throw TeeTrustError("it has fields this parser does not read") }
            guard let nextUpdate else { throw TeeTrustError("CRL is missing nextUpdate") }
            let algorithm = try Der.oid(try Der.children(outerAlgorithm).first)
            let crl = X509Crl(
                der: der, tbs: tbs.whole, issuer: issuer, thisUpdate: thisUpdate, nextUpdate: nextUpdate,
                revokedSerials: serials, signatureAlgorithm: algorithm, signature: Array(signatureBits.dropFirst()))
            parsed = (crl: crl, version: version)
            flags = (hasExtensions: hasExtensions, critical: critical)
            algorithmsMatch = innerAlgorithm.whole == outerAlgorithm.whole
        } catch let error as TeeTrustError where error.message == "CRL is missing nextUpdate" {
            throw error
        } catch {
            throw TeeTrustError("Failed to parse CRL")
        }
        if let version = parsed.version, version != 1 { throw TeeTrustError("Unsupported CRL version") }
        if flags.hasExtensions, parsed.version != 1 {
            throw TeeTrustError("CRL extensions require a version 2 CRL")
        }
        guard algorithmsMatch else { throw TeeTrustError("CRL signature algorithm mismatch") }
        if let critical = flags.critical { throw TeeTrustError(critical) }
        return parsed.crl
    }

    /// What a CRL's `revokedCertificates` list holds: each serial, whether any
    /// entry carries extensions, and the first critical entry extension found.
    struct RevokedEntries {
        var serials: [[UInt8]] = []
        var hasExtensions = false
        var critical: String?
    }

    private static func revokedEntries(_ field: DerElement) throws -> RevokedEntries {
        var revoked = RevokedEntries()
        for entry in try Der.children(field, strict: true) {
            let parts = try Der.children(try Der.expect(entry, DerTag.sequence, "A revoked certificate"))
            revoked.serials.append(try Der.expect(parts.first, DerTag.integer, "A revoked serial").contents)
            guard parts.count > 2 else { continue }
            let found = try extensionList(parts[2])
            revoked.hasExtensions = revoked.hasExtensions || !found.isEmpty
            if revoked.critical == nil, let id = found.first(where: { $0.critical })?.id {
                revoked.critical = "Unsupported critical CRL entry extension: \(id)"
            }
        }
        return revoked
    }

    /// The extensions in a DER `Extensions` list: each one's id and whether it is critical.
    private static func extensionList(_ list: DerElement) throws -> [(id: String, critical: Bool)] {
        try Der.children(try Der.expect(list, DerTag.sequence, "The extensions")).map { extensionElement in
            let parts = try Der.children(try Der.expect(extensionElement, DerTag.sequence, "An extension"))
            let critical = parts.count == 3 && parts[1].tag == DerTag.boolean && parts[1].contents.first != 0
            return (id: try Der.oid(parts.first), critical: critical)
        }
    }

    static func validateCrlTime(_ crl: X509Crl, _ nowSecs: Int64) throws {
        let nowMs = nowSecs * 1000
        guard crl.nextUpdate >= crl.thisUpdate else { throw TeeTrustError("CRL has an invalid validity window") }
        guard nowMs >= crl.thisUpdate else { throw TeeTrustError("CRL is not yet valid") }
        guard nowMs <= crl.nextUpdate else { throw TeeTrustError("CRL is expired") }
    }

    /// Check `crl` was issued and signed by `issuerDer`, a CA allowed to sign
    /// CRLs, and is current. (dcap-qvl `verifyCrl`.)
    static func verifyCrl(_ crl: X509Crl, issuer issuerDer: [UInt8], nowSecs: Int64) throws {
        let issuer = try parseCertificateStrict(issuerDer)
        guard try isCA(issuer) else { throw TeeTrustError("CRL issuer is not a CA certificate") }
        try requireKeyUsage(issuer, crlSign, "sign CRLs")
        guard crl.issuer == issuer.subject else {
            throw TeeTrustError("CRL issuer does not match the expected issuer certificate")
        }
        try validateCrlTime(crl, nowSecs)
        guard let hash = crlSignatureHashes[crl.signatureAlgorithm] else {
            throw TeeTrustError("Unsupported CRL signature algorithm: \(crl.signatureAlgorithm)")
        }
        guard
            X509.verifyEcdsa(
                spki: issuer.spki, curve: issuer.curve, hash: hash, signature: crl.signature, data: crl.tbs)
        else { throw TeeTrustError("CRL signature is invalid") }
    }

    /// Refuse `certDer` unless a CRL from its issuer, verified against
    /// `issuerDer`, is at hand and does not list it.
    static func checkRevocation(
        _ certDer: [UInt8], issuer issuerDer: [UInt8], crls: [X509Crl], nowSecs: Int64
    ) throws {
        let certificate = try parseCertificateStrict(certDer)
        let matching = crls.filter { $0.issuer == certificate.issuer }
        guard !matching.isEmpty else { throw TeeTrustError("Certificate revocation status is unknown") }
        let serial = serialBytes(certificate.serial)
        for crl in matching {
            try verifyCrl(crl, issuer: issuerDer, nowSecs: nowSecs)
            if crl.revokedSerials.contains(where: { serialBytes($0) == serial }) {
                throw TeeTrustError("Certificate is revoked")
            }
        }
    }

    // MARK: - Chains

    /// Verify `leaf` up through `intermediates` to `trustAnchor`, with
    /// current, authenticated revocation status for every certificate but the
    /// anchor. (dcap-qvl `verifyCertificatePath`.)
    static func verifyPath(
        leaf: [UInt8], intermediates: [[UInt8]], nowSecs: Int64, crls: [X509Crl], trustAnchor: [UInt8],
        leafMustBeCa: Bool
    ) throws {
        let anchor = try parseCertificateStrict(trustAnchor)
        let path = try ([leaf] + intermediates).map(parseCertificateStrict)
        if leafMustBeCa {
            guard try isCA(path[0]) else { throw TeeTrustError("CRL issuer certificate is not a CA") }
        } else if try isCA(path[0]) {
            throw TeeTrustError("CaUsedAsEndEntity")
        }

        for (index, current) in path.enumerated() {
            if current.der == anchor.der {
                guard index == path.count - 1 else {
                    throw TeeTrustError("Certificate chain continues after the trust anchor")
                }
                continue
            }
            let issuer = index + 1 < path.count ? path[index + 1] : anchor
            if index > 0 {
                guard try isCA(current) else { throw TeeTrustError("Non-CA certificate used as an intermediate") }
                try requireKeyUsage(current, keyCertSign, "sign certificates")
                // RFC 5280 4.2.1.9: pathLenConstraint bounds the non-self-issued
                // intermediates that may follow this certificate.
                if let pathLength = try basicConstraints(current)?.pathLength {
                    let following = path[1..<index].filter { !isSelfIssued($0) }.count
                    guard following <= pathLength else {
                        throw TeeTrustError("Certificate path length constraint violated")
                    }
                }
            }
            guard current.issuer == issuer.subject else {
                throw TeeTrustError("Failed to verify certificate chain - issuer name mismatch")
            }
            guard Sigstore.issuedBy(current, issuer) else {
                throw TeeTrustError("Failed to verify certificate chain - signature invalid")
            }
            try validateTime(current, nowSecs)
            try checkRevocation(current.der, issuer: issuer.der, crls: crls, nowSecs: nowSecs)
        }
    }

    /// An end-entity chain to the trust anchor. (dcap-qvl `verifyCertificateChain`.)
    static func verifyChain(
        leaf: [UInt8], intermediates: [[UInt8]], nowSecs: Int64, crls: [X509Crl], trustAnchor: [UInt8]
    ) throws {
        guard !crls.isEmpty else { throw TeeTrustError("CRLs are required") }
        try verifyPath(
            leaf: leaf, intermediates: intermediates, nowSecs: nowSecs, crls: crls, trustAnchor: trustAnchor,
            leafMustBeCa: false)
    }

    /// Authenticate the two collateral CRLs before any revocation result is
    /// used: the root CRL by the trust anchor (and applied to the root), the
    /// PCK CRL by its issuer chain. (dcap-qvl `validateCollateralCrls`.)
    static func validateCollateralCrls(
        root: [UInt8], pckCrlIssuerChain: String, rootCaCrl: [UInt8], pckCrl: [UInt8], nowSecs: Int64
    ) throws -> [X509Crl] {
        let rootCrl = try parseCrl(rootCaCrl)
        let pck = try parseCrl(pckCrl)
        try verifyCrl(rootCrl, issuer: root, nowSecs: nowSecs)
        try checkRevocation(root, issuer: root, crls: [rootCrl], nowSecs: nowSecs)
        let issuers = X509.pemCertificates(pckCrlIssuerChain)
        guard let pckIssuer = issuers.first else { throw TeeTrustError("PCK CRL issuer chain is empty or malformed") }
        try verifyPath(
            leaf: pckIssuer, intermediates: Array(issuers.dropFirst()), nowSecs: nowSecs, crls: [rootCrl],
            trustAnchor: root, leafMustBeCa: true)
        try verifyCrl(pck, issuer: pckIssuer, nowSecs: nowSecs)
        return [rootCrl, pck]
    }
}
