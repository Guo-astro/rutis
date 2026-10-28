# {{name}}

A [rutis](https://github.com/arcships/rutis) plugin in Go.

```bash
go mod tidy
go test ./...        # unit tests, no host needed
rutis-host dev       # run it in a local host; changes rebuild and restart it
```

A host runs the binary `cmd/{{name}}` builds. Push a `v*` tag and the workflow in `.github/workflows/release.yml` tests the plugin and attaches binaries for Linux, macOS and Windows (with SHA-256 sums) to a GitHub release. A host installs it with `rutis-host add example.com/{{name}}/cmd/{{name}}@latest`, which also adds its row; by hand, it puts the binary in its Go plugin directory (`go install example.com/{{name}}/cmd/{{name}}@latest` with `GOBIN` set to it) and adds a row `{ "id": "{{id}}", "name": "go:{{id}}" }`.
