plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.serialization)
}

// Test-support library — the Android analog of the Swift SDK's `MeroKitTestSupport` target.
// Ships `InMemoryAppBackend` and `RecordingContextContract`, so an app's view models (and the
// sample app's mock mode) run with no cluster and no passkey. Not published (no maven-publish here).
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
}

dependencies {
    // InMemoryAppBackend implements mero-core's app interfaces (`com.calimero.mero.app`), as the
    // Swift SDK's `MeroKitTestSupport` depends on `MeroKit` for the same fake.
    api(project(":mero-core"))
    implementation(libs.kotlinx.serialization.json)
    api(libs.kotlinx.coroutines.core)
}
