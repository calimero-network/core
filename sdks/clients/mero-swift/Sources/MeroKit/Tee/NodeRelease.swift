import Foundation

#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

// Trusting a TEE node's image by its signed release, fetched at run time from
// anywhere, instead of shipping the release's `published-mrtds.json` with the
// app. Port of mero-js `src/sealed/release.ts`.
//
// Each mero-tee node release publishes `published-mrtds.json` (the
// measurements of its image) with a cosign keyless bundle, signed by the
// mero-tee release workflow on GitHub Actions. `verifySignedNodeRelease`
// checks that signature here, against the Sigstore trust root embedded in this
// SDK, so the file can come from any source: the cloud's mirror
// (`GET https://cloud.calimero.network/api/tee/node-releases/{version}`), a
// mirror of your own, or a file bundled with the app. None of them is
// trusted; a file whose signature does not verify is refused.
//
// What is trusted: the signer (``TeeTrust/nodeReleaseSigner``, the identity
// core pins as `NODE_RELEASE_IDENTITY`), the Sigstore public-good trust root,
// and the minimum release you pass. What is not: the transport, the server
// that serves the release, and the relay's choice of release. A relay can
// present any genuinely signed release at or above the minimum, and no
// other; its quote must then match that release's measurements exactly, so it
// cannot claim one release and run another.

extension TeeTrust {
    /// The workflow that signs every mero-tee node release, and the issuer of
    /// its identity. (== mero-js `NODE_RELEASE_SIGNER`.)
    public static let nodeReleaseSigner = SignerIdentity(
        identity: "https://github.com/calimero-network/mero-tee/.github/workflows/"
            + "release-node-image-gcp.yaml@refs/heads/master",
        issuer: "https://token.actions.githubusercontent.com")

    /// The public mirror of signed node releases, used when no other source
    /// is given. Only a transport: what it serves is verified, so it can fail
    /// to answer but cannot make a release trusted. (== mero-js `DEFAULT_RELEASE_MIRROR`.)
    public static let defaultReleaseMirror = "https://cloud.calimero.network"
    /// Where clients get Intel's collateral for a quote: the cloud's mirror of
    /// the PCCS paths ``IntelPcs`` reads. It only carries Intel-signed items.
    /// (== mero-js `DEFAULT_COLLATERAL_MIRROR`.)
    public static let defaultCollateralMirror = "\(defaultReleaseMirror)/api/tee/pcs"
}

/// A node release as the servers hand it out, wrapped as `{"data": ...}`: the
/// release's `published-mrtds.json`, byte for byte, and its cosign bundle.
/// Unverified until ``verifySignedNodeRelease(_:bundle:expectedVersion:identity:issuer:signature:)``
/// says otherwise. (== mero-js `SignedNodeRelease`.)
public struct SignedNodeRelease: Codable, Sendable, Equatable {
    /// The release, e.g. `2.3.87`. It must be the signed file's `tag`.
    public var version: String
    /// `published-mrtds.json`, exactly as signed.
    public var publishedMrtds: String
    /// `published-mrtds.json.bundle.json`, as JSON text.
    public var bundle: String

    public init(version: String, publishedMrtds: String, bundle: String) {
        self.version = version
        self.publishedMrtds = publishedMrtds
        self.bundle = bundle
    }
}

/// Verify a node release's `published-mrtds.json` against its cosign bundle,
/// as `cosign verify-blob` would, and return the file parsed.
/// (== mero-js `verifySignedNodeRelease`.) Throws ``TeeTrustError`` unless:
///
/// - the signature over the exact bytes verifies with the bundle's certificate;
/// - that certificate chains to a Fulcio CA in the Sigstore trust root, and it
///   and the CA were valid when Rekor logged the signature;
/// - it names the node release workflow and GitHub Actions as its issuer
///   (``TeeTrust/nodeReleaseSigner`` unless `identity` / `issuer` say otherwise);
/// - Rekor's signed entry timestamp verifies, for an entry that is this
///   signature over this file by this certificate;
/// - the file is a node release (`role: "node"`) listing measured profiles,
///   and `expectedVersion`, when given, is its `tag`.
///
/// Nothing is read from the clock: the certificate is checked at the time
/// Rekor logged the signature, so a release that verifies once always does.
/// `signature` is the release's detached `.sig`, when you have it.
public func verifySignedNodeRelease(
    _ publishedMrtds: Data, bundle: String, expectedVersion: String? = nil, identity: String? = nil,
    issuer: String? = nil, signature: String? = nil
) throws -> PublishedMrtds {
    guard let parsed = try? MeroJSON.decode(JSONValue.self, from: Data(bundle.utf8)) else {
        throw TeeTrustError("The bundle is not JSON")
    }
    return try verifySignedNodeRelease(
        publishedMrtds, bundle: parsed, expectedVersion: expectedVersion, identity: identity, issuer: issuer,
        signature: signature)
}

