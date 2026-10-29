# {{name}}

A [rutis](https://github.com/arcships/rutis) plugin in Swift (macOS 14 or later). The [Swift plugin guide](https://github.com/arcships/rutis/blob/main/docs/guide/swift-plugin.en.md) goes on from here.

```bash
swift test           # unit tests, no host needed
rutis-host dev       # run it in a local host; changes rebuild and restart it
```

A host runs the binary the product `{{name}}` builds (`Sources/{{name}}/main.swift`). `Info.plist` is linked into it: when the plugin asks for access to calendars, contacts and the like, macOS names this binary and shows the usage description you add there.

Push a `v*` tag and the workflow in `.github/workflows/release.yml` tests the plugin and attaches a universal binary (arm64 and x86_64, with SHA-256 sums) to a GitHub release, signed with a Developer ID and notarized when the repository has the secrets it names. A host puts the binary in its Swift plugin directory (`runtimes.swift.dir`) and adds a row `{ "id": "{{id}}", "name": "swift:{{id}}" }`.
