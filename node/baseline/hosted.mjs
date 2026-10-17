// A published plugin that depends on a service its host provides, in native
// Cordis: dsh-persona contributes its sections to `systemPrompt`. The host
// service records every call the plugin makes, as the rutis side does in
// the end-to-end scenario S6 (tests/e2e/tests/cordis.rs), which compares
// the two.
//
// `node hosted.mjs` prints { config, phases } as JSON. `config` is the
// persona's configuration (the scenario uses the same); `phases` has, for
// each step (start, withdrawing the host service, providing it again), the
// calls made during it (`calls`) and the sections registered after it
// (`sections`, ordered by `order`).
import { Context } from '@deepseek-ai/cordis'
import * as persona from '@deepseek-ai/dsh-persona'

const config = { prefix: 'You are a careful reviewer.', suffix: 'Answer briefly.' }

// The orders dsh-system-prompt's registry gives the persona's sections.
const ORDERS = { DEPLOYMENT_PERSONA_PREFIX: 0, DEPLOYMENT_PERSONA_SUFFIX: 10200 }

let calls = []
let sections = []

// The host service, as a plugin of its own: withdrawn by disposing it.
const host = {
  name: 'system-prompt-host',
  apply(ctx) {
    sections = []
    ctx.provide('systemPrompt', {
      section(section) {
        calls.push(['section', section.name, section.order, section.text])
        sections.push(section)
        return () => {
          calls.push(['dispose', section.name])
          sections = sections.filter(item => item !== section)
        }
      },
      getSectionOrder(name) {
        calls.push(['getSectionOrder', name])
        return ORDERS[name]
      },
      suppressRuntimeContext() {
        calls.push(['suppressRuntimeContext'])
      },
    })
  },
}

const phases = {}
const phase = name => {
  phases[name] = {
    calls,
    sections: sections.map(({ name, order, text }) => ({ name, order, text })).sort((a, b) => a.order - b.order),
  }
  calls = []
}

const ctx = new Context()
let provider = ctx.plugin(host)
const fiber = ctx.plugin(persona, config)
await Promise.all([provider.await(), fiber.await()])
phase('start')
await provider.dispose()
await fiber.await()
phase('withdraw')
provider = ctx.plugin(host)
await Promise.all([provider.await(), fiber.await()])
phase('restore')
await ctx.fiber.dispose()
process.stdout.write(JSON.stringify({ config, phases }))
process.exit(0)
