// The e2e probe for the Node runtime: calls the services the harness asks
// for and prints each result as one JSON line on stdout.
//
// The harness writes a request as `<seq>.json` into `config.dir`:
// `{ "service": "report", "method": "summary", "args": ["greeting"] }`. The
// probe answers `{"probe":"<id>","seq":<seq>,"ok":<result>}` or
// `{"probe":"<id>","seq":<seq>,"error":"<message>"}`, and reports
// `{"probe":"<id>","event":"started"}` / `"stopped"` as it loads and unloads.
// A request it cannot read is reported as `{"probe":"<id>","failed":"<message>"}`,
// and it goes on polling.
// The harness writes one copy per probe, with the services it calls in
// place of `INJECT` (a plugin declares what it uses), so it starts once they
// run. A watching probe has its services in place of `OPTIONAL` instead:
// declared `optional`, it runs while they are down and reads each with
// `ctx.get` at the call, answering `{"error":"no <service> now"}` while it
// is not there.
import { definePlugin } from '@arcships/rutis'
import { readdirSync, readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'

interface Config {
  id: string
  dir: string
}

interface Request {
  service: string
  method: string
  args: unknown[]
}

type Service = Record<string, (...args: unknown[]) => unknown>

const optional: string[] = OPTIONAL

export default definePlugin<Config>({
  inject: INJECT,
  // Only when there are any: the releases before `optional` meet this
  // probe too (tests/e2e/tests/cross_version.rs).
  ...(optional.length > 0 ? { optional } : {}),
  apply(ctx, config) {
    const say = (record: object) => console.log(JSON.stringify({ probe: config.id, ...record }))
    let busy = false
    const poll = async () => {
      if (busy) return
      busy = true
      try {
        const names = readdirSync(config.dir)
          .filter(name => name.endsWith('.json'))
          .sort((a, b) => Number.parseInt(a) - Number.parseInt(b))
        for (const name of names) {
          const path = join(config.dir, name)
          const request: Request = JSON.parse(readFileSync(path, 'utf8'))
          rmSync(path)
          const seq = Number.parseInt(name)
          try {
            const service = optional.includes(request.service)
              ? ctx.get<Service>(request.service)
              : ctx.use<Service>(request.service)
            if (service === undefined) throw new Error(`no ${request.service} now`)
            say({ seq, ok: (await service[request.method](...request.args)) ?? null })
          } catch (error) {
            say({ seq, error: String((error as Error)?.message ?? error) })
          }
        }
      } catch (error) {
        // The harness fails the scenario on it.
        say({ failed: String((error as Error)?.message ?? error) })
      } finally {
        busy = false
      }
    }
    const timer = setInterval(poll, 50)
    say({ event: 'started' })
    return () => {
      clearInterval(timer)
      say({ event: 'stopped' })
    }
  },
})
