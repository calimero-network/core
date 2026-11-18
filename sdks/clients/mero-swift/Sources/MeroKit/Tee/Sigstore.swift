import CryptoKit
import Foundation

// Verifying a cosign keyless signature, as `cosign verify-blob --bundle` does,
// with CryptoKit and the Sigstore trust root embedded here. Port of mero-js
// `src/sealed/sigstore.ts` and `sigstore-trust-root.ts`.
//
// mero-tee signs each release asset with cosign keyless: there is no release
// key, only a Fulcio certificate that lives ten minutes, issued to one GitHub
// Actions workflow run, and a Rekor log entry that proves the signature was
// made while the certificate was valid. So the checks are: the signature over
// the bytes verifies with the certificate's key; Rekor's signed entry
// timestamp verifies, and the entry it signs is this signature over these
// bytes by this certificate; the certificate chains to a Fulcio CA and was
// valid when Rekor logged it; and the certificate names the workflow expected.
// The same checks core runs in `crates/tee-release/src/sigstore_verify.rs`.
//
// Nothing here reads the clock. Every time is Rekor's `integratedTime`, which
// its signature covers, so a verification that passes once passes forever,
// whatever the device's clock says.

/// Who must have signed: the certificate's SAN URI and its OIDC issuer, each
/// matched exactly. (== mero-js `SignerIdentity`.)
public struct SignerIdentity: Sendable, Equatable {
    public var identity: String
    public var issuer: String

    public init(identity: String, issuer: String) {
        self.identity = identity
        self.issuer = issuer
    }
}

/// A Fulcio CA: its chain from the certificate that issues leaves up to the
/// root, DER as base64, and when the trust root says it issued certificates.
/// (== mero-js `FulcioAuthority`.)
public struct FulcioAuthority: Sendable, Equatable {
    public let uri: String
    public let validFrom: String
    /// `nil`: it still issues.
    public let validUntil: String?
    public let certChain: [String]
}

/// A Rekor log whose signed entry timestamps are accepted. (== mero-js `RekorLog`.)
public struct RekorLog: Sendable, Equatable {
    public let baseUrl: String
    /// The log id a bundle names it by: the SHA-256 of its public key, hex.
    public let logId: String
    /// Its public key: DER SubjectPublicKeyInfo, base64. ECDSA P-256 with SHA-256.
    public let publicKey: String
    public let validFrom: String
    public let validUntil: String?
}

