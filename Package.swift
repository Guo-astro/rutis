// swift-tools-version:5.9
// The Swift SDK of rutis (swift/rutis): the library `Rutis` that writes
// plugins and runs them as a runtime. Design:
// docs/design-swift-sdk-2026-10-12.md. SwiftPM takes a git dependency from
// the repository root only, so the manifest is here and its targets point
// into swift/rutis.
import PackageDescription

let package = Package(
    name: "rutis",
    platforms: [.macOS(.v14), .iOS(.v17)],
    products: [
        .library(name: "Rutis", targets: ["Rutis"]),
    ],
    targets: [
        .target(
            name: "Rutis",
            path: "swift/rutis/Sources/Rutis"
        ),
        // The session conformance target (rutis_bridge::session::testing).
        .executableTarget(
            name: "RutisSessionFixture",
            dependencies: ["Rutis"],
            path: "swift/rutis/Sources/RutisSessionFixture"
        ),
        .testTarget(
            name: "RutisTests",
            dependencies: ["Rutis"],
            path: "swift/rutis/Tests/RutisTests"
        ),
    ],
    swiftLanguageVersions: [.v5]
)
