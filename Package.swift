// swift-tools-version: 5.9
// mero-swift, the Swift SDK, as Swift Package Manager installs it from this
// repository: `.package(url: "https://github.com/calimero-network/core", exact: "<VERSION>")`
// and `.product(name: "MeroKit", package: "core")`. SwiftPM reads a package
// from the root of a git repository, so this manifest points at the sources in
// sdks/clients/mero-swift, and a core release tag is its version.
//
// The libraries only. sdks/clients/mero-swift/Package.swift is the SDK's own
// manifest (tests, the example) and the one to develop with; the products,
// targets and dependencies here match it, and sdk-mero-swift.yml checks that.
import PackageDescription

let sources = "sdks/clients/mero-swift/Sources"

let package = Package(
    name: "mero-swift",
    platforms: [
        .iOS(.v15),
        .macOS(.v12),
    ],
    products: [
        .library(name: "MeroKit", targets: ["MeroKit"]),
        .library(name: "MeroAPI", targets: ["MeroAPI"]),
        .library(name: "MeroKitUI", targets: ["MeroKitUI"]),
        .library(name: "MeroKitTestSupport", targets: ["MeroKitTestSupport"]),
    ],
    dependencies: [
        .package(url: "https://github.com/apple/swift-openapi-runtime", exact: "1.9.0"),
        .package(url: "https://github.com/apple/swift-http-types", exact: "1.5.1"),
    ],
    targets: [
        .target(
            name: "MeroAPI",
            dependencies: [
                .product(name: "OpenAPIRuntime", package: "swift-openapi-runtime"),
                .product(name: "HTTPTypes", package: "swift-http-types"),
            ],
            path: "\(sources)/MeroAPI"
        ),
        .target(
            name: "MeroKit",
            dependencies: [
                "MeroAPI",
                .product(name: "OpenAPIRuntime", package: "swift-openapi-runtime"),
                .product(name: "HTTPTypes", package: "swift-http-types"),
            ],
            path: "\(sources)/MeroKit",
            swiftSettings: [
                .enableExperimentalFeature("StrictConcurrency"),
            ]
        ),
        .target(
            name: "MeroKitUI",
            dependencies: ["MeroKit", "MeroAPI", .product(name: "OpenAPIRuntime", package: "swift-openapi-runtime")],
            path: "\(sources)/MeroKitUI",
            swiftSettings: [
                .enableExperimentalFeature("StrictConcurrency"),
            ]
        ),
        .target(
            name: "MeroKitTestSupport",
            dependencies: ["MeroKit"],
            path: "\(sources)/MeroKitTestSupport"
        ),
    ]
)