public enum Sigstore {
    /// The parts of the Sigstore public-good trust root that verifying a
    /// mero-tee release's cosign bundle needs: the Fulcio certificate
    /// authorities that issue the release workflow's short-lived signing
    /// certificates. Copied from the `trusted_root.json` Sigstore publishes
    /// through its TUF repository, as mero-js embeds it
    /// (`FULCIO_AUTHORITIES`). Embedded rather than fetched: fetching it at
    /// run time would trust whoever serves it, the very thing this check
    /// exists to avoid.
    public static let fulcioAuthorities: [FulcioAuthority] = [
        FulcioAuthority(
            uri: "https://fulcio.sigstore.dev", validFrom: "2021-03-07T03:20:29Z",
            validUntil: "2022-12-31T23:59:59.999Z",
            certChain: [
                [
                    "MIIB+DCCAX6gAwIBAgITNVkDZoCiofPDsy7dfm6geLbuhzAKBggqhkjOPQQDAzAqMRUwEwYDVQQKEwxzaWdzdG9y",
                    "ZS5kZXYxETAPBgNVBAMTCHNpZ3N0b3JlMB4XDTIxMDMwNzAzMjAyOVoXDTMxMDIyMzAzMjAyOVowKjEVMBMGA1UE",
                    "ChMMc2lnc3RvcmUuZGV2MREwDwYDVQQDEwhzaWdzdG9yZTB2MBAGByqGSM49AgEGBSuBBAAiA2IABLSyA7Ii5k+p",
                    "NO8ZEWY0ylemWDowOkNa3kL+GZE5Z5GWehL9/A9bRNA3RbrsZ5i0JcastaRL7Sp5fp/jD5dxqc/UdTVnlvS16an+",
                    "2Yfswe/QuLolRUCrcOE2+2iA5+tzd6NmMGQwDgYDVR0PAQH/BAQDAgEGMBIGA1UdEwEB/wQIMAYBAf8CAQEwHQYD",
                    "VR0OBBYEFMjFHQBBmiQpMlEk6w2uSu1KBtPsMB8GA1UdIwQYMBaAFMjFHQBBmiQpMlEk6w2uSu1KBtPsMAoGCCqG",
                    "SM49BAMDA2gAMGUCMH8liWJfMui6vXXBhjDgY4MwslmN/TJxVe/83WrFomwmNf056y1X48F9c4m3a3ozXAIxAKjR",
                    "ay5/aj/jsKKGIkmQatjI8uupHr/+CxFvaJWmpYqNkLDGRU+9orzh5hI2RrcuaQ==",
                ].joined()
            ]),
        FulcioAuthority(
            uri: "https://fulcio.sigstore.dev", validFrom: "2022-04-13T20:06:15Z",
            validUntil: nil,
            certChain: [
                [
                    "MIICGjCCAaGgAwIBAgIUALnViVfnU0brJasmRkHrn/UnfaQwCgYIKoZIzj0EAwMwKjEVMBMGA1UEChMMc2lnc3Rv",
                    "cmUuZGV2MREwDwYDVQQDEwhzaWdzdG9yZTAeFw0yMjA0MTMyMDA2MTVaFw0zMTEwMDUxMzU2NThaMDcxFTATBgNV",
                    "BAoTDHNpZ3N0b3JlLmRldjEeMBwGA1UEAxMVc2lnc3RvcmUtaW50ZXJtZWRpYXRlMHYwEAYHKoZIzj0CAQYFK4EE",
                    "ACIDYgAE8RVS/ysH+NOvuDZyPIZtilgUF9NlarYpAd9HP1vBBH1U5CV77LSS7s0ZiH4nE7Hv7ptS6LvvR/STk798",
                    "LVgMzLlJ4HeIfF3tHSaexLcYpSASr1kS0N/RgBJz/9jWCiXno3sweTAOBgNVHQ8BAf8EBAMCAQYwEwYDVR0lBAww",
                    "CgYIKwYBBQUHAwMwEgYDVR0TAQH/BAgwBgEB/wIBADAdBgNVHQ4EFgQU39Ppz1YkEZb5qNjpKFWixi4YZD8wHwYD",
                    "VR0jBBgwFoAUWMAeX5FFpWapesyQoZMi0CrFxfowCgYIKoZIzj0EAwMDZwAwZAIwPCsQK4DYiZYDPIaDi5HFKnfx",
                    "Xx6ASSVmERfsynYBiX2X6SJRnZU84/9DZdnFvvxmAjBOt6QpBlc4J/0DxvkTCqpclvziL6BCCPnjdlIB3Pu3BxsP",
                    "mygUY7Ii2zbdCdliiow=",
                ].joined(),
                [
                    "MIIB9zCCAXygAwIBAgIUALZNAPFdxHPwjeDloDwyYChAO/4wCgYIKoZIzj0EAwMwKjEVMBMGA1UEChMMc2lnc3Rv",
                    "cmUuZGV2MREwDwYDVQQDEwhzaWdzdG9yZTAeFw0yMTEwMDcxMzU2NTlaFw0zMTEwMDUxMzU2NThaMCoxFTATBgNV",
                    "BAoTDHNpZ3N0b3JlLmRldjERMA8GA1UEAxMIc2lnc3RvcmUwdjAQBgcqhkjOPQIBBgUrgQQAIgNiAAT7XeFT4rb3",
                    "PQGwS4IajtLk3/OlnpgangaBclYpsYBr5i+4ynB07ceb3LP0OIOZdxexX69c5iVuyJRQ+Hz05yi+UF3uBWAlHpiS",
                    "5sh0+H2GHE7SXrk1EC5m1Tr19L9gg92jYzBhMA4GA1UdDwEB/wQEAwIBBjAPBgNVHRMBAf8EBTADAQH/MB0GA1Ud",
                    "DgQWBBRYwB5fkUWlZql6zJChkyLQKsXF+jAfBgNVHSMEGDAWgBRYwB5fkUWlZql6zJChkyLQKsXF+jAKBggqhkjO",
                    "PQQDAwNpADBmAjEAj1nHeXZp+13NWBNa+EDsDP8G1WWg1tCMWP/WHPqpaVo0jhsweNFZgSs0eE7wYI4qAjEA2WB9",
                    "ot98sIkoF3vZYdd3/VtWB5b9TNMea7Ix/stJ5TfcLLeABLE4BNJOsQ4vnBHJ",
                ].joined(),
            ]),
    ]

