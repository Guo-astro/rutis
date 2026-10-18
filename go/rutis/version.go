package rutis

// Version is this SDK's version: the runtime's implementation version and
// the manifest's `sdk`. The release train checks it.
const Version = "0.12.1"

// PluginAPI is the plugin API this SDK writes plugins against and its
// runtime supports.
const PluginAPI = 3

// Implementation is the name the runtime reports itself by: the module it
// is published as.
const Implementation = "github.com/arcships/rutis/go/rutis"

// marker is in every binary built with this SDK; hosts look for it before
// running a file to read its manifest.
const marker = "rutis-go-runtime:1"
