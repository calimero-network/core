import Foundation

/// A Calimero Cloud (mero-js `CLOUD_ENVIRONMENTS`): where its node directory
/// is, where it serves app frontends, and what a client trusts its nodes by.
/// A native app has no hostname a browser checked, so the app's build chooses
/// one: a build people install from the App Store is ``production``, and only
/// a separate staging build is ``staging``. Never chosen from what a person
/// types or a server answers.
///
/// ```swift
/// #if MERO_STAGING
/// let cloud = CloudEnvironment.staging
/// #else
/// let cloud = CloudEnvironment.production
/// #endif
/// let client = try await CalimeroClient.connect(
///     CalimeroConnectOptions(cluster: cluster, policy: cloud.policy(), directory: cloud.directory))
/// ```
public enum CloudEnvironment: String, Sendable, CaseIterable {
    /// Production: `locked-read-only` images only.
    case production
    /// Staging: all three image profiles, debug ones included, whose memory the operator can read.
    case staging

    /// The node directory to read a cluster's nodes from (``CalimeroConnectOptions/directory``).
    public var directory: String {
        switch self {
        case .production: return defaultDirectory
        case .staging: return "https://manager.staging.cloud.calimero.network/api/directory"
        }
    }

    /// Where its clusters serve app frontends, `{label}.{appsDomain}`.
    public var appsDomain: String {
        switch self {
        case .production: return "apps.calimero.network"
        case .staging: return "apps.relay.staging.cloud.calimero.network"
        }
    }

    /// What every node of this Cloud must attest to: a signed mero-tee release
    /// at or above `minReleaseVersion`; on staging, of any of its profiles (the
    /// node's own, whose signed image its quote must still be).
    public func policy(minReleaseVersion: String = defaultMinRelease) -> CloudPolicy {
        switch self {
        case .production:
            return CloudPolicy(release: CloudReleasePolicy(minReleaseVersion: minReleaseVersion))
        case .staging:
            return CloudPolicy(
                release: CloudReleasePolicy(
                    minReleaseVersion: minReleaseVersion, allowDebugProfile: true,
                    profiles: [TeeTrust.productionProfile, "debug", "debug-read-only"]))
        }
    }
}