/// As the overload taking the file's bytes, with the file as text (its UTF-8 bytes are what was signed).
public func verifySignedNodeRelease(
    _ publishedMrtds: String, bundle: String, expectedVersion: String? = nil, identity: String? = nil,
    issuer: String? = nil, signature: String? = nil
) throws -> PublishedMrtds {
    try verifySignedNodeRelease(
        Data(publishedMrtds.utf8), bundle: bundle, expectedVersion: expectedVersion, identity: identity,
        issuer: issuer, signature: signature)
}

/// As the overload taking the bundle as text, with the bundle parsed.
public func verifySignedNodeRelease(
    _ publishedMrtds: Data, bundle: JSONValue, expectedVersion: String? = nil, identity: String? = nil,
    issuer: String? = nil, signature: String? = nil
) throws -> PublishedMrtds {
    try Sigstore.verifyCosignBundle(
        publishedMrtds, bundle: bundle,
        signer: SignerIdentity(
            identity: identity ?? TeeTrust.nodeReleaseSigner.identity,
            issuer: issuer ?? TeeTrust.nodeReleaseSigner.issuer),
        signature: signature)

    guard let release = try? MeroJSON.decode(JSONValue.self, from: publishedMrtds) else {
        throw TeeTrustError("The signed release is not JSON")
    }
    guard release["role"]?.stringValue == "node" else {
        let role = release["role"].map(CanonicalJSON.string) ?? "undefined"
        throw TeeTrustError("The signed release is a \(role) release, not a node one")
    }
    // The workflow signs other files too, some with a `role` and `profiles`
    // of their own (`release-provenance.json`): the signature covers bytes,
    // not a file name, so check this is the measurements file.
    let profiles = release["profiles"]?.objectValue ?? [:]
    let registers = ["mrtd", "rtmr0", "rtmr1", "rtmr2", "rtmr3"]
    let measured = { (image: JSONValue) -> Bool in
        registers.allSatisfy { register in
            guard let value = image[register]?.stringValue else { return false }
            return value.utf8.count == 96 && Hex.decodeUnsizedOrNil(value) != nil
        }
    }
    guard let tag = release["tag"]?.stringValue, !profiles.isEmpty, profiles.values.allSatisfy(measured),
        let decoded = try? MeroJSON.decode(PublishedMrtds.self, from: publishedMrtds)
    else {
        throw TeeTrustError("The signed file is not a release's published-mrtds.json: it lists no measured profiles")
    }
    if let expectedVersion, tag != expectedVersion {
        throw TeeTrustError("The signed release is \(tag), not \(expectedVersion)")
    }
    return decoded
}

extension Hex {
    /// Hex of any even length, or `nil`.
    static func decodeUnsizedOrNil(_ value: String) -> Data? { try? decodeUnsized(value, label: "hex") }
}

/// Fetch a ``SignedNodeRelease`` from a server that wraps it as
/// `{"data": ...}`, such as ``cloudNodeReleaseUrl(_:version:)``. It is only
/// fetched: hand it to ``trustSignedRelease(_:)`` before using it.
/// (== mero-js `fetchNodeRelease`.)
public func fetchNodeRelease(_ url: String, session: URLSession = .shared) async throws -> SignedNodeRelease {
    let (data, status) = try await TeeHTTP.get(url, session: session)
    guard (200..<300).contains(status) else {
        throw TeeTrustError("Fetching the node release from \(url) failed: HTTP \(status)")
    }
    guard let json = try? MeroJSON.decode(JSONValue.self, from: data) else {
        throw TeeTrustError("\(url) did not answer with JSON")
    }
    let release = json["data"]
    guard let version = release?["version"]?.stringValue, let published = release?["publishedMrtds"]?.stringValue,
        let bundle = release?["bundle"]?.stringValue
    else { throw TeeTrustError("\(url) did not answer with a signed node release") }
    return SignedNodeRelease(version: version, publishedMrtds: published, bundle: bundle)
}

