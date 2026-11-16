// The OpenAPI client generator, as a build of its own (`./gradlew -p codegen generateMeroClient`).
//
// Kept out of the SDK's build on purpose: the generated sources are checked in under
// mero-core/src/generated/kotlin, so building, testing or releasing the SDK needs neither the
// generator nor its ~100 dependencies, and works offline from the Gradle cache it already needs.
// Only refreshing the spec (scripts/refresh-openapi.sh) runs this build.
pluginManagement {
    repositories {
        gradlePluginPortal()
        mavenCentral()
    }
}

dependencyResolutionManagement {
    repositories {
        mavenCentral()
    }
}

rootProject.name = "mero-codegen"
