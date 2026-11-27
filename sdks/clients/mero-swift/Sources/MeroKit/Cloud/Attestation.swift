import Foundation

#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

// The check every cloud client runs before it sends anything: the node's
// quote must be genuine (or, in development, an explicitly allowed mock),
// from an image this client trusts, and its report data must bind this
// nonce, the expected app and the transport key the sealed channel will be
// opened to. Port of mero-js `cloud-v2/attestation.ts`.
//
// Report data, as core's `calimero-cloud-client::attest` builds it:
// `nonce(32) ‖ SHA-256("calimero.tee-attest.transport-key.v1" ‖ app_hash ‖ transport_key)`.
//
// A real quote is judged as the Rust client judges it (`Measurements::Release`):
// the node names the mero-tee release it runs; that release must be at least
// `minReleaseVersion`; its `published-mrtds.json` is fetched and its Sigstore
// signature checked against the node release workflow; the quote's signature
// chain and TCB status are checked against Intel's collateral (fetched here
// from the cloud's PCCS mirror, or Intel's PCS: the node sends none); the TD must not be a debug TD; and
// its registers must be the release's image for the trusted profile,
// `locked-read-only` unless a debug profile is explicitly allowed. Nothing
// here reads the environment: mock quotes pass only with `allowMock`.
//
// The client asks for the interval form (`form=interval`): the node answers
// from a quote it makes once a minute over an Ed25519 key of its own and signs
// this request's nonce with that key, so no quote is made per request. A node
// that does not know the form answers a quote over this nonce, checked as
// above. Spec: https://calimero-network.github.io/core/protocol/tee-attestation/#interval-quotes

/// The mock quote format development clusters use. (== mero-js `MOCK_QUOTE_HEADER`.)
public let mockQuoteHeader = "MOCK_TDX_QUOTE_V1"

/// The `form` that asks `GET /attestation` for the interval form (core's `INTERVAL_FORM`).
public let intervalForm = "interval"
/// Domain of an interval quote's binding (core's `ATTEST_INTERVAL_KEY_DOMAIN`).
public let intervalKeyDomain = "calimero.tee-attest.interval-key.v1"
/// Domain of what an interval key signs (core's `ATTEST_INTERVAL_SIG_DOMAIN`).
public let intervalSigDomain = "calimero.tee-attest.interval-sig.v1\u{0}"
/// The oldest interval quote taken, by its `issuedAt` and this clock (core's `INTERVAL_QUOTE_MAX_AGE_SECS`).
public let intervalQuoteMaxAgeSecs: Int64 = 300
/// How far ahead of this clock an interval quote may be dated (core's `INTERVAL_QUOTE_MAX_SKEW_SECS`).
public let intervalQuoteMaxSkewSecs: Int64 = 60

/// Where the signed release a node names comes from. Untrusted either way:
/// its Sigstore signature is checked. (== mero-js `CloudReleasePolicy.releaseSource`.)
public enum ReleaseSource: Sendable {
    /// The GitHub release assets, what core's Rust client reads.
    case github
    /// A mirror's base URL serving ``cloudNodeReleaseUrl(_:version:)``.
    case mirror(String)
    /// A function of the version (a file bundled with the app, say).
    case custom(@Sendable (_ version: String) async throws -> SignedNodeRelease)
}

