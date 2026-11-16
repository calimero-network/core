pluginManagement {
    repositories {
        google {
            content {
                includeGroupByRegex("com\\.android.*")
                includeGroupByRegex("com\\.google.*")
                includeGroupByRegex("androidx.*")
            }
        }
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "mero-kotlin"

include(":mero-core")
include(":mero-compose")
include(":mero-testkit")
// The Mero Chat sample, under examples/ as every SDK's examples are; still `:sample-app`.
include(":sample-app")
project(":sample-app").projectDir = file("examples/sample-app")