/// The release a mero-tee node image was built from, read from its name:
/// `merotee-ubuntu-questing-25-10-locked-read-only-2-3-87` is `2.3.87`.
/// Throws for any other name, including the suffixed images of unreleased
/// builds. (== mero-js `nodeReleaseVersionFromImage`.)
public func nodeReleaseVersionFromImage(_ osImage: String) throws -> String {
    let refusal = TeeTrustError("\(CanonicalJSON.quote(osImage)) is not the image of a mero-tee node release")
    guard osImage.hasPrefix("merotee-") else { throw refusal }
    let parts = osImage.split(separator: "-", omittingEmptySubsequences: false)
    guard parts.count >= 5 else { throw refusal }
    let numbers = parts.suffix(3).map { String($0) }
    let middle = parts.dropFirst().dropLast(3)
    guard !middle.joined(separator: "-").isEmpty,
        numbers.allSatisfy({ !$0.isEmpty && $0.utf8.allSatisfy { (0x30...0x39).contains($0) } })
    else { throw refusal }
    return numbers.map { String(Int($0) ?? 0) }.joined(separator: ".")
}

/// Where mero-tee publishes its releases' assets, as core's
/// `calimero-tee-release` fetches them. (== mero-js `GITHUB_RELEASES`.)
public let githubReleases = "https://github.com/calimero-network/mero-tee/releases/download"

/// Node release `version` straight from its GitHub release
/// (`mero-tee-v{version}`): `published-mrtds.json` and its cosign bundle,
/// the files core's Rust client verifies. Only fetched: verify it with
/// ``trustSignedRelease(_:)``. (== mero-js `fetchGithubNodeRelease`.)
public func fetchGithubNodeRelease(
    _ version: String, base: String = githubReleases, session: URLSession = .shared
) async throws -> SignedNodeRelease {
    var root = base
    while root.hasSuffix("/") { root.removeLast() }
    let url = "\(root)/mero-tee-v\(try ReleaseVersion.checked(version, "version"))/published-mrtds.json"
    func get(_ url: String) async throws -> String {
        let (data, status) = try await TeeHTTP.get(url, session: session)
        guard (200..<300).contains(status) else { throw TeeTrustError("Fetching \(url) failed: HTTP \(status)") }
        return String(decoding: data, as: UTF8.self)
    }
    async let published = get(url)
    async let bundle = get(url + ".bundle.json")
    return SignedNodeRelease(version: version, publishedMrtds: try await published, bundle: try await bundle)
}

/// Where the cloud at `cloudURL` (e.g. ``TeeTrust/defaultReleaseMirror``)
/// mirrors node release `version`. (== mero-js `cloudNodeReleaseUrl`.)
/// Plain http is refused for anything but loopback (``TransportSecurity``).
public func cloudNodeReleaseUrl(_ cloudURL: String, version: String) throws -> String {
    var base = cloudURL
    while base.hasSuffix("/") { base.removeLast() }
    if let url = URL(string: base) { try TransportSecurity.checkServiceURL(url, role: "release mirror") }
    return "\(base)/api/tee/node-releases/\(try ReleaseVersion.checked(version, "version"))"
}

/// What ``trustSignedRelease(_:)`` needs. (== mero-js `TrustSignedReleaseOptions`.)
public struct TrustSignedReleaseOptions: Sendable {
    /// The release, from any source; it is verified.
    public var release: SignedNodeRelease
    /// The image profile to trust, e.g. ``TeeTrust/productionProfile``.
    public var profile: String
    /// Trust a profile other than ``TeeTrust/productionProfile``, such as
    /// `debug`. Its image is readable by whoever operates the node: testing only.
    public var allowDebugProfile: Bool
    /// The oldest release accepted, e.g. `2.3.87`. Required: every release
    /// ever signed stays validly signed, so without a floor a relay could
    /// present an old release, with a flaw since fixed. Raise it when a
    /// release must no longer be trusted.
    public var minReleaseVersion: String
    /// Override the signer; ``TeeTrust/nodeReleaseSigner`` by default.
    public var identity: String?
    public var issuer: String?