    /// The Rekor log key that signs a bundle's signed entry timestamp
    /// (`REKOR_LOGS`). The Rekor v2 log, the CT logs and the timestamp
    /// authority are left out: a legacy cosign bundle never uses them, and a
    /// bundle naming any other log is refused.
    public static let rekorLogs: [RekorLog] = [
        RekorLog(
            baseUrl: "https://rekor.sigstore.dev",
            logId: "c0d23d6ad406973f9559f3ba2d1ca01f84147d8ffc5b8445c224f98b9591801d",
            publicKey: [
                "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAE2G2Y+2tabdTV5BcGiBIx0a9fAFwrkBbmLSGtks4L3qX6yYY0zufB",
                "nhC8Ur/iy55GhWP/9A/bY2LhC30M9+RYtw==",
            ].joined(),
            validFrom: "2021-01-12T11:53:27Z", validUntil: nil)
    ]

    static let subjectAltName = "2.5.29.17"
    static let extendedKeyUsage = "2.5.29.37"
    static let codeSigning = "1.3.6.1.5.5.7.3.3"
    /// Fulcio's OIDC issuer extension, a DER UTF8String.
    static let fulcioIssuerV2 = "1.3.6.1.4.1.57264.1.8"
    /// Its deprecated predecessor, the raw issuer bytes; read only when the other is absent.
    static let fulcioIssuerV1 = "1.3.6.1.4.1.57264.1.1"
    static let sanURI: UInt8 = 0x86

    /// Verify a cosign keyless `bundle` (JSON text) over `body`, signed by
    /// `signer`. Returns the time Rekor logged it (seconds); throws
    /// ``TeeTrustError`` with the reason otherwise. `signature`, when given,
    /// is the detached `.sig` published beside the file, and must be the
    /// bundle's. (== mero-js `verifyCosignBundle`.)
    @discardableResult
    public static func verifyCosignBundle(
        _ body: Data, bundle: String, signer: SignerIdentity, signature: String? = nil
    ) throws -> Int64 {
        guard let parsed = try? MeroJSON.decode(JSONValue.self, from: Data(bundle.utf8)) else {
            throw TeeTrustError("The bundle is not JSON")
        }
        return try verifyCosignBundle(body, bundle: parsed, signer: signer, signature: signature)
    }

    /// As the overload that takes the bundle as text, with the bundle parsed.
    @discardableResult
    public static func verifyCosignBundle(
        _ body: Data, bundle: JSONValue, signer: SignerIdentity, signature: String? = nil
    ) throws -> Int64 {
        let parsed = try CosignBundle(bundle)
        let detached = signature?.trimmingCharacters(in: .whitespacesAndNewlines)
        if let detached, detached != parsed.base64Signature.trimmingCharacters(in: .whitespacesAndNewlines) {
            throw TeeTrustError("The detached signature is not the one in the bundle")
        }
        let sig = try X509.fromBase64(parsed.base64Signature, "The bundle signature")
        let leafPem = String(decoding: try X509.fromBase64(parsed.cert, "The bundle certificate"), as: UTF8.self)
        let leafDer = try X509.pemCertificate(leafPem, "The bundle certificate")
        let leaf = try X509.parseCertificate(leafDer)

        // Rekor vouches for the entry and its time: nothing below is trusted before this.
        guard let log = rekorLogs.first(where: { $0.logId == parsed.logID.lowercased() }) else {
            throw TeeTrustError(
                "The bundle names Rekor log \(parsed.logID), which the Sigstore trust root does not know")
        }
        let canonical =
            "{\"body\":\(CanonicalJSON.quote(parsed.body)),\"integratedTime\":\(parsed.integratedTime),"
            + "\"logID\":\(CanonicalJSON.quote(parsed.logID)),\"logIndex\":\(parsed.logIndex)}"
        let setVerifies = X509.verifyEcdsa(
            spki: try X509.fromBase64(log.publicKey, "The Rekor key"), curve: .p256, hash: .sha256,
            signature: try X509.fromBase64(parsed.signedEntryTimestamp, "The signed entry timestamp"),
            data: Array(canonical.utf8))
        guard setVerifies else {
            throw TeeTrustError(
                "Rekor's signed entry timestamp does not verify: the log entry, its time or its index was altered")
        }
        let at = parsed.integratedTime * 1000
        guard within(at, from: log.validFrom, until: log.validUntil) else {
            throw TeeTrustError(
                "Rekor log \(log.baseUrl) was not in use at \(IsoTime.string(at)), when the entry says it was logged")
        }

        // The entry Rekor signed is this signature, over these bytes, by this certificate.
        let entry = try HashedRekord(parsed.body)
        guard entry.hash == Hex.encode(sha256(body)) else {
            throw TeeTrustError("The file's SHA-256 is not the one Rekor logged: the file is not the one signed")
        }
        guard try X509.fromBase64(entry.signature, "The logged signature") == sig else {
            throw TeeTrustError("The bundle's signature is not the one Rekor logged")
        }
        let loggedPem = String(decoding: try X509.fromBase64(entry.publicKey, "The logged certificate"), as: UTF8.self)
        guard try X509.pemCertificate(loggedPem, "The logged certificate") == leafDer else {
            throw TeeTrustError("The bundle's certificate is not the one Rekor logged")
        }

        guard leaf.curve == .p256,
            X509.verifyEcdsa(spki: leaf.spki, curve: .p256, hash: .sha256, signature: sig, data: [UInt8](body))
        else {
            throw TeeTrustError("The signature over the file does not verify with the bundle's certificate")
        }

        try chainsToFulcio(leaf, at: at)
        try checkSigner(leaf, signer)
        return parsed.integratedTime
    }

