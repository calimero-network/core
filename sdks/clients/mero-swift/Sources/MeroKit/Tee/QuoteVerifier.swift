import Foundation

// Verifying a TEE node's quote on the device, before trusting the key it
// binds. Port of mero-js `src/sealed/verify.ts`.
//
// A quote proves what runs in a TD only once it has been checked: its
// signatures against Intel's root, the TCB status of the platform, the
// measurements of the image, and that its report data is the nonce and the
// binding this client asked for. ``createQuoteVerifier(_:)`` does all of
// that here, with nothing but the node: the node hands over the Intel-signed
// collateral with its quote (`includeCollateral`), and a node cannot forge
// collateral, only choose which valid collateral to serve. The DCAP
// verification itself is ``Dcap/verify(_:collateral:nowSecs:)`` (mero-js takes
// `@phala/dcap-qvl`'s `verify` for it; this SDK carries its own port).

/// What a ``TransportQuoteVerifier`` is asked to check: a quote, the nonce
/// this client sent, and the binding it expects after it — the report data
/// must be `nonce ‖ reportDataSuffix` (both hex). (== the argument of mero-js
/// `VerifyTransportQuote`.)
public struct TransportQuoteArgs: Sendable, Equatable {
    public var quoteB64: String
    public var nonce: String
    public var reportDataSuffix: String
    /// What the node sent with the quote, when the verifier asked for it.
    public var collateral: DcapCollateral?

    public init(quoteB64: String, nonce: String, reportDataSuffix: String, collateral: DcapCollateral? = nil) {
        self.quoteB64 = quoteB64
        self.nonce = nonce
        self.reportDataSuffix = reportDataSuffix
        self.collateral = collateral
    }
}

/// Decides whether a node's quote is one to trust. (== mero-js `VerifyTransportQuote`.)
///
/// Verify the quote — never by asking the node being attested — and check
/// that its report data is `nonce ‖ reportDataSuffix`. Return `true` only when
/// the quote is genuine, its measurements are ones you accept, and the report
/// data matches; throw (or return `false`) otherwise. ``createQuoteVerifier(_:)``
/// does all of this on the device.
public protocol TransportQuoteVerifier: Sendable {
    /// Ask the node for the collateral its quote verifies against. Off unless
    /// set, because a node that predates the field refuses the request.
    var includeCollateral: Bool { get }
    func verify(_ args: TransportQuoteArgs) async throws -> Bool
}

/// A ``TransportQuoteVerifier`` from a closure.
public struct AnyTransportQuoteVerifier: TransportQuoteVerifier {
    public let includeCollateral: Bool
    private let body: @Sendable (TransportQuoteArgs) async throws -> Bool

    public init(
        includeCollateral: Bool = false, _ body: @escaping @Sendable (TransportQuoteArgs) async throws -> Bool
    ) {
        self.includeCollateral = includeCollateral
        self.body = body
    }

    public func verify(_ args: TransportQuoteArgs) async throws -> Bool { try await body(args) }
}

/// One image's measurements, all five registers, hex. A quote matches it only
/// when every register does, so measurements of different images never
/// combine. (== mero-js `TrustedMeasurements`.)
public struct TrustedMeasurements: Sendable, Equatable, Codable {
    public var mrtd: String
    public var rtmr0: String
    public var rtmr1: String
    public var rtmr2: String
    public var rtmr3: String

    public init(mrtd: String, rtmr0: String, rtmr1: String, rtmr2: String, rtmr3: String) {
        self.mrtd = mrtd
        self.rtmr0 = rtmr0
        self.rtmr1 = rtmr1
        self.rtmr2 = rtmr2
        self.rtmr3 = rtmr3
    }

    var registers: [String] { [mrtd, rtmr0, rtmr1, rtmr2, rtmr3] }
}