    public init(
        release: SignedNodeRelease, profile: String, minReleaseVersion: String, allowDebugProfile: Bool = false,
        identity: String? = nil, issuer: String? = nil
    ) {
        self.release = release
        self.profile = profile
        self.allowDebugProfile = allowDebugProfile
        self.minReleaseVersion = minReleaseVersion
        self.identity = identity
        self.issuer = issuer
    }
}

/// The options that trust one profile of a signed node release, from an
/// untrusted source: its signature is verified, its signed `tag` must be
/// `release.version` and no older than `minReleaseVersion`, and then it is
/// read as ``trustedMeasurementsFromReleases(_:profile:allowDebugProfile:)``
/// reads a bundled file. (== mero-js `trustSignedRelease`.)
public func trustSignedRelease(_ options: TrustSignedReleaseOptions) throws -> TrustedReleaseOptions {
    let minimum = try ReleaseVersion.checked(options.minReleaseVersion, "minReleaseVersion")
    let version = try ReleaseVersion.checked(options.release.version, "The release version")
    let published = try verifySignedNodeRelease(
        options.release.publishedMrtds, bundle: options.release.bundle, expectedVersion: version,
        identity: options.identity, issuer: options.issuer)
    if ReleaseVersion.compare(version, minimum) < 0 {
        throw TeeTrustError("Release \(version) is older than the minimum trusted, \(minimum)")
    }
    return try trustedMeasurementsFromReleases(
        [published], profile: options.profile, allowDebugProfile: options.allowDebugProfile)
}

/// What ``createSignedReleaseVerifier(_:)`` needs. (== mero-js `SignedReleaseVerifierOptions`.)
public struct SignedReleaseVerifierOptions: Sendable {
    /// Where to get the node's release, asked again each time the node is
    /// attested (a restarted node may run a newer one). Untrusted: it is
    /// verified, and the quote must match it.
    public var release: @Sendable () async throws -> SignedNodeRelease
    public var profile: String
    public var allowDebugProfile: Bool
    public var minReleaseVersion: String
    public var identity: String?
    public var issuer: String?
    /// DCAP verification; ``Dcap/verify(_:collateral:nowSecs:)`` by default.
    public var dcapVerify: DcapVerify
    /// As ``QuoteVerifierOptions/fetchCollateral``.
    public var fetchCollateral: (@Sendable (Data) async throws -> DcapCollateral)?
    /// The current time in milliseconds, for the quote's collateral. For tests.
    public var now: @Sendable () -> Int64

    public init(
        release: @escaping @Sendable () async throws -> SignedNodeRelease, profile: String, minReleaseVersion: String,
        allowDebugProfile: Bool = false, identity: String? = nil, issuer: String? = nil,
        dcapVerify: @escaping DcapVerify = { try Dcap.verify($0, collateral: $1, nowSecs: $2) },
        fetchCollateral: (@Sendable (Data) async throws -> DcapCollateral)? = nil,
        now: @escaping @Sendable () -> Int64 = { Int64(Date().timeIntervalSince1970 * 1000) }
    ) {
        self.release = release
        self.profile = profile
        self.allowDebugProfile = allowDebugProfile
        self.minReleaseVersion = minReleaseVersion
        self.identity = identity
        self.issuer = issuer
        self.dcapVerify = dcapVerify
        self.fetchCollateral = fetchCollateral
        self.now = now
    }
}

/// A ``QuoteVerifier`` whose trusted image is the signed release the node
/// says it runs. (== mero-js `createSignedReleaseVerifier`'s result.)
public struct SignedReleaseVerifier: TransportQuoteVerifier {
    public var includeCollateral: Bool { true }
    let options: SignedReleaseVerifierOptions

    public func verify(_ args: TransportQuoteArgs) async throws -> Bool {
        let trusted = try trustSignedRelease(
            TrustSignedReleaseOptions(
                release: try await options.release(), profile: options.profile,
                minReleaseVersion: options.minReleaseVersion, allowDebugProfile: options.allowDebugProfile,
                identity: options.identity, issuer: options.issuer))
        let verifier = try createQuoteVerifier(
            QuoteVerifierOptions(
                trusting: trusted, dcapVerify: options.dcapVerify, fetchCollateral: options.fetchCollateral,
                now: options.now))
        return try await verifier.verify(args)
    }
}

