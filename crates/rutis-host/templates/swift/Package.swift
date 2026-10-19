// swift-tools-version:6.0
// The {{name}} rutis plugin: the library {{Type}}Plugin (the plugin), the
// binary {{name}} a host runs (Sources/{{name}}/main.swift), and its tests.
import Foundation
import PackageDescription

// Info.plist is linked into the binary: macOS shows its usage descriptions
// when the plugin asks for access (calendars, contacts, ...), and the access
// it grants is the binary's own.
let infoPlist = URL(fileURLWithPath: #filePath).deletingLastPathComponent().appendingPathComponent("Info.plist").path

let package = Package(
    name: "{{name}}",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "{{name}}", targets: ["{{name}}"]),
        .library(name: "{{Type}}Plugin", targets: ["{{Type}}Plugin"]),
    ],
    dependencies: [
        // The Swift SDK released with the rutis-host that made this project.
        .package(url: "https://github.com/arcships/rutis", from: "{{version}}"),
    ],
    targets: [
        .target(
            name: "{{Type}}Plugin",
            dependencies: [.product(name: "Rutis", package: "rutis")]
        ),
        .executableTarget(
            name: "{{name}}",
            dependencies: ["{{Type}}Plugin", .product(name: "Rutis", package: "rutis")],
            linkerSettings: [
                .unsafeFlags(["-Xlinker", "-sectcreate", "-Xlinker", "__TEXT", "-Xlinker", "__info_plist", "-Xlinker", infoPlist]),
            ]
        ),
        .testTarget(
            name: "{{Type}}PluginTests",
            dependencies: ["{{Type}}Plugin", .product(name: "RutisTest", package: "rutis")]
        ),
    ],
    swiftLanguageModes: [.v5]
)