/// How the signed release a node names is trusted (core's
/// `Measurements::Release`). (== mero-js `CloudReleasePolicy`.)
public struct CloudReleasePolicy: Sendable {
    /// The oldest mero-tee release trusted, e.g. `2.3.109`. Required: every
    /// release ever signed stays validly signed, so without a floor a node
    /// could name an old release with a flaw since fixed.
    public var minReleaseVersion: String
    /// The image profile trusted; ``TeeTrust/productionProfile`` (`locked-read-only`) by default.
    /// Not with ``profiles``.
    public var profile: String
    /// Trust a debug profile. Its image is readable by the operator: testing only.
    public var allowDebugProfile: Bool
    /// Where the release's signed measurements come from; the Calimero Cloud mirror by default.
    public var releaseSource: ReleaseSource
    /// DCAP verification; ``Dcap/verify(_:collateral:nowSecs:)`` by default.
    public var dcapVerify: DcapVerify
    /// Intel's collateral for a quote; ``IntelPcs/collateral(for:baseURL:session:)``
    /// from ``collateralSource`` by default.
    public var fetchCollateral: (@Sendable (Data) async throws -> DcapCollateral)?
    /// Where the default ``fetchCollateral`` gets Intel's collateral: a PCCS
    /// base URL (default ``TeeTrust/defaultCollateralMirror``, the cloud's
    /// mirror), or `"intel"` for Intel's PCS itself. Untrusted either way:
    /// every item is signed by Intel and verified up to Intel's root CA.
    public var collateralSource: String
    /// Refuse collateral whose TCB info Intel issued more than this many days
    /// ago. `nil`: only Intel's own `nextUpdate` (about 30 days out) bounds
    /// how old it may be. A mirror cannot forge collateral, but it can replay
    /// an older, still valid copy; this shortens that window.
    public var maxCollateralAgeDays: Double?
    /// Refuse collateral whose TCB info names Intel TCB evaluation data older
    /// than this. Raises ``TeeTrust/defaultMinTcbEvaluationDataNumber`` (core's
    /// floor), never lowers it. (== mero-js `minTcbEvaluationDataNumber`.)
    public var minTcbEvaluationDataNumber: Int?
    /// Override the release signer (defaults to the mero-tee node release workflow).
    public var identity: String?
    public var issuer: String?
    /// The current time in milliseconds. For tests.
    public var now: @Sendable () -> Int64
    /// Trust any of these image profiles, as core's `Measurements::Release`
    /// `allowed_profiles` and mero-js `release.profiles` do: the one the node
    /// names is used when it is listed, and the quote must still be that
    /// profile's signed image, so naming another profile only gets a node
    /// refused. For a cluster whose profile the client does not know:
    /// staging's (``CloudEnvironment/staging``). A listed debug profile still
    /// needs ``allowDebugProfile``. When set, ``profile`` stays at its default.
    public var profiles: [String]?

    public init(
        minReleaseVersion: String, profile: String = TeeTrust.productionProfile, allowDebugProfile: Bool = false,
        releaseSource: ReleaseSource = .mirror(TeeTrust.defaultReleaseMirror),
        dcapVerify: @escaping DcapVerify = { try Dcap.verify($0, collateral: $1, nowSecs: $2) },
        fetchCollateral: (@Sendable (Data) async throws -> DcapCollateral)? = nil,
        collateralSource: String = TeeTrust.defaultCollateralMirror, maxCollateralAgeDays: Double? = nil,
        minTcbEvaluationDataNumber: Int? = nil, identity: String? = nil,
        issuer: String? = nil, now: @escaping @Sendable () -> Int64 = { Int64(Date().timeIntervalSince1970 * 1000) },
        profiles: [String]? = nil
    ) {
        self.minReleaseVersion = minReleaseVersion
        self.profile = profile
        self.allowDebugProfile = allowDebugProfile
        self.releaseSource = releaseSource
        self.dcapVerify = dcapVerify
        self.fetchCollateral = fetchCollateral
        self.collateralSource = collateralSource
        self.maxCollateralAgeDays = maxCollateralAgeDays
        self.minTcbEvaluationDataNumber = minTcbEvaluationDataNumber
        self.identity = identity
        self.issuer = issuer
        self.now = now
        self.profiles = profiles
    }
}