/// What ``createQuoteVerifier(_:)`` trusts. (== mero-js `QuoteVerifierOptions`.)
public struct QuoteVerifierOptions: Sendable {
    /// DCAP verification; ``Dcap/verify(_:collateral:nowSecs:)`` by default.
    public var dcapVerify: DcapVerify
    /// The images trusted, each by all of its measurements: the way to say
    /// which code may read the traffic. Build it with
    /// ``trustedMeasurementsFromReleases(_:profile:allowDebugProfile:)``.
    public var allowedMeasurements: [TrustedMeasurements]?
    /// MRTDs (hex) accepted. On its own this does not name an image: on GCP
    /// the MRTD measures the platform's TD firmware, which every image
    /// shares, and the image is in RTMR1–3. So a verifier needs either
    /// ``allowedMeasurements``, or this with all of ``allowedRtmr1``–``allowedRtmr3``.
    public var allowedMrtd: [String]?
    /// RTMR allowlists (hex); a register is not checked when its list is `nil`.
    public var allowedRtmr0: [String]?
    public var allowedRtmr1: [String]?
    public var allowedRtmr2: [String]?
    public var allowedRtmr3: [String]?
    /// TCB statuses accepted; `["UpToDate"]` by default. `Revoked` is refused even when listed.
    public var allowedTcbStatuses: [String]?
    /// Where to get collateral when the node sends none (it predates
    /// `includeCollateral`). Without it, such a node's quote is refused.
    public var fetchCollateral: (@Sendable (Data) async throws -> DcapCollateral)?
    /// The current time in milliseconds. For tests.
    public var now: @Sendable () -> Int64

    public init(
        dcapVerify: @escaping DcapVerify = { try Dcap.verify($0, collateral: $1, nowSecs: $2) },
        allowedMeasurements: [TrustedMeasurements]? = nil, allowedMrtd: [String]? = nil,
        allowedRtmr0: [String]? = nil, allowedRtmr1: [String]? = nil, allowedRtmr2: [String]? = nil,
        allowedRtmr3: [String]? = nil, allowedTcbStatuses: [String]? = nil,
        fetchCollateral: (@Sendable (Data) async throws -> DcapCollateral)? = nil,
        now: @escaping @Sendable () -> Int64 = { Int64(Date().timeIntervalSince1970 * 1000) }
    ) {
        self.dcapVerify = dcapVerify
        self.allowedMeasurements = allowedMeasurements
        self.allowedMrtd = allowedMrtd
        self.allowedRtmr0 = allowedRtmr0
        self.allowedRtmr1 = allowedRtmr1
        self.allowedRtmr2 = allowedRtmr2
        self.allowedRtmr3 = allowedRtmr3
        self.allowedTcbStatuses = allowedTcbStatuses
        self.fetchCollateral = fetchCollateral
        self.now = now
    }

    /// Trust what `trusted` names (``trustedMeasurementsFromReleases(_:profile:allowDebugProfile:)``,
    /// ``trustSignedRelease(_:)``).
    public init(
        trusting trusted: TrustedReleaseOptions,
        dcapVerify: @escaping DcapVerify = { try Dcap.verify($0, collateral: $1, nowSecs: $2) },
        fetchCollateral: (@Sendable (Data) async throws -> DcapCollateral)? = nil,
        now: @escaping @Sendable () -> Int64 = { Int64(Date().timeIntervalSince1970 * 1000) }
    ) {
        self.init(
            dcapVerify: dcapVerify, allowedMeasurements: trusted.allowedMeasurements,
            allowedTcbStatuses: trusted.allowedTcbStatuses, fetchCollateral: fetchCollateral, now: now)
    }
}

/// A verifier that checks a TEE node's quote on the device; it asks the node
/// for collateral. (== mero-js `QuoteVerifier`.)
public struct QuoteVerifier: TransportQuoteVerifier {
    public var includeCollateral: Bool { true }

    let options: QuoteVerifierOptions
    let allowedMrtd: [String]
    let allowedImages: [String]
    let allowedRtmr: [[String]]
    let allowedStatuses: [String]