    /// Check that `leaf` was issued by a Fulcio CA in the trust root, and
    /// that it, the CA's certificates and the CA itself (by the trust root's
    /// window) were all valid at `at` (milliseconds): when Rekor logged the
    /// signature, not now, since a Fulcio certificate lives ten minutes.
    /// Every signature on the way is checked, with the algorithm its
    /// certificate names. (== mero-js `chainsToFulcio`.)
    static func chainsToFulcio(_ leaf: X509Certificate, at: Int64) throws {
        guard validAt(leaf, at) else {
            throw TeeTrustError(
                "The signing certificate was valid \(IsoTime.string(leaf.notBefore)) to "
                    + "\(IsoTime.string(leaf.notAfter)), not at \(IsoTime.string(at)) when Rekor logged it")
        }
        var reasons: [String] = []
        for authority in fulcioAuthorities {
            let chain = try authority.certChain.map {
                try X509.parseCertificate(try X509.fromBase64($0, "A Fulcio certificate"))
            }
            guard let first = chain.first, issuedBy(leaf, first) else { continue }
            // The issuer is found; from here a failure is this CA's, so say which.
            guard let reason = authorityRefusal(chain, authority, at: at) else { return }
            reasons.append("\(authority.uri) (valid from \(authority.validFrom)): \(reason)")
        }
        throw TeeTrustError(
            reasons.isEmpty
                ? "The signing certificate was not issued by a Fulcio CA in the Sigstore trust root"
                : "The signing certificate's CA is not trusted at \(IsoTime.string(at)): "
                    + reasons.joined(separator: "; "))
    }

    private static func authorityRefusal(
        _ chain: [X509Certificate], _ authority: FulcioAuthority, at: Int64
    ) -> String? {
        guard within(at, from: authority.validFrom, until: authority.validUntil) else {
            return "the trust root says it was not issuing certificates then"
        }
        for (index, certificate) in chain.enumerated() {
            guard validAt(certificate, at) else { return "its certificate \(index) was not valid then" }
            // The root signs itself.
            let parent = index + 1 < chain.count ? chain[index + 1] : certificate
            guard issuedBy(certificate, parent) else { return "its certificate \(index) is not signed by the next" }
        }
        return nil
    }

    static func issuedBy(_ child: X509Certificate, _ parent: X509Certificate) -> Bool {
        guard child.issuer == parent.subject else { return false }
        return X509.verifyEcdsa(
            spki: parent.spki, curve: parent.curve, hash: child.signatureHash, signature: child.signature,
            data: child.tbs)
    }

