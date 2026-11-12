#!/usr/bin/env python3
"""Compile the Swift and Kotlin clients `mero-app-build codegen` writes, warnings as errors,
against the SDK releases they are written for.

The golden files (crates/app-abi/tests/golden/*.swift, *.kt) pin what the generator writes;
this checks that what it writes compiles:

    python3 scripts/check-app-abi-clients.py swift    # macOS: swift-sdk (MeroKit), `swift build`
    python3 scripts/check-app-abi-clients.py kotlin   # JDK 17 and the Android SDK: kotlin-sdk's mero-core

Swift: a package with one target per golden (each its own module, as an app gives each
service's client one), depending on swift-sdk at SWIFT_SDK_VERSION.
Kotlin: kotlin-sdk 2.0 is not on Maven Central, so its tag KOTLIN_SDK_REF is cloned and the
goldens compiled as a module of that build, depending on `:mero-core`.
"""

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GOLDEN = ROOT / "crates" / "app-abi" / "tests" / "golden"
SWIFT_SDK_VERSION = os.environ.get("SWIFT_SDK_VERSION", "2.0.0")
KOTLIN_SDK_REF = os.environ.get("KOTLIN_SDK_REF", "v2.0.0")


def module_name(stem):
    """`kitchen-sink` → `KitchenSink`."""
    return "".join(part[:1].upper() + part[1:] for part in stem.split("-"))


def run(cmd, cwd):
    print("+", " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=cwd, check=True)


def swift(work):
    goldens = sorted(GOLDEN.glob("*.swift"))
    targets = []
    for golden in goldens:
        name = module_name(golden.stem)
        source = work / "Sources" / name
        source.mkdir(parents=True)
        shutil.copy(golden, source / f"{name}Client.swift")
        targets.append(
            f"""        .target(
            name: "{name}",
            dependencies: [.product(name: "MeroKit", package: "swift-sdk")],
            swiftSettings: [
                .enableExperimentalFeature("StrictConcurrency"),
                .unsafeFlags(["-warnings-as-errors"]),
            ]
        ),"""
        )
    (work / "Package.swift").write_text(
        f"""// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "GoldenClients",
    platforms: [.iOS(.v15), .macOS(.v12)],
    dependencies: [
        .package(url: "https://github.com/calimero-network/swift-sdk", exact: "{SWIFT_SDK_VERSION}"),
    ],
    targets: [
{chr(10).join(targets)}
    ]
)
"""
    )
    run(["swift", "build"], work)


def kotlin(work):
    sdk = work / "kotlin-sdk"
    run(
        [
            "git",
            "clone",
            "--quiet",
            "--depth",
            "1",
            "--branch",
            KOTLIN_SDK_REF,
            "https://github.com/calimero-network/kotlin-sdk",
            str(sdk),
        ],
        work,
    )
    module = sdk / "golden-clients"
    source = module / "src" / "main" / "kotlin"
    source.mkdir(parents=True)
    for golden in sorted(GOLDEN.glob("*.kt")):
        shutil.copy(golden, source / f"{module_name(golden.stem)}Client.kt")
    (module / "build.gradle.kts").write_text(
        """plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
}

android {
    namespace = "network.calimero.goldenclients"
    compileSdk = 34

    defaultConfig {
        minSdk = 24
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
        allWarningsAsErrors = true
    }
}

dependencies {
    implementation(project(":mero-core"))
    implementation(libs.kotlinx.coroutines.core)
}
"""
    )
    with open(sdk / "settings.gradle.kts", "a") as settings:
        settings.write('\ninclude(":golden-clients")\n')
    run(["./gradlew", "--no-daemon", "--stacktrace", ":golden-clients:compileReleaseKotlin"], sdk)


def main():
    if len(sys.argv) != 2 or sys.argv[1] not in ("swift", "kotlin"):
        sys.exit(__doc__)
    with tempfile.TemporaryDirectory() as work:
        {"swift": swift, "kotlin": kotlin}[sys.argv[1]](Path(work))


if __name__ == "__main__":
    main()
