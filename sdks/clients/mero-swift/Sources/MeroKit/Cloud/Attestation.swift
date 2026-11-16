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

/// The mock quote format development clusters use. (== mero-js `MOCK_QUOTE_HEADER`.)
public let mockQuoteHeader = "MOCK_TDX_QUOTE_V1"

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
    /// Override the release signer (defaults to the mero-tee node release workflow).
    public var identity: String?
    public var issuer: String?
    /// The current time in milliseconds. For tests.
    public var now: @Sendable () -> Int64

    public init(
        minReleaseVersion: String, profile: String = TeeTrust.productionProfile, allowDebugProfile: Bool = false,
        releaseSource: ReleaseSource = .mirror(TeeTrust.defaultReleaseMirror),
        dcapVerify: @escaping DcapVerify = { try Dcap.verify($0, collateral: $1, nowSecs: $2) },
        fetchCollateral: (@Sendable (Data) async throws -> DcapCollateral)? = nil,
        collateralSource: String = TeeTrust.defaultCollateralMirror, maxCollateralAgeDays: Double? = nil,
        identity: String? = nil,
        issuer: String? = nil, now: @escaping @Sendable () -> Int64 = { Int64(Date().timeIntervalSince1970 * 1000) }
    ) {
        self.minReleaseVersion = minReleaseVersion
        self.profile = profile
        self.allowDebugProfile = allowDebugProfile
        self.releaseSource = releaseSource
        self.dcapVerify = dcapVerify
        self.fetchCollateral = fetchCollateral
        self.collateralSource = collateralSource
        self.maxCollateralAgeDays = maxCollateralAgeDays
        self.identity = identity
        self.issuer = issuer
        self.now = now
    }
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

    public init(
        allowMock: Bool = false, release: CloudReleasePolicy? = nil, verifyQuote: (any TransportQuoteVerifier)? = nil,
        appHash: String? = nil
    ) {
        self.allowMock = allowMock
        self.release = release
        self.verifyQuote = verifyQuote
        self.appHash = appHash
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
        [URLQueryItem(name: "nonce", value: nonce)]
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
    let suffix = transportKeyBinding(inner, transportKey)
    guard let quote = Data(base64Encoded: doc.quote) else { throw TeeTrustError("The quote is not base64") }
    let header = Data(mockQuoteHeader.utf8)
    if quote.prefix(header.count) == header {
        guard policy.allowMock else { throw TeeTrustError("mock quote refused: this client only trusts real TEEs") }
        let report = quote.dropFirst(header.count).prefix(64)
        guard report.count == 64, Data(report.prefix(32)) == (try Hex.decode(nonce, label: "nonce", bytes: 32)),
            Data(report.suffix(32)) == suffix
        else { throw TeeTrustError("the quote does not bind this transport key, app and nonce") }
        return transportKey
    }
    let args = TransportQuoteArgs(quoteB64: doc.quote, nonce: nonce, reportDataSuffix: Hex.encode(suffix))
    let ok: Bool
    if let verifier = policy.verifyQuote {
        ok = try await verifier.verify(args)
    } else if let release = policy.release {
        ok = try await verifyRelease(doc, args, release, session: session)
    } else {
        throw TeeTrustError("real quote refused: this client trusts no TD image (set policy.release)")
    }
    guard ok else { throw TeeTrustError("The attestation did not verify, so its transport key is not trusted") }
    return transportKey
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
            release: release, profile: policy.profile, minReleaseVersion: policy.minReleaseVersion,
            allowDebugProfile: policy.allowDebugProfile, identity: policy.identity, issuer: policy.issuer))
    let source = policy.collateralSource == "intel" ? IntelPcs.defaultBaseURL : policy.collateralSource
    let fetchCollateral = freshCollateral(
        policy.fetchCollateral ?? { @Sendable quote in
            try await IntelPcs.collateral(for: quote, baseURL: source, session: session)
        }, maxCollateralAgeDays: policy.maxCollateralAgeDays, now: policy.now)
    let verifier = try createQuoteVerifier(
        QuoteVerifierOptions(
            trusting: trusted, dcapVerify: policy.dcapVerify, fetchCollateral: fetchCollateral, now: policy.now))
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
    public static let production = CloudPolicy(release: CloudReleasePolicy(minReleaseVersion: defaultMinRelease))
}