/// The profile a real quote is judged against: ``CloudReleasePolicy/profile``,
/// or, with ``CloudReleasePolicy/profiles``, the one the node names when it is
/// listed (mero-js `trustedProfile`). Every listed debug profile needs
/// ``CloudReleasePolicy/allowDebugProfile``, so the list never trusts one silently.
func trustedProfile(_ doc: AttestationDoc, _ policy: CloudReleasePolicy) throws -> String {
    guard let profiles = policy.profiles else { return policy.profile }
    guard policy.profile == TeeTrust.productionProfile else {
        throw TeeTrustError("give CloudReleasePolicy.profile or profiles, not both")
    }
    guard !profiles.isEmpty else {
        throw TeeTrustError("CloudReleasePolicy.profiles is empty, so no image would be trusted")
    }
    let debug = profiles.filter { $0 != TeeTrust.productionProfile }
    guard debug.isEmpty || policy.allowDebugProfile else {
        let listed = debug.map { "\"\($0)\"" }.joined(separator: ", ")
        throw TeeTrustError(
            "CloudReleasePolicy.profiles lists \(listed), which an operator can read: "
                + "pass allowDebugProfile to trust them")
    }
    let named = doc.release?.profile ?? ""
    guard profiles.contains(named) else {
        throw TeeTrustError(
            "The node runs profile \"\(named)\", which is not one of \(profiles.joined(separator: ", "))")
    }
    return named
}

/// What a ``CalimeroClient`` trusts a node to be. (== mero-js `CloudPolicy`.)
public struct CloudPolicy: Sendable {
    /// Accept mock quotes. Development clusters (`merod local`) only; never set from the environment.
    public var allowMock: Bool
    /// Trust real quotes from the signed release the node runs.
    public var release: CloudReleasePolicy?
    /// Verify a real quote yourself instead (e.g. ``createQuoteVerifier(_:)``
    /// with pinned measurements). Takes precedence over ``release``.
    public var verifyQuote: (any TransportQuoteVerifier)?
    /// The app bundle hash (hex) the quote must bind, when talking to one app.
    public var appHash: String?
    /// The current time in milliseconds, which an interval quote's age is judged by. For tests.
    public var now: @Sendable () -> Int64

    public init(
        allowMock: Bool = false, release: CloudReleasePolicy? = nil, verifyQuote: (any TransportQuoteVerifier)? = nil,
        appHash: String? = nil, now: @escaping @Sendable () -> Int64 = { Int64(Date().timeIntervalSince1970 * 1000) }
    ) {
        self.allowMock = allowMock
        self.release = release
        self.verifyQuote = verifyQuote
        self.appHash = appHash
        self.now = now
    }

    /// A development cluster's: mock quotes accepted, nothing else.
    public static let development = CloudPolicy(allowMock: true)
}

/// `GET /attestation`, as the node answers it. (== mero-js `AttestationDoc`.)
public struct AttestationDoc: Codable, Sendable, Equatable {
    public struct Release: Codable, Sendable, Equatable {
        public var version: String
        public var profile: String
    }

    public var quote: String
    public var transportKey: String
    public var appHash: String?
    public var nonce: String?
    public var release: Release?
    public var cluster: String?
    public var node: Int?
    public var mock: Bool?
    /// With `form=interval`, from a node that has a current interval quote.
    public var interval: IntervalProof?
}

/// What a `form=interval` answer adds (core's `IntervalProof`).
public struct IntervalProof: Codable, Sendable, Equatable {
    /// The interval key (Ed25519) the quote binds, hex.
    public var key: String
    /// The first half of the quote's report data, which the node chose, hex.
    public var quoteNonce: String
    /// When the node made the quote, Unix seconds by its clock.
    public var issuedAt: Int64
    /// Ed25519 over `intervalMessage`, hex.
    public var signature: String

    public init(key: String, quoteNonce: String, issuedAt: Int64, signature: String) {
        self.key = key
        self.quoteNonce = quoteNonce
        self.issuedAt = issuedAt
        self.signature = signature
    }
}

