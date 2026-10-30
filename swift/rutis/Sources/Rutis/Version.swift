/// The rutis namespace: plugin definitions and the runtime. What a plugin
/// author rarely writes is in `Rutis.Advanced`; what the macros generate
/// code with is in `Rutis._Generated`.
public enum Rutis {
    /// What a plugin author rarely writes (#692): the SDK's version
    /// constants, the value types Swift lacks, serving and manifests by
    /// hand, the types of events and of objects of another process, and
    /// method kinds and shapes. `Plugin`, `Rutis.define`, `Rutis.serve`,
    /// `Rutis.Ctx` and the macros are the plugin author's.
    public enum Advanced {}

    /// What the code `@RutisService`, `@RutisClient` and `@RutisConfig`
    /// generate uses: not written by hand.
    public enum _Generated {}

    /// In every binary built with this SDK: hosts look for it before running
    /// a file to read its manifest.
    static let marker = "rutis-swift-runtime:1"
}

extension Rutis.Advanced {
    /// This SDK's version: the runtime's implementation version and the
    /// manifest's `sdk`. The release train checks it (scripts/train.mjs).
    public static let sdkVersion = "0.19.1"

    /// The plugin API this SDK writes plugins against and its runtime
    /// supports.
    public static let pluginAPI = 3

    /// The name the runtime reports itself by: the package it is published as.
    public static let implementation = "github.com/arcships/rutis"
}
