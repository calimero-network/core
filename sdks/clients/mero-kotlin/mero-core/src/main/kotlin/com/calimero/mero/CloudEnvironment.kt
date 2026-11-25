package com.calimero.mero

import com.calimero.mero.tee.PRODUCTION_PROFILE

/**
 * A Calimero Cloud (mero-js `CLOUD_ENVIRONMENTS`): where its node directory is, where it serves app
 * frontends, and what a client trusts its nodes by. A native app has no hostname a browser checked,
 * so the app's build chooses one: a build people install from a store is [PRODUCTION], and only a
 * separate staging build is [STAGING]. Never chosen from what a person types or a server answers.
 *
 * ```kotlin
 * val env = if (BuildConfig.MERO_STAGING) CloudEnvironment.STAGING else CloudEnvironment.PRODUCTION
 * CalimeroClient.connect(CalimeroConnectOptions(cluster, policy = env.policy(), directory = env.directory))
 * ```
 */
enum class CloudEnvironment(
    /** The node directory to read a cluster's nodes from ([CalimeroConnectOptions.directory]). */
    val directory: String,
    /** Where its clusters serve app frontends, `{label}.{appsDomain}`. */
    val appsDomain: String,
    /** The image profiles its clusters run, when that is more than `locked-read-only`. */
    private val profiles: List<String>?,
) {
    /** Production: `locked-read-only` images only. */
    PRODUCTION(DEFAULT_DIRECTORY, "apps.calimero.network", null),

    /** Staging: all three image profiles, debug ones included, whose memory the operator can read. */
    STAGING(
        "https://manager.staging.cloud.calimero.network/api/directory",
        "apps.relay.staging.cloud.calimero.network",
        listOf(PRODUCTION_PROFILE, "debug", "debug-read-only"),
    ),
    ;

    /**
     * What every node of this Cloud must attest to: a signed mero-tee release at or above
     * [minReleaseVersion]; on staging, of any of its profiles (the node's own, whose signed image its
     * quote must still be).
     */
    fun policy(minReleaseVersion: String = DEFAULT_MIN_RELEASE): CloudPolicy =
        CloudPolicy(
            release =
                if (profiles == null) {
                    CloudReleasePolicy(minReleaseVersion)
                } else {
                    CloudReleasePolicy(minReleaseVersion, allowDebugProfile = true, profiles = profiles)
                },
        )
}