/// An interval quote's report data after its nonce (core's `interval_key_binding`).
public func intervalKeyBinding(_ transportKey: Data, _ intervalKey: Data, issuedAt: Int64) -> Data {
    var time = UInt64(bitPattern: issuedAt).bigEndian
    let timeBytes = withUnsafeBytes(of: &time) { Data($0) }
    return sha256(Data(intervalKeyDomain.utf8) + transportKey + intervalKey + timeBytes)
}

/// What an interval key signs for one request (core's `interval_message`).
public func intervalMessage(nonce: Data, appHash: Data, transportKey: Data) -> Data {
    Data(intervalSigDomain.utf8) + nonce + appHash + transportKey
}

/// The interval form (core's `verify_interval`): the quote is fresh by `policy.now`, the interval
/// key signed this nonce, app hash and transport key, and the quote binds that key, the transport
/// key and the time.
func verifyInterval(
    _ doc: AttestationDoc, _ interval: IntervalProof, nonce: Data, appHash: Data, transportKey: Data,
    policy: CloudPolicy, session: URLSession
) async throws {
    let now = policy.now() / 1000
    if interval.issuedAt > now + intervalQuoteMaxSkewSecs {
        throw TeeTrustError("The interval quote is dated \(interval.issuedAt - now)s ahead of this clock")
    }
    if now - interval.issuedAt > intervalQuoteMaxAgeSecs {
        throw TeeTrustError(
            "The interval quote is \(now - interval.issuedAt)s old, more than the \(intervalQuoteMaxAgeSecs)s "
                + "this client takes")
    }
    let key = try Hex.decode(interval.key, label: "interval.key", bytes: 32)
    let signature = try Hex.decode(interval.signature, label: "interval.signature", bytes: 64)
    let message = intervalMessage(nonce: nonce, appHash: appHash, transportKey: transportKey)
    guard Ed25519.verify(publicKey: key, signature: signature, message: message) else {
        throw TeeTrustError("The interval key did not sign this nonce, app and transport key")
    }
    let quoteNonce = try Hex.decode(interval.quoteNonce, label: "interval.quoteNonce", bytes: 32)
    try await verifyQuoteBinding(
        doc, nonce: quoteNonce, suffix: intervalKeyBinding(transportKey, key, issuedAt: interval.issuedAt),
        policy: policy, what: "interval key", session: session)
}

/// The release a node names, checked against the floor before anything is
/// fetched. Accepts `2.3.109` and the tag form `mero-tee-v2.3.109`.
/// (== mero-js `claimedRelease`.)
public func claimedRelease(_ doc: AttestationDoc, minReleaseVersion: String) throws -> String {
    let floor = try ReleaseVersion.checked(minReleaseVersion, "minReleaseVersion")
    var raw = (doc.release?.version ?? "").trimmingCharacters(in: .whitespaces)
    if raw.hasPrefix("mero-tee-v") { raw.removeFirst("mero-tee-v".count) }
    if raw.hasPrefix("v") { raw.removeFirst() }
    let version = try ReleaseVersion.checked(raw, "The release the node names")
    if ReleaseVersion.compare(version, floor) < 0 {
        throw TeeTrustError("mero-tee release \(version) is older than the minimum \(floor)")
    }
    return version
}

/// Semver order (core's `compare_versions`); text order for versions that
/// are not semver. (== mero-js `compareVersions`.)
public func compareVersions(_ a: String, _ b: String) -> Int {
    if ReleaseVersion.split(a) != nil, ReleaseVersion.split(b) != nil { return ReleaseVersion.compare(a, b) }
    return a < b ? -1 : (a > b ? 1 : 0)
}

