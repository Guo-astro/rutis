#!/usr/bin/env node
// A local npm registry over the tarballs a release gate built
// (`RUTIS_E2E_ARTIFACTS/npm`, tools/install/local-artifacts.sh or
// package.yml): the guides' examples name packages (`npx @arcships/rutis-host
// new …`, `npm install`), so with this registry they run as written, before
// those packages are published (docs/design-function-refs-2026-11-03.md §6).
//
//   node tools/install/artifact-registry.mjs <dir-with-tgz>
//
// Prints the registry's URL on the first line of stdout, then serves:
//
//   GET /<name>              the packument: one version per tarball, each with
//                            its own `bin`, `os`, `cpu` and `dist.tarball`
//   GET /<name>/-/<file>     the tarball, as npm packed it
//
// Package metadata is read from the tarballs themselves (`package/package.json`),
// so nothing here needs a manifest beside them. Only what a tarball says is
// served: no other versions, no search, no publish.
//
// Any other name a project asks for (a guide's template also installs `tsx`,
// `typescript`, and a Python example's `uv` toolchain) is passed through to
// the public registry, so one `npm_config_registry` is enough: the gate has
// the network, and every package that is not this build's comes from where a
// user's would. `RUTIS_ARTIFACT_REGISTRY_UPSTREAM` names another one.
import { createServer } from 'node:http'
import { readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { execFileSync } from 'node:child_process'

const dir = process.argv[2]
if (!dir) {
  console.error('usage: node tools/install/artifact-registry.mjs <dir-with-tgz>')
  process.exit(1)
}

// name -> version -> { file, manifest }
const packages = new Map()
for (const file of readdirSync(dir).filter(name => name.endsWith('.tgz')).sort()) {
  const manifest = JSON.parse(
    execFileSync('tar', ['-xzOf', join(dir, file), 'package/package.json'], { encoding: 'utf8' }),
  )
  const versions = packages.get(manifest.name) ?? new Map()
  versions.set(manifest.version, { file, manifest })
  packages.set(manifest.name, versions)
}

const upstream = (process.env.RUTIS_ARTIFACT_REGISTRY_UPSTREAM ?? 'https://registry.npmjs.org').replace(/\/$/, '')

// A name this registry does not hold: the public registry's answer, as it is
// (its tarball URLs point there, so npm fetches those from it directly).
async function passThrough(url, response) {
  try {
    const answer = await fetch(`${upstream}${url}`)
    const body = Buffer.from(await answer.arrayBuffer())
    response.writeHead(answer.status, {
      'content-type': answer.headers.get('content-type') ?? 'application/json',
    })
    response.end(body)
  } catch (error) {
    response.writeHead(502, { 'content-type': 'application/json' })
    response.end(JSON.stringify({ error: `${upstream}: ${error}` }))
  }
}

const server = createServer((request, response) => {
  const url = decodeURIComponent((request.url ?? '/').split('?')[0])
  const tarball = url.match(/^\/(.+)\/-\/([^/]+)$/)
  if (tarball) {
    for (const versions of packages.values()) {
      for (const { file } of versions.values()) {
        if (file !== tarball[2]) continue
        response.writeHead(200, { 'content-type': 'application/octet-stream' })
        response.end(readFileSync(join(dir, file)))
        return
      }
    }
    passThrough(url, response)
    return
  }
  const name = url.replace(/^\//, '')
  const versions = packages.get(name)
  if (!versions) {
    passThrough(url, response)
    return
  }
  const origin = `http://${request.headers.host}`
  const latest = [...versions.keys()].at(-1)
  const document = {
    name,
    'dist-tags': { latest },
    // Each version as its own manifest says (dependencies, `bin`, `os`, ...),
    // with the tarball this registry serves.
    versions: Object.fromEntries(
      [...versions].map(([version, { file, manifest }]) => [
        version,
        { ...manifest, name, version, dist: { tarball: `${origin}/${name}/-/${file}` } },
      ]),
    ),
  }
  response.writeHead(200, { 'content-type': 'application/json' })
  response.end(JSON.stringify(document))
})

server.listen(0, '127.0.0.1', () => {
  console.log(`http://127.0.0.1:${server.address().port}/`)
})
