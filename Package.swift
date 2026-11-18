// swift-tools-version:6.0
// The Swift SDK of rutis (swift/rutis): the library `Rutis` that writes
// plugins and runs them as a runtime, its macros, and the testing tool
// `RutisTest`. Design: docs/design-swift-sdk-2026-10-12.md. SwiftPM takes a
// git dependency from the repository root only, so the manifest is here and
// its targets point into swift/rutis.
import CompilerPluginSupport
import PackageDescription

// Swift 6's strict concurrency, enforced (#630, #1273): `swiftLanguageModes`
// below is `.v6` for every target, so a diagnostic `tools/swift-strict-
// concurrency.sh` lists fails the build; the ones the compiler cannot prove
// (`@unchecked Sendable`) say in place why they are safe. Tools version 6.0
// is what makes `.v6` available.
let package = Package(
    name: "rutis",
    platforms: [.macOS(.v14), .iOS(.v17)],
    products: [
        .library(name: "Rutis", targets: ["Rutis"]),
        .library(name: "RutisTest", targets: ["RutisTest"]),
    ],
    dependencies: [
        // Build time only: the macros run in the compiler, not in the binary.
        .package(url: "https://github.com/swiftlang/swift-syntax.git", "602.0.0"..<"604.0.0"),
    ],
    targets: [
        // What the macros generate: a library, so the tests check it too.
        .target(
            name: "RutisMacrosCore",
            dependencies: [
                .product(name: "SwiftSyntax", package: "swift-syntax"),
                .product(name: "SwiftSyntaxBuilder", package: "swift-syntax"),
                .product(name: "SwiftSyntaxMacros", package: "swift-syntax"),
                .product(name: "SwiftDiagnostics", package: "swift-syntax"),
            ],
            path: "swift/rutis/Sources/RutisMacrosCore"
        ),
        .macro(
            name: "RutisMacros",
            dependencies: [
                "RutisMacrosCore",
                .product(name: "SwiftSyntax", package: "swift-syntax"),
                .product(name: "SwiftSyntaxMacros", package: "swift-syntax"),
                .product(name: "SwiftCompilerPlugin", package: "swift-syntax"),
            ],
            path: "swift/rutis/Sources/RutisMacros"
        ),
        .target(
            name: "Rutis",
            dependencies: ["RutisMacros"],
            path: "swift/rutis/Sources/Rutis",
        ),
        .target(
            name: "RutisTest",
            dependencies: ["Rutis"],
            path: "swift/rutis/Sources/RutisTest",
        ),
        // The test plugins: the conformance plugins the Rust suites and
        // RutisTest are checked with.
        .target(
            name: "RutisFixturePlugins",
            dependencies: ["Rutis"],
            path: "swift/rutis/Sources/RutisFixturePlugins",
        ),
        // The test plugins in one binary (weather, probe, conformance-events).
        .executableTarget(
            name: "RutisConformance",
            dependencies: ["Rutis", "RutisFixturePlugins"],
            path: "swift/rutis/Sources/RutisConformance"
        ),
        // The plugins of the suites that run several runtimes.
        .executableTarget(
            name: "RutisMultilang",
            dependencies: ["Rutis", "RutisFixturePlugins"],
            path: "swift/rutis/Sources/RutisMultilang"
        ),
        // `ping` alone: copied under two names, two binaries with one plugin.
        .executableTarget(
            name: "RutisPing",
            dependencies: ["Rutis", "RutisFixturePlugins"],
            path: "swift/rutis/Sources/RutisPing"
        ),
        // The dial conformance plugin `marker` (dial_conformance.rs).
        .executableTarget(
            name: "RutisDial",
            dependencies: ["Rutis", "RutisFixturePlugins"],
            path: "swift/rutis/Sources/RutisDial"
        ),
        // The session-end fixtures `swift_lease` and `host_gone`
        // (lease_e2e.rs, host_gone.rs).
        .executableTarget(
            name: "RutisLeaseFixture",
            dependencies: ["Rutis", "RutisFixturePlugins"],
            path: "swift/rutis/Sources/RutisLeaseFixture"
        ),
        // An app hosting a plugin with RutisApp, on macOS (rutis-host's
        // tests/swift.rs).
        .executableTarget(
            name: "RutisAppFixture",
            dependencies: ["Rutis"],
            path: "swift/rutis/Sources/RutisAppFixture"
        ),
        // The session conformance target (rutis_bridge::session::testing).
        .executableTarget(
            name: "RutisSessionFixture",
            dependencies: ["Rutis"],
            path: "swift/rutis/Sources/RutisSessionFixture"
        ),
        // The SDK's own tests, RutisTest's conformance and the macros':
        // `swift run RutisSelfTest` (a harness of its own, so that they run
        // with any toolchain, Command Line Tools too).
        .executableTarget(
            name: "RutisSelfTest",
            dependencies: [
                "Rutis", "RutisTest", "RutisFixturePlugins", "RutisMacrosCore",
                .product(name: "SwiftSyntax", package: "swift-syntax"),
                .product(name: "SwiftParser", package: "swift-syntax"),
                .product(name: "SwiftSyntaxMacros", package: "swift-syntax"),
                .product(name: "SwiftSyntaxMacroExpansion", package: "swift-syntax"),
            ],
            path: "swift/rutis/Tests/SelfTest",
        ),
    ],
    swiftLanguageModes: [.v6]
)