/// Fetch the node's attestation and return its transport key once it checks
/// out. (== mero-js `fetchAttestedKey`.)
public func fetchAttestedKey(_ baseURL: String, policy: CloudPolicy, session: URLSession = .shared) async throws -> Data
{
    let nonce = Hex.encode(randomBytes(32))
    var base = baseURL
    while base.hasSuffix("/") { base.removeLast() }
    guard var parts = URLComponents(string: base + "/attestation") else {
        throw TeeTrustError("bad node URL \(baseURL)")
    }
    parts.queryItems =
        [URLQueryItem(name: "nonce", value: nonce), URLQueryItem(name: "form", value: intervalForm)]
        + (policy.appHash.map { [URLQueryItem(name: "appHash", value: $0)] } ?? [])
    guard let url = parts.url else { throw TeeTrustError("bad node URL \(baseURL)") }
    var request = URLRequest(url: url)
    request.setValue("application/json", forHTTPHeaderField: "Accept")
    let (data, response) = try await session.data(for: request)
    let status = (response as? HTTPURLResponse)?.statusCode ?? 0
    guard (200..<300).contains(status) else { throw TeeTrustError("The node refused to attest: HTTP \(status)") }
    let doc: AttestationDoc
    do {
        doc = try MeroJSON.decode(AttestationDoc.self, from: data)
    } catch {
        throw TeeTrustError("The attestation is malformed")
    }
    return try await checkAttestation(doc, nonce: nonce, policy: policy, session: session)
}

/// Judge an attestation `doc` the node answered for `nonce`; its transport key once it checks out.
func checkAttestation(
    _ doc: AttestationDoc, nonce: String, policy: CloudPolicy, session: URLSession
) async throws
    -> Data
{
    let transportKey = try Hex.decode(doc.transportKey, label: "transportKey", bytes: 32)
    let inner = try policy.appHash.map { try Hex.decode($0, label: "appHash", bytes: 32) } ?? Data(count: 32)
    if let interval = doc.interval {
        try await verifyInterval(
            doc, interval, nonce: try Hex.decode(nonce, label: "nonce", bytes: 32), appHash: inner,
            transportKey: transportKey, policy: policy, session: session)
        return transportKey
    }
    let suffix = transportKeyBinding(inner, transportKey)
    try await verifyQuoteBinding(
        doc, nonce: try Hex.decode(nonce, label: "nonce", bytes: 32), suffix: suffix, policy: policy,
        what: "transport key", session: session)
    return transportKey
}

/// Check that `doc`'s quote binds `nonce` and then `suffix` in its report
/// data, under `policy`: a mock quote only with `allowMock`, a real one by
/// ``CloudPolicy/verifyQuote`` or the signed release the document names.
/// `what` names the bound thing in the refusal. (== mero-js `verifyQuoteBinding`.)
public func verifyQuoteBinding(
    _ doc: AttestationDoc, nonce: Data, suffix: Data, policy: CloudPolicy, what: String,
    session: URLSession = .shared
) async throws {
    guard let quote = Data(base64Encoded: doc.quote) else { throw TeeTrustError("The quote is not base64") }
    let header = Data(mockQuoteHeader.utf8)
    if quote.prefix(header.count) == header {
        guard policy.allowMock else { throw TeeTrustError("mock quote refused: this client only trusts real TEEs") }
        let report = quote.dropFirst(header.count).prefix(64)
        guard report.count == 64, Data(report.prefix(32)) == nonce, Data(report.suffix(32)) == suffix else {
            throw TeeTrustError("the quote does not bind this \(what) and nonce")
        }
        return
    }
    let args = TransportQuoteArgs(quoteB64: doc.quote, nonce: Hex.encode(nonce), reportDataSuffix: Hex.encode(suffix))
    let ok: Bool
    if let verifier = policy.verifyQuote {
        ok = try await verifier.verify(args)
    } else if let release = policy.release {
        ok = try await verifyRelease(doc, args, release, session: session)
    } else {
        throw TeeTrustError("real quote refused: this client trusts no TD image (set policy.release)")
    }
    guard ok else { throw TeeTrustError("The attestation did not verify, so its \(what) is not trusted") }
}

