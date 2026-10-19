/// The rutis namespace: plugin definitions, the runtime, and the value types
/// Swift lacks.
public enum Rutis {
    /// This SDK's version: the runtime's implementation version and the
    /// manifest's `sdk`. The release train checks it (scripts/train.mjs).
    public static let sdkVersion = "0.13.0"

    /// The plugin API this SDK writes plugins against and its runtime
    /// supports.
    public static let pluginAPI = 3

    /// The name the runtime reports itself by: the package it is published as.
    public static let implementation = "github.com/arcships/rutis"

    /// In every binary built with this SDK: hosts look for it before running
    /// a file to read its manifest.
    static let marker = "rutis-swift-runtime:1"
}
