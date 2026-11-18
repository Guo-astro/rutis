// swift-tools-version:5.9
// The Swift SDK of rutis (swift/rutis): the library `Rutis` that writes
// plugins and runs them as a runtime, its macros, and the testing tool
// `RutisTest`. Design: docs/design-swift-sdk-2026-10-12.md. SwiftPM takes a
// git dependency from the repository root only, so the manifest is here and
// its targets point into swift/rutis.
import CompilerPluginSupport
import PackageDescription

// Swift 6's strict concurrency, as a warning (#630). `swiftLanguageVersions`
// below stays Swift 5, so these diagnostics do not fail a build: a target
// moves when its count reaches zero. The library `Rutis`, its test tool
// `RutisTest`, the fixture plugins and `RutisSelfTest` have the checking on;
// the executables that carry the plugins follow. What is left in each is
// printed by `tools/swift-strict-concurrency.sh`. Not `unsafeFlags`: SwiftPM
// rejects those in a package that is a git dependency.
let strictConcurrency: [SwiftSetting] = [
    .enableUpcomingFeature("StrictConcurrency"),
]

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
            swiftSettings: strictConcurrency
        ),
        .target(
            name: "RutisTest",
            dependencies: ["Rutis"],
            path: "swift/rutis/Sources/RutisTest",
            swiftSettings: strictConcurrency
        ),
        // The test plugins: the conformance plugins the Rust suites and
        // RutisTest are checked with.
        .target(
            name: "RutisFixturePlugins",
            dependencies: ["Rutis"],
            path: "swift/rutis/Sources/RutisFixturePlugins",
            swiftSettings: strictConcurrency
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
            swiftSettings: strictConcurrency
        ),
    ],
    swiftLanguageVersions: [.v5]
)
