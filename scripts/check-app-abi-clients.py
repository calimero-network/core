#!/usr/bin/env python3
"""Compile the Swift and Kotlin clients `mero-app-build codegen` writes, warnings as errors,
against the SDK releases they are written for, and run their round-trip tests.

The golden files (crates/app-abi/tests/golden/*.swift, *.kt) pin what the generator writes;
this checks that what it writes compiles, and that the app types' JSON (records, the four serde
enum tagging styles, recursive and generic types) survives a decode and an encode
(crates/app-abi/tests/native/{swift,kotlin}):

    python3 scripts/check-app-abi-clients.py swift    # macOS: swift-sdk (MeroKit), `swift build`, `swift test`
    python3 scripts/check-app-abi-clients.py kotlin   # JDK 17 and the Android SDK: kotlin-sdk's mero-core

Swift: a package with one target per golden (each its own module, as an app gives each
service's client one), depending on swift-sdk at SWIFT_SDK_VERSION, and a test target with the
round-trip tests.
Kotlin: kotlin-sdk's tag KOTLIN_SDK_REF is cloned and the goldens compiled as a module of that
build, depending on `:mero-core`, with the round-trip tests as its unit tests; the `*.plugin.kt`
goldens (`--kotlin-serialization-plugin`) in a second module that applies the kotlinx.serialization
compiler plugin, with the same tests.
"""

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GOLDEN = ROOT / "crates" / "app-abi" / "tests" / "golden"
NATIVE_TESTS = ROOT / "crates" / "app-abi" / "tests" / "native"
SWIFT_SDK_VERSION = os.environ.get("SWIFT_SDK_VERSION", "4.1.0")
KOTLIN_SDK_REF = os.environ.get("KOTLIN_SDK_REF", "v4.0.0")


def module_name(stem):
    """`kitchen-sink` → `KitchenSink`."""
    return "".join(part[:1].upper() + part[1:] for part in stem.split("-"))


def run(cmd, cwd):
    print("+", " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=cwd, check=True)


def swift(work):
    goldens = sorted(GOLDEN.glob("*.swift"))
    settings = """swiftSettings: [
                .enableExperimentalFeature("StrictConcurrency"),
                .unsafeFlags(["-warnings-as-errors"]),
            ]"""
    targets = []
    names = []
    for golden in goldens:
        name = module_name(golden.stem)
        names.append(name)
        source = work / "Sources" / name
        source.mkdir(parents=True)
        shutil.copy(golden, source / f"{name}Client.swift")
        targets.append(
            f"""        .target(
            name: "{name}",
            dependencies: [.product(name: "MeroKit", package: "swift-sdk")],
            {settings}
        ),"""
        )
    tests = work / "Tests" / "GoldenClientsTests"
    tests.mkdir(parents=True)
    for test in sorted((NATIVE_TESTS / "swift").glob("*.swift")):
        shutil.copy(test, tests / test.name)
    dependencies = ", ".join(f'"{name}"' for name in names)
    targets.append(
        f"""        .testTarget(
            name: "GoldenClientsTests",
            dependencies: [{dependencies}, .product(name: "MeroKit", package: "swift-sdk")],
            {settings}
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
    run(["swift", "test"], work)


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
    # The goldens as written for a module without the kotlinx.serialization plugin, and the
    # kitchen-sink's as written for one with it (`*.plugin.kt`, `--kotlin-serialization-plugin`):
    # each compiled the way it is meant to be, with the same round-trip tests.
    modules = [
        ("golden-clients", [g for g in sorted(GOLDEN.glob("*.kt")) if not g.name.endswith(".plugin.kt")], ""),
        ("golden-clients-plugin", sorted(GOLDEN.glob("*.plugin.kt")), "\n    alias(libs.plugins.kotlin.serialization)"),
    ]
    tasks = []
    for name, goldens, plugin in modules:
        module = sdk / name
        source = module / "src" / "main" / "kotlin"
        source.mkdir(parents=True)
        for golden in goldens:
            stem = golden.name.removesuffix(".plugin.kt").removesuffix(".kt")
            shutil.copy(golden, source / f"{module_name(stem)}Client.kt")
        tests = module / "src" / "test" / "kotlin"
        tests.mkdir(parents=True)
        for test in sorted((NATIVE_TESTS / "kotlin").glob("*.kt")):
            shutil.copy(test, tests / test.name)
        (module / "build.gradle.kts").write_text(
            """plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)PLUGIN
}

android {
    namespace = "network.calimero.NAMESPACE"
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
    testImplementation(libs.junit)
}
""".replace("PLUGIN", plugin).replace("NAMESPACE", name.replace("-", ""))
        )
        with open(sdk / "settings.gradle.kts", "a") as settings:
            settings.write(f'\ninclude(":{name}")\n')
        tasks += [f":{name}:compileReleaseKotlin", f":{name}:testReleaseUnitTest"]
    run(["./gradlew", "--no-daemon", "--stacktrace", *tasks], sdk)


def main():
    if len(sys.argv) != 2 or sys.argv[1] not in ("swift", "kotlin"):
        sys.exit(__doc__)
    with tempfile.TemporaryDirectory() as work:
        {"swift": swift, "kotlin": kotlin}[sys.argv[1]](Path(work))


if __name__ == "__main__":
    main()