    public func verify(_ args: TransportQuoteArgs) async throws -> Bool {
        guard let quoteData = Data(base64Encoded: args.quoteB64) else {
            throw TeeTrustError("The quote did not verify: it is not base64")
        }
        var against = args.collateral
        if against == nil, let fetch = options.fetchCollateral { against = try await fetch(quoteData) }
        guard let collateral = against else {
            throw TeeTrustError(
                "The node sent no collateral for its quote, and no fetchCollateral was given to get it")
        }

        let verified: DcapVerifiedReport
        do {
            verified = try options.dcapVerify(quoteData, collateral, options.now() / 1000)
        } catch {
            let reason = (error as? LocalizedError)?.errorDescription ?? "\(error)"
            throw TeeTrustError("The quote did not verify: \(reason)")
        }
        guard verified.status != "Revoked", allowedStatuses.contains(verified.status) else {
            throw TeeTrustError("The platform's TCB status is \(verified.status), which is not accepted")
        }
        guard let td = verified.report.td10 else {
            throw TeeTrustError("The quote is a \(verified.report.type) quote, not a TDX one")
        }
        // TUD bit 0. A debug TD's memory and registers are open to the host,
        // so its measurements prove nothing about what runs in it. Checked
        // here, not left to `dcapVerify`, which a caller may swap for one
        // that allows debug; attributes that cannot be read count as debug.
        guard td.tdAttributes.count == 8, (td.tdAttributes.first ?? 1) & 0x01 == 0 else {
            throw TeeTrustError("The quote is from a debug TD, or its TD attributes cannot be read")
        }
        let measured = [td.mrTd, td.rtMr0, td.rtMr1, td.rtMr2, td.rtMr3].map { Hex.encode($0) }
        if !allowedImages.isEmpty, !allowedImages.contains(measured.joined(separator: ",")) {
            let rtmrs = measured.dropFirst().joined(separator: ", ")
            throw TeeTrustError(
                "The measurements (MRTD \(measured[0]), RTMR0-3 \(rtmrs)) are not an image this verifier trusts")
        }
        if !allowedMrtd.isEmpty, !allowedMrtd.contains(measured[0]) {
            throw TeeTrustError("MRTD \(measured[0]) is not an image this verifier trusts")
        }
        for (index, allowed) in allowedRtmr.enumerated() where !allowed.isEmpty {
            let register = measured[index + 1]
            guard allowed.contains(register) else {
                throw TeeTrustError("RTMR\(index) \(register) is not one this verifier trusts")
            }
        }
        guard Hex.encode(td.reportData) == (args.nonce + args.reportDataSuffix).lowercased() else {
            throw TeeTrustError("The quote does not commit to this nonce and binding")
        }
        return true
    }
}

