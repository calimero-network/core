plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.serialization)
    `maven-publish`
    signing
    // Per-module half of the Central bundle; the root aggregates these.
    alias(libs.plugins.nmcp)
}

android {
    namespace = "com.calimero.mero"
    compileSdk = 34

    defaultConfig {
        minSdk = 24
        testOptions.targetSdk = 34
        consumerProguardFiles("consumer-rules.pro")
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    publishing {
        singleVariant("release") {
            withSourcesJar()
            // Central rejects a release with no javadoc jar, even an empty one.
            withJavadocJar()
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    sourceSets {
        // The OpenAPI client and its facade, generated from openapi/openapi.json by the codegen build
        // (`./gradlew -p codegen generateMeroClient`) and checked in: see openapi/README.md.
        getByName("main").java.srcDir("src/generated/kotlin")
    }

    testOptions {
        unitTests {
            isReturnDefaultValues = true
            all { it.useJUnit() }
        }
    }
}

dependencies {
    implementation(libs.kotlinx.coroutines.core)
    implementation(libs.kotlinx.coroutines.android)
    api(libs.kotlinx.serialization.json)

    api(libs.okhttp)
    api(libs.okhttp.sse)

    implementation(libs.androidx.security.crypto)
    // Ed25519 signing and X25519 key generation for the Cloud account layer. The JCA has
    // neither below API 33, and minSdk is 24. Only the lightweight API is used (no provider
    // registration), so it does not clash with the platform's repackaged copy.
    implementation(libs.bouncycastle.prov)

    testImplementation(libs.junit)
    testImplementation(libs.kotlinx.coroutines.test)
    testImplementation(libs.okhttp.mockwebserver)
    testImplementation(project(":mero-testkit"))
}

group = providers.gradleProperty("GROUP").getOrElse("com.calimero.mero")
version = providers.gradleProperty("VERSION_NAME").getOrElse("0.1.0")

publishing {
    publications {
        register<MavenPublication>("release") {
            groupId = group.toString()
            artifactId = "mero-core"
            version = project.version.toString()
            afterEvaluate { from(components["release"]) }
            pom {
                name.set("mero-core")
                description.set("Calimero Android SDK — core: the Calimero 1.0 client (attestation, sealed transport, passkeys).")
                url.set("https://github.com/calimero-network/kotlin-sdk")
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
                    url.set("https://github.com/calimero-network/kotlin-sdk")
                    connection.set("scm:git:https://github.com/calimero-network/kotlin-sdk.git")
                }
            }
        }
    }
    repositories {
        maven {
            name = "GitHubPackages"
            url = uri("https://maven.pkg.github.com/calimero-network/kotlin-sdk")
            credentials {
                username = System.getenv("GITHUB_ACTOR") ?: providers.gradleProperty("gpr.user").orNull
                password = System.getenv("GITHUB_TOKEN") ?: providers.gradleProperty("gpr.token").orNull
            }
        }
    }
}

// Maven Central rejects unsigned artifacts. Opted into only when a key is present, so
// `publishToMavenLocal` and a GitHub Packages publish still work on a machine with no
// GPG key; a Central release sets ORG_GRADLE_PROJECT_signingInMemoryKey, which Gradle
// maps onto this project property itself. Placed after `publishing` so the publication
// it signs already exists.
signing {
    val signingKey = providers.gradleProperty("signingInMemoryKey").orNull
    if (signingKey != null) {
        // Empty string, not null, when the key has no passphrase: the null overload
        // resolves but builds no signatory, and the failure surfaces only when a
        // signing task runs — "no configured signatory" at release time.
        useInMemoryPgpKeys(
            signingKey,
            providers.gradleProperty("signingInMemoryKeyPassword").getOrElse(""),
        )
        sign(publishing.publications)
    }
}