/// Judge a real quote as core's `Measurements::Release` does.
private func verifyRelease(
    _ doc: AttestationDoc, _ args: TransportQuoteArgs, _ policy: CloudReleasePolicy, session: URLSession
) async throws -> Bool {
    // Refuse what no release could admit before reaching for the network.
    let version = try claimedRelease(doc, minReleaseVersion: policy.minReleaseVersion)
    let release: SignedNodeRelease
    switch policy.releaseSource {
    case .github: release = try await fetchGithubNodeRelease(version, session: session)
    case .mirror(let mirror):
        release = try await fetchNodeRelease(try cloudNodeReleaseUrl(mirror, version: version), session: session)
    case .custom(let fetch): release = try await fetch(version)
    }
    let trusted = try trustSignedRelease(
        TrustSignedReleaseOptions(
            release: release, profile: try trustedProfile(doc, policy), minReleaseVersion: policy.minReleaseVersion,
            allowDebugProfile: policy.allowDebugProfile, identity: policy.identity, issuer: policy.issuer))
    let source = policy.collateralSource == "intel" ? IntelPcs.defaultBaseURL : policy.collateralSource
    let fetchCollateral = freshCollateral(
        policy.fetchCollateral ?? { @Sendable quote in
            try await IntelPcs.collateral(for: quote, baseURL: source, session: session)
        }, maxCollateralAgeDays: policy.maxCollateralAgeDays, now: policy.now)
    let verifier = try createQuoteVerifier(
        QuoteVerifierOptions(
            trusting: trusted, dcapVerify: policy.dcapVerify,
            minTcbEvaluationDataNumber: policy.minTcbEvaluationDataNumber, fetchCollateral: fetchCollateral,
            now: policy.now))
    return try await verifier.verify(args)
}

/// `fetch`, refusing collateral older than `maxCollateralAgeDays` (by its TCB
/// info's `issueDate`). (== mero-js `freshCollateral`.)
public func freshCollateral(
    _ fetch: @escaping @Sendable (Data) async throws -> DcapCollateral, maxCollateralAgeDays: Double?,
    now: @escaping @Sendable () -> Int64 = { Int64(Date().timeIntervalSince1970 * 1000) }
) -> @Sendable (Data) async throws -> DcapCollateral {
    guard let maxDays = maxCollateralAgeDays else { return fetch }
    return { quote in
        let collateral = try await fetch(quote)
        let tcb = try? MeroJSON.decode(JSONValue.self, from: Data(collateral.tcbInfo.utf8))
        guard let raw = tcb?["issueDate"]?.stringValue, let issued = parseRfc3339(raw) else {
            throw TeeTrustError("The quote's collateral names no TCB info issue date")
        }
        let ageDays = (Double(now()) - issued.timeIntervalSince1970 * 1000) / 86_400_000
        if ageDays > maxDays {
            throw TeeTrustError(
                "The quote's collateral was issued \(Int(ageDays.rounded(.down))) days ago, more than the "
                    + "\(String(format: "%g", maxDays)) this client accepts")
        }
        return collateral
    }
}

private func parseRfc3339(_ raw: String) -> Date? {
    let plain = ISO8601DateFormatter()
    if let date = plain.date(from: raw) { return date }
    let fractional = ISO8601DateFormatter()
    fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    return fractional.date(from: raw)
}

/// The oldest mero-tee release a production client trusts by default (core's
/// `DEFAULT_MIN_RELEASE`).
public let defaultMinRelease = "2.3.109"

extension CloudPolicy {
    /// A production cluster's: real quotes only, from a signed mero-tee release
    /// at or above ``defaultMinRelease``, `locked-read-only` profile.
    public static let production = CloudEnvironment.production.policy()

    /// A staging cluster's: real quotes only, from a signed mero-tee release at
    /// or above ``defaultMinRelease``, of any of staging's image profiles, debug
    /// ones included (``CloudEnvironment/staging``). Testing only: never in a
    /// build people install from the App Store.
    public static let staging = CloudEnvironment.staging.policy()
}