/// A verifier that checks a TEE node's quote on the device, and accepts it
/// only when every check passes (== mero-js `createQuoteVerifier`):
///
/// - its signatures verify against Intel's root, with collateral valid now;
/// - the platform's TCB status is one allowed (`UpToDate` by default), and
///   never `Revoked`;
/// - it is not a debug TD;
/// - its measurements are those of a trusted image: one of
///   `allowedMeasurements` in full, and the MRTD and any RTMRs listed;
/// - its report data is the nonce followed by the binding the client computed.
///
/// Throws ``TeeTrustError`` when the options would accept nothing, or name a
/// measurement that is not one. Mock quotes never pass: they carry no signature.
public func createQuoteVerifier(_ options: QuoteVerifierOptions) throws -> QuoteVerifier {
    let allowedMrtd = try measurements(options.allowedMrtd ?? [], "allowedMrtd")
    let allowedImages = try (options.allowedMeasurements ?? []).enumerated().map { index, image in
        let names = ["mrtd", "rtmr0", "rtmr1", "rtmr2", "rtmr3"]
        return try zip(names, image.registers).map { name, value in
            try measurements([value], "allowedMeasurements[\(index)].\(name)")[0]
        }.joined(separator: ",")
    }
    let allowedRtmr = [
        try measurements(options.allowedRtmr0 ?? [], "allowedRtmr0"),
        try measurements(options.allowedRtmr1 ?? [], "allowedRtmr1"),
        try measurements(options.allowedRtmr2 ?? [], "allowedRtmr2"),
        try measurements(options.allowedRtmr3 ?? [], "allowedRtmr3"),
    ]
    if options.allowedMeasurements != nil, allowedImages.isEmpty {
        throw TeeTrustError("allowedMeasurements is empty, so no quote would ever be accepted")
    }
    if allowedImages.isEmpty {
        guard !allowedMrtd.isEmpty else {
            throw TeeTrustError("No image is trusted: give allowedMeasurements, or allowedMrtd with allowedRtmr1-3")
        }
        guard allowedRtmr.dropFirst().allSatisfy({ !$0.isEmpty }) else {
            throw TeeTrustError(
                "allowedMrtd alone does not name an image: the MRTD measures the TD firmware, which "
                    + "images share, and the image is in RTMR1-3. Give allowedMeasurements, or allowedRtmr1-3 too")
        }
    }
    return QuoteVerifier(
        options: options, allowedMrtd: allowedMrtd, allowedImages: allowedImages, allowedRtmr: allowedRtmr,
        allowedStatuses: options.allowedTcbStatuses ?? ["UpToDate"])
}

private func measurements(_ values: [String], _ label: String) throws -> [String] {
    try values.map { value in
        let normalized = value.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard normalized.utf8.count == 96, normalized.utf8.allSatisfy({ Hex.isLowerHexDigit($0) }) else {
            throw TeeTrustError("\(label) holds \(CanonicalJSON.quote(value)), which is not a 48-byte hex measurement")
        }
        return normalized
    }
}

extension Hex {
    static func isLowerHexDigit(_ c: UInt8) -> Bool { (0x30...0x39).contains(c) || (0x61...0x66).contains(c) }
}

// MARK: - Published measurements

/// One profile of a release's image, as `published-mrtds.json` lists it.
public struct PublishedProfile: Sendable, Equatable, Codable {
    public var mrtd: String
    public var rtmr0: String
    public var rtmr1: String
    public var rtmr2: String
    public var rtmr3: String
    /// The TCB statuses this image is accepted at, in any case (`uptodate`).
    public var allowedTcbStatuses: [String]?

    enum CodingKeys: String, CodingKey {
        case mrtd, rtmr0, rtmr1, rtmr2, rtmr3
        case allowedTcbStatuses = "allowed_tcb_statuses"
    }

    public init(
        mrtd: String, rtmr0: String, rtmr1: String, rtmr2: String, rtmr3: String, allowedTcbStatuses: [String]? = nil
    ) {
        self.mrtd = mrtd
        self.rtmr0 = rtmr0
        self.rtmr1 = rtmr1
        self.rtmr2 = rtmr2
        self.rtmr3 = rtmr3
        self.allowedTcbStatuses = allowedTcbStatuses
    }
}

/// A mero-tee node release's `published-mrtds.json`: the measurements the
/// release workflow observed booting each profile of its image on TDX,
/// signed with the release. Only the fields read here. (== mero-js `PublishedMrtds`.)
public struct PublishedMrtds: Sendable, Equatable, Codable {
    public var role: String?
    public var tag: String?
    public var profiles: [String: PublishedProfile]

    public init(role: String? = nil, tag: String? = nil, profiles: [String: PublishedProfile]) {
        self.role = role
        self.tag = tag
        self.profiles = profiles
    }
}

/// What ``trustedMeasurementsFromReleases(_:profile:allowDebugProfile:)`` and
/// ``trustSignedRelease(_:)`` hand ``QuoteVerifierOptions/init(trusting:dcapVerify:fetchCollateral:now:)``.
public struct TrustedReleaseOptions: Sendable, Equatable {
    public var allowedMeasurements: [TrustedMeasurements]
    public var allowedTcbStatuses: [String]