/// A verifier whose trusted image is the signed release the node says it
/// runs: on each attestation it gets the release, checks it with
/// ``trustSignedRelease(_:)``, and verifies the quote against that release's
/// measurements only. (== mero-js `createSignedReleaseVerifier`.)
///
/// The node's choice of release is the one thing taken from it, and it is
/// bounded: only a genuinely signed release at or above `minReleaseVersion`
/// passes, and the quote must be of exactly that release's image. Use it as
/// ``ConnectCloudOptions/seal``, with `SealedTransport.attested(baseURL:verifier:)`
/// or as ``DcapRelayKeyVerifier``'s verifier.
public func createSignedReleaseVerifier(_ options: SignedReleaseVerifierOptions) throws -> SignedReleaseVerifier {
    _ = try ReleaseVersion.checked(options.minReleaseVersion, "minReleaseVersion")
    return SignedReleaseVerifier(options: options)
}

// MARK: - Versions and HTTP

/// Release versions (`2.3.87`, `2.3.88-rc.1`), ordered as semver and core's
/// `compare_release_versions` order them.
enum ReleaseVersion {
    private static let pattern = #"^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$"#

    /// `X.Y.Z` and the pre-release, or `nil` when `value` is not a version.
    static func split(_ value: String) -> (core: [Int], pre: String?)? {
        let range = NSRange(value.startIndex..., in: value)
        guard let regex = try? NSRegularExpression(pattern: pattern),
            let match = regex.firstMatch(in: value, range: range)
        else { return nil }
        func group(_ index: Int) -> String? {
            Range(match.range(at: index), in: value).map { String(value[$0]) }
        }
        let core = (1...3).compactMap { group($0).flatMap { Int($0) } }
        guard core.count == 3 else { return nil }
        let pre = group(4)
        if let pre, pre.split(separator: ".", omittingEmptySubsequences: false).contains(where: \.isEmpty) {
            return nil
        }
        return (core: core, pre: pre)
    }

    /// `value` if it is a release version; throws naming `what` otherwise.
    static func checked(_ value: String, _ what: String) throws -> String {
        guard split(value) != nil else {
            throw TeeTrustError("\(what) is \(CanonicalJSON.quote(value)), not a release version like 2.3.87")
        }
        return value
    }

    /// Numerically by `X.Y.Z`, a pre-release before its release, build metadata ignored.
    static func compare(_ a: String, _ b: String) -> Int {
        guard let x = split(a), let y = split(b) else { return 0 }
        for i in 0..<3 where x.core[i] != y.core[i] { return x.core[i] < y.core[i] ? -1 : 1 }
        guard let xPre = x.pre, let yPre = y.pre else {
            if x.pre == nil, y.pre == nil { return 0 }
            return x.pre == nil ? 1 : -1
        }
        let xIds = xPre.split(separator: ".", omittingEmptySubsequences: false).map(String.init)
        let yIds = yPre.split(separator: ".", omittingEmptySubsequences: false).map(String.init)
        for i in 0..<max(xIds.count, yIds.count) {
            guard i < xIds.count, i < yIds.count else { return i < xIds.count ? 1 : -1 }
            let order = identifierOrder(xIds[i], yIds[i])
            if order != 0 { return order }
        }
        return 0
    }

    private static func identifierOrder(_ x: String, _ y: String) -> Int {
        let digits = { (s: String) in !s.isEmpty && s.utf8.allSatisfy { (0x30...0x39).contains($0) } }
        let kinds = (digits(x), digits(y))
        switch kinds {
        case (true, true):
            let (a, b) = (Double(x) ?? 0, Double(y) ?? 0)
            return a == b ? 0 : (a < b ? -1 : 1)
        case (true, false): return -1
        case (false, true): return 1
        case (false, false): return x == y ? 0 : (x < y ? -1 : 1)
        }
    }
}

enum TeeHTTP {

    /// `GET url` with `Accept: application/json`: the body and the status.
    static func get(_ url: String, session: URLSession) async throws -> (Data, Int) {
        guard let target = URL(string: url) else { throw TeeTrustError("bad URL \(url)") }
        var request = URLRequest(url: target)
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        let (data, response) = try await session.data(for: request)
        return (data, (response as? HTTPURLResponse)?.statusCode ?? 0)
    }
}
