plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    // @Serializable models (the invitation pending-intent store).
    alias(libs.plugins.kotlin.serialization)
    alias(libs.plugins.compose.compiler)
    `maven-publish`
    signing
    // Per-module half of the Central bundle; the root aggregates these.
    alias(libs.plugins.nmcp)
}

android {
    namespace = "com.calimero.mero.compose"
    compileSdk = 34

    defaultConfig {
        minSdk = 24
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    buildFeatures {
        compose = true
    }

    testOptions {
        unitTests {
            isReturnDefaultValues = true
            all { it.useJUnit() }
        }
    }

    publishing {
        singleVariant("release") {
            withSourcesJar()
            // No withJavadocJar(): AGP 8.5's Dokka reads mero-core's classes with an ASM that
            // predates Java 17's PermittedSubclasses, so it fails on any sealed type this
            // module's API names (NodeEvent, …). The placeholder below takes its place.
        }
    }
}

dependencies {
    api(project(":mero-core"))

    implementation(libs.kotlinx.coroutines.android)
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.browser)
    api(libs.androidx.credentials)
    implementation(libs.androidx.credentials.play.services)
    implementation(libs.androidx.security.crypto)
    implementation(libs.kotlinx.serialization.json)

    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.foundation)
    implementation(libs.compose.material3)
    implementation(libs.compose.ui.tooling.preview)
    debugImplementation(libs.compose.ui.tooling)

    testImplementation(libs.junit)
    testImplementation(libs.kotlinx.coroutines.test)
    testImplementation(project(":mero-testkit"))
}

// Central rejects a release with no javadoc jar; a placeholder one is accepted. The API docs
// are the KDoc in the sources jar and the docs site.
val javadocJar by tasks.registering(Jar::class) {
    archiveClassifier.set("javadoc")
    from(layout.projectDirectory.file("JAVADOC.md"))
}

group = providers.gradleProperty("GROUP").getOrElse("com.calimero.mero")
version = providers.gradleProperty("VERSION_NAME").getOrElse("0.1.0")

publishing {
    publications {
        register<MavenPublication>("release") {
            groupId = group.toString()
            artifactId = "mero-compose"
            version = project.version.toString()
            afterEvaluate { from(components["release"]) }
            artifact(javadocJar)
            pom {
                name.set("mero-compose")
                description.set("Calimero Android SDK — app session, passkeys and consent for Jetpack Compose apps.")
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