    private static func checkSigner(_ leaf: X509Certificate, _ signer: SignerIdentity) throws {
        guard let san = leaf.extensions[subjectAltName] else {
            throw TeeTrustError("The signing certificate names no identity")
        }
        let names = try Der.children(
            try Der.expect(try Der.parse(san), DerTag.sequence, "The subject alternative name"))
        let uris = names.filter { $0.tag == sanURI }.map { String(decoding: $0.contents, as: UTF8.self) }
        guard uris.count == 1, names.count == 1, uris[0] == signer.identity else {
            let named =
                uris.count == 1
                ? CanonicalJSON.quote(uris[0])
                : "[" + uris.map(CanonicalJSON.quote).joined(separator: ",") + "]"
            throw TeeTrustError("The release was signed by \(named), not by \(signer.identity)")
        }

        var issuer: String?
        if let v2 = leaf.extensions[fulcioIssuerV2] {
            let value = try Der.expect(try Der.parse(v2), DerTag.utf8String, "The OIDC issuer")
            issuer = String(decoding: value.contents, as: UTF8.self)
        } else if let v1 = leaf.extensions[fulcioIssuerV1] {
            issuer = String(decoding: v1, as: UTF8.self)
        }
        guard issuer == signer.issuer else {
            let named = issuer.map(CanonicalJSON.quote) ?? "missing"
            throw TeeTrustError("The signing certificate's OIDC issuer is \(named), not \(signer.issuer)")
        }

        guard let usage = leaf.extensions[extendedKeyUsage], try Der.objectIdentifiers(usage).contains(codeSigning)
        else {
            throw TeeTrustError("The signing certificate is not for code signing")
        }
    }

    static func validAt(_ certificate: X509Certificate, _ at: Int64) -> Bool {
        certificate.notBefore <= at && at <= certificate.notAfter
    }

    static func within(_ at: Int64, from start: String, until end: String?) -> Bool {
        guard let from = IsoTime.parse(start), from <= at else { return false }
        guard let end else { return true }
        guard let until = IsoTime.parse(end) else { return false }
        return at <= until
    }
}

/// A legacy cosign bundle (`cosign sign-blob --bundle`), as the fields verified here.
private struct CosignBundle {
    let base64Signature: String
    let cert: String
    let signedEntryTimestamp: String
    let body: String
    let integratedTime: Int64
    let logIndex: Int64
    let logID: String

    init(_ value: JSONValue) throws {
        let refusal = TeeTrustError("The bundle is not a cosign bundle with a Rekor entry")
        let rekor = value["rekorBundle"]
        let payload = rekor?["Payload"]
        guard let base64Signature = value["base64Signature"]?.stringValue,
            let cert = value["cert"]?.stringValue,
            let signedEntryTimestamp = rekor?["SignedEntryTimestamp"]?.stringValue,
            let body = payload?["body"]?.stringValue,
            let logID = payload?["logID"]?.stringValue, logID.utf8.count == 64, Hex.is32(logID),
            let integratedTime = Self.safeInteger(payload?["integratedTime"]),
            let logIndex = Self.safeInteger(payload?["logIndex"])
        else { throw refusal }
        self.base64Signature = base64Signature
        self.cert = cert
        self.signedEntryTimestamp = signedEntryTimestamp
        self.body = body
        self.integratedTime = integratedTime
        self.logIndex = logIndex
        self.logID = logID
    }

    /// A non-negative integer JavaScript holds exactly (`Number.isSafeInteger`).
    private static func safeInteger(_ value: JSONValue?) -> Int64? {
        guard let number = value?.doubleValue, number >= 0, number <= 9_007_199_254_740_991,
            number == number.rounded()
        else { return nil }
        return Int64(number)
    }
}

/// The fields of a logged `hashedrekord` entry that are compared.
private struct HashedRekord {
    let hash: String
    let signature: String
    let publicKey: String

    init(_ body: String) throws {
        let decoded = try X509.fromBase64(body, "The logged entry")
        guard let entry = try? MeroJSON.decode(JSONValue.self, from: Data(decoded)) else {
            throw TeeTrustError("The logged entry is not JSON")
        }
        let hash = entry["spec"]?["data"]?["hash"]
        let signature = entry["spec"]?["signature"]
        guard entry["kind"]?.stringValue == "hashedrekord", hash?["algorithm"]?.stringValue == "sha256",
            let value = hash?["value"]?.stringValue,
            let content = signature?["content"]?.stringValue,
            let publicKey = signature?["publicKey"]?["content"]?.stringValue
        else {
            throw TeeTrustError("The logged entry is not a SHA-256 hashedrekord")
        }
        self.hash = value.lowercased()
        self.signature = content
        self.publicKey = publicKey
    }
}
