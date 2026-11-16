plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.serialization)
    `maven-publish`
    signing
    // Per-module half of the Central bundle; the root aggregates these.
    alias(libs.plugins.nmcp)
}

// Test-support library — the Android analog of the Swift SDK's `MeroKitTestSupport` target.
// Ships `InMemoryAppBackend` and `RecordingContextContract`, so an app's view models (and the
// sample app's mock mode) run with no cluster and no passkey. Published as
// `network.calimero:mero-kotlin-testkit`, for an app's own tests.
android {
    namespace = "com.calimero.mero.testkit"
    compileSdk = 34

    defaultConfig {
        minSdk = 24
        testOptions.targetSdk = 34
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    publishing {
        singleVariant("release") {
            withSourcesJar()
            // No withJavadocJar(), as in mero-compose: AGP 8.5's Dokka cannot read mero-core's
            // sealed types. The placeholder below takes its place.
        }
    }
}

dependencies {
    // InMemoryAppBackend implements mero-core's app interfaces (`com.calimero.mero.app`), as the
    // Swift SDK's `MeroKitTestSupport` depends on `MeroKit` for the same fake.
    api(project(":mero-core"))
    implementation(libs.kotlinx.serialization.json)
    api(libs.kotlinx.coroutines.core)
}

// Central rejects a release with no javadoc jar; a placeholder one is accepted.
val javadocJar by tasks.registering(Jar::class) {
    archiveClassifier.set("javadoc")
    from(layout.projectDirectory.file("JAVADOC.md"))
}

group = providers.gradleProperty("GROUP").get()
version = providers.gradleProperty("VERSION_NAME").get()

publishing {
    publications {
        register<MavenPublication>("release") {
            groupId = group.toString()
            artifactId = "mero-kotlin-testkit"
            version = project.version.toString()
            afterEvaluate { from(components["release"]) }
            artifact(javadocJar)
            pom {
                name.set("mero-kotlin-testkit")
                description.set("Calimero Android SDK — an in-memory app backend for tests with no cluster.")
                url.set("https://github.com/calimero-network/core/tree/master/sdks/clients/mero-kotlin")
                licenses {
                    license {
                        name.set("MIT")
                        url.set("https://opensource.org/licenses/MIT")
                    }
                }
                developers {
                    developer {
                        id.set("calimero-network")
                        name.set("Calimero Network")
                    }
                }
                scm {
                    url.set("https://github.com/calimero-network/core")
                    connection.set("scm:git:https://github.com/calimero-network/core.git")
                }
            }
        }
    }
    repositories {
        maven {
            name = "GitHubPackages"
            url = uri("https://maven.pkg.github.com/calimero-network/core")
            credentials {
                username = System.getenv("GITHUB_ACTOR") ?: providers.gradleProperty("gpr.user").orNull
                password = System.getenv("GITHUB_TOKEN") ?: providers.gradleProperty("gpr.token").orNull
            }
        }
    }
}

// Signed only when a key is present, as mero-core and mero-compose are.
signing {
    val signingKey = providers.gradleProperty("signingInMemoryKey").orNull
    if (signingKey != null) {
        useInMemoryPgpKeys(
            signingKey,
            providers.gradleProperty("signingInMemoryKeyPassword").getOrElse(""),
        )
        sign(publishing.publications)
    }
}