    public init(allowedMeasurements: [TrustedMeasurements], allowedTcbStatuses: [String]) {
        self.allowedMeasurements = allowedMeasurements
        self.allowedTcbStatuses = allowedTcbStatuses
    }
}

/// Constants of TEE trust. (mero-js `PRODUCTION_PROFILE`, `NODE_RELEASE_SIGNER`,
/// `DEFAULT_RELEASE_MIRROR`, `KMS_RELEASE_SIGNER`.)
public enum TeeTrust {
    /// The one image profile a release publishes for production. The `debug`
    /// and `debug-read-only` images are signed and published beside it, but
    /// run with SSH and the operator's tooling inside the TD, so trusting one
    /// trusts an image the operator can read. (== mero-js `PRODUCTION_PROFILE`.)
    public static let productionProfile = "locked-read-only"

    /// TCB statuses as DCAP reports them; releases list them in lower case.
    static let tcbStatuses = [
        "UpToDate", "SWHardeningNeeded", "ConfigurationNeeded", "ConfigurationAndSWHardeningNeeded", "OutOfDate",
        "OutOfDateConfigurationNeeded",
    ]

    /// DCAP's spelling of a TCB status, whatever its case.
    static func tcbStatus(_ status: String) -> String? {
        tcbStatuses.first { $0.lowercased() == status.lowercased() }
    }
}

/// The options that trust exactly one profile of the given releases: each
/// release's image by all of its measurements, and only the TCB statuses
/// every one of those releases accepts. (== mero-js `trustedMeasurementsFromReleases`.)
///
/// Pass every release a node you talk to may run: during a rollout, the old
/// one and the new one. Where the file comes from is the trust decision:
/// shipping it with the app trusts that release; fetching it at run time
/// trusts whoever serves it (see ``trustSignedRelease(_:)`` for a signed one).
public func trustedMeasurementsFromReleases(
    _ releases: [PublishedMrtds], profile: String, allowDebugProfile: Bool = false
) throws -> TrustedReleaseOptions {
    guard !releases.isEmpty else { throw TeeTrustError("No release given, so no image would be trusted") }
    var statuses: [String]?
    var allowed: [TrustedMeasurements] = []
    for (index, release) in releases.enumerated() {
        let name = release.tag ?? "releases[\(index)]"
        if let role = release.role, role != "node" {
            throw TeeTrustError("\(name) lists the measurements of a \(role), not of a node image")
        }
        guard let image = release.profiles[profile] else {
            let known = release.profiles.keys.sorted().joined(separator: ", ")
            throw TeeTrustError(
                "\(name) has no profile \(CanonicalJSON.quote(profile)) (it has: \(known.isEmpty ? "none" : known))")
        }
        if profile != TeeTrust.productionProfile, !allowDebugProfile {
            let quoted = CanonicalJSON.quote(profile)
            throw TeeTrustError(
                "\(quoted) is not a production profile: its image is readable by whoever operates the node. "
                    + "Trust \(CanonicalJSON.quote(TeeTrust.productionProfile)), or pass allowDebugProfile to test")
        }
        let accepted = try (image.allowedTcbStatuses ?? ["uptodate"]).map { status -> String in
            guard let known = TeeTrust.tcbStatus(status) else {
                throw TeeTrustError(
                    "\(name) accepts TCB status \(CanonicalJSON.quote(status)), which this verifier does not know")
            }
            return known
        }
        statuses = statuses.map { common in common.filter { accepted.contains($0) } } ?? accepted
        allowed.append(
            TrustedMeasurements(
                mrtd: image.mrtd, rtmr0: image.rtmr0, rtmr1: image.rtmr1, rtmr2: image.rtmr2, rtmr3: image.rtmr3))
    }
    guard let statuses, !statuses.isEmpty else {
        throw TeeTrustError("The releases given accept no TCB status in common")
    }
    return TrustedReleaseOptions(allowedMeasurements: allowed, allowedTcbStatuses: statuses)
}
